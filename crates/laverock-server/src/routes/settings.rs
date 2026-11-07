//! Machine-wide settings and LAN access toggling.

use axum::{
    extract::{ConnectInfo, State},
    routing::get,
    Json, Router,
};
use std::net::SocketAddr;

use super::{is_loopback, ApiError, AppState};
use crate::{
    proxy,
    remote::{self, LanCmd},
    settings::{self, Settings},
};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/settings/lan", get(get_lan).post(set_lan))
        .route("/api/settings", get(get_settings).post(set_settings))
}

#[derive(serde::Deserialize)]
struct LanSetting {
    enabled: bool,
}

async fn get_lan(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    Ok(Json(serde_json::json!({
        "enabled": state.remote.lan_enabled(),
        "url": if state.remote.lan_enabled() { remote::lan_url(state.port) } else { None },
        "fingerprint": state.remote.fingerprint(),
    })))
}

async fn set_lan(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<LanSetting>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }

    if body.enabled {
        let (tx, rx) = tokio::sync::oneshot::channel();
        state
            .lan
            .send(LanCmd::Start(tx))
            .await
            .map_err(|_| ApiError::internal("LAN supervisor stopped"))?;
        match rx
            .await
            .map_err(|_| ApiError::internal("LAN supervisor dropped"))?
        {
            Ok(info) => {
                state.remote.set_lan_enabled(true)?;
                state.remote.set_fingerprint(Some(info.fingerprint.clone()));
                return Ok(Json(serde_json::json!({
                    "enabled": true,
                    "url": info.url,
                    "fingerprint": info.fingerprint,
                })));
            }
            Err(e) => return Err(ApiError::bad_request(e)),
        }
    }

    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .lan
        .send(LanCmd::Stop(tx))
        .await
        .map_err(|_| ApiError::internal("LAN supervisor stopped"))?;
    let _ = rx.await;
    state.remote.set_lan_enabled(false)?;
    state.remote.set_fingerprint(None);

    Ok(Json(serde_json::json!({
        "enabled": false,
        "url": null,
    })))
}

async fn get_settings(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    let settings = state.settings.get();
    let system_proxy = match settings.proxy_mode {
        settings::ProxyMode::System => proxy::system(),
        settings::ProxyMode::Manual | settings::ProxyMode::Direct => None,
    };
    Ok(Json(serde_json::json!({
        "proxy_mode": settings.proxy_mode,
        "proxy": settings.proxy,
        "system_proxy": system_proxy,
        "hf_endpoint": settings.hf_endpoint,
        "pip_index": settings.pip_index,
        "workspace_dir": settings.workspace_dir,
        "language": settings.language,
        "models_dir": state.models.models_dir().display().to_string(),
        "tools_dir": state.models.tools_dir().map(|p| p.display().to_string()),
        "scripts_dir": state.models.scripts_dir().map(|p| p.display().to_string()),
    })))
}

async fn set_settings(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Settings>,
) -> Result<Json<Settings>, ApiError> {
    if !is_loopback(&peer) {
        return Err(ApiError::forbidden());
    }
    Ok(Json(state.settings.update(body)?))
}
