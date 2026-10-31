//! Model catalog and download jobs.
//!
//! Downloads stay in the shell scripts under `scripts/` — they already
//! know how to fetch, verify and write a manifest. The server only runs
//! them with the user's proxy/mirror settings and streams their output
//! back to the UI.
//!
//! All installs are desktop-only; the routes are gated on loopback in
//! `main.rs`.

use crate::{
    proxy,
    settings::{ProxyMode, SettingsStore, PROXY_VARS},
};
use laverock_core::i18n::{self, Key};
use laverock_core::{CoreError, EngineManager, Localized, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

const LOG_LIMIT: usize = 24_000;
const LOG_TAIL: usize = 6_000;

#[derive(Debug, Clone, Deserialize)]
struct CatalogEntry {
    id: String,
    dir: String,
    name: String,
    /// Localized: `{"en": …, "zh": …}` for our entries, a plain string
    /// for third-party catalogs.
    description: Localized,
    size: String,
    languages: Vec<String>,
    capabilities: Vec<String>,
    script: String,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    note: Option<Localized>,
    /// Operating systems where this model cannot be used, spelled as
    /// `std::env::consts::OS` (`windows`, `macos`, `linux`). The UI then
    /// hides the download and the API refuses to start it.
    #[serde(default)]
    unsupported_on: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobStatus {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
struct Job {
    status: JobStatus,
    started_at: u64,
    finished_at: Option<u64>,
    log: String,
    /// Human-readable current step from the script (`@@LV stage`).
    stage: Option<String>,
    /// 0-100 from the script or a progress meter; `None` = indeterminate.
    progress: Option<f32>,
}

impl Job {
    fn running() -> Self {
        Self {
            status: JobStatus::Running,
            started_at: now_secs(),
            finished_at: None,
            log: String::new(),
            stage: Some("stage_waiting".to_string()),
            progress: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct JobView {
    status: JobStatus,
    started_at: u64,
    finished_at: Option<u64>,
    /// Last few KB of script output; the full log stays in memory.
    log: String,
    stage: Option<String>,
    progress: Option<f32>,
}

#[derive(Debug, Serialize)]
pub struct ModelView {
    pub id: String,
    pub dir: String,
    pub name: String,
    pub description: String,
    pub size: String,
    pub languages: Vec<String>,
    pub capabilities: Vec<String>,
    pub note: Option<String>,
    /// Set when the catalog excludes this model on the current OS; the
    /// UI shows it and disables the download.
    pub unsupported: Option<String>,
    pub installed: bool,
    /// Set when the installed directory exists but cannot be used (old
    /// runner layout, unparsable manifest): the UI offers a reinstall.
    pub problem: Option<String>,
    pub engine_id: Option<String>,
    pub job: Option<JobView>,
}

#[derive(Debug, Serialize)]
pub struct ModelList {
    pub models: Vec<ModelView>,
    pub models_dir: String,
    pub scripts_dir: Option<String>,
}

pub struct ModelService {
    models_root: PathBuf,
    tools_root: Option<PathBuf>,
    scripts_dir: Option<PathBuf>,
    settings: Arc<SettingsStore>,
    manager: Arc<EngineManager>,
    jobs: Arc<Mutex<HashMap<String, Job>>>,
}

impl ModelService {
    pub fn new(
        models_root: impl Into<PathBuf>,
        tools_root: Option<PathBuf>,
        scripts_dir: Option<PathBuf>,
        settings: Arc<SettingsStore>,
        manager: Arc<EngineManager>,
    ) -> Self {
        // Displaying an absolute path is friendlier than the default
        // relative "models"; the directory may not exist yet.
        let models_root = models_root.into();
        let models_root = clean_native(std::fs::canonicalize(&models_root).unwrap_or(models_root));
        let tools_root = tools_root.map(|p| clean_native(std::fs::canonicalize(&p).unwrap_or(p)));
        // Paths handed in by the desktop shell can contain `..`; show
        // clean paths in the UI and hand scripts a stable one.
        let scripts_dir = scripts_dir.map(|p| clean_native(std::fs::canonicalize(&p).unwrap_or(p)));
        Self {
            models_root,
            tools_root,
            scripts_dir,
            settings,
            manager,
            jobs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn list(&self, lang: &str) -> ModelList {
        let engines = self.manager.list();
        let jobs = self.jobs.lock().expect("jobs lock poisoned");
        let mut models: Vec<ModelView> = Vec::new();
        let mut known = Vec::new();

        for entry in catalog() {
            let manifest = self.models_root.join(&entry.dir).join("engine.json");
            let engine_id = read_engine_id(&manifest);
            let installed = manifest.is_file();
            let problem = if installed {
                manifest_problem(&manifest, lang)
            } else {
                None
            };
            if let Some(id) = &engine_id {
                known.push(id.clone());
            } else if installed {
                known.push(entry.dir.clone());
            }
            models.push(ModelView {
                id: entry.id.clone(),
                dir: entry.dir.clone(),
                name: entry.name.clone(),
                description: entry.description.for_lang(lang).to_string(),
                size: entry.size.clone(),
                languages: entry.languages.clone(),
                capabilities: entry.capabilities.clone(),
                note: entry
                    .note
                    .as_ref()
                    .map(|n| n.for_lang(lang).to_string())
                    .filter(|n| !n.is_empty()),
                unsupported: unsupported_here(entry, lang),
                installed,
                problem,
                engine_id,
                job: jobs.get(&entry.id).map(|job| JobView::localized(job, lang)),
            });
        }

        // Models placed manually (or shipped separately) still show up.
        for engine in engines {
            if known.contains(&engine.id) {
                continue;
            }
            let dir = self.models_root.join(&engine.id);
            if !dir.join("engine.json").is_file() {
                continue;
            }
            let description = Localized::Translations {
                en: Some(
                    "Manually installed model (manifest from the model directory).".to_string(),
                ),
                zh: Some("手动安装的模型（manifest 来自模型目录）。".to_string()),
            };
            models.push(ModelView {
                id: engine.id.clone(),
                dir: engine.id.clone(),
                name: engine.name,
                description: description.for_lang(lang).to_string(),
                size: String::new(),
                languages: engine.languages,
                capabilities: engine.capabilities,
                note: None,
                unsupported: None,
                installed: true,
                problem: None,
                engine_id: Some(engine.id.clone()),
                job: None,
            });
        }

        ModelList {
            models,
            models_dir: self.models_root.display().to_string(),
            scripts_dir: self.scripts_dir.as_ref().map(|p| p.display().to_string()),
        }
    }

    /// Start a download in the background. Progress is polled through
    /// `list()`; the engine registry is reloaded when the script exits 0.
    pub fn install(&self, id: &str) -> Result<()> {
        let entry = catalog()
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or_else(|| {
                CoreError::UserError(i18n::Message::new(Key::UnknownModel).arg("model", id))
            })?;

        // Some runtimes do not work on every OS; the catalog lists those
        // platforms so the UI can explain the refusal instead of letting
        // the user download a model that will crash.
        if entry
            .unsupported_on
            .iter()
            .any(|os| os == std::env::consts::OS)
        {
            return Err(CoreError::UserError(
                i18n::Message::new(Key::UnsupportedPlatform).arg("model", &entry.name),
            ));
        }

        // A healthy install is left alone; a broken/outdated one (e.g. the
        // VoxCPM2 layout from the retired llama.cpp runner) may be
        // reinstalled in place — the script rewrites engine.json.
        let manifest_path = self.models_root.join(&entry.dir).join("engine.json");
        if manifest_path.is_file() && manifest_problem(&manifest_path, "en").is_none() {
            return Err(CoreError::UserError(
                i18n::Message::new(Key::AlreadyInstalled).arg("model", &entry.name),
            ));
        }

        let scripts = self
            .scripts_dir
            .as_ref()
            .ok_or_else(|| CoreError::UserError(i18n::Message::new(Key::ScriptsDirMissing)))?;
        let script = scripts.join(&entry.script);
        if !script.is_file() {
            return Err(CoreError::UserError(
                i18n::Message::new(Key::ScriptMissing).arg("path", script.display()),
            ));
        }

        {
            let mut jobs = self.jobs.lock().expect("jobs lock poisoned");
            if jobs
                .get(&entry.id)
                .is_some_and(|job| job.status == JobStatus::Running)
            {
                return Err(CoreError::UserError(i18n::Message::new(
                    Key::DownloadInProgress,
                )));
            }
            jobs.insert(entry.id.clone(), Job::running());
        }

        let jobs = self.jobs.clone();
        let settings = self.settings.clone();
        let manager = self.manager.clone();
        let models_root = self.models_root.clone();
        let tools_root = self.tools_root.clone();
        let script_dir = scripts.clone();
        let entry_for_thread = entry.clone();

        std::thread::Builder::new()
            .name(format!("install-{}", entry.id))
            .spawn(move || {
                run_install(
                    entry_for_thread,
                    script,
                    script_dir,
                    models_root,
                    tools_root,
                    jobs,
                    settings,
                    manager,
                );
            })
            .map_err(|e| CoreError::Synth(format!("无法启动下载线程：{e}")))?;

        Ok(())
    }

    pub fn reload(&self) -> Result<usize> {
        self.manager.reload()
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_root
    }

    pub fn scripts_dir(&self) -> Option<&Path> {
        self.scripts_dir.as_deref()
    }

    pub fn tools_dir(&self) -> Option<&Path> {
        self.tools_root.as_deref()
    }
}

impl JobView {
    /// Render a job for a UI language. A stage emitted as a script key
    /// (`stage_download_file name=… size=…`) is translated here; ad-hoc
    /// stage text is shown as-is.
    fn localized(job: &Job, lang: &str) -> Self {
        let lang = laverock_core::i18n::Lang::parse(lang);
        let stage = job.stage.as_deref().map(|stage| {
            laverock_core::i18n::message_from_marker(stage)
                .map(|message| message.render(lang))
                .unwrap_or_else(|| stage.to_string())
        });
        Self {
            status: job.status,
            started_at: job.started_at,
            finished_at: job.finished_at,
            log: tail(&job.log, LOG_TAIL),
            stage,
            progress: job.progress,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_install(
    entry: CatalogEntry,
    script: PathBuf,
    script_dir: PathBuf,
    models_root: PathBuf,
    tools_root: Option<PathBuf>,
    jobs: Arc<Mutex<HashMap<String, Job>>>,
    settings: Arc<SettingsStore>,
    manager: Arc<EngineManager>,
) {
    let settings = settings.get();
    let system_proxy = match settings.proxy_mode {
        ProxyMode::System => proxy::system(),
        ProxyMode::Manual | ProxyMode::Direct => None,
    };

    let shell = resolve_bash(&script_dir);
    let mut cmd = Command::new(&shell.program);
    // Script argv and the env vars the scripts consume must use the bash
    // path form (forward slashes): a verbatim/backslash path reaches
    // MSYS bash with its backslashes eaten as escapes.
    cmd.arg(bash_path(&script))
        .env("LAVEROCK_MODELS_DIR", bash_path(&models_root))
        .env("LAVEROCK_SCRIPTS_DIR", bash_path(&script_dir))
        // Let installers fetch the ONNX variant that suits *this* machine
        // (fp32 for a usable GPU or an x86 CPU without AVX2, INT8 otherwise).
        .env("LAVEROCK_ONNX_VARIANT", laverock_core::model_variant())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Pin the venv root when the workspace setting or launcher supplied
    // one; otherwise the scripts default to `<repo>/.tools`.
    if let Some(tools) = &tools_root {
        cmd.env("LAVEROCK_TOOLS_DIR", bash_path(tools));
    }
    // Bundled MinGit: make its bash/coreutils/curl visible to the script and
    // every process it spawns, without relying on the system PATH.
    if let Some(path) = shell.augmented_path() {
        cmd.env("PATH", path);
    }
    for (key, value) in &entry.env {
        cmd.env(key, value);
    }
    for (key, value) in settings.download_env(system_proxy.as_deref()) {
        cmd.env(key, value);
    }
    if settings.proxy_mode == ProxyMode::Direct {
        // A proxy exported in the server's own environment would
        // otherwise be inherited by the script.
        for key in PROXY_VARS {
            cmd.env_remove(key);
        }
    }

    let network = match (&settings.proxy_mode, &system_proxy) {
        (ProxyMode::System, Some(addr)) => format!("跟随系统代理 {addr}"),
        (ProxyMode::System, None) => "直连（未检测到系统代理）".to_string(),
        (ProxyMode::Manual, _) => format!("手动代理 {}", settings.proxy),
        (ProxyMode::Direct, _) => "直连".to_string(),
    };
    append_log(&jobs, &entry.id, &format!("下载网络：{network}\n"));
    if shell.bundled {
        append_log(
            &jobs,
            &entry.id,
            "命令行环境：内置 MinGit（无需安装 Git for Windows / WSL）\n",
        );
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let hint = if cfg!(windows) {
                "（Windows 需要安装 Git for Windows，它提供 bash）"
            } else {
                ""
            };
            finish(
                &jobs,
                &entry.id,
                JobStatus::Failed,
                format!("无法启动下载脚本：{e} {hint}\n"),
            );
            return;
        }
    };

    let mut pipes: Vec<Box<dyn Read + Send>> = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        pipes.push(Box::new(stdout));
    }
    if let Some(stderr) = child.stderr.take() {
        pipes.push(Box::new(stderr));
    }

    let mut readers = Vec::new();
    for pipe in pipes {
        let jobs = jobs.clone();
        let id = entry.id.clone();
        readers.push(std::thread::spawn(move || pump_pipe(pipe, &jobs, &id)));
    }

    let status = child.wait();
    for reader in readers {
        let _ = reader.join();
    }

    match status {
        Ok(status) if status.success() => {
            set_progress(&jobs, &entry.id, Some("安装完成".to_string()), Some(100.0));
            append_log(&jobs, &entry.id, "\n安装完成。\n");
            match manager.reload() {
                Ok(count) => append_log(
                    &jobs,
                    &entry.id,
                    &format!("引擎注册表已刷新，共 {count} 个引擎。\n"),
                ),
                Err(e) => append_log(
                    &jobs,
                    &entry.id,
                    &format!("引擎刷新失败（重启服务即可）：{e}\n"),
                ),
            }
            finish(&jobs, &entry.id, JobStatus::Done, String::new());
        }
        Ok(status) => finish(
            &jobs,
            &entry.id,
            JobStatus::Failed,
            format!("\n安装失败，退出码 {:?}。\n", status.code()),
        ),
        Err(e) => finish(
            &jobs,
            &entry.id,
            JobStatus::Failed,
            format!("\n等待下载进程失败：{e}\n"),
        ),
    }
}

fn finish(jobs: &Arc<Mutex<HashMap<String, Job>>>, id: &str, status: JobStatus, extra: String) {
    let mut jobs = jobs.lock().expect("jobs lock poisoned");
    if let Some(job) = jobs.get_mut(id) {
        if !extra.is_empty() {
            push_bounded(&mut job.log, &extra);
        }
        job.status = status;
        job.finished_at = Some(now_secs());
    }
}

fn append_log(jobs: &Arc<Mutex<HashMap<String, Job>>>, id: &str, chunk: &str) {
    let mut jobs = jobs.lock().expect("jobs lock poisoned");
    if let Some(job) = jobs.get_mut(id) {
        push_bounded(&mut job.log, chunk);
    }
}

fn set_progress(
    jobs: &Arc<Mutex<HashMap<String, Job>>>,
    id: &str,
    stage: Option<String>,
    progress: Option<f32>,
) {
    let mut jobs = jobs.lock().expect("jobs lock poisoned");
    if let Some(job) = jobs.get_mut(id) {
        if stage.is_some() {
            job.stage = stage;
        }
        if progress.is_some() {
            job.progress = progress.map(|p| p.clamp(0.0, 100.0));
        }
    }
}

/// Read one piped script output stream, splitting on both `\n` and `\r`
/// so both normal log lines and progress-meter redraws are handled.
fn pump_pipe(mut pipe: Box<dyn Read + Send>, jobs: &Arc<Mutex<HashMap<String, Job>>>, id: &str) {
    let mut pending = String::new();
    let mut buf = [0u8; 4096];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                while let Some(pos) = pending.find(['\n', '\r']) {
                    let cr = pending.as_bytes()[pos] == b'\r';
                    let line: String = pending.drain(..=pos).collect();
                    handle_output_line(jobs, id, &line, cr);
                }
                // Meters can emit many redraws; keep the buffer bounded if
                // a script never terminates a line.
                if pending.len() > 8192 {
                    handle_output_line(jobs, id, &std::mem::take(&mut pending), true);
                }
            }
        }
    }
    if !pending.trim().is_empty() {
        handle_output_line(jobs, id, &pending, false);
    }
}

fn handle_output_line(jobs: &Arc<Mutex<HashMap<String, Job>>>, id: &str, raw: &str, cr: bool) {
    let line = raw.trim_matches(['\n', '\r']);
    if let Some(rest) = line.strip_prefix("@@LV ") {
        let mut parts = rest.splitn(2, ' ');
        match (parts.next(), parts.next()) {
            (Some("stage"), Some(label)) if !label.trim().is_empty() => {
                set_progress(jobs, id, Some(label.trim().to_string()), None);
            }
            (Some("progress"), Some(value)) => {
                if let Ok(pct) = value.trim().parse::<f32>() {
                    set_progress(jobs, id, None, Some(pct));
                }
            }
            _ => {}
        }
        return;
    }

    if cr {
        // Progress meters (git/tqdm/pip) redraw with \r. Extract a
        // percentage when possible; drop meter-only redraws otherwise so
        // the visible log is not flooded with control garbage.
        if let Some(pct) = parse_meter(line) {
            set_progress(jobs, id, None, Some(pct));
        } else if looks_like_meter(line) {
            // Not worth logging.
        } else if !line.trim().is_empty() {
            append_log(jobs, id, line);
            append_log(jobs, id, "\n");
        }
    } else if !line.is_empty() {
        append_log(jobs, id, line);
        append_log(jobs, id, "\n");
    }
}

/// Find the first `NN%` / `NN.N%` token in a progress-meter redraw.
fn parse_meter(line: &str) -> Option<f32> {
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'%' {
            continue;
        }
        let mut j = i;
        while j > 0 && (bytes[j - 1].is_ascii_digit() || bytes[j - 1] == b'.') {
            j -= 1;
        }
        if j < i {
            if let Ok(value) = line[j..i].parse::<f32>() {
                if (0.0..=100.0).contains(&value) {
                    return Some(value);
                }
            }
        }
    }
    None
}

/// Whether a CR-terminated line is meter-only noise (bar redraws without
/// a percentage token). Percent-bearing redraws are caught by
/// [`parse_meter`] before this runs.
fn looks_like_meter(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    const BARS: [char; 11] = ['█', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '▇', '#', '='];
    let bar_count = t.chars().filter(|c| BARS.contains(c)).count();
    bar_count > 0 && bar_count * 2 >= t.chars().count()
}

fn push_bounded(log: &mut String, chunk: &str) {
    log.push_str(chunk);
    if log.len() > LOG_LIMIT {
        let mut cut = log.len() - LOG_LIMIT;
        while !log.is_char_boundary(cut) {
            cut += 1;
        }
        log.drain(..cut);
    }
}

fn tail(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

fn read_engine_id(manifest: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(manifest).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    parsed
        .get("id")?
        .as_str()
        .map(std::string::ToString::to_string)
}

/// Why the catalog excludes this model on the current OS, if it does.
fn unsupported_here(entry: &CatalogEntry, lang: &str) -> Option<String> {
    if !entry
        .unsupported_on
        .iter()
        .any(|os| os == std::env::consts::OS)
    {
        return None;
    }
    Some(
        i18n::Message::new(Key::UnsupportedPlatform)
            .arg("model", &entry.name)
            .render(laverock_core::i18n::Lang::parse(lang)),
    )
}

/// Why an installed directory is unusable, if it is. An unparsable
/// manifest counts: the engine never registers in that case, so the
/// model page must offer a reinstall rather than claiming it is fine.
fn manifest_problem(manifest: &Path, lang: &str) -> Option<String> {
    let raw = match std::fs::read_to_string(manifest) {
        Ok(raw) => raw,
        Err(e) => {
            return Some(match lang {
                "en" => format!("cannot read engine.json: {e}"),
                _ => format!("读取 engine.json 失败：{e}"),
            })
        }
    };
    match serde_json::from_str::<laverock_core::Manifest>(&raw) {
        Ok(m) => m.problem().map(|p| p.for_lang(lang).to_string()),
        Err(e) => Some(match lang {
            "en" => format!("engine.json cannot be parsed: {e}"),
            _ => format!("engine.json 无法解析：{e}"),
        }),
    }
}

fn catalog() -> &'static [CatalogEntry] {
    use std::sync::OnceLock;
    static CATALOG: OnceLock<Vec<CatalogEntry>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/catalog.json")).expect("valid model catalog")
    })
}

/// Locate the repository's `scripts/` directory. Explicit environment
/// first, then the working directory (development), then beside the
/// binary and in the macOS/Windows app resource directories.
pub fn resolve_scripts_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("LAVEROCK_SCRIPTS_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_dir() {
            return Some(dir);
        }
    }

    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("scripts"));
        candidates.push(cwd.join("../../scripts"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("scripts"));
            candidates.push(dir.join("../Resources/scripts"));
            candidates.push(dir.join("../../scripts"));
        }
    }
    candidates.into_iter().find(|dir| dir.is_dir())
}

/// The POSIX shell used to run `download-*.sh`. On Windows the installer
/// carries a portable MinGit (no Git-for-Windows/WSL needed on the target
/// machine); we prefer that over any `bash` on PATH so installs work on a
/// clean system.
struct BashShell {
    program: PathBuf,
    /// MinGit bin directories to prepend to PATH (bash, coreutils, curl…).
    prepend_path: Vec<PathBuf>,
    bundled: bool,
}

impl BashShell {
    fn system() -> Self {
        Self {
            program: PathBuf::from("bash"),
            prepend_path: Vec::new(),
            bundled: false,
        }
    }

    /// PATH with the MinGit bin directories in front, if any.
    fn augmented_path(&self) -> Option<std::ffi::OsString> {
        if self.prepend_path.is_empty() {
            return None;
        }
        let mut entries = self.prepend_path.clone();
        if let Some(cur) = std::env::var_os("PATH") {
            entries.extend(std::env::split_paths(&cur));
        }
        std::env::join_paths(entries).ok()
    }
}

/// `std::fs::canonicalize` returns verbatim paths on Windows
/// (`\\?\C:\…`). Neither the Win32 PATH search nor MSYS bash accepts that
/// prefix (bash's command-line parsing eats the backslashes as escapes and
/// reports `\?C:…: No such file or directory`), so strip it back to the
/// plain `C:\…` form. Verbatim UNC paths (`\\?\UNC\server\share`) become
/// `\\server\share`.
#[cfg(windows)]
fn clean_native(path: PathBuf) -> PathBuf {
    let Some(rest) = path.to_str() else {
        return path;
    };
    if let Some(unc) = rest.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    match rest.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

#[cfg(not(windows))]
fn clean_native(path: PathBuf) -> PathBuf {
    path
}

/// Path form handed to MSYS bash: drive letter with forward slashes
/// (`E:/workspace/…`). Backslashes in argv or env values reach bash mangled
/// (treated as escape characters), while forward slashes are accepted by
/// both bash/coreutils and the native Windows python the scripts spawn.
#[cfg(windows)]
fn bash_path(path: &Path) -> std::ffi::OsString {
    let cleaned = clean_native(path.to_path_buf());
    std::ffi::OsString::from(cleaned.to_string_lossy().replace('\\', "/"))
}

#[cfg(not(windows))]
fn bash_path(path: &Path) -> std::ffi::OsString {
    path.as_os_str().to_os_string()
}

fn resolve_bash(scripts_dir: &Path) -> BashShell {
    if let Some(p) = std::env::var_os("LAVEROCK_BASH") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return BashShell {
                program: p,
                prepend_path: Vec::new(),
                bundled: false,
            };
        }
    }

    if cfg!(windows) {
        let mut roots = vec![
            scripts_dir.join("../mingit"), // packaged resource, sibling of scripts/
            scripts_dir.join("mingit"),
            scripts_dir.join("../crates/laverock-desktop/mingit"), // development checkout
        ];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                roots.push(dir.join("mingit"));
                roots.push(dir.join("../mingit"));
            }
        }
        for root in roots {
            let bash = root.join("bin/bash.exe");
            if bash.is_file() {
                // Resolve ../ so the PATH entries we hand to children are real.
                // clean_native drops canonicalize's `\\?\` prefix, which
                // neither the Win32 PATH search nor bash accepts.
                let root = clean_native(std::fs::canonicalize(&root).unwrap_or(root));
                return BashShell {
                    program: clean_native(
                        std::fs::canonicalize(root.join("bin/bash.exe")).unwrap_or(bash),
                    ),
                    // PortableGit 2.56 ships its mingw tools under ucrt64/,
                    // not the older mingw64/; curl/git live in ucrt64/bin.
                    prepend_path: vec![
                        root.join("bin"),
                        root.join("usr/bin"),
                        root.join("ucrt64/bin"),
                    ],
                    bundled: true,
                };
            }
        }
    }

    BashShell::system()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_valid() {
        let entries = catalog();
        assert!(entries.len() >= 3);
        for entry in entries {
            assert!(!entry.id.is_empty());
            assert!(!entry.dir.is_empty());
            assert!(entry.script.ends_with(".sh"));
        }
    }

    #[test]
    fn meter_percentages() {
        assert_eq!(
            parse_meter("Receiving objects: 45% (1234/9000)"),
            Some(45.0)
        );
        assert_eq!(
            parse_meter("model.safetensors:  42.5%|████▎ | 700M/1.6G"),
            Some(42.5)
        );
        assert_eq!(
            parse_meter("##################################  41.7%"),
            Some(41.7)
        );
        assert_eq!(parse_meter("plain log line without progress"), None);
        assert_eq!(parse_meter("150% impossible"), None);
    }

    #[test]
    fn progress_markers_and_meters() {
        let jobs: Arc<Mutex<HashMap<String, Job>>> = Arc::new(Mutex::new(HashMap::new()));
        jobs.lock().unwrap().insert("m".to_string(), Job::running());

        handle_output_line(&jobs, "m", "@@LV stage 下载权重\n", false);
        handle_output_line(&jobs, "m", "@@LV progress 33.3\n", false);
        {
            let j = jobs.lock().unwrap();
            let job = j.get("m").unwrap();
            assert_eq!(job.stage.as_deref(), Some("下载权重"));
            assert!((job.progress.unwrap() - 33.3).abs() < 0.01);
            assert!(!job.log.contains("@@LV"));
        }

        // A tqdm-style CR redraw updates the percentage, not the log.
        handle_output_line(&jobs, "m", "file: 80%|████████ | 8/10\r", true);
        {
            let j = jobs.lock().unwrap();
            let job = j.get("m").unwrap();
            assert!((job.progress.unwrap() - 80.0).abs() < 0.01);
            assert!(job.log.is_empty());
        }

        // Plain output survives, including CR-terminated status text.
        handle_output_line(&jobs, "m", "Building voxcpm2-cli ...", false);
        handle_output_line(&jobs, "m", "\n", false);
        {
            let j = jobs.lock().unwrap();
            assert!(j.get("m").unwrap().log.contains("Building voxcpm2-cli"));
        }
    }

    #[test]
    fn tail_respects_char_boundaries() {
        let text = "你好，世界";
        let cut = tail(text, 4);
        assert!(text.ends_with(&cut));
        assert!(cut.chars().count() <= 2);
    }

    // The installer runs download scripts with `bash`, which the Windows
    // CI image does not put on PATH; in-app installs there require Git
    // Bash anyway, so exercise this path on Unix only.
    #[cfg(unix)]
    #[test]
    fn installer_records_output_and_finishes() {
        let base = std::env::temp_dir().join(format!(
            "laverock-models-test-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let models = base.join("models");
        let scripts = base.join("scripts");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::create_dir_all(&scripts).unwrap();

        let script = scripts.join("fake.sh");
        std::fs::write(
            &script,
            "#!/usr/bin/env bash\necho downloading\nmkdir -p \"$LAVEROCK_MODELS_DIR/fake\"\n\
             echo '{\"id\":\"fake\"}' > \"$LAVEROCK_MODELS_DIR/fake/engine.json\"\n",
        )
        .unwrap();

        let settings = Arc::new(SettingsStore::open(&base).unwrap());
        let manager = Arc::new(EngineManager::open(&models).unwrap());
        let jobs: Arc<Mutex<HashMap<String, Job>>> = Arc::new(Mutex::new(HashMap::new()));
        jobs.lock()
            .unwrap()
            .insert("fake".to_string(), Job::running());

        let entry = CatalogEntry {
            id: "fake".to_string(),
            dir: "fake".to_string(),
            name: "Fake".to_string(),
            description: Localized::Translations {
                en: Some("English note".into()),
                zh: Some("中文说明".into()),
            },
            size: String::new(),
            languages: Vec::new(),
            capabilities: Vec::new(),
            script: "fake.sh".to_string(),
            env: BTreeMap::new(),
            note: None,
            unsupported_on: Vec::new(),
        };
        run_install(
            entry,
            script,
            scripts,
            models.clone(),
            None,
            jobs.clone(),
            settings,
            manager,
        );

        let jobs = jobs.lock().unwrap();
        let job = jobs.get("fake").unwrap();
        assert_eq!(job.status, JobStatus::Done);
        assert!(job.log.contains("downloading"));
        assert!(job.finished_at.is_some());
        assert!(models.join("fake/engine.json").is_file());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn clean_native_normalizes_canonicalized_paths() {
        let input = if cfg!(windows) {
            PathBuf::from(r"\\?\E:\workspace\laverock\scripts")
        } else {
            PathBuf::from("/home/laverock/scripts")
        };
        let expected = if cfg!(windows) {
            PathBuf::from(r"E:\workspace\laverock\scripts")
        } else {
            PathBuf::from("/home/laverock/scripts")
        };
        assert_eq!(clean_native(input), expected);

        #[cfg(windows)]
        {
            assert_eq!(
                clean_native(PathBuf::from(r"\\?\UNC\server\share\dir")),
                PathBuf::from(r"\\server\share\dir")
            );
            // Plain paths pass through untouched.
            assert_eq!(
                clean_native(PathBuf::from(r"E:\plain\path")),
                PathBuf::from(r"E:\plain\path")
            );
        }
    }

    #[test]
    fn bash_path_uses_forward_slashes() {
        use std::ffi::OsString;
        let native = if cfg!(windows) {
            PathBuf::from(r"E:\workspace\laverock\models")
        } else {
            PathBuf::from("/home/laverock/models")
        };
        let expected = if cfg!(windows) {
            OsString::from("E:/workspace/laverock/models")
        } else {
            OsString::from("/home/laverock/models")
        };
        assert_eq!(bash_path(&native), expected);
    }

    #[test]
    fn job_stages_translate_and_legacy_text_passes_through() {
        let mut job = Job::running();
        job.stage = Some("stage_download_file name=model.onnx size=12.3%20MB".to_string());
        let en = JobView::localized(&job, "en");
        assert_eq!(
            en.stage.as_deref(),
            Some("Downloading model.onnx (12.3 MB)")
        );
        let zh = JobView::localized(&job, "zh");
        assert_eq!(zh.stage.as_deref(), Some("下载 model.onnx（12.3 MB）"));

        // Stages from older scripts are shown verbatim.
        job.stage = Some("下载权重".to_string());
        assert_eq!(
            JobView::localized(&job, "en").stage.as_deref(),
            Some("下载权重")
        );
    }

    #[test]
    fn localized_text_resolves_per_language() {
        let entry = CatalogEntry {
            id: "x".into(),
            dir: "x".into(),
            name: "X".into(),
            description: Localized::Translations {
                en: Some("English".into()),
                zh: Some("中文".into()),
            },
            size: String::new(),
            languages: Vec::new(),
            capabilities: Vec::new(),
            script: "x.sh".into(),
            env: BTreeMap::new(),
            note: Some(Localized::Translations {
                en: Some("English note".into()),
                zh: Some("中文备注".into()),
            }),
            unsupported_on: Vec::new(),
        };
        assert_eq!(entry.description.for_lang("en"), "English");
        assert_eq!(entry.description.for_lang("zh"), "中文");
        // Unknown languages fall back to the other translation.
        assert_eq!(entry.description.for_lang("de"), "English");
        assert_eq!(entry.note.as_ref().unwrap().for_lang("en"), "English note");
        // Plain strings (third-party manifests) pass through unchanged.
        let plain = Localized::Text("raw".into());
        assert_eq!(plain.for_lang("zh"), "raw");
    }

    #[test]
    fn unsupported_models_are_flagged_only_on_the_listed_os() {
        let mut entry = CatalogEntry {
            id: "m".into(),
            dir: "m".into(),
            name: "M".into(),
            description: Localized::Text(String::new()),
            size: String::new(),
            languages: Vec::new(),
            capabilities: Vec::new(),
            script: "m.sh".into(),
            env: BTreeMap::new(),
            note: None,
            unsupported_on: vec![std::env::consts::OS.to_string()],
        };
        let reason = unsupported_here(&entry, "en").expect("current OS is listed");
        assert!(reason.contains('M'), "message names the model: {reason}");
        assert!(unsupported_here(&entry, "zh").is_some());

        entry.unsupported_on = vec!["plan9".into()];
        assert_eq!(unsupported_here(&entry, "en"), None);
        entry.unsupported_on.clear();
        assert_eq!(unsupported_here(&entry, "en"), None);
    }
}
