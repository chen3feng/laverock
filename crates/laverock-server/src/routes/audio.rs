//! Ad-hoc audio helpers: transcription and WAV-to-MP3 encoding.

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Multipart, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};

use super::{subtitle_response, ApiError, AppState};
use crate::{export, subtitles};

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
        .route(
            "/api/transcribe/details",
            post(transcribe_details).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route(
            "/api/transcribe/subtitles",
            post(transcribe_subtitles).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
}

#[derive(Debug, serde::Serialize)]
struct TranscribeResponse {
    text: String,
}

/// Read the `audio` field of a multipart upload.
async fn read_audio(mut multipart: Multipart) -> Result<Vec<u8>, ApiError> {
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
    audio.ok_or_else(|| ApiError::bad_request("missing required field: audio"))
}

fn panic_error(what: &str) -> impl FnOnce(tokio::task::JoinError) -> ApiError + '_ {
    move |e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("{what} task panicked: {e}"),
    }
}

/// Transcribe a reference WAV clip with the on-device STT model.
async fn transcribe(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Json<TranscribeResponse>, ApiError> {
    let audio = read_audio(multipart).await?;

    // Loading and running the model is blocking; keep it off the runtime.
    let transcriber = state.transcriber.clone();
    let text = tokio::task::spawn_blocking(move || transcriber.transcribe(&audio))
        .await
        .map_err(panic_error("transcribe"))??;
    Ok(Json(TranscribeResponse { text }))
}

/// Same input as `/api/transcribe`, but with timed segments.
async fn transcribe_details(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Json<laverock_core::Transcript>, ApiError> {
    let audio = read_audio(multipart).await?;
    let transcriber = state.transcriber.clone();
    let transcript = tokio::task::spawn_blocking(move || transcriber.transcribe_detailed(&audio))
        .await
        .map_err(panic_error("transcribe"))??;
    Ok(Json(transcript))
}

#[derive(Debug, serde::Deserialize)]
struct SubtitleQuery {
    /// `srt` (default) or `vtt`.
    format: Option<String>,
}

/// Transcribe an upload and return it as an SRT/WebVTT file.
async fn transcribe_subtitles(
    State(state): State<AppState>,
    Query(query): Query<SubtitleQuery>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    let raw = query.format.as_deref().unwrap_or("");
    let format = subtitles::Format::parse(raw)
        .ok_or_else(|| ApiError::bad_request(format!("unknown subtitle format: {raw}")))?;
    let audio = read_audio(multipart).await?;
    let transcriber = state.transcriber.clone();
    let transcript = tokio::task::spawn_blocking(move || transcriber.transcribe_detailed(&audio))
        .await
        .map_err(panic_error("transcribe"))??;
    Ok(subtitle_response(&transcript.segments, format, "laverock"))
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
