//! Laverock server: thin HTTP control plane over laverock-core.
//!
//! MVP surface (loopback-only by default):
//!   GET  /                    embedded web UI
//!   GET  /api/health          service + engine count
//!   GET  /api/engines         registered engines
//!   POST /api/engines/:id/synthesize   multipart: text + prompt/profile -> WAV
//!   GET  /api/profiles        saved voices
//!   POST /api/profiles        multipart: name + prompt_text + reference WAV
//!   GET  /api/profiles/:id/audio        reference clip
//!   DELETE /api/profiles/:id
//!
//! Phone/LAN access (pairing, TLS, device tokens) comes later; until
//! then the server binds loopback only.
//!
//! Route handlers live in [`routes`], one module per feature; this file
//! only wires the stores together and starts the listener.

// Release builds run headless as a desktop sidecar: no console window.
// Debug builds keep the console so the tracing logs stay visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod captures;
mod export;
mod history;
mod jobs;
mod models;
mod openai_compat;
mod profiles;
mod proxy;
mod reference;
mod remote;
mod routes;
mod settings;
mod subtitles;
mod system;

pub(crate) use routes::AppState;

use laverock_core::EngineManager;
use models::ModelService;
use profiles::ProfileStore;
use remote::{LanCmd, RemoteState};
use settings::SettingsStore;
use std::{net::SocketAddr, sync::Arc};

/// tracing emits ANSI color codes by default, but the legacy Windows console
/// (conhost) keeps virtual-terminal processing off, so they show up as raw
/// `\x1b[32m…` sequences. Enable it so the colors actually render. A no-op
/// when stdout is not a console (release GUI builds, or redirected output).
#[cfg(windows)]
fn enable_ansi_colors() {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        STD_OUTPUT_HANDLE,
    };

    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0u32;
        if GetConsoleMode(handle, &mut mode) != 0 {
            let _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    }
}

#[tokio::main]
async fn main() {
    #[cfg(windows)]
    enable_ansi_colors();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,laverock_server=debug".into()),
        )
        .init();

    let host = std::env::var("LAVEROCK_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("LAVEROCK_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8787);
    let data_dir = std::env::var("LAVEROCK_DATA_DIR").unwrap_or_else(|_| "data".to_string());

    // Settings are read from the data directory (which stays put, so it can
    // never chase its own tail). An explicit workspace root wins over the
    // launcher's `LAVEROCK_MODELS_DIR`/`LAVEROCK_TOOLS_DIR`: one switch to
    // move the multi-GB models and venvs to a bigger drive, shared by the
    // desktop app and the standalone server alike.
    let settings = Arc::new(SettingsStore::open(&data_dir).expect("open settings"));
    let stored = settings.get();
    let workspace = stored.workspace_paths();
    // Several models directories may be layered: the first that provides an
    // engine id wins and new installs go to the first. `split_paths` uses the
    // platform separator (`:` on Unix, `;` on Windows).
    let models_dirs: Vec<std::path::PathBuf> = match &workspace {
        Some((models, _)) => vec![models.clone()],
        None => {
            let raw = std::env::var("LAVEROCK_MODELS_DIR").unwrap_or_else(|_| "models".into());
            let paths: Vec<_> = std::env::split_paths(&raw).collect();
            if paths.is_empty() {
                vec![std::path::PathBuf::from("models")]
            } else {
                paths
            }
        }
    };
    let tools_dir: Option<std::path::PathBuf> = match workspace {
        Some((_, tools)) => Some(tools),
        None => std::env::var_os("LAVEROCK_TOOLS_DIR").map(std::path::PathBuf::from),
    };

    let manager = Arc::new(EngineManager::open_all(models_dirs.clone()).expect("scan models dir"));
    let profiles = Arc::new(ProfileStore::open(&data_dir).expect("open data dir"));
    // Voices saved before the recording library existed become one-take
    // voices. Idempotent, but only pay for it when there is something to do.
    let migrated = profiles.migrate_captures();
    if migrated > 0 {
        tracing::info!("migrated {migrated} saved voices into the recording library");
    }
    let history = Arc::new(history::HistoryStore::open(&data_dir).expect("open history"));
    let jobs = Arc::new(jobs::JobStore::open(std::path::Path::new(&data_dir)).expect("open jobs"));
    let remote = Arc::new(RemoteState::open(&data_dir).expect("open remote state"));
    let scripts_dir = models::resolve_scripts_dir();
    let model_service = Arc::new(ModelService::new_multi(
        models_dirs.clone(),
        tools_dir.clone(),
        scripts_dir.clone(),
        settings.clone(),
        manager.clone(),
    ));
    tracing::info!(
        dirs = ?models_dirs,
        tools = ?tools_dir,
        engines = manager.len(),
        data = %data_dir,
        workspace = %stored.workspace_dir,
        scripts = ?scripts_dir,
        "engine registry ready"
    );
    tracing::info!(proxy = ?proxy::system(), "download proxy");

    let (lan_tx, lan_rx) = tokio::sync::mpsc::channel(8);
    let boot_lan = lan_tx.clone();
    let state = AppState {
        manager,
        profiles,
        history,
        remote: remote.clone(),
        settings,
        models: model_service,
        transcriber: Arc::new(laverock_core::Transcriber::new_all(models_dirs.clone())),
        jobs,
        lan: lan_tx,
        port,
    };

    let app = routes::router(state);

    // The supervisor owns the router so the LAN listener can be started
    // and stopped at runtime.
    let lan_supervisor = tokio::spawn(remote::lan_supervisor(
        lan_rx,
        app.clone(),
        port,
        std::path::PathBuf::from(&data_dir).join("tls"),
    ));

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("valid LAVEROCK_HOST:PORT");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    tracing::info!("Laverock listening on http://{addr}");

    if remote.lan_enabled() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if boot_lan.send(LanCmd::Start(tx)).await.is_ok() {
            match rx.await {
                Ok(Ok(info)) => {
                    remote.set_fingerprint(Some(info.fingerprint.clone()));
                    tracing::info!(url = %info.url, "LAN access enabled");
                }
                Ok(Err(e)) => tracing::warn!("LAN access enabled but listener failed: {e}"),
                Err(_) => tracing::warn!("LAN supervisor dropped the start request"),
            }
        }
    }

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
    })
    .await
    .expect("server error");

    lan_supervisor.abort();
}
