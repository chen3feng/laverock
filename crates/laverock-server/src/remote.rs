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
use tokio::{sync::oneshot, task::JoinHandle};

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

/// Information returned when the LAN listener starts.
#[derive(Debug, Clone, Serialize)]
pub struct LanInfo {
    pub url: String,
    /// SHA-256 of the certificate DER, uppercase colon-separated.
    pub fingerprint: String,
}

pub struct RemoteState {
    path: PathBuf,
    config: Mutex<RemoteConfig>,
    pair: Mutex<Option<PairSession>>,
    fingerprint: Mutex<Option<String>>,
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
            fingerprint: Mutex::new(None),
        })
    }

    pub fn fingerprint(&self) -> Option<String> {
        self.fingerprint.lock().expect("fingerprint lock").clone()
    }

    pub fn set_fingerprint(&self, value: Option<String>) {
        *self.fingerprint.lock().expect("fingerprint lock") = value;
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
    Start(oneshot::Sender<Result<LanInfo, String>>),
    Stop(oneshot::Sender<()>),
}

/// Owns the optional LAN listener. The router is moved in here so it can
/// be cloned for each listener without creating a state cycle.
///
/// The LAN listener speaks HTTPS with a self-signed certificate whose SAN
/// covers the current LAN address; browsers require a secure context for
/// microphone access and PWA installation.
pub async fn lan_supervisor(
    mut rx: tokio::sync::mpsc::Receiver<LanCmd>,
    app: axum::Router,
    port: u16,
    tls_dir: PathBuf,
) {
    use axum_server::Handle;

    let mut current: Option<(SocketAddr, axum_server::Handle<SocketAddr>, JoinHandle<()>)> = None;

    while let Some(cmd) = rx.recv().await {
        match cmd {
            LanCmd::Start(reply) => {
                if let Some((addr, ..)) = &current {
                    let fingerprint = fingerprint_for(&tls_dir, addr.ip()).unwrap_or_default();
                    let _ = reply.send(Ok(LanInfo {
                        url: format!("https://{addr}"),
                        fingerprint,
                    }));
                    continue;
                }
                let Some(ip) = discover_lan_ip() else {
                    let _ = reply.send(Err("无法获取本机局域网地址".into()));
                    continue;
                };
                let addr = SocketAddr::new(ip, port);

                let (cert, key, fingerprint) = match ensure_cert(&tls_dir, ip) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = reply.send(Err(format!("生成证书失败: {e}")));
                        continue;
                    }
                };
                rustls::crypto::ring::default_provider()
                    .install_default()
                    .ok();
                let tls = match axum_server::tls_rustls::RustlsConfig::from_pem(cert, key).await {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = reply.send(Err(format!("加载证书失败: {e}")));
                        continue;
                    }
                };

                let handle = Handle::new();
                let server_handle = handle.clone();
                let app = app.clone();
                let join = tokio::spawn(async move {
                    let _ = axum_server::bind_rustls(addr, tls)
                        .handle(server_handle)
                        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                        .await;
                });

                tracing::info!(%addr, "LAN listener started (https)");
                current = Some((addr, handle, join));
                let _ = reply.send(Ok(LanInfo {
                    url: format!("https://{addr}"),
                    fingerprint,
                }));
            }
            LanCmd::Stop(reply) => {
                if let Some((addr, handle, join)) = current.take() {
                    tracing::info!(%addr, "LAN listener stopped");
                    handle.graceful_shutdown(Some(Duration::from_secs(1)));
                    let _ = join.await;
                }
                let _ = reply.send(());
            }
        }
    }
}

/// Generate (or reuse) a self-signed certificate for `ip`.
fn ensure_cert(dir: &Path, ip: IpAddr) -> Result<(Vec<u8>, Vec<u8>, String), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let ip_path = dir.join("ip.txt");

    let same_ip = fs::read_to_string(&ip_path)
        .map(|s| s.trim() == ip.to_string())
        .unwrap_or(false);
    if same_ip && cert_path.is_file() && key_path.is_file() {
        if let Ok(cert) = fs::read(&cert_path) {
            if let Ok(key) = fs::read(&key_path) {
                let fingerprint = fingerprint_pem(&cert);
                return Ok((cert, key, fingerprint));
            }
        }
    }

    let mut params =
        rcgen::CertificateParams::new(vec!["localhost".to_string()]).map_err(|e| e.to_string())?;
    params.subject_alt_names.push(rcgen::SanType::IpAddress(ip));
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, format!("Laverock on {ip}"));
    let key_pair = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key_pair).map_err(|e| e.to_string())?;

    let cert_pem = cert.pem().into_bytes();
    let key_pem = key_pair.serialize_pem().into_bytes();
    let fingerprint = fingerprint_der(cert.der());

    fs::write(&cert_path, &cert_pem).map_err(|e| e.to_string())?;
    fs::write(&key_path, &key_pem).map_err(|e| e.to_string())?;
    fs::write(&ip_path, ip.to_string()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600));
    }

    Ok((cert_pem, key_pem, fingerprint))
}

fn fingerprint_for(dir: &Path, ip: IpAddr) -> Option<String> {
    let same_ip = fs::read_to_string(dir.join("ip.txt"))
        .map(|s| s.trim() == ip.to_string())
        .unwrap_or(false);
    if !same_ip {
        return None;
    }
    fs::read(dir.join("cert.pem"))
        .ok()
        .map(|pem| fingerprint_pem(&pem))
}

/// SHA-256 of the certificate DER, formatted as `AA:BB:…`.
fn fingerprint_der(der: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(der);
    fingerprint_hex(&hasher.finalize())
}

/// Best effort for a PEM file: hash the decoded base64 body.
fn fingerprint_pem(pem: &[u8]) -> String {
    let text = String::from_utf8_lossy(pem);
    let body: String = text.lines().filter(|l| !l.starts_with("-----")).collect();
    match base64_decode(&body) {
        Some(der) => fingerprint_der(&der),
        None => fingerprint_der(pem),
    }
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for c in input.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = TABLE.iter().position(|&t| t == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

fn fingerprint_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(':');
        }
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// Base URL the phone should use, if a LAN address can be determined.
pub fn lan_url(port: u16) -> Option<String> {
    discover_lan_ip().map(|ip| format!("https://{ip}:{port}"))
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
