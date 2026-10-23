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

mod models;
mod profiles;
mod proxy;
mod remote;
mod settings;

use axum::{
    body::Body,
    extract::{ConnectInfo, Multipart, Path, State},
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

async fn list_models(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<models::ModelList>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    Ok(Json(state.models.list()))
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

#[allow(clippy::too_many_arguments)]
async fn synthesize(
    Path(engine_id): Path<String>,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let mut text: Option<String> = None;
    let mut prompt_bytes: Option<Vec<u8>> = None;
    let mut prompt_text: Option<String> = None;
    let mut profile_id: Option<String> = None;
    let mut speed: Option<f32> = None;
    let mut num_steps: Option<i32> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        match field.name() {
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
            Some("speed") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad speed: {e}")))?;
                speed = raw.parse().ok().filter(|v: &f32| (0.2..=3.0).contains(v));
            }
            Some("num_steps") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad num_steps: {e}")))?;
                num_steps = raw.parse().ok().filter(|v: &i32| (1..=30).contains(v));
            }
            _ => {}
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

    // Defaults come from the engine manifest.
    let defaults = state.manager.manifest_defaults(&engine_id)?;
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

    let request = SynthRequest {
        text,
        prompt_text,
        prompt_samples,
        prompt_sr,
        speed: speed.unwrap_or(defaults.speed),
        num_steps: num_steps.unwrap_or(defaults.num_steps),
    };

    // Blocking ONNX inference off the async runtime.
    let manager = state.manager.clone();
    let Audio {
        samples,
        sample_rate,
    } = tokio::task::spawn_blocking(move || manager.synthesize(&engine_id, &request))
        .await
        .map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("engine task panicked: {e}"),
        })??;

    let wav = encode_wav(&samples, sample_rate)?;
    Ok((
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
        .into_response())
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
        .route("/api/profiles/{id}", delete(delete_profile))
        .route("/api/profiles/{id}/audio", get(profile_audio))
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
