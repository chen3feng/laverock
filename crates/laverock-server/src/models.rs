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
use laverock_core::{CoreError, EngineManager, Result};
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
    description: String,
    size: String,
    languages: Vec<String>,
    capabilities: Vec<String>,
    script: String,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    note: Option<String>,
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
            stage: Some("等待开始".to_string()),
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
    pub installed: bool,
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
    scripts_dir: Option<PathBuf>,
    settings: Arc<SettingsStore>,
    manager: Arc<EngineManager>,
    jobs: Arc<Mutex<HashMap<String, Job>>>,
}

impl ModelService {
    pub fn new(
        models_root: impl Into<PathBuf>,
        scripts_dir: Option<PathBuf>,
        settings: Arc<SettingsStore>,
        manager: Arc<EngineManager>,
    ) -> Self {
        // Displaying an absolute path is friendlier than the default
        // relative "models"; the directory may not exist yet.
        let models_root = models_root.into();
        let models_root = std::fs::canonicalize(&models_root).unwrap_or(models_root);
        // Paths handed in by the desktop shell can contain `..`; show
        // clean paths in the UI and hand scripts a stable one.
        let scripts_dir = scripts_dir.map(|p| std::fs::canonicalize(&p).unwrap_or(p));
        Self {
            models_root,
            scripts_dir,
            settings,
            manager,
            jobs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn list(&self) -> ModelList {
        let engines = self.manager.list();
        let jobs = self.jobs.lock().expect("jobs lock poisoned");
        let mut models: Vec<ModelView> = Vec::new();
        let mut known = Vec::new();

        for entry in catalog() {
            let manifest = self.models_root.join(&entry.dir).join("engine.json");
            let engine_id = read_engine_id(&manifest);
            let installed = manifest.is_file();
            if let Some(id) = &engine_id {
                known.push(id.clone());
            } else if installed {
                known.push(entry.dir.clone());
            }
            models.push(ModelView {
                id: entry.id.clone(),
                dir: entry.dir.clone(),
                name: entry.name.clone(),
                description: entry.description.clone(),
                size: entry.size.clone(),
                languages: entry.languages.clone(),
                capabilities: entry.capabilities.clone(),
                note: entry.note.clone(),
                installed,
                engine_id,
                job: jobs.get(&entry.id).map(JobView::from),
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
            models.push(ModelView {
                id: engine.id.clone(),
                dir: engine.id.clone(),
                name: engine.name,
                description: "手动安装的模型（manifest 来自模型目录）。".to_string(),
                size: String::new(),
                languages: engine.languages,
                capabilities: engine.capabilities,
                note: None,
                installed: true,
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
            .ok_or_else(|| CoreError::BadRequest(format!("未知的模型：{id}")))?;

        if self
            .models_root
            .join(&entry.dir)
            .join("engine.json")
            .is_file()
        {
            return Err(CoreError::BadRequest(format!("{} 已经安装", entry.name)));
        }

        let scripts = self.scripts_dir.as_ref().ok_or_else(|| {
            CoreError::BadRequest(
                "找不到下载脚本目录。请设置 LAVEROCK_SCRIPTS_DIR 指向仓库的 scripts/ 目录。"
                    .to_string(),
            )
        })?;
        let script = scripts.join(&entry.script);
        if !script.is_file() {
            return Err(CoreError::BadRequest(format!(
                "缺少下载脚本：{}",
                script.display()
            )));
        }

        {
            let mut jobs = self.jobs.lock().expect("jobs lock poisoned");
            if jobs
                .get(&entry.id)
                .is_some_and(|job| job.status == JobStatus::Running)
            {
                return Err(CoreError::BadRequest("该模型正在下载中".to_string()));
            }
            jobs.insert(entry.id.clone(), Job::running());
        }

        let jobs = self.jobs.clone();
        let settings = self.settings.clone();
        let manager = self.manager.clone();
        let models_root = self.models_root.clone();
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
}

impl From<&Job> for JobView {
    fn from(job: &Job) -> Self {
        Self {
            status: job.status,
            started_at: job.started_at,
            finished_at: job.finished_at,
            log: tail(&job.log, LOG_TAIL),
            stage: job.stage.clone(),
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
    jobs: Arc<Mutex<HashMap<String, Job>>>,
    settings: Arc<SettingsStore>,
    manager: Arc<EngineManager>,
) {
    let settings = settings.get();
    let system_proxy = match settings.proxy_mode {
        ProxyMode::System => proxy::system(),
        ProxyMode::Manual | ProxyMode::Direct => None,
    };

    let mut cmd = Command::new("bash");
    cmd.arg(&script)
        .env("LAVEROCK_MODELS_DIR", &models_root)
        .env("LAVEROCK_SCRIPTS_DIR", &script_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
            description: String::new(),
            size: String::new(),
            languages: Vec::new(),
            capabilities: Vec::new(),
            script: "fake.sh".to_string(),
            env: BTreeMap::new(),
            note: None,
        };
        run_install(
            entry,
            script,
            scripts,
            models.clone(),
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
}
