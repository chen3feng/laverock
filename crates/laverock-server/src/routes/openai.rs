//! OpenAI-compatible endpoints for off-the-shelf clients.

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};

use super::AppState;
use crate::openai_compat;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/models", get(openai_compat::list_models))
        .route("/v1/models/{model_id}", get(openai_compat::get_model))
        .route(
            "/v1/audio/speech",
            post(openai_compat::create_speech).layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
}
