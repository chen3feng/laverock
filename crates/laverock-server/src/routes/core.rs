//! Service metadata and the embedded single-page UI.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Json, Router,
};

use super::{ApiError, AppState};
use crate::{export, models};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/api/health", get(health))
        .route("/api/engines", get(engines))
        .route("/api/engines/unload", post(unload_all))
        .route("/api/engines/{id}/unload", post(unload))
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../../assets/index.html"))
}

async fn health(State(state): State<AppState>) -> impl axum::response::IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "engines": state.manager.len(),
        "mp3": export::mp3_available(),
        "transcribe": state.transcriber.available(),
    }))
}

async fn engines(State(state): State<AppState>) -> impl axum::response::IntoResponse {
    // Models the catalog excludes on this OS stay out of the picker even
    // when an older install is still on disk.
    let hidden = state.models.hidden_engine_ids();
    let mut list = state.manager.list();
    list.retain(|engine| !hidden.contains(&engine.id));
    // Installed manifests do not repeat the catalog blurb; fill it in so
    // every client (paired phones included) gets a description without
    // the desktop-only /api/models route.
    for engine in &mut list {
        if engine.description.is_empty() {
            if let Some(description) = models::engine_description(&engine.id) {
                engine.description = description;
            }
        }
    }
    Json(list)
}

/// Release one engine's model, or every loaded model.
async fn unload(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    if !state.manager.list().iter().any(|engine| engine.id == id) {
        return Err(ApiError::not_found(format!("engine not found: {id}")));
    }
    state.manager.unload(&id);
    Ok(StatusCode::NO_CONTENT)
}

async fn unload_all(State(state): State<AppState>) -> StatusCode {
    state.manager.unload_all();
    StatusCode::NO_CONTENT
}
