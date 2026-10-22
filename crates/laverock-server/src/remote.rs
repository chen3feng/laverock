//! LAN access: pairing, device tokens, and a second listener.
//!
//! Threat model: loopback requests are trusted (same as a local desktop
//! app talking to its own daemon). Requests arriving on the LAN listener
//! must carry a device token obtained through a one-time pairing code
//! that only the desktop UI can create.
//!
//! The LAN listener is not opened until the user enables it, and it
//! binds the machine's LAN address rather than 0.0.0.0 so the loopback
//! listener stays separate.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::ErrorKind,
    net::{IpAddr, SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

const PAIR_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_PAIR_FAILURES: u32 = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// Hex SHA-256 of the bearer token; the token itself is never stored.
    token_sha256: String,
    pub created_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RemoteConfig {
    #[serde(default)]
    lan_enabled: bool,
    #[serde(default)]
    devices: Vec<Device>,
}

struct PairSession {
    code: String,
    expires: SystemTime,
    failures: u32,
}

/// Device as exposed to the desktop UI; the token hash never leaves the
/// server.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub created_at: u64,
}

impl From<&Device> for DeviceView {
    fn from(d: &Device) -> Self {
        Self {
            id: d.id.clone(),
            name: d.name.clone(),
            created_at: d.created_at,
        }
    }
}

pub struct RemoteState {
    path: PathBuf,
    config: Mutex<RemoteConfig>,
    pair: Mutex<Option<PairSession>>,
}

impl RemoteState {
    pub fn open(data_dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = data_dir.as_ref().join("remote.json");
        let config = match fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(e) if e.kind() == ErrorKind::NotFound => RemoteConfig::default(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path,
            config: Mutex::new(config),
            pair: Mutex::new(None),
        })
    }

    pub fn lan_enabled(&self) -> bool {
        self.config.lock().expect("remote lock").lan_enabled
    }

    pub fn set_lan_enabled(&self, enabled: bool) -> std::io::Result<()> {
        let mut config = self.config.lock().expect("remote lock");
        config.lan_enabled = enabled;
        save(&self.path, &config)
    }

    pub fn devices(&self) -> Vec<DeviceView> {
        self.config
            .lock()
            .expect("remote lock")
            .devices
            .iter()
            .map(DeviceView::from)
            .collect()
    }

    pub fn revoke(&self, id: &str) -> std::io::Result<()> {
        let mut config = self.config.lock().expect("remote lock");
        config.devices.retain(|d| d.id != id);
        save(&self.path, &config)
    }

    /// Create a one-time pairing code, replacing any previous one.
    pub fn start_pairing(&self) -> (String, u64) {
        let code = format!("{:06}", random_u32() % 1_000_000);
        let expires = SystemTime::now() + PAIR_TTL;
        *self.pair.lock().expect("pair lock") = Some(PairSession {
            code: code.clone(),
            expires,
            failures: 0,
        });
        let epoch = expires
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        (code, epoch)
    }

    /// Exchange a pairing code for a device token. Returns the plaintext
    /// token once; only its hash is persisted.
    pub fn claim(&self, code: &str, device_name: &str) -> Result<String, String> {
        let mut guard = self.pair.lock().expect("pair lock");
        let session = guard.as_mut().ok_or("没有进行中的配对")?;
        if SystemTime::now() > session.expires {
            *guard = None;
            return Err("配对码已过期，请在电脑上重新生成".into());
        }
        if session.code != code.trim() {
            session.failures += 1;
            if session.failures >= MAX_PAIR_FAILURES {
                *guard = None;
                return Err("尝试次数过多，请重新生成配对码".into());
            }
            return Err("配对码不正确".into());
        }
        let code = session.code.clone();
        drop(guard);

        let name = sanitize_name(device_name);
        let token = random_hex(32);
        let device = Device {
            id: format!("d{}-{}", now_secs(), random_hex(4)),
            name,
            token_sha256: hash_token(&token),
            created_at: now_secs(),
        };

        {
            let mut config = self.config.lock().expect("remote lock");
            config.devices.push(device);
            save(&self.path, &config).map_err(|e| format!("保存设备失败: {e}"))?;
        }
        *self.pair.lock().expect("pair lock") = None;
        tracing::info!(code_len = code.len(), "device paired");
        Ok(token)
    }

    pub fn authorize(&self, token: &str) -> Option<Device> {
        let digest = hash_token(token);
        self.config
            .lock()
            .expect("remote lock")
            .devices
            .iter()
            .find(|d| d.token_sha256 == digest)
            .cloned()
    }
}

pub enum LanCmd {
    Start(oneshot::Sender<Result<String, String>>),
    Stop(oneshot::Sender<()>),
}

/// Owns the optional LAN listener. The router is moved in here so it can
/// be cloned for each listener without creating a state cycle.
pub async fn lan_supervisor(
    mut rx: tokio::sync::mpsc::Receiver<LanCmd>,
    app: axum::Router,
    port: u16,
) {
    let mut current: Option<(SocketAddr, oneshot::Sender<()>, JoinHandle<()>)> = None;

    while let Some(cmd) = rx.recv().await {
        match cmd {
            LanCmd::Start(reply) => {
                if let Some((addr, ..)) = &current {
                    let _ = reply.send(Ok(format!("http://{addr}")));
                    continue;
                }
                let Some(ip) = discover_lan_ip() else {
                    let _ = reply.send(Err("无法获取本机局域网地址".into()));
                    continue;
                };
                let addr = SocketAddr::new(ip, port);
                match TcpListener::bind(addr).await {
                    Ok(listener) => {
                        tracing::info!(%addr, "LAN listener started");
                        let (shutdown_tx, shutdown_rx) = oneshot::channel();
                        let app = app.clone();
                        let handle = tokio::spawn(async move {
                            let _ = axum::serve(
                                listener,
                                app.into_make_service_with_connect_info::<SocketAddr>(),
                            )
                            .with_graceful_shutdown(async {
                                let _ = shutdown_rx.await;
                            })
                            .await;
                        });
                        current = Some((addr, shutdown_tx, handle));
                        let _ = reply.send(Ok(format!("http://{addr}")));
                    }
                    Err(e) => {
                        tracing::warn!(%addr, error = %e, "cannot bind LAN listener");
                        let _ = reply.send(Err(format!("无法监听 {addr}: {e}")));
                    }
                }
            }
            LanCmd::Stop(reply) => {
                if let Some((addr, tx, handle)) = current.take() {
                    tracing::info!(%addr, "LAN listener stopped");
                    let _ = tx.send(());
                    let _ = handle.await;
                }
                let _ = reply.send(());
            }
        }
    }
}

/// Base URL the phone should use, if a LAN address can be determined.
pub fn lan_url(port: u16) -> Option<String> {
    discover_lan_ip().map(|ip| format!("http://{ip}:{port}"))
}

pub fn make_qr_svg(text: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(220, 220)
        .dark_color(qrcode::render::svg::Color("#0e0f13"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

fn discover_lan_ip() -> Option<IpAddr> {
    // UDP connect performs no traffic but makes the OS pick the outgoing
    // interface and exposes its address.
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    let ip = addr.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        None
    } else {
        Some(ip)
    }
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    to_hex(&hasher.finalize())
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    if getrandom::fill(&mut buf).is_err() {
        // Extremely unlikely; fall back to a time-derived value rather
        // than panicking inside a request.
        let seed = now_secs() ^ (std::process::id() as u64);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = ((seed >> ((i % 8) * 8)) & 0xff) as u8;
        }
    }
    buf
}

fn random_hex(len: usize) -> String {
    to_hex(&random_bytes(len))
}

fn random_u32() -> u32 {
    let b = random_bytes(4);
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sanitize_name(name: &str) -> String {
    let trimmed: String = name.trim().chars().take(40).collect();
    if trimmed.is_empty() {
        "未命名设备".to_string()
    } else {
        trimmed
    }
}

fn save(path: &Path, config: &RemoteConfig) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(config)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(tmp, path)
}
