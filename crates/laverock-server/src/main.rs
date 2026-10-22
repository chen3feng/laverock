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

mod profiles;

use axum::{
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use laverock_core::{decode_wav, encode_wav, Audio, CoreError, EngineManager, SynthRequest};
use profiles::ProfileStore;
use std::{net::SocketAddr, sync::Arc};
use tracing::error;

#[derive(Clone)]
struct AppState {
    manager: Arc<EngineManager>,
    profiles: Arc<ProfileStore>,
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
        "engines": state.manager.len(),
    }))
}

async fn engines(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.manager.list())
}

async fn list_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<profiles::ProfileMeta>>, ApiError> {
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
    // A saved voice takes precedence over an ad-hoc upload.
    let (prompt_bytes, prompt_text) = if let Some(id) = profile_id {
        let (meta, wav) = state.profiles.get(&id)?;
        (wav, Some(meta.prompt_text))
    } else {
        let wav = prompt_bytes.filter(|b| !b.is_empty()).ok_or_else(|| {
            ApiError::bad_request("missing required field: prompt (WAV reference) or profile_id")
        })?;
        // ZipVoice requires the transcript of the reference clip; without it
        // sherpa-onnx logs "reference_text is empty" and returns no audio.
        let text = prompt_text
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                ApiError::bad_request(
                    "missing required field: prompt_text (transcript of the reference audio)",
                )
            })?;
        (wav, Some(text))
    };

    // Defaults come from the engine manifest.
    let defaults = state.manager.manifest_defaults(&engine_id)?;
    let (prompt_samples, prompt_sr) = decode_wav(&prompt_bytes)?;
    if prompt_samples.is_empty() {
        return Err(ApiError::bad_request("reference audio is empty"));
    }

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
    tracing::info!(
        dir = %models_dir,
        engines = manager.len(),
        data = %data_dir,
        "engine registry ready"
    );

    let app = Router::new()
        .route("/", get(index))
        .route("/api/health", get(health))
        .route("/api/engines", get(engines))
        .route("/api/engines/{id}/synthesize", post(synthesize))
        .route("/api/profiles", get(list_profiles).post(create_profile))
        .route("/api/profiles/{id}", delete(delete_profile))
        .route("/api/profiles/{id}/audio", get(profile_audio))
        .with_state(AppState { manager, profiles });

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("valid LAVEROCK_HOST:PORT");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    tracing::info!("Laverock listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await
        .expect("server error");
}
