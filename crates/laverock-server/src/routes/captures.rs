//! Recording library: independent takes used to build voices.

use axum::{
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

use super::{ApiError, AppState};
use crate::captures::{CaptureMeta, CaptureView};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/captures", get(list_captures).post(create_capture))
        .route("/api/captures/dedupe", post(dedupe_captures))
        .route("/api/enhance", get(enhance_status))
        .route(
            "/api/captures/{id}",
            post(update_capture).delete(delete_capture),
        )
        .route("/api/captures/{id}/audio", get(capture_audio))
        .route("/api/captures/{id}/enhance", post(enhance_capture))
        .route("/api/captures/{id}/transcribe", post(transcribe_capture))
}

async fn list_captures(State(state): State<AppState>) -> Result<Json<Vec<CaptureView>>, ApiError> {
    Ok(Json(state.profiles.captures().list()?))
}

/// `POST /api/captures` — multipart: `audio` (required), `name`,
/// `prompt_text`. The transcript may be empty; transcribe afterwards.
async fn create_capture(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<CaptureMeta>, ApiError> {
    let mut name = String::new();
    let mut prompt_text = String::new();
    let mut enhance = laverock_core::EnhanceMode::Off;
    let mut wav: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("name") => {
                name = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad name: {e}")))?;
            }
            Some("prompt_text") => {
                prompt_text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad prompt_text: {e}")))?;
            }
            Some("enhance") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad enhance mode: {e}")))?;
                enhance = laverock_core::EnhanceMode::parse(&raw)
                    .ok_or_else(|| ApiError::bad_request(format!("unknown enhance mode: {raw}")))?;
            }
            Some("audio") | Some("prompt") => {
                wav = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad audio upload: {e}")))?
                        .to_vec(),
                );
            }
            _ => {}
        }
    }
    let wav = wav
        .filter(|b| !b.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing required field: audio"))?;

    // Denoising takes a fraction of a second; vocal separation runs at
    // about half real time, so keep both off the async runtime.
    let store = state.profiles.captures().clone();
    let models = state.models.models_dir().to_path_buf();
    let tools = state.models.tools_dir().map(std::path::Path::to_path_buf);
    let meta = tokio::task::spawn_blocking(move || {
        let enhancer = (!enhance.is_off())
            .then(|| laverock_core::Enhancer::discover(&models, tools.as_deref()));
        store.create_enhanced(&name, &prompt_text, &wav, true, enhance, enhancer.as_ref())
    })
    .await
    .map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("enhance task panicked: {e}"),
    })??;
    Ok(Json(meta))
}

/// `POST /api/captures/{id}/enhance` — JSON `{"mode": "denoise"|"vocals"|"both"|"off"}`.
/// Re-runs enhancement from the untouched upload, or reverts with `off`.
async fn enhance_capture(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<EnhanceBody>,
) -> Result<Json<CaptureMeta>, ApiError> {
    let mode = laverock_core::EnhanceMode::parse(&body.mode)
        .ok_or_else(|| ApiError::bad_request(format!("unknown enhance mode: {}", body.mode)))?;
    let store = state.profiles.captures().clone();
    let models = state.models.models_dir().to_path_buf();
    let tools = state.models.tools_dir().map(std::path::Path::to_path_buf);
    let meta = tokio::task::spawn_blocking(move || {
        let enhancer = laverock_core::Enhancer::discover(&models, tools.as_deref());
        store.enhance(&id, mode, &enhancer)
    })
    .await
    .map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("enhance task panicked: {e}"),
    })??;
    Ok(Json(meta))
}

#[derive(serde::Deserialize)]
struct EnhanceBody {
    mode: String,
}

/// `GET /api/enhance` — which enhancement stages the installed pack covers.
async fn enhance_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let enhancer =
        laverock_core::Enhancer::discover(state.models.models_dir(), state.models.tools_dir());
    Json(serde_json::json!({
        "denoise": enhancer.has_denoise(),
        "vocals": enhancer.has_separation(),
    }))
}

/// `POST /api/captures/{id}` — multipart: `name` and/or `prompt_text`.
async fn update_capture(
    State(state): State<AppState>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<CaptureMeta>, ApiError> {
    let mut name = None;
    let mut prompt_text = None;
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
                );
            }
            Some("prompt_text") => {
                prompt_text = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad prompt_text: {e}")))?,
                );
            }
            _ => {}
        }
    }
    let meta = state
        .profiles
        .captures()
        .update(&id, name.as_deref(), prompt_text.as_deref())?;
    Ok(Json(meta))
}

async fn delete_capture(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.profiles.delete_capture(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize, Default)]
struct CaptureAudioQuery {
    /// `1`, `true` or `yes` serves the untouched upload instead of the
    /// enhanced take. Kept as a string so both spellings work.
    #[serde(default)]
    original: Option<String>,
}

impl CaptureAudioQuery {
    fn wants_original(&self) -> bool {
        matches!(
            self.original.as_deref(),
            Some("1" | "true" | "yes" | "TRUE" | "YES")
        )
    }
}

async fn capture_audio(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<CaptureAudioQuery>,
) -> Result<Response, ApiError> {
    let wav = if query.wants_original() {
        state
            .profiles
            .captures()
            .get_original(&id)?
            .ok_or_else(|| ApiError::not_found("this take has no original recording"))?
    } else {
        state.profiles.captures().get(&id)?.1
    };
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "audio/wav".to_string())],
        wav,
    )
        .into_response())
}

/// Re-run speech recognition on a stored take. Voices built from it get
/// their transcript refreshed too.
async fn transcribe_capture(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CaptureMeta>, ApiError> {
    let (_, wav) = state.profiles.captures().get(&id)?;
    let transcriber = state.transcriber.clone();
    let text = tokio::task::spawn_blocking(move || transcriber.transcribe(&wav))
        .await
        .map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("transcribe task panicked: {e}"),
        })??;
    if text.trim().is_empty() {
        return Err(ApiError::bad_request("no speech recognized"));
    }
    Ok(Json(state.profiles.set_capture_transcript(&id, &text)?))
}

/// Delete takes whose audio is byte-identical to an older take, and
/// repoint the voices that used them.
async fn dedupe_captures(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let removed = state.profiles.dedupe_captures()?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}
