//! System resource snapshot.

use axum::{extract::State, routing::get, Json, Router};

use super::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new().route("/api/system", get(system))
}

async fn system(State(state): State<AppState>) -> Json<crate::system::SystemView> {
    Json(crate::system::snapshot(&state.manager))
}
