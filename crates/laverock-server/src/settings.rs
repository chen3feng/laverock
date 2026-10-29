//! User-editable download settings: proxy and package mirrors.
//!
//! Persisted as `<data>/settings.json` and applied to the environment of
//! every model download script. By default downloads follow the system
//! browser proxy; the user can override the URL or force a direct
//! connection. An empty mirror means "use the official endpoint", so a
//! fresh install behaves exactly like running the scripts by hand.

use laverock_core::i18n::{self, Key};
use laverock_core::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    /// Follow the operating system's browser proxy (the default).
    #[default]
    System,
    /// Use the URL in [`Settings::proxy`].
    Manual,
    /// Never use a proxy, even if one is exported in the environment.
    Direct,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub proxy_mode: ProxyMode,
    /// Proxy URL used in [`ProxyMode::Manual`], e.g.
    /// `http://127.0.0.1:10808`.
    pub proxy: String,
    /// Hugging Face endpoint; empty means `https://huggingface.co`.
    pub hf_endpoint: String,
    /// pip index used to build the Qwen3-TTS venv; empty means PyPI.
    pub pip_index: String,
    /// Optional root for the model and tool (venv) directories. Empty
    /// means the launcher's defaults (app-data for the desktop app,
    /// `models/` + `.tools/` for the standalone server). When set, models
    /// live in `<workspace_dir>/models` and venvs in `<workspace_dir>/tools`
    /// — one switch to move the multi-GB assets to a bigger drive. The
    /// server is authoritative: it overrides `LAVEROCK_MODELS_DIR`/
    /// `LAVEROCK_TOOLS_DIR` from the environment. Profiles/history stay in
    /// the data directory (next to this file), so it never chases itself.
    pub workspace_dir: String,
    /// UI language: "" follows the browser, otherwise a code such as
    /// `zh` or `en`. Only the web UI interprets it.
    pub language: String,
}

impl Settings {
    /// Environment variables handed to download scripts. Empty values
    /// are omitted so the scripts' own defaults apply. `system_proxy` is
    /// the proxy discovered by [`crate::proxy::system`] and is used only
    /// in [`ProxyMode::System`].
    pub fn download_env(&self, system_proxy: Option<&str>) -> Vec<(String, String)> {
        let mut env = Vec::new();
        let proxy = match self.proxy_mode {
            ProxyMode::System => system_proxy,
            ProxyMode::Manual => Some(self.proxy.as_str()).filter(|p| !p.is_empty()),
            ProxyMode::Direct => None,
        };
        if let Some(proxy) = proxy {
            // curl and git disagree about case; set both spellings.
            for key in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ] {
                env.push((key.to_string(), proxy.to_string()));
            }
            for key in ["NO_PROXY", "no_proxy"] {
                env.push((key.to_string(), no_proxy_value()));
            }
        }
        if !self.hf_endpoint.is_empty() {
            env.push(("HF_ENDPOINT".to_string(), self.hf_endpoint.clone()));
        }
        if !self.pip_index.is_empty() {
            env.push(("PIP_INDEX_URL".to_string(), self.pip_index.clone()));
        }
        env
    }

    /// The workspace's `models` and `tools` directories, or `None` when no
    /// workspace is set (the caller keeps its launcher defaults).
    pub fn workspace_paths(&self) -> Option<(PathBuf, PathBuf)> {
        if self.workspace_dir.is_empty() {
            return None;
        }
        let root = PathBuf::from(&self.workspace_dir);
        Some((root.join("models"), root.join("tools")))
    }

    fn normalize(mut self) -> Result<Self> {
        self.proxy = self.proxy.trim().to_string();
        self.hf_endpoint = self.hf_endpoint.trim().trim_end_matches('/').to_string();
        self.pip_index = self.pip_index.trim().trim_end_matches('/').to_string();

        match self.proxy_mode {
            ProxyMode::Manual if self.proxy.is_empty() => {
                return Err(CoreError::UserError(i18n::Message::new(
                    Key::ManualProxyRequired,
                )));
            }
            ProxyMode::Manual => check_url(
                Key::LabelProxy,
                &self.proxy,
                &["http", "https", "socks5", "socks5h"],
            )?,
            ProxyMode::System | ProxyMode::Direct => {}
        }
        check_url(Key::LabelHfMirror, &self.hf_endpoint, &["http", "https"])?;
        check_url(Key::LabelPipMirror, &self.pip_index, &["http", "https"])?;

        self.workspace_dir = expand_home(self.workspace_dir.trim());
        if !self.workspace_dir.is_empty() && !Path::new(&self.workspace_dir).is_absolute() {
            return Err(CoreError::UserError(
                i18n::Message::new(Key::WorkspaceDirNotAbsolute)
                    .arg_key("label", Key::LabelWorkspace),
            ));
        }

        self.language = self.language.trim().to_string();
        if !matches!(self.language.as_str(), "" | "zh" | "en") {
            return Err(CoreError::UserError(
                i18n::Message::new(Key::UnsupportedLanguage).arg("lang", &self.language),
            ));
        }
        Ok(self)
    }
}

/// Variables that would otherwise leak a proxy into a direct download.
pub const PROXY_VARS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// Expand a leading `~`, `~/`, or `~\` to the user's home directory so
/// the UI can accept the familiar short form. Anything else is returned
/// unchanged.
pub(crate) fn expand_home(path: &str) -> String {
    let rest = if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        rest
    } else if path == "~" {
        ""
    } else {
        return path.to_string();
    };
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from);
    match home {
        Some(home) if rest.is_empty() => home.to_string_lossy().into_owned(),
        Some(home) => home.join(rest).to_string_lossy().into_owned(),
        None => path.to_string(),
    }
}

/// Keep any inherited exceptions, and always bypass loopback so a local
/// mirror keeps working.
fn no_proxy_value() -> String {
    let inherited = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default();
    let mut parts: Vec<&str> = inherited
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    for extra in ["127.0.0.1", "localhost"] {
        if !parts.iter().any(|part| part.eq_ignore_ascii_case(extra)) {
            parts.push(extra);
        }
    }
    parts.join(",")
}

pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsStore {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let path = data_dir.as_ref().join("settings.json");
        let current = match fs::read_to_string(&path) {
            Ok(raw) => match parse(&raw) {
                Ok(settings) => settings,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "ignoring invalid settings.json: {e}");
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        };
        Ok(Self {
            path,
            current: Mutex::new(current),
        })
    }

    pub fn get(&self) -> Settings {
        self.current.lock().expect("settings lock poisoned").clone()
    }

    pub fn update(&self, settings: Settings) -> Result<Settings> {
        let settings = settings.normalize()?;
        let bytes = serde_json::to_vec_pretty(&settings)?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &self.path)?;
        *self.current.lock().expect("settings lock poisoned") = settings.clone();
        Ok(settings)
    }
}

/// Parse stored settings. Files written before `proxy_mode` existed are
/// migrated: a proxy URL means manual, an empty value means system.
fn parse(raw: &str) -> Result<Settings> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    let had_mode = value.get("proxy_mode").is_some();
    let mut settings: Settings = serde_json::from_value(value)?;
    if !had_mode && !settings.proxy.trim().is_empty() {
        settings.proxy_mode = ProxyMode::Manual;
    }
    Ok(settings)
}

/// Accepts an empty value (engine default) or an absolute URL whose
/// scheme is in `schemes`.
fn check_url(label: Key, value: &str, schemes: &[&str]) -> Result<()> {
    if value.is_empty() {
        return Ok(());
    }
    let (scheme, rest) = value.split_once("://").ok_or_else(|| {
        CoreError::UserError(i18n::Message::new(Key::UrlSchemeRequired).arg_key("label", label))
    })?;
    if !schemes.contains(&scheme.to_ascii_lowercase().as_str()) {
        return Err(CoreError::UserError(
            i18n::Message::new(Key::UrlSchemeUnsupported)
                .arg_key("label", label)
                .arg("scheme", scheme)
                .arg("allowed", schemes.join(", ")),
        ));
    }
    if rest.is_empty() || rest.starts_with('/') {
        return Err(CoreError::UserError(
            i18n::Message::new(Key::UrlHostMissing).arg_key("label", label),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual(proxy: &str) -> Settings {
        Settings {
            proxy_mode: ProxyMode::Manual,
            proxy: proxy.into(),
            ..Settings::default()
        }
    }

    #[test]
    fn normalizes_and_validates() {
        let settings = Settings {
            proxy_mode: ProxyMode::Manual,
            proxy: "  http://127.0.0.1:10808  ".into(),
            hf_endpoint: " https://hf-mirror.com/ ".into(),
            pip_index: "https://pypi.tuna.tsinghua.edu.cn/simple/".into(),
            workspace_dir: "  ".into(),
            language: String::new(),
        }
        .normalize()
        .expect("valid settings");
        assert_eq!(settings.proxy, "http://127.0.0.1:10808");
        assert_eq!(settings.hf_endpoint, "https://hf-mirror.com");
        assert_eq!(
            settings.pip_index,
            "https://pypi.tuna.tsinghua.edu.cn/simple"
        );
        assert_eq!(settings.workspace_dir, "");
        assert!(settings.workspace_paths().is_none());
    }

    #[test]
    fn rejects_bad_urls() {
        for bad in ["127.0.0.1:10808", "ftp://example.com", "http://"] {
            assert!(manual(bad).normalize().is_err(), "{bad} should be rejected");
        }
        assert!(Settings::default().normalize().is_ok());
    }

    #[test]
    fn manual_mode_needs_a_url() {
        assert!(manual("  ").normalize().is_err());
        // The other modes ignore a stale value instead of failing.
        let settings = Settings {
            proxy_mode: ProxyMode::Direct,
            proxy: "not-a-url".into(),
            ..Settings::default()
        };
        assert!(settings.normalize().is_ok());
    }

    #[test]
    fn empty_settings_produce_no_env() {
        assert!(Settings::default().download_env(None).is_empty());
    }

    #[test]
    fn manual_proxy_sets_both_spellings() {
        let settings = Settings {
            proxy_mode: ProxyMode::Manual,
            proxy: "http://127.0.0.1:10808".into(),
            hf_endpoint: "https://hf-mirror.com".into(),
            pip_index: "https://pypi.tuna.tsinghua.edu.cn/simple".into(),
            workspace_dir: String::new(),
            language: String::new(),
        };
        let env = settings.download_env(None);
        let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("HTTPS_PROXY"), Some("http://127.0.0.1:10808"));
        assert_eq!(get("https_proxy"), Some("http://127.0.0.1:10808"));
        let no_proxy = get("no_proxy").expect("no_proxy set");
        assert!(no_proxy.contains("127.0.0.1") && no_proxy.contains("localhost"));
        assert_eq!(get("HF_ENDPOINT"), Some("https://hf-mirror.com"));
        assert_eq!(
            get("PIP_INDEX_URL"),
            Some("https://pypi.tuna.tsinghua.edu.cn/simple")
        );
    }

    #[test]
    fn system_mode_uses_detected_proxy() {
        let env = Settings::default().download_env(Some("socks5h://127.0.0.1:7890"));
        let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("all_proxy"), Some("socks5h://127.0.0.1:7890"));
    }

    #[test]
    fn direct_mode_ignores_system_proxy() {
        let settings = Settings {
            proxy_mode: ProxyMode::Direct,
            proxy: "http://127.0.0.1:10808".into(),
            ..Settings::default()
        };
        assert!(settings
            .download_env(Some("http://127.0.0.1:7890"))
            .is_empty());
    }

    #[test]
    fn accepts_supported_languages() {
        for lang in ["", "zh", "en"] {
            let s = Settings {
                language: lang.into(),
                ..Settings::default()
            };
            assert!(s.normalize().is_ok(), "{lang} accepted");
        }
        let s = Settings {
            language: "fr".into(),
            ..Settings::default()
        };
        assert!(s.normalize().is_err());
    }

    #[test]
    fn workspace_dir_must_be_absolute() {
        for bad in ["models", "./models", "../shared"] {
            let s = Settings {
                workspace_dir: bad.into(),
                ..Settings::default()
            };
            assert!(s.normalize().is_err(), "{bad} should be rejected");
        }
        let s = Settings {
            workspace_dir: "  /data/laverock  ".into(),
            ..Settings::default()
        }
        .normalize()
        .expect("absolute path is valid");
        assert_eq!(s.workspace_dir, "/data/laverock");
        let (models, tools) = s.workspace_paths().expect("paths derived");
        assert_eq!(models, PathBuf::from("/data/laverock/models"));
        assert_eq!(tools, PathBuf::from("/data/laverock/tools"));
    }

    #[test]
    fn workspace_dir_expands_home() {
        if std::env::var_os("HOME").is_none() {
            return;
        }
        let s = Settings {
            workspace_dir: "~/laverock-tts".into(),
            ..Settings::default()
        }
        .normalize()
        .expect("expanded path is absolute");
        assert!(Path::new(&s.workspace_dir).is_absolute());
        assert!(s.workspace_dir.ends_with("laverock-tts"));
    }

    #[test]
    fn migrates_legacy_settings() {
        let parsed = parse(r#"{"proxy":"http://127.0.0.1:10808"}"#).unwrap();
        assert_eq!(parsed.proxy_mode, ProxyMode::Manual);

        let parsed = parse(r#"{"proxy":""}"#).unwrap();
        assert_eq!(parsed.proxy_mode, ProxyMode::System);

        let parsed = parse(r#"{"proxy_mode":"direct","proxy":"http://x:1"}"#).unwrap();
        assert_eq!(parsed.proxy_mode, ProxyMode::Direct);
    }
}
