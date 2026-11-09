//! Process memory reporting for the resource panel.
//!
//! RSS is read without an extra dependency: Linux exposes it in procfs,
//! macOS through `ps`, and Windows through `tasklist`. When a value cannot
//! be read (process gone, restricted environment) the field is `None` and
//! the UI falls back to the loaded/unloaded state alone.

use laverock_core::EngineManager;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ProcessView {
    pub pid: u32,
    /// Engine id that owns the child process.
    pub engine: String,
    pub rss: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct SystemView {
    /// Resident set size of the server process itself, where in-process
    /// ONNX engines run.
    pub rss: Option<u64>,
    /// Resident child processes (audio.cpp servers).
    pub children: Vec<ProcessView>,
    /// Runtimes of the loaded engines, de-duplicated.
    pub backends: Vec<String>,
}

/// Snapshot of the server and every loaded engine's child process.
pub fn snapshot(manager: &EngineManager) -> SystemView {
    let mut children = Vec::new();
    let mut backends = Vec::new();
    for engine in manager.list() {
        if !engine.loaded {
            continue;
        }
        if !backends.contains(&engine.backend) {
            backends.push(engine.backend.clone());
        }
        if let Some(pid) = engine.pid {
            children.push(ProcessView {
                pid,
                engine: engine.id,
                rss: rss_bytes(pid),
            });
        }
    }
    SystemView {
        rss: server_rss(),
        children,
        backends,
    }
}

/// RSS of this server process.
pub fn server_rss() -> Option<u64> {
    rss_bytes(std::process::id())
}

/// Resident set size of `pid`, in bytes.
#[cfg(target_os = "linux")]
pub fn rss_bytes(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// RSS of `pid`, in bytes. macOS `ps` reports RSS in kilobytes.
#[cfg(target_os = "macos")]
pub fn rss_bytes(pid: u32) -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let kb: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    Some(kb * 1024)
}

/// RSS of `pid`, in bytes. `tasklist` reports the working set in kilobytes
/// as the last CSV column (`"12,345 K"`).
#[cfg(windows)]
pub fn rss_bytes(pid: u32) -> Option<u64> {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let field = text
        .split(',')
        .find(|field| field.trim().trim_matches('"').ends_with(" K"))?;
    let digits: String = field.chars().filter(char::is_ascii_digit).collect();
    let kb: u64 = digits.parse().ok()?;
    Some(kb * 1024)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn rss_bytes(_pid: u32) -> Option<u64> {
    None
}

/// Total physical memory, in bytes.
///
/// The catalog gates the largest music models on this: on Apple Silicon it
/// is the unified memory the GPU shares, and on a CUDA machine system RAM
/// is normally at least the VRAM budget. `None` (an unreadable value)
/// means no gate rather than blocking the install.
#[cfg(target_os = "macos")]
pub fn total_memory_bytes() -> Option<u64> {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(target_os = "linux")]
pub fn total_memory_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = meminfo.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// `wmic` is absent from some recent Windows builds; an unreadable total
/// simply disables the memory gate.
#[cfg(windows)]
pub fn total_memory_bytes() -> Option<u64> {
    let output = std::process::Command::new("wmic")
        .args(["ComputerSystem", "get", "TotalPhysicalMemory", "/value"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.starts_with("TotalPhysicalMemory="))?;
    line.split('=').nth(1)?.trim().parse().ok()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn total_memory_bytes() -> Option<u64> {
    None
}
