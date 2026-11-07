//! Synthesis history: list, delete, and serve the generated audio.

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use laverock_core::TaskKind;

use super::{ApiError, AppState};
use crate::export;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/history", get(list_history).delete(clear_history))
        .route("/api/history/{id}", delete(delete_history))
        .route("/api/history/{id}/audio", get(history_audio))
}

#[derive(Debug, serde::Deserialize)]
struct HistoryQuery {
    task: Option<String>,
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
    Ok(Json(
        serde_json::json!({ "history": state.history.list_by_task(task) }),
    ))
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
