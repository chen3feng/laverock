//! Synthesis history: list, delete, and serve generated audio, text and
//! transcript cues.

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use laverock_core::TaskKind;

use super::{subtitle_response, ApiError, AppState};
use crate::{export, subtitles};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/history", get(list_history).delete(clear_history))
        .route("/api/history/{id}", delete(delete_history))
        .route("/api/history/{id}/audio", get(history_audio))
        .route("/api/history/{id}/text", get(history_text))
        .route("/api/history/{id}/subtitles", get(history_subtitles))
        .route("/api/history/{id}/input", get(history_input))
}

#[derive(Debug, serde::Deserialize)]
struct HistoryQuery {
    task: Option<String>,
    q: Option<String>,
}

async fn list_history(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let task = match query.task.as_deref() {
        None | Some("") => None,
        Some(raw) => Some(
            TaskKind::parse(raw)
                .ok_or_else(|| ApiError::bad_request(format!("unknown task: {raw}")))?,
        ),
    };
    Ok(Json(serde_json::json!({
        "history": state.history.list(task, query.q.as_deref()),
    })))
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
            [
                (header::CONTENT_TYPE, "audio/wav".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"laverock-{id}.wav\""),
                ),
            ],
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
        [
            (header::CONTENT_TYPE, "audio/mpeg".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"laverock-{id}.mp3\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// The full result text: a transcript for ASR entries, the prompt otherwise.
async fn history_text(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let text = state.history.text(&id)?;
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "text/plain; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"laverock-{id}.txt\""),
            ),
        ],
        text,
    )
        .into_response())
}

/// Timed cues as an SRT/WebVTT download.
async fn history_subtitles(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AudioQuery>,
) -> Result<Response, ApiError> {
    let raw = query.format.as_deref().unwrap_or("");
    let format = subtitles::Format::parse(raw)
        .ok_or_else(|| ApiError::bad_request(format!("unknown subtitle format: {raw}")))?;
    let segments = state.history.segments(&id)?;
    if segments.is_empty() {
        return Err(ApiError::bad_request("this entry has no timed cues"));
    }
    Ok(subtitle_response(
        &segments,
        format,
        &format!("laverock-{id}"),
    ))
}

/// The uploaded source clip of a voice conversion.
async fn history_input(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let path = state.history.input_path(&id)?;
    let bytes = tokio::fs::read(path).await?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "audio/wav".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"laverock-{id}-source.wav\""),
            ),
        ],
        bytes,
    )
        .into_response())
}
