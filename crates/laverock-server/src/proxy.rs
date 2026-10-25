//! Discover the operating system's proxy settings.
//!
//! Laverock's default download mode is "follow the system", the same proxy
//! the user's browser uses. An exported `HTTPS_PROXY`/`ALL_PROXY` wins
//! (that is what most terminals, CI and Linux desktops use); otherwise we
//! read the platform's own store:
//!
//! - macOS: `scutil --proxy`
//! - Windows: the Internet Settings registry keys used by WinINET
//! - other platforms: environment only
//!
//! PAC scripts are not evaluated. If one is configured the user can pick
//! "手动指定" and type the proxy directly.

#[cfg(any(target_os = "macos", windows))]
use std::process::Command;

/// Environment variable (either case) that specifies a proxy, in
/// precedence order.
const ENV_KEYS: &[&str] = &[
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "HTTP_PROXY",
    "http_proxy",
];

/// Proxy for the current user, or `None` for a direct connection.
pub fn system() -> Option<String> {
    from_env().or_else(platform)
}

/// Proxy explicitly exported in the environment, if any.
pub fn from_env() -> Option<String> {
    for key in ENV_KEYS {
        if let Some(value) = std::env::var_os(key) {
            let value = value.to_string_lossy();
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn platform() -> Option<String> {
    let output = Command::new("scutil").arg("--proxy").output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_scutil(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(target_os = "macos")]
fn parse_scutil(text: &str) -> Option<String> {
    let mut values = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        // Entries are `Key : Value`; array members look like `0 : host`.
        let Some((key, value)) = line.split_once(" : ") else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        values.insert(key, value.trim());
    }

    let enabled = |key: &str| values.get(key).is_some_and(|v| *v == "1");
    let endpoint = |host: &str, port: &str| {
        let host = values.get(host)?;
        Some(match values.get(port) {
            Some(port) => format!("{host}:{port}"),
            None => (*host).to_string(),
        })
    };

    if enabled("HTTPSEnable") {
        if let Some(addr) = endpoint("HTTPSProxy", "HTTPSPort") {
            return Some(format!("http://{addr}"));
        }
    }
    if enabled("HTTPEnable") {
        if let Some(addr) = endpoint("HTTPProxy", "HTTPPort") {
            return Some(format!("http://{addr}"));
        }
    }
    if enabled("SOCKSEnable") {
        if let Some(addr) = endpoint("SOCKSProxy", "SOCKSPort") {
            return Some(format!("socks5h://{addr}"));
        }
    }
    None
}

#[cfg(windows)]
fn platform() -> Option<String> {
    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    let enabled = reg_query(KEY, "ProxyEnable")?;
    if enabled.trim() != "0x1" {
        return None;
    }
    let server = reg_query(KEY, "ProxyServer")?;
    let server = server.trim();
    if server.is_empty() {
        return None;
    }
    // Either `host:port` or `http=host:port;https=host:port`.
    let addr = if server.contains('=') {
        server
            .split(';')
            .filter_map(|part| part.split_once('='))
            .find(|(scheme, _)| *scheme == "https")
            .or_else(|| {
                server
                    .split(';')
                    .filter_map(|part| part.split_once('='))
                    .next()
            })
            .map(|(_, addr)| addr)
            .unwrap_or(server)
    } else {
        server
    };
    if addr.contains("://") {
        Some(addr.to_string())
    } else {
        Some(format!("http://{addr}"))
    }
}

#[cfg(windows)]
fn reg_query(key: &str, name: &str) -> Option<String> {
    let output = Command::new("reg")
        .args(["query", key, "/v", name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // `    ProxyServer    REG_SZ    127.0.0.1:7890`
    text.lines()
        .find(|line| line.contains(name))
        .and_then(|line| line.split_whitespace().last())
        .map(std::string::ToString::to_string)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform() -> Option<String> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::parse_scutil;

    #[test]
    fn prefers_https_proxy() {
        let text = "<dictionary> {
  HTTPEnable : 1
  HTTPPort : 10808
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 1
  HTTPSPort : 10809
  HTTPSProxy : 127.0.0.1
  ExceptionsList : <array> {
    0 : localhost
    1 : 127.0.0.0/8
  }
  SOCKSEnable : 1
  SOCKSPort : 10808
  SOCKSProxy : 127.0.0.1
}";
        assert_eq!(
            parse_scutil(text).as_deref(),
            Some("http://127.0.0.1:10809")
        );
    }

    #[test]
    fn falls_back_to_socks() {
        let text = "<dictionary> {
  HTTPEnable : 0
  HTTPSEnable : 0
  SOCKSEnable : 1
  SOCKSPort : 7890
  SOCKSProxy : 127.0.0.1
}";
        assert_eq!(
            parse_scutil(text).as_deref(),
            Some("socks5h://127.0.0.1:7890")
        );
    }

    #[test]
    fn no_proxy_configured() {
        let text = "<dictionary> {\n  HTTPEnable : 0\n  HTTPSEnable : 0\n  SOCKSEnable : 0\n}";
        assert_eq!(parse_scutil(text), None);
    }
}
