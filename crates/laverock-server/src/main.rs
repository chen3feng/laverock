//! Laverock server: thin HTTP control plane over laverock-core.
//!
//! MVP surface (loopback-only by default):
//!   GET  /                    embedded web UI
//!   GET  /api/health          service + engine count
//!   GET  /api/engines         registered engines
//!   POST /api/engines/:id/synthesize   multipart: text + prompt/profile -> WAV
//!   GET  /api/profiles        saved voices
//!   POST /api/profiles        multipart: name + prompt_text + reference WAV
//!   GET  /api/profiles/:id/audio        reference clip
//!   DELETE /api/profiles/:id
//!
//! Phone/LAN access (pairing, TLS, device tokens) comes later; until
//! then the server binds loopback only.

mod export;
mod history;
mod models;
mod profiles;
mod proxy;
mod remote;
mod settings;

use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use laverock_core::{decode_wav, encode_wav, Audio, CoreError, EngineManager, SynthRequest};
use models::ModelService;
use profiles::ProfileStore;
use remote::{LanCmd, RemoteState};
use settings::{Settings, SettingsStore};
use std::{net::SocketAddr, sync::Arc};
use tracing::error;

#[derive(Clone)]
struct AppState {
    manager: Arc<EngineManager>,
    profiles: Arc<ProfileStore>,
    history: Arc<history::HistoryStore>,
    remote: Arc<RemoteState>,
    settings: Arc<SettingsStore>,
    models: Arc<ModelService>,
    lan: tokio::sync::mpsc::Sender<LanCmd>,
    port: u16,
}

/// Error type mapping core errors to HTTP.
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: "该操作只能在电脑端进行".to_string(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        error!(error = %err, "io error");
        Self::internal(err.to_string())
    }
}

impl From<CoreError> for ApiError {
    fn from(err: CoreError) -> Self {
        match err {
            CoreError::NotFound(id) => Self {
                status: StatusCode::NOT_FOUND,
                message: format!("not found: {id}"),
            },
            CoreError::Wav(msg) => Self {
                status: StatusCode::BAD_REQUEST,
                message: msg,
            },
            CoreError::BadRequest(msg) => Self {
                status: StatusCode::BAD_REQUEST,
                message: msg,
            },
            other => {
                error!(error = %other, "request failed");
                Self {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    message: other.to_string(),
                }
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../assets/index.html"))
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "engines": state.manager.len(),
        "mp3": export::mp3_available(),
    }))
}

async fn engines(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.manager.list())
}

// ---------------------------------------------------------------- remote

/// Paths a browser may reach before pairing: the SPA itself and the
/// endpoints needed to pair.
fn is_public_path(path: &str) -> bool {
    matches!(
        path,
        "/" | "/api/session" | "/api/pair/claim" | "/favicon.ico"
    )
}

fn token_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        return Some(token.trim().to_string());
    }
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        part.trim()
            .strip_prefix("laverock_device=")
            .map(|v| v.trim().to_string())
    })
}

/// Loopback is trusted; LAN requests need a paired device token, either
/// as `Authorization: Bearer …` or as the pairing cookie (needed for
/// `<audio src>` and EventSource, which cannot set headers).
async fn auth_middleware(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if peer.ip().is_loopback() || is_public_path(req.uri().path()) {
        return next.run(req).await;
    }

    let authorized = token_from_headers(req.headers())
        .and_then(|token| state.remote.authorize(&token))
        .is_some();

    if authorized {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "未配对的设备，请先在电脑上扫码配对" })),
        )
            .into_response()
    }
}

fn is_loopback(peer: &SocketAddr) -> bool {
    peer.ip().is_loopback()
}

async fn session(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let device = token_from_headers(&headers).and_then(|t| state.remote.authorize(&t));
    let loopback = is_loopback(&peer);

    Json(serde_json::json!({
        "loopback": loopback,
        "paired": device.is_some(),
        "device_name": device.map(|d| d.name),
        // Machine-wide default UI language ("" = follow the browser);
        // clients may override it locally.
        "language": state.settings.get().language,
        "lan_enabled": state.remote.lan_enabled(),
        // Do not hand the network address to unpaired peers.
        "lan_url": if loopback && state.remote.lan_enabled() {
            remote::lan_url(state.port)
        } else {
            None
        },
    }))
}

#[derive(serde::Deserialize)]
struct LanSetting {
    enabled: bool,
}

async fn get_lan(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    Ok(Json(serde_json::json!({
        "enabled": state.remote.lan_enabled(),
        "url": if state.remote.lan_enabled() { remote::lan_url(state.port) } else { None },
        "fingerprint": state.remote.fingerprint(),
    })))
}

async fn set_lan(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<LanSetting>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }

    if body.enabled {
        let (tx, rx) = tokio::sync::oneshot::channel();
        state
            .lan
            .send(LanCmd::Start(tx))
            .await
            .map_err(|_| ApiError::internal("LAN supervisor stopped"))?;
        match rx
            .await
            .map_err(|_| ApiError::internal("LAN supervisor dropped"))?
        {
            Ok(info) => {
                state.remote.set_lan_enabled(true)?;
                state.remote.set_fingerprint(Some(info.fingerprint.clone()));
                return Ok(Json(serde_json::json!({
                    "enabled": true,
                    "url": info.url,
                    "fingerprint": info.fingerprint,
                })));
            }
            Err(e) => return Err(ApiError::bad_request(e)),
        }
    }

    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .lan
        .send(LanCmd::Stop(tx))
        .await
        .map_err(|_| ApiError::internal("LAN supervisor stopped"))?;
    let _ = rx.await;
    state.remote.set_lan_enabled(false)?;
    state.remote.set_fingerprint(None);

    Ok(Json(serde_json::json!({
        "enabled": false,
        "url": null,
    })))
}

async fn start_pairing(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    let Some(base) = remote::lan_url(state.port) else {
        return Err(ApiError::bad_request(
            "请先开启局域网访问，并确认电脑已连接到网络",
        ));
    };
    let (code, expires_at) = state.remote.start_pairing();
    let url = format!("{base}/?pair={code}");
    let qr_svg = remote::make_qr_svg(&url).map_err(ApiError::internal)?;

    Ok(Json(serde_json::json!({
        "code": code,
        "url": url,
        "expires_at": expires_at,
        "qr_svg": qr_svg,
        "fingerprint": state.remote.fingerprint(),
    })))
}

#[derive(serde::Deserialize)]
struct ClaimRequest {
    code: String,
    #[serde(default)]
    device_name: String,
}

async fn claim(
    State(state): State<AppState>,
    Json(body): Json<ClaimRequest>,
) -> Result<Response, ApiError> {
    let token = state
        .remote
        .claim(&body.code, &body.device_name)
        .map_err(ApiError::bad_request)?;

    let device = state.remote.authorize(&token);
    let cookie =
        format!("laverock_device={token}; Path=/; Max-Age=31536000; HttpOnly; SameSite=Lax");

    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({
            "paired": true,
            "device_name": device.map(|d| d.name),
        })),
    )
        .into_response())
}

async fn list_devices(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<Vec<remote::DeviceView>>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    Ok(Json(state.remote.devices()))
}

async fn revoke_device(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    state.remote.revoke(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------- settings / models

async fn get_settings(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    let settings = state.settings.get();
    let system_proxy = match settings.proxy_mode {
        settings::ProxyMode::System => proxy::system(),
        settings::ProxyMode::Manual | settings::ProxyMode::Direct => None,
    };
    Ok(Json(serde_json::json!({
        "proxy_mode": settings.proxy_mode,
        "proxy": settings.proxy,
        "system_proxy": system_proxy,
        "hf_endpoint": settings.hf_endpoint,
        "pip_index": settings.pip_index,
        "language": settings.language,
        "models_dir": state.models.models_dir().display().to_string(),
        "scripts_dir": state.models.scripts_dir().map(|p| p.display().to_string()),
    })))
}

async fn set_settings(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Settings>,
) -> Result<Json<Settings>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    Ok(Json(state.settings.update(body)?))
}

#[derive(serde::Deserialize)]
struct LangQuery {
    #[serde(default)]
    lang: String,
}

async fn list_models(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(query): Query<LangQuery>,
) -> Result<Json<models::ModelList>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    let lang = if query.lang == "en" { "en" } else { "zh" };
    Ok(Json(state.models.list(lang)))
}

async fn install_model(
    Path(id): Path<String>,
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    state.models.install(&id)?;
    Ok(Json(serde_json::json!({ "started": true, "id": id })))
}

async fn reload_models(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    let engines = state.models.reload()?;
    Ok(Json(serde_json::json!({ "engines": engines })))
}

async fn list_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<profiles::ProfileView>>, ApiError> {
    Ok(Json(state.profiles.list()?))
}

async fn create_profile(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<profiles::ProfileMeta>, ApiError> {
    let mut name = None;
    let mut prompt_text = None;
    let mut engine_id = None;
    let mut wav: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("name") => {
                name = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad name: {e}")))?,
                )
            }
            Some("prompt_text") => {
                prompt_text = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad prompt_text: {e}")))?,
                )
            }
            Some("engine_id") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad engine_id: {e}")))?;
                if !raw.trim().is_empty() {
                    engine_id = Some(raw);
                }
            }
            Some("prompt") => {
                wav = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad prompt upload: {e}")))?
                        .to_vec(),
                );
            }
            _ => {}
        }
    }

    let name = name.ok_or_else(|| ApiError::bad_request("missing required field: name"))?;
    let prompt_text =
        prompt_text.ok_or_else(|| ApiError::bad_request("missing required field: prompt_text"))?;
    let wav = wav
        .filter(|b| !b.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing required field: prompt (WAV reference)"))?;

    Ok(Json(state.profiles.create(
        &name,
        &prompt_text,
        engine_id,
        &wav,
    )?))
}

async fn update_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<profiles::ProfileMeta>, ApiError> {
    let mut name = None;
    let mut prompt_text = None;
    let mut wav: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("name") => {
                name = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad name: {e}")))?,
                )
            }
            Some("prompt_text") => {
                prompt_text = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad prompt_text: {e}")))?,
                )
            }
            Some("prompt") => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad prompt upload: {e}")))?;
                if !bytes.is_empty() {
                    wav = Some(bytes.to_vec());
                }
            }
            _ => {}
        }
    }

    // Current values fill in whatever the form did not send.
    let (current, _) = state.profiles.get(&id)?;
    let name = name.unwrap_or(current.name);
    let prompt_text = prompt_text.unwrap_or(current.prompt_text);
    Ok(Json(state.profiles.update(
        &id,
        &name,
        &prompt_text,
        wav.as_deref(),
    )?))
}

async fn delete_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.profiles.delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn profile_audio(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let (_, wav) = state.profiles.get(&id)?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "audio/wav".to_string())],
        wav,
    )
        .into_response())
}

// ---------------------------------------------------------------- history

async fn list_history(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "history": state.history.list() }))
}

async fn delete_history(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.history.delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_history(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    state.history.clear()?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
struct AudioQuery {
    #[serde(default)]
    format: Option<String>,
}

async fn history_audio(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AudioQuery>,
) -> Result<Response, ApiError> {
    let wav_path = state.history.audio_path(&id)?;
    if query.format.as_deref() != Some("mp3") {
        let bytes = tokio::fs::read(&wav_path).await?;
        return Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "audio/wav".to_string())],
            bytes,
        )
            .into_response());
    }

    // Encode once and cache next to the WAV (history audio is immutable).
    let mp3_path = wav_path.with_file_name("audio.mp3");
    let bytes = if mp3_path.is_file() {
        tokio::fs::read(&mp3_path).await?
    } else {
        let wav = tokio::fs::read(&wav_path).await?;
        let mp3 = tokio::task::spawn_blocking(move || export::wav_to_mp3(&wav))
            .await
            .map_err(|e| ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("mp3 task panicked: {e}"),
            })??;
        let tmp = mp3_path.with_extension("mp3.tmp");
        tokio::fs::write(&tmp, &mp3).await?;
        let _ = tokio::fs::rename(&tmp, &mp3_path).await;
        mp3
    };
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "audio/mpeg".to_string())],
        bytes,
    )
        .into_response())
}

async fn export_mp3(body: Bytes) -> Result<Response, ApiError> {
    let mp3 = tokio::task::spawn_blocking(move || export::wav_to_mp3(&body))
        .await
        .map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("mp3 task panicked: {e}"),
        })??;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "audio/mpeg".to_string())],
        mp3,
    )
        .into_response())
}

#[allow(clippy::too_many_arguments)]
async fn synthesize(
    Path(engine_id): Path<String>,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let mut text: Option<String> = None;
    let mut prompt_bytes: Option<Vec<u8>> = None;
    let mut prompt_text: Option<String> = None;
    let mut profile_id: Option<String> = None;
    // Raw values for manifest-declared synthesis parameters.
    let mut raw_params: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        let name = field.name().map(str::to_string);
        match name.as_deref() {
            Some("text") => {
                text = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad text field: {e}")))?,
                );
            }
            Some("prompt") => {
                prompt_bytes =
                    Some(
                        field.bytes().await.map(|v| v.to_vec()).map_err(|e| {
                            ApiError::bad_request(format!("bad prompt upload: {e}"))
                        })?,
                    );
            }
            Some("prompt_text") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad prompt_text: {e}")))?;
                if !raw.trim().is_empty() {
                    prompt_text = Some(raw);
                }
            }
            Some("profile_id") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad profile_id: {e}")))?;
                if !raw.trim().is_empty() {
                    profile_id = Some(raw.trim().to_string());
                }
            }
            Some(other) => {
                // Anything else is a candidate engine parameter; validated
                // against the engine's declared parameter list below.
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad {other} field: {e}")))?;
                raw_params.insert(other.to_string(), raw);
            }
            None => {}
        }
    }

    let text = text
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing required field: text"))?;
    // A saved voice takes precedence over an ad-hoc upload. Reference audio
    // is optional at the API level: preset/voice-design engines do not need
    // one, and each engine reports what it requires.
    let (prompt_bytes, prompt_text) = if let Some(id) = profile_id {
        let (meta, wav) = state.profiles.get(&id)?;
        (Some(wav), Some(meta.prompt_text))
    } else {
        let text = prompt_text
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        (prompt_bytes.filter(|b| !b.is_empty()), text)
    };

    // Validate all engine parameters against the manifest's list.
    let param_specs = state.manager.parameters(&engine_id)?;
    let params = resolve_params(&param_specs, &raw_params)?;

    // The two historical typed fields live in the parameter list too.
    let defaults = state.manager.manifest_defaults(&engine_id)?;
    let speed = params
        .get("speed")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(defaults.speed);
    let num_steps = params
        .get("num_steps")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .unwrap_or(defaults.num_steps);

    let (prompt_samples, prompt_sr) = match prompt_bytes {
        Some(bytes) => {
            let (samples, rate) = decode_wav(&bytes)?;
            if samples.is_empty() {
                return Err(ApiError::bad_request("reference audio is empty"));
            }
            (samples, rate)
        }
        None => (Vec::new(), 0),
    };

    let manager = state.manager.clone();
    let history = state.history.clone();
    let engine_name = state
        .manager
        .list()
        .into_iter()
        .find(|e| e.id == engine_id)
        .map(|e| e.name)
        .unwrap_or_else(|| engine_id.clone());

    // The desktop UI asks for a Server-Sent-Events stream so long Qwen
    // generations can report progress; other clients keep getting the WAV.
    let wants_stream = headers
        .get("x-laverock-stream")
        .and_then(|v| v.to_str().ok())
        == Some("1");
    if !wants_stream {
        // Blocking inference off the async runtime.
        let request = SynthRequest {
            text: text.clone(),
            prompt_text: prompt_text.clone(),
            prompt_samples: prompt_samples.clone(),
            prompt_sr,
            speed,
            num_steps,
            params: params.clone(),
            progress: None,
        };
        let history_engine = engine_id.clone();
        let Audio {
            samples,
            sample_rate,
        } = tokio::task::spawn_blocking(move || manager.synthesize(&history_engine, &request))
            .await
            .map_err(|e| ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("engine task panicked: {e}"),
            })??;

        let wav = encode_wav(&samples, sample_rate)?;
        if let Err(e) = history.add(&engine_id, &engine_name, &text, &wav, sample_rate) {
            tracing::warn!("history add failed: {e}");
        }
        return Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "audio/wav".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"laverock.wav\"".to_string(),
                ),
            ],
            wav,
        )
            .into_response());
    }

    use axum::body::Bytes;
    use base64::Engine;
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::ReceiverStream;

    let sse = |event: &str, data: &str| -> Bytes {
        Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
    };
    let (tx, rx) = mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(16);

    let history_text = text.clone();
    tokio::task::spawn_blocking(move || {
        let tx2 = tx.clone();
        let sink: laverock_core::ProgressSink = Arc::new(move |event| {
            let frame = match event {
                laverock_core::ProgressEvent::Stage(label) => sse("stage", &label),
                laverock_core::ProgressEvent::Steps(n) => sse("steps", &n.to_string()),
            };
            // Progress frames are lossy: never block the engine on a
            // disconnected or slow UI. The final WAV frame is allowed to
            // wait for buffer space (it errors immediately if closed).
            let _ = tx2.try_send(Ok(frame));
        });
        let request = SynthRequest {
            text,
            prompt_text,
            prompt_samples,
            prompt_sr,
            speed,
            num_steps,
            params,
            progress: Some(sink),
        };
        match manager.synthesize(&engine_id, &request) {
            Ok(Audio {
                samples,
                sample_rate,
            }) => match encode_wav(&samples, sample_rate) {
                Ok(wav) => {
                    if let Err(e) =
                        history.add(&engine_id, &engine_name, &history_text, &wav, sample_rate)
                    {
                        tracing::warn!("history add failed: {e}");
                    }
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&wav);
                    let _ = tx.blocking_send(Ok(sse("wav", &b64)));
                }
                Err(e) => {
                    let _ = tx.blocking_send(Ok(error_frame(&sse, &e.to_string())));
                }
            },
            Err(e) => {
                let _ = tx.blocking_send(Ok(error_frame(&sse, &e.to_string())));
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(ReceiverStream::new(rx)))
        .map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: e.to_string(),
        })
}

fn error_frame(sse: &impl Fn(&str, &str) -> Bytes, message: &str) -> Bytes {
    let json = serde_json::json!({ "message": message }).to_string();
    sse("error", &json)
}

/// Parse and range-check raw form fields against the engine's declared
/// parameter list. Unknown fields are rejected; absent fields take the
/// manifest defaults.
fn resolve_params(
    specs: &[laverock_core::ParamSpec],
    raw: &std::collections::BTreeMap<String, String>,
) -> Result<std::collections::BTreeMap<String, serde_json::Value>, ApiError> {
    use laverock_core::{ParamKind, ParamSpec};
    let mut out = std::collections::BTreeMap::new();
    for spec in specs {
        let ParamSpec {
            name,
            kind,
            min,
            max,
            options,
            default,
            hint,
            ..
        } = spec;
        let value = match raw.get(name) {
            Some(v) if !v.trim().is_empty() => parse_param(spec, v.trim())?,
            _ => default.clone(),
        };
        let _ = (min, max, hint, kind);
        if *kind == ParamKind::Choice {
            if let serde_json::Value::String(choice) = &value {
                if !options.iter().any(|o| &o.value == choice) {
                    return Err(ApiError::bad_request(format!("{name}: invalid option")));
                }
            }
        }
        out.insert(name.clone(), value);
    }
    let known: std::collections::HashSet<&str> = specs.iter().map(|p| p.name.as_str()).collect();
    if let Some(name) = raw.keys().map(String::as_str).find(|n| !known.contains(n)) {
        return Err(ApiError::bad_request(format!("unknown parameter: {name}")));
    }
    Ok(out)
}

fn parse_param(spec: &laverock_core::ParamSpec, raw: &str) -> Result<serde_json::Value, ApiError> {
    use laverock_core::ParamKind;
    let name = &spec.name;
    let range_ok = |n: f64| -> bool {
        spec.min
            .as_ref()
            .and_then(|v| v.as_f64())
            .is_none_or(|min| n >= min)
            && spec
                .max
                .as_ref()
                .and_then(|v| v.as_f64())
                .is_none_or(|max| n <= max)
    };
    let value = match spec.kind {
        ParamKind::Float => {
            let n: f64 = raw
                .parse()
                .map_err(|_| ApiError::bad_request(format!("{name}: expected a number")))?;
            if !range_ok(n) {
                return Err(ApiError::bad_request(format!("{name}: out of range")));
            }
            serde_json::json!(n)
        }
        ParamKind::Int => {
            let n: i64 = raw
                .parse()
                .map_err(|_| ApiError::bad_request(format!("{name}: expected an integer")))?;
            if !range_ok(n as f64) {
                return Err(ApiError::bad_request(format!("{name}: out of range")));
            }
            serde_json::json!(n)
        }
        ParamKind::Bool => serde_json::json!(matches!(
            raw.to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        )),
        ParamKind::Choice => serde_json::Value::String(raw.to_string()),
        ParamKind::Text | ParamKind::TextArea => {
            // Free-form text: bounded so a stray paste cannot bloat the
            // request or the engine command line.
            const MAX_TEXT_PARAM: usize = 2000;
            let text = raw.trim();
            if text.chars().count() > MAX_TEXT_PARAM {
                return Err(ApiError::bad_request(format!(
                    "{name}: 文本过长（最多 {MAX_TEXT_PARAM} 字）"
                )));
            }
            serde_json::Value::String(text.to_string())
        }
    };
    Ok(value)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,laverock_server=debug".into()),
        )
        .init();

    let host = std::env::var("LAVEROCK_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("LAVEROCK_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8787);
    let models_dir = std::env::var("LAVEROCK_MODELS_DIR").unwrap_or_else(|_| "models".to_string());
    let data_dir = std::env::var("LAVEROCK_DATA_DIR").unwrap_or_else(|_| "data".to_string());

    let manager = Arc::new(EngineManager::open(&models_dir).expect("scan models dir"));
    let profiles = Arc::new(ProfileStore::open(&data_dir).expect("open data dir"));
    let history = Arc::new(history::HistoryStore::open(&data_dir).expect("open history"));
    let remote = Arc::new(RemoteState::open(&data_dir).expect("open remote state"));
    let settings = Arc::new(SettingsStore::open(&data_dir).expect("open settings"));
    let scripts_dir = models::resolve_scripts_dir();
    let model_service = Arc::new(ModelService::new(
        &models_dir,
        scripts_dir.clone(),
        settings.clone(),
        manager.clone(),
    ));
    tracing::info!(
        dir = %models_dir,
        engines = manager.len(),
        data = %data_dir,
        scripts = ?scripts_dir,
        "engine registry ready"
    );
    tracing::info!(proxy = ?proxy::system(), "download proxy");

    let (lan_tx, lan_rx) = tokio::sync::mpsc::channel(8);
    let boot_lan = lan_tx.clone();
    let state = AppState {
        manager,
        profiles,
        history,
        remote: remote.clone(),
        settings,
        models: model_service,
        lan: lan_tx,
        port,
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/api/health", get(health))
        .route("/api/engines", get(engines))
        .route("/api/engines/{id}/synthesize", post(synthesize))
        .route("/api/profiles", get(list_profiles).post(create_profile))
        .route(
            "/api/profiles/{id}",
            delete(delete_profile).post(update_profile),
        )
        .route("/api/profiles/{id}/audio", get(profile_audio))
        .route("/api/history", get(list_history).delete(clear_history))
        .route("/api/history/{id}", delete(delete_history))
        .route("/api/history/{id}/audio", get(history_audio))
        .route(
            "/api/audio/mp3",
            // A minute of 48 kHz WAV exceeds axum's 2 MB default.
            post(export_mp3).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route("/api/session", get(session))
        .route("/api/pair/start", post(start_pairing))
        .route("/api/pair/claim", post(claim))
        .route("/api/devices", get(list_devices))
        .route("/api/devices/{id}", delete(revoke_device))
        .route("/api/settings/lan", get(get_lan).post(set_lan))
        .route("/api/settings", get(get_settings).post(set_settings))
        .route("/api/models", get(list_models))
        .route("/api/models/reload", post(reload_models))
        .route("/api/models/{id}/install", post(install_model))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state);

    // The supervisor owns the router so the LAN listener can be started
    // and stopped at runtime.
    let lan_supervisor = tokio::spawn(remote::lan_supervisor(
        lan_rx,
        app.clone(),
        port,
        std::path::PathBuf::from(&data_dir).join("tls"),
    ));

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("valid LAVEROCK_HOST:PORT");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    tracing::info!("Laverock listening on http://{addr}");

    if remote.lan_enabled() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if boot_lan.send(LanCmd::Start(tx)).await.is_ok() {
            match rx.await {
                Ok(Ok(info)) => {
                    remote.set_fingerprint(Some(info.fingerprint.clone()));
                    tracing::info!(url = %info.url, "LAN access enabled");
                }
                Ok(Err(e)) => tracing::warn!("LAN access enabled but listener failed: {e}"),
                Err(_) => tracing::warn!("LAN supervisor dropped the start request"),
            }
        }
    }

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
    })
    .await
    .expect("server error");

    lan_supervisor.abort();
}
