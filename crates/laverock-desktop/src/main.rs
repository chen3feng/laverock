//! Laverock desktop shell.
//!
//! The shell owns almost nothing: it supervises the local server process,
//! opens a window pointed at that server, and adds the OS-level pieces a
//! browser tab cannot provide — tray menu, hide-on-close, autostart.
//!
//! The UI is the exact same SPA the browser (and later the phone) uses,
//! so there is no second frontend to keep in sync.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread::sleep,
    time::{Duration, Instant},
};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, RunEvent, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

const DEFAULT_URL: &str = "http://127.0.0.1:8787";

struct ServerProcess(Mutex<Option<Child>>);

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(ServerProcess(Mutex::new(None)))
        .setup(|app| {
            let base_url = env::var("LAVEROCK_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
            let addr = parse_addr(&base_url).unwrap_or_else(|| "127.0.0.1:8787".parse().unwrap());

            if !http_ok(addr) {
                match spawn_server(app) {
                    Ok(child) => {
                        let state = app.state::<ServerProcess>();
                        *state.0.lock().expect("server lock") = Some(child);
                    }
                    Err(e) => eprintln!("cannot start laverock-server: {e}"),
                }
            }

            let ready = wait_for_server(addr, Duration::from_secs(20));
            let target = if ready {
                match base_url.parse() {
                    Ok(url) => WebviewUrl::External(url),
                    Err(_) => WebviewUrl::App("index.html".into()),
                }
            } else {
                eprintln!("server did not become ready; showing help page");
                WebviewUrl::App("index.html".into())
            };

            let window = WebviewWindowBuilder::new(app, "main", target)
                .title("Laverock")
                .inner_size(1040.0, 800.0)
                .min_inner_size(400.0, 560.0)
                .build()?;

            // Closing the window hides it; quitting happens from the tray.
            let handle = window.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = handle.hide();
                }
            });

            build_tray(app)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Laverock deskop app")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                if let Some(mut child) = app
                    .state::<ServerProcess>()
                    .0
                    .lock()
                    .expect("server lock")
                    .take()
                {
                    let _ = child.kill();
                }
            }
        });
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);

    let show = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "开机启动",
        true,
        autostart_on,
        None::<&str>,
    )?;
    let data = MenuItem::with_id(app, "data", "打开数据目录", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &autostart, &data, &quit])?;

    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "autostart" => {
                let ext = app.autolaunch();
                let enabled = ext.is_enabled().unwrap_or(false);
                let _ = if enabled { ext.disable() } else { ext.enable() };
            }
            "data" => {
                if let Ok(dir) = app.path().app_data_dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    open_path(&dir);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        });

    if let Ok(icon) = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png")) {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

fn open_path(path: &Path) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let _ = Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "linux")]
    let _ = Command::new("xdg-open").arg(path).spawn();
}

/// Start `laverock-server`, pointing it at the model and data directories.
fn spawn_server(app: &tauri::App) -> Result<Child, String> {
    let bin = server_binary().ok_or_else(|| {
        "laverock-server not found (build it with `cargo build -p laverock-server`, \
         or set LAVEROCK_SERVER_BIN)"
            .to_string()
    })?;

    let data_dir = env::var("LAVEROCK_DATA_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| default_data_dir(app))
        .unwrap_or_else(|| PathBuf::from("data"));
    let _ = std::fs::create_dir_all(&data_dir);

    let mut cmd = Command::new(&bin);
    cmd.env("LAVEROCK_DATA_DIR", &data_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if env::var_os("LAVEROCK_MODELS_DIR").is_none() {
        let models = find_models_dir(&bin).unwrap_or_else(|| {
            let dir = data_dir.join("models");
            let _ = std::fs::create_dir_all(&dir);
            dir
        });
        cmd.env("LAVEROCK_MODELS_DIR", models);
    }
    if let Some(parent) = bin.parent() {
        cmd.current_dir(parent);
    }

    cmd.spawn()
        .map_err(|e| format!("spawn {}: {e}", bin.display()))
}

/// Development builds share the repository's `data/` directory with the
/// standalone server; release builds keep user data in the OS app-data
/// directory.
fn default_data_dir(app: &tauri::App) -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        return Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data"));
    }
    app.path().app_data_dir().ok()
}

fn server_binary() -> Option<PathBuf> {
    if let Ok(p) = env::var("LAVEROCK_SERVER_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
        eprintln!(
            "LAVEROCK_SERVER_BIN does not point to a file: {}",
            p.display()
        );
    }

    let exe = env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let ext = if cfg!(windows) { ".exe" } else { "" };

    let mut candidates = Vec::new();

    // Bundled next to the app binary (ad-hoc layouts and development).
    candidates.push(exe_dir.join(format!("laverock{ext}")));
    candidates.push(exe_dir.join("../Resources").join(format!("laverock{ext}")));

    // Tauri sidecar: the CLI appends the target triple to the file name.
    if let Some(triple) = option_env!("TAURI_ENV_TARGET_TRIPLE") {
        candidates.push(exe_dir.join(format!("laverock-{triple}{ext}")));
        candidates.push(
            exe_dir
                .join("../Resources")
                .join(format!("laverock-{triple}{ext}")),
        );
    }
    // Fallback: any `laverock-<triple>` sibling (except the shell itself).
    if let Ok(entries) = std::fs::read_dir(exe_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(ext).unwrap_or(&name);
            if stem.starts_with("laverock-") && stem != "laverock-desktop" && entry.path().is_file()
            {
                candidates.push(entry.path());
            }
        }
    }

    // Development: <workspace>/target/{debug,release}/laverock
    for profile in ["debug", "release"] {
        candidates.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join(profile)
                .join(format!("laverock{ext}")),
        );
    }

    candidates.into_iter().find(|p| p.is_file())
}

/// Locate the `models` directory relative to the server binary, falling
/// back to the workspace layout used in development.
fn find_models_dir(bin: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = bin.parent() {
        candidates.push(dir.join("models"));
        candidates.push(dir.join("../../models"));
        candidates.push(dir.join("../Resources/models"));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../models")
            .to_path_buf(),
    );
    candidates.into_iter().find(|p| p.is_dir())
}

fn parse_addr(url: &str) -> Option<SocketAddr> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let authority = rest.split('/').next()?;
    let host_port = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:8787")
    };
    host_port.parse().ok()
}

fn http_ok(addr: SocketAddr) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(400)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let _ = stream.write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.starts_with("HTTP/1.0 200") || response.starts_with("HTTP/1.1 200")
}

fn wait_for_server(addr: SocketAddr, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if http_ok(addr) {
            return true;
        }
        sleep(Duration::from_millis(150));
    }
    false
}
