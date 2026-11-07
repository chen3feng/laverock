//! Model catalog: list, install, delete and reload manifests.

use axum::{
    extract::{ConnectInfo, Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use std::net::SocketAddr;

use super::{is_loopback, ApiError, AppState};
use crate::models;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/models", get(list_models))
        .route("/api/models/reload", post(reload_models))
        .route("/api/models/{id}/install", post(install_model))
        .route("/api/models/{id}", delete(delete_model))
}

#[derive(serde::Deserialize)]
struct LangQuery {
    #[serde(default)]
    lang: String,
}

async fn list_models(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(query): Query<LangQuery>,
) -> Result<Json<models::ModelList>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    let lang = if query.lang == "en" { "en" } else { "zh" };
    Ok(Json(state.models.list(lang)))
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

async fn delete_model(
    Path(id): Path<String>,
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    state.models.remove(&id)?;
    Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
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
