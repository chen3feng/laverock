//! HTTP surface, one module per feature.
//!
//! `main` owns process startup and the engine/data stores; this module owns
//! the state every handler shares, the auth and language middleware, and the
//! router that merges the feature modules. Adding a feature means adding a
//! module with a `router()` and one `merge` line below.

pub(crate) mod audio;
pub(crate) mod captures;
pub(crate) mod core;
pub(crate) mod history;
pub(crate) mod jobs;
pub(crate) mod models;
pub(crate) mod openai;
pub(crate) mod profiles;
pub(crate) mod projects;
pub(crate) mod session;
pub(crate) mod settings;
pub(crate) mod synth;
pub(crate) mod system;

use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, State},
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use laverock_core::{CoreError, EngineManager};
use std::{net::SocketAddr, sync::Arc};
use tracing::error;

use crate::{
    history::HistoryStore,
    jobs::JobStore,
    models::ModelService,
    profiles::ProfileStore,
    projects::ProjectStore,
    remote::{LanCmd, RemoteState},
    settings::SettingsStore,
};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) manager: Arc<EngineManager>,
    pub(crate) profiles: Arc<ProfileStore>,
    pub(crate) projects: Arc<ProjectStore>,
    pub(crate) history: Arc<HistoryStore>,
    pub(crate) remote: Arc<RemoteState>,
    pub(crate) settings: Arc<SettingsStore>,
    pub(crate) models: Arc<ModelService>,
    pub(crate) transcriber: Arc<laverock_core::Transcriber>,
    pub(crate) jobs: Arc<JobStore>,
    pub(crate) lan: tokio::sync::mpsc::Sender<LanCmd>,
    pub(crate) port: u16,
}

tokio::task_local! {
    /// UI language of the request being served (see `lang_middleware`).
    static REQUEST_LANG: laverock_core::i18n::Lang;
}

/// Language of the current request; English outside one.
pub(crate) fn request_lang() -> laverock_core::i18n::Lang {
    REQUEST_LANG
        .try_with(|lang| *lang)
        .unwrap_or(laverock_core::i18n::Lang::En)
}

/// Rendered subtitle file download (SRT or WebVTT).
pub(crate) fn subtitle_response(
    segments: &[laverock_core::TranscriptSegment],
    format: crate::subtitles::Format,
    stem: &str,
) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, format.content_type().to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{stem}.{}\"", format.extension()),
            ),
        ],
        format.render(segments),
    )
        .into_response()
}

/// One framed Server-Sent Event.
pub(crate) fn sse_frame(event: &str, data: &str) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
}

pub(crate) fn error_frame(message: &str) -> Bytes {
    sse_frame(
        "error",
        &serde_json::json!({ "message": message }).to_string(),
    )
}

/// Make the request's language available to handlers and error mapping.
async fn lang_middleware(req: Request<Body>, next: Next) -> Response {
    let lang = req
        .headers()
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| laverock_core::i18n::Lang::from_accept_language(Some(v)))
        // An explicit ?lang= wins (the SPA sends it).
        .or_else(|| {
            req.uri().query().and_then(|q| {
                q.split('&')
                    .find_map(|pair| pair.strip_prefix("lang="))
                    .map(laverock_core::i18n::Lang::parse)
            })
        })
        .unwrap_or_default();
    REQUEST_LANG.scope(lang, next.run(req)).await
}

/// Error type mapping core errors to HTTP.
#[derive(Debug)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) message: String,
}

impl ApiError {
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub(crate) fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: laverock_core::i18n::Message::new(laverock_core::i18n::Key::LoopbackOnly)
                .render(request_lang()),
        }
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        error!(error = %err, "io error");
        Self::internal(err.to_string())
    }
}

impl From<CoreError> for ApiError {
    fn from(err: CoreError) -> Self {
        match err {
            CoreError::NotFound(id) => Self {
                status: StatusCode::NOT_FOUND,
                message: format!("not found: {id}"),
            },
            CoreError::Wav(msg) => Self {
                status: StatusCode::BAD_REQUEST,
                message: msg,
            },
            CoreError::BadRequest(msg) => Self {
                status: StatusCode::BAD_REQUEST,
                message: msg,
            },
            CoreError::UserError(message) => Self {
                status: StatusCode::BAD_REQUEST,
                message: message.render(request_lang()),
            },
            other => {
                error!(error = %other, "request failed");
                Self {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    message: other.to_string(),
                }
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            axum::Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

/// Paths a browser may reach before pairing: the SPA itself and the
/// endpoints needed to pair.
fn is_public_path(path: &str) -> bool {
    matches!(
        path,
        "/" | "/api/session" | "/api/pair/claim" | "/favicon.ico"
    )
}

pub(crate) fn token_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        return Some(token.trim().to_string());
    }
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        part.trim()
            .strip_prefix("laverock_device=")
            .map(|v| v.trim().to_string())
    })
}

/// Loopback is trusted; LAN requests need a paired device token, either
/// as `Authorization: Bearer …` or as the pairing cookie (needed for
/// `<audio src>` and EventSource, which cannot set headers).
async fn auth_middleware(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if peer.ip().is_loopback() || is_public_path(req.uri().path()) {
        return next.run(req).await;
    }

    let authorized = token_from_headers(req.headers())
        .and_then(|token| state.remote.authorize(&token))
        .is_some();

    if authorized {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(
                serde_json::json!({ "error": laverock_core::i18n::Message::new(
                laverock_core::i18n::Key::DeviceUnpaired,
            ).render(request_lang()) }),
            ),
        )
            .into_response()
    }
}

pub(crate) fn is_loopback(peer: &SocketAddr) -> bool {
    peer.ip().is_loopback()
}

/// Every feature router, wrapped in the shared middleware layers.
pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .merge(core::router())
        .merge(synth::router())
        .merge(profiles::router())
        .merge(captures::router())
        .merge(projects::router())
        .merge(history::router())
        .merge(audio::router())
        .merge(session::router())
        .merge(settings::router())
        .merge(models::router())
        .merge(openai::router())
        .merge(jobs::router())
        .merge(system::router())
        .merge(crate::tools::router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        // Outermost: every handler and error mapping sees the request language.
        .layer(middleware::from_fn(lang_middleware))
        .with_state(state)
}
