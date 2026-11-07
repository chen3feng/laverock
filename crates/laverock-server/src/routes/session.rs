//! Session state, LAN pairing and paired-device management.

use axum::{
    extract::{ConnectInfo, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use std::net::SocketAddr;

use super::{is_loopback, request_lang, token_from_headers, ApiError, AppState};
use crate::remote;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/session", get(session))
        .route("/api/pair/start", post(start_pairing))
        .route("/api/pair/claim", post(claim))
        .route("/api/devices", get(list_devices))
        .route("/api/devices/{id}", delete(revoke_device))
}

async fn session(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let device = token_from_headers(&headers).and_then(|t| state.remote.authorize(&t));
    let loopback = is_loopback(&peer);

    Json(serde_json::json!({
        "loopback": loopback,
        "paired": device.is_some(),
        "device_name": device.map(|d| d.name),
        // Machine-wide default UI language ("" = follow the browser);
        // clients may override it locally.
        "language": state.settings.get().language,
        "lan_enabled": state.remote.lan_enabled(),
        // Do not hand the network address to unpaired peers.
        "lan_url": if loopback && state.remote.lan_enabled() {
            remote::lan_url(state.port)
        } else {
            None
        },
    }))
}

async fn start_pairing(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    let Some(base) = remote::lan_url(state.port) else {
        return Err(ApiError::bad_request(
            laverock_core::i18n::Message::new(laverock_core::i18n::Key::LanUnavailable)
                .render(request_lang()),
        ));
    };
    let (code, expires_at) = state.remote.start_pairing();
    let url = format!("{base}/?pair={code}");
    let qr_svg = remote::make_qr_svg(&url).map_err(ApiError::internal)?;

    Ok(Json(serde_json::json!({
        "code": code,
        "url": url,
        "expires_at": expires_at,
        "qr_svg": qr_svg,
        "fingerprint": state.remote.fingerprint(),
    })))
}

#[derive(serde::Deserialize)]
struct ClaimRequest {
    code: String,
    #[serde(default)]
    device_name: String,
}

async fn claim(
    State(state): State<AppState>,
    Json(body): Json<ClaimRequest>,
) -> Result<Response, ApiError> {
    let token = state
        .remote
        .claim(&body.code, &body.device_name)
        .map_err(|message| ApiError {
            status: StatusCode::BAD_REQUEST,
            message: message.render(request_lang()),
        })?;

    let device = state.remote.authorize(&token);
    let cookie =
        format!("laverock_device={token}; Path=/; Max-Age=31536000; HttpOnly; SameSite=Lax");

    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({
            "paired": true,
            "device_name": device.map(|d| d.name),
        })),
    )
        .into_response())
}

async fn list_devices(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<Vec<remote::DeviceView>>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    Ok(Json(state.remote.devices()))
}

async fn revoke_device(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    state.remote.revoke(&id)?;
    Ok(StatusCode::NO_CONTENT)
}
