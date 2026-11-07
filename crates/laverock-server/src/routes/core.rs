//! Service metadata and the embedded single-page UI.

use axum::{extract::State, response::Html, routing::get, Json, Router};

use super::AppState;
use crate::{export, models};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/api/health", get(health))
        .route("/api/engines", get(engines))
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
