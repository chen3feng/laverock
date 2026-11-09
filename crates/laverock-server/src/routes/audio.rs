//! Ad-hoc audio helpers: transcription and WAV-to-MP3 encoding.

use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Multipart, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{error_frame, sse_frame, subtitle_response, ApiError, AppState};
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
            "/api/transcribe/stream",
            post(transcribe_stream).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
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

/// One multipart audio upload.
struct AudioUpload {
    audio: Vec<u8>,
    /// `record=1` asks the server to keep the transcript in history. The
    /// capture widget sends it only for an explicit transcribe click, not
    /// for the automatic pass that fills a reference's text.
    record: bool,
}

/// Read the `audio` field of a multipart upload.
async fn read_audio(mut multipart: Multipart) -> Result<AudioUpload, ApiError> {
    let mut audio: Option<Vec<u8>> = None;
    let mut record = false;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("audio") => {
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
            Some("record") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad record field: {e}")))?;
                record = matches!(raw.trim(), "1" | "true" | "yes");
            }
            _ => {}
        }
    }
    let audio = audio.ok_or_else(|| ApiError::bad_request("missing required field: audio"))?;
    Ok(AudioUpload { audio, record })
}

/// Keep a finished transcript in history. Best-effort: recognition has
/// already succeeded, so a storage failure must not fail the request.
pub(crate) fn record_transcript(
    state: &AppState,
    transcript: &laverock_core::Transcript,
    input_name: Option<&str>,
) {
    if transcript.text.trim().is_empty() {
        return;
    }
    let (engine_id, engine_name) = state
        .transcriber
        .model_info()
        .unwrap_or_else(|| ("stt".to_string(), "SenseVoice".to_string()));
    if let Err(e) = state.history.add_text(
        &engine_id,
        &engine_name,
        &transcript.text,
        &transcript.segments,
        transcript.duration,
        input_name,
    ) {
        tracing::warn!("history add failed: {e}");
    }
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
    let AudioUpload { audio, record } = read_audio(multipart).await?;

    // Loading and running the model is blocking; keep it off the runtime.
    let transcriber = state.transcriber.clone();
    let transcript = tokio::task::spawn_blocking(move || transcriber.transcribe_detailed(&audio))
        .await
        .map_err(panic_error("transcribe"))??;
    if record {
        record_transcript(&state, &transcript, None);
    }
    Ok(Json(TranscribeResponse {
        text: transcript.text,
    }))
}

/// Same input as `/api/transcribe`, but with timed segments.
async fn transcribe_details(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Json<laverock_core::Transcript>, ApiError> {
    let AudioUpload { audio, .. } = read_audio(multipart).await?;
    let transcriber = state.transcriber.clone();
    let transcript = tokio::task::spawn_blocking(move || transcriber.transcribe_detailed(&audio))
        .await
        .map_err(panic_error("transcribe"))??;
    record_transcript(&state, &transcript, None);
    Ok(Json(transcript))
}

/// Stream a transcript: one `segment` event per finished cue, then a
/// `done` event with the whole transcript. Long clips fill in as they are
/// decoded instead of after one full pass.
async fn transcribe_stream(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    let AudioUpload { audio, record } = read_audio(multipart).await?;
    let transcriber = state.transcriber.clone();
    let history_state = state.clone();
    let (tx, rx) = mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(16);

    tokio::task::spawn_blocking(move || {
        let result = transcriber.transcribe_progressive(&audio, |segment| {
            let payload = serde_json::json!({
                "text": segment.text,
                "start": segment.start,
                "end": segment.end,
            });
            let _ = tx.blocking_send(Ok(sse_frame("segment", &payload.to_string())));
        });
        match result {
            Ok(transcript) => {
                if record {
                    record_transcript(&history_state, &transcript, None);
                }
                let recorded = record && !transcript.text.trim().is_empty();
                let payload = serde_json::json!({
                    "text": transcript.text,
                    "segments": transcript.segments,
                    "language": transcript.language,
                    "duration": transcript.duration,
                    "recorded": recorded,
                });
                let _ = tx.blocking_send(Ok(sse_frame("done", &payload.to_string())));
            }
            Err(e) => {
                let _ = tx.blocking_send(Ok(error_frame(&e.to_string())));
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(ReceiverStream::new(rx)))
        .map_err(|e| ApiError::internal(e.to_string()))
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
    let AudioUpload { audio, .. } = read_audio(multipart).await?;
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
