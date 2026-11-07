//! Ad-hoc audio helpers: transcription and WAV-to-MP3 encoding.

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Multipart, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};

use super::{ApiError, AppState};
use crate::export;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/audio/mp3",
            // A minute of 48 kHz WAV exceeds axum's 2 MB default.
            post(export_mp3).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route(
            "/api/transcribe",
            post(transcribe).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
}

#[derive(Debug, serde::Serialize)]
struct TranscribeResponse {
    text: String,
}

/// Transcribe a reference WAV clip with the on-device STT model.
async fn transcribe(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<TranscribeResponse>, ApiError> {
    let mut audio: Option<Vec<u8>> = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        if field.name() == Some("audio") {
            let mut bytes = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|e| ApiError::bad_request(format!("bad upload: {e}")))?
            {
                bytes.extend_from_slice(&chunk);
            }
            audio = Some(bytes);
        }
    }
    let audio = audio.ok_or_else(|| ApiError::bad_request("missing required field: audio"))?;

    // Loading and running the model is blocking; keep it off the runtime.
    let transcriber = state.transcriber.clone();
    let text = tokio::task::spawn_blocking(move || transcriber.transcribe(&audio))
        .await
        .map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("transcribe task panicked: {e}"),
        })??;
    Ok(Json(TranscribeResponse { text }))
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
        [
            (header::CONTENT_TYPE, "audio/mpeg".to_string()),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"laverock.mp3\"".to_string(),
            ),
        ],
        mp3,
    )
        .into_response())
}
