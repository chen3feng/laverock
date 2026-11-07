//! Engine registry: scans model directories for manifests and lazily
//! builds engines on first use.
//!
//! The manager is executor-agnostic. Inference is blocking ONNX work;
//! the embedding application should call it from a blocking thread.

use crate::{
    audiocpp::{AudioCppEngine, Family},
    chatterbox::ChatterboxEngine,
    kokoro::KokoroEngine,
    longcat::LongCatEngine,
    ming3tts::Ming3TtsEngine,
    mosstts::MossTtsEngine,
    qwen3tts::Qwen3TtsEngine,
    zipvoice::ZipVoiceEngine,
    Audio, CoreError, EngineKind, Localized, Manifest, ParamSpec, Result,
};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

/// Progress reported by an engine while it works. Long engines (the
/// Python Qwen3-TTS runner) emit stages and codec-step counts so a UI can
/// show something more informative than a spinner.
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// Human-readable phase, e.g. "model loaded; generating".
    Stage(String),
    /// Number of codec steps generated so far.
    Steps(u32),
}

/// Optional per-request callback. Engines that cannot report progress
/// simply never call it.
pub type ProgressSink = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

/// A chunk of audio produced while a streaming request is still running.
///
/// Unlike [`ProgressSink`], this channel is lossless: returning `false`
/// tells the engine the consumer has gone away, so it can stop early.
pub type AudioSink = Arc<dyn Fn(Audio) -> bool + Send + Sync>;

/// A voice-conversion request: transform `input_samples` into the voice
/// described by the reference clip.
///
/// Kept separate from [`SynthRequest`] because the input is audio rather
/// than text; TTS engines reject it with the default `convert`.
pub struct ConvertRequest {
    pub input_samples: Vec<f32>,
    pub input_sr: i32,
    pub prompt_samples: Vec<f32>,
    pub prompt_sr: i32,
    pub params: std::collections::BTreeMap<String, serde_json::Value>,
    pub progress: Option<ProgressSink>,
}

/// A loaded, ready-to-run engine.
pub trait SpeechEngine: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn synthesize(&self, req: &SynthRequest) -> Result<Audio>;

    /// Convert an existing clip into this engine's target voice.
    ///
    /// Only voice-conversion engines implement this; the default rejects
    /// the request so callers get a clear error instead of silence.
    fn convert(&self, _req: &ConvertRequest) -> Result<Audio> {
        Err(CoreError::BadRequest(format!(
            "{} does not support voice conversion",
            self.name()
        )))
    }

    /// PID of a resident child process, when the engine keeps one between
    /// requests. In-process engines and one-shot subprocesses return `None`.
    fn resident_pid(&self) -> Option<u32> {
        None
    }

    /// Give back memory held between requests, if the engine keeps any.
    ///
    /// The manager calls this on every engine except the one about to run,
    /// so switching models releases the previous model's weights instead of
    /// holding them all at once. Implementations must be cheap when nothing
    /// is resident and must not interrupt a request in flight; the default
    /// is a no-op for engines whose weights live in this process.
    fn unload(&self) {}

    /// How this engine wants long text split before one call. `None` means
    /// it already chunks internally (or accepts any length), so the manager
    /// hands it the text unchanged. Engines that return a policy get
    /// sentence-aware pieces synthesized one by one and joined with a pause.
    fn long_text_policy(&self) -> Option<crate::segment::LongTextPolicy> {
        None
    }
}

/// A synthesis request. Reference audio is required for cloning engines.
#[derive(Clone)]
pub struct SynthRequest {
    pub text: String,
    pub prompt_text: Option<String>,
    pub prompt_samples: Vec<f32>,
    pub prompt_sr: i32,
    pub speed: f32,
    pub num_steps: i32,
    /// Engine-specific options declared by the manifest parameter list
    /// (e.g. voxcpm2 cfg/temperature/seed). Values are validated server
    /// side against the ParamSpec range.
    pub params: std::collections::BTreeMap<String, serde_json::Value>,
    /// Source audio for editing or voice-conversion tasks. Empty for plain
    /// text-to-speech.
    pub input_samples: Vec<f32>,
    pub input_sr: i32,
    /// Optional progress callback; absent for engines/clients without UI.
    pub progress: Option<ProgressSink>,
    /// Optional lossless audio sink for streaming synthesis. Engines that
    /// cannot stream leave it unused and return the whole clip as usual.
    pub audio: Option<AudioSink>,
}

impl std::fmt::Debug for SynthRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SynthRequest")
            .field("text", &self.text)
            .field("prompt_text", &self.prompt_text)
            .field("prompt_samples", &self.prompt_samples.len())
            .field("prompt_sr", &self.prompt_sr)
            .field("speed", &self.speed)
            .field("num_steps", &self.num_steps)
            .field("params", &self.params)
            .field("input_samples", &self.input_samples.len())
            .field("input_sr", &self.input_sr)
            .field("progress", &self.progress.is_some())
            .field("audio", &self.audio.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineInfo {
    pub id: String,
    pub kind: String,
    /// What the model does (synthesis, music, transcription, …). The UI
    /// groups models and picks a workspace from this, not from `kind`.
    pub task: crate::TaskKind,
    pub name: String,
    /// Localized one-line description from the manifest; empty when the
    /// manifest has none (the server fills built-in models from the
    /// catalog).
    pub description: Localized,
    pub languages: Vec<String>,
    pub capabilities: Vec<String>,
    pub loaded: bool,
    /// Whether the engine keeps a long-lived child process between
    /// requests (the audio.cpp resident server).
    pub resident: bool,
    /// PID of that child, when `resident`.
    pub pid: Option<u32>,
    /// Runtime that executes the engine (`onnx`, `audiocpp`, `llama.cpp`,
    /// `python`), shown in the resource panel.
    pub backend: String,
    pub defaults: serde_json::Value,
    pub parameters: Vec<ParamSpec>,
    /// How much of a voice's reference audio this engine uses.
    pub reference_limit_secs: f32,
    /// Another models directory earlier in the search path already provides
    /// this id; this copy is listed for diagnostics but cannot be loaded.
    pub shadowed: bool,
}

/// One scanned `engine.json`, with the directory it came from. Engines are
/// built relative to that directory, not to the models root.
#[derive(Debug, Clone)]
struct ManifestEntry {
    manifest: Manifest,
    dir: PathBuf,
    /// A directory earlier in the search path already provides this id.
    shadowed: bool,
}

pub struct EngineManager {
    roots: Vec<PathBuf>,
    entries: RwLock<Vec<ManifestEntry>>,
    loaded: Mutex<HashMap<String, Arc<dyn SpeechEngine>>>,
}

impl EngineManager {
    /// Scan one models directory. Every immediate subdirectory containing an
    /// `engine.json` is a registered engine.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        Self::open_all(vec![root.into()])
    }

    /// Scan several models directories in order. The first directory that
    /// provides an engine id wins; later copies are reported as shadowed so
    /// a shared read-only weights library and a local overlay can coexist.
    pub fn open_all(roots: Vec<PathBuf>) -> Result<Self> {
        let entries = scan_all(&roots)?;
        Ok(Self {
            roots,
            entries: RwLock::new(entries),
            loaded: Mutex::new(HashMap::new()),
        })
    }

    /// Re-scan the models directories, e.g. after a model was downloaded
    /// from the UI. Already-loaded engines stay loaded as long as their
    /// manifest still exists.
    pub fn reload(&self) -> Result<usize> {
        let entries = scan_all(&self.roots)?;
        let count = entries.iter().filter(|e| !e.shadowed).count();
        let ids: HashSet<String> = entries
            .iter()
            .filter(|e| !e.shadowed)
            .map(|e| e.manifest.id.clone())
            .collect();

        *self.entries.write().expect("manifest lock poisoned") = entries;
        self.loaded
            .lock()
            .expect("engine lock poisoned")
            .retain(|id, _| ids.contains(id));
        Ok(count)
    }

    pub fn len(&self) -> usize {
        self.entries
            .read()
            .expect("manifest lock poisoned")
            .iter()
            .filter(|e| !e.shadowed)
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn list(&self) -> Vec<EngineInfo> {
        let entries = self.entries.read().expect("manifest lock poisoned");
        // Clone the handles out: querying a resident engine locks its own
        // state, which should not happen under the registry lock.
        let loaded: HashMap<String, Arc<dyn SpeechEngine>> = self
            .loaded
            .lock()
            .expect("engine lock poisoned")
            .iter()
            .map(|(id, engine)| (id.clone(), engine.clone()))
            .collect();
        entries
            .iter()
            .map(|entry| {
                let m = &entry.manifest;
                let handle = if entry.shadowed {
                    None
                } else {
                    loaded.get(&m.id)
                };
                let pid = handle.and_then(|engine| engine.resident_pid());
                EngineInfo {
                    id: m.id.clone(),
                    kind: m.kind.as_str().to_string(),
                    task: m.task(),
                    name: m.name.clone(),
                    description: m.description.clone(),
                    languages: m.languages.clone(),
                    capabilities: m.capabilities.clone(),
                    loaded: handle.is_some(),
                    resident: pid.is_some(),
                    pid,
                    backend: m.kind.backend().to_string(),
                    defaults: serde_json::to_value(&m.defaults).unwrap_or(serde_json::json!({})),
                    parameters: m.parameters_or_defaults(),
                    reference_limit_secs: m.reference_limit_secs(),
                    shadowed: entry.shadowed,
                }
            })
            .collect()
    }

    /// Tunable parameters declared by the manifest, or the kind
    /// defaults. Used both for the UI and request validation.
    pub fn parameters(&self, id: &str) -> Result<Vec<ParamSpec>> {
        Ok(self.manifest(id)?.parameters_or_defaults())
    }

    /// Manifests are cloned out so no lock is held while an engine is
    /// constructed (construction can take seconds).
    fn manifest(&self, id: &str) -> Result<Manifest> {
        self.entries
            .read()
            .expect("manifest lock poisoned")
            .iter()
            .find(|e| !e.shadowed && e.manifest.id == id)
            .map(|e| e.manifest.clone())
            .ok_or_else(|| CoreError::NotFound(id.to_string()))
    }

    /// Return the engine as a shared handle, constructing it on first
    /// call. Construction is serialized; inference itself is not.
    pub fn engine(&self, id: &str) -> Result<Arc<dyn SpeechEngine>> {
        if let Some(engine) = self.loaded.lock().expect("engine lock poisoned").get(id) {
            return Ok(engine.clone());
        }

        let entry = self
            .entries
            .read()
            .expect("manifest lock poisoned")
            .iter()
            .find(|e| !e.shadowed && e.manifest.id == id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(id.to_string()))?;
        let manifest = entry.manifest;
        let dir = entry.dir;
        let engine: Arc<dyn SpeechEngine> = match manifest.kind {
            EngineKind::ZipVoice => Arc::new(ZipVoiceEngine::build(&dir, &manifest)?),
            EngineKind::VoxCpm2 => {
                Arc::new(AudioCppEngine::build(&dir, &manifest, Family::VoxCpm2)?)
            }
            EngineKind::Qwen3Tts => Arc::new(Qwen3TtsEngine::build(&dir, &manifest)?),
            EngineKind::LongCat => Arc::new(LongCatEngine::build(&dir, &manifest)?),
            EngineKind::Dots => Arc::new(AudioCppEngine::build(&dir, &manifest, Family::Dots)?),
            EngineKind::CosyVoice => {
                Arc::new(AudioCppEngine::build(&dir, &manifest, Family::CosyVoice)?)
            }
            EngineKind::Kokoro => Arc::new(KokoroEngine::build(&dir, &manifest)?),
            EngineKind::Ming3Tts => Arc::new(Ming3TtsEngine::build(&dir, &manifest)?),
            EngineKind::Chatterbox => Arc::new(ChatterboxEngine::build(&dir, &manifest)?),
            EngineKind::IndexTts2 => {
                Arc::new(AudioCppEngine::build(&dir, &manifest, Family::IndexTts2)?)
            }
            EngineKind::MossTts => Arc::new(MossTtsEngine::build(&dir, &manifest)?),
            EngineKind::Music => Arc::new(AudioCppEngine::build(&dir, &manifest, Family::AceStep)?),
            // STT models are discovered by the transcriber, not built as
            // synthesis engines.
            EngineKind::Stt => {
                return Err(CoreError::Load("unsupported synthesis kind: stt".into()))
            }
        };

        let mut loaded = self.loaded.lock().expect("engine lock poisoned");
        // A parallel first request may have built one meanwhile; keep
        // the first handle so callers share a warmed engine.
        Ok(loaded.entry(id.to_string()).or_insert(engine).clone())
    }

    /// Convenience: load if needed and run one synthesis request.
    pub fn synthesize(&self, id: &str, req: &SynthRequest) -> Result<Audio> {
        let engine = self.engine(id)?;
        self.release_others(id);
        match engine.long_text_policy() {
            Some(policy) => crate::segment::synthesize_long(engine.as_ref(), req, policy),
            None => engine.synthesize(req),
        }
    }

    /// The engine's long-text policy, if it declares one. Used by the server
    /// to decide whether it must split text itself for streaming.
    pub fn long_text_policy(&self, id: &str) -> Option<crate::segment::LongTextPolicy> {
        self.engine(id)
            .ok()
            .and_then(|engine| engine.long_text_policy())
    }

    /// Convenience: load if needed and run one voice-conversion request.
    pub fn convert(&self, id: &str, req: &ConvertRequest) -> Result<Audio> {
        let engine = self.engine(id)?;
        self.release_others(id);
        engine.convert(req)
    }

    /// PID of the resident child process an engine keeps between requests,
    /// if any. `None` for engines that run in-process or spawn per request.
    pub fn resident_pid(&self, id: &str) -> Option<u32> {
        // Clone the handle out first: querying an engine can lock its own
        // state, which should not happen under the registry lock.
        let engine = self
            .loaded
            .lock()
            .expect("engine lock poisoned")
            .get(id)
            .cloned();
        engine.and_then(|engine| engine.resident_pid())
    }

    /// Release whatever the other engines keep loaded.
    ///
    /// One model at a time is the only thing that fits on an 8 GB card, and
    /// keeping several resident audio.cpp servers alive is exactly what ran
    /// out of VRAM. Engines that hold nothing between requests ignore it.
    fn release_others(&self, keep: &str) {
        // Cloned out first: unloading can stop a child process and wait for
        // it, which must not happen while the registry lock is held.
        let others: Vec<Arc<dyn SpeechEngine>> = {
            let loaded = self.loaded.lock().expect("engine lock poisoned");
            loaded
                .iter()
                .filter(|(id, _)| id.as_str() != keep)
                .map(|(_, engine)| engine.clone())
                .collect()
        };
        for other in others {
            other.unload();
        }
    }

    /// Drop a loaded engine and stop anything it keeps running, so the
    /// model directory can be deleted (Windows refuses to remove files a
    /// resident server still holds open).
    pub fn unload(&self, id: &str) {
        let engine = self.loaded.lock().expect("engine lock poisoned").remove(id);
        if let Some(engine) = engine {
            engine.unload();
        }
    }

    /// Release every loaded engine. Used by the resource panel's
    /// "unload all" action.
    pub fn unload_all(&self) {
        // Drain under the lock, then unload outside it: stopping a child
        // process can wait, and must not block other registry users.
        let engines: Vec<Arc<dyn SpeechEngine>> = self
            .loaded
            .lock()
            .expect("engine lock poisoned")
            .drain()
            .map(|(_, engine)| engine)
            .collect();
        for engine in engines {
            engine.unload();
        }
    }

    pub fn manifest_defaults(&self, id: &str) -> Result<crate::manifest::SynthDefaults> {
        Ok(self.manifest(id)?.defaults)
    }

    /// Longest reference clip the engine uses (see
    /// [`Manifest::reference_limit_secs`]).
    pub fn reference_limit_secs(&self, id: &str) -> Result<f32> {
        Ok(self.manifest(id)?.reference_limit_secs())
    }

    /// The first (highest-priority) models directory.
    pub fn root(&self) -> &Path {
        self.roots
            .first()
            .map(PathBuf::as_path)
            .unwrap_or(Path::new(""))
    }

    /// Every models directory, highest priority first.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
}

/// Read every valid manifest under every root, highest priority first, and
/// flag ids that a previous root already provided.
fn scan_all(roots: &[PathBuf]) -> Result<Vec<ManifestEntry>> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for root in roots {
        for (dir, manifest) in scan(root)? {
            let shadowed = !seen.insert(manifest.id.clone());
            if shadowed {
                tracing::warn!(
                    engine = %manifest.id,
                    dir = %dir.display(),
                    "shadowed by an earlier models directory"
                );
            }
            entries.push(ManifestEntry {
                manifest,
                dir,
                shadowed,
            });
        }
    }
    Ok(entries)
}

/// Read every valid manifest under `root`, sorted by engine id.
fn scan(root: &Path) -> Result<Vec<(PathBuf, Manifest)>> {
    let mut manifests = Vec::new();

    if root.is_dir() {
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            // `DirEntry::file_type` describes the link itself, so a model
            // directory symlinked into place would be skipped. `fs::metadata`
            // follows it, which is what "is this a model directory" means.
            if !fs::metadata(entry.path())
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let manifest_path = entry.path().join("engine.json");
            if !manifest_path.is_file() {
                continue;
            }
            match fs::read_to_string(&manifest_path) {
                Ok(raw) => match serde_json::from_str::<Manifest>(&raw) {
                    Ok(m) if m.kind == crate::manifest::EngineKind::Stt => {
                        // Speech-to-text models live in the same directory but
                        // are not synthesis engines; the transcriber finds them.
                        tracing::debug!(model = %m.id, "skipping STT model in engine scan");
                    }
                    Ok(m) => {
                        tracing::info!(engine = %m.id, kind = ?m.kind, "registered engine");
                        manifests.push((entry.path(), m));
                    }
                    Err(e) => tracing::warn!(
                        dir = %entry.path().display(),
                        "skipping invalid engine.json: {e}"
                    ),
                },
                Err(e) => tracing::warn!("cannot read {}: {e}", manifest_path.display()),
            }
        }
    } else {
        tracing::warn!(dir = %root.display(), "models directory does not exist yet");
    }

    manifests.sort_by(|a, b| a.1.id.cmp(&b.1.id));
    Ok(manifests)
}

// Unix-only: the test needs a symlink, and creating one is not the subject
// here (Windows would need a privilege or a developer mode).
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{
        "id": "zipvoice",
        "kind": "zipvoice",
        "name": "ZipVoice"
    }"#;

    /// `models/` may hold symlinks to a shared weight store (the dev and
    /// desktop installs do); they must register like real directories.
    #[test]
    fn scan_follows_symlinked_engine_dirs() {
        let base = std::env::temp_dir().join(format!("laverock-scan-{}", std::process::id()));
        let store = base.join("store").join("zipvoice");
        let models = base.join("models");
        fs::create_dir_all(&store).expect("store");
        fs::create_dir_all(&models).expect("models");
        fs::write(store.join("engine.json"), MANIFEST).expect("manifest");
        std::os::unix::fs::symlink(&store, models.join("zipvoice")).expect("symlink");
        // A dangling link and a plain file are not engines.
        std::os::unix::fs::symlink(base.join("missing"), models.join("dangling"))
            .expect("dangling");
        fs::write(models.join("notes.txt"), "not an engine").expect("file");

        let found = scan(&models).expect("scan");
        assert_eq!(
            found.len(),
            1,
            "found: {:?}",
            found.iter().map(|(_, m)| &m.id)
        );
        assert_eq!(found[0].1.id, "zipvoice");

        fs::remove_dir_all(&base).ok();
    }

    /// Two models directories: the first one that has an id wins, and the
    /// later copy is listed as shadowed rather than loaded.
    #[test]
    fn later_roots_shadow_duplicate_ids() {
        let base = std::env::temp_dir().join(format!("laverock-roots-{}", std::process::id()));
        let first = base.join("first");
        let second = base.join("second");
        fs::create_dir_all(first.join("zipvoice")).expect("first");
        fs::create_dir_all(second.join("zipvoice")).expect("second");
        fs::write(
            first.join("zipvoice").join("engine.json"),
            r#"{"id":"zipvoice","kind":"zipvoice","name":"first"}"#,
        )
        .expect("first manifest");
        fs::write(
            second.join("zipvoice").join("engine.json"),
            r#"{"id":"zipvoice","kind":"zipvoice","name":"second"}"#,
        )
        .expect("second manifest");

        let manager = EngineManager::open_all(vec![first, second]).expect("manager");
        assert_eq!(manager.len(), 1);
        let list = manager.list();
        let copies: Vec<_> = list.iter().filter(|e| e.id == "zipvoice").collect();
        assert_eq!(copies.len(), 2);
        assert_eq!(copies[0].name, "first");
        assert!(!copies[0].shadowed);
        assert_eq!(copies[1].name, "second");
        assert!(copies[1].shadowed);
        assert_eq!(manager.root(), base.join("first"));

        fs::remove_dir_all(&base).ok();
    }
}

#[cfg(test)]
mod unload_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Records how often the manager asked it to release its model.
    struct MockEngine {
        id: String,
        unloads: Arc<AtomicUsize>,
    }

    impl SpeechEngine for MockEngine {
        fn id(&self) -> &str {
            &self.id
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn synthesize(&self, _req: &SynthRequest) -> Result<Audio> {
            Ok(Audio::mono(vec![0.0; 16], 24_000))
        }

        fn unload(&self) {
            self.unloads.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn request() -> SynthRequest {
        SynthRequest {
            text: "hello".into(),
            prompt_text: None,
            prompt_samples: Vec::new(),
            prompt_sr: 24_000,
            speed: 1.0,
            num_steps: 1,
            params: Default::default(),
            input_samples: Vec::new(),
            input_sr: 0,
            progress: None,
            audio: None,
        }
    }

    /// `unload` releases the handle and `list` reports the new state.
    #[test]
    fn unload_reports_and_drops_the_engine() {
        let root =
            std::env::temp_dir().join(format!("laverock-unload-list-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("zipvoice")).expect("model dir");
        fs::write(
            root.join("zipvoice/engine.json"),
            r#"{"id":"zipvoice","kind":"zipvoice","name":"ZipVoice"}"#,
        )
        .expect("manifest");

        let manager = EngineManager::open(&root).expect("manager");
        let unloads = Arc::new(AtomicUsize::new(0));
        manager.loaded.lock().expect("engine lock").insert(
            "zipvoice".into(),
            Arc::new(MockEngine {
                id: "zipvoice".into(),
                unloads: unloads.clone(),
            }),
        );

        let info = manager
            .list()
            .into_iter()
            .find(|engine| engine.id == "zipvoice")
            .expect("listed");
        assert!(info.loaded);
        assert!(!info.resident);
        assert_eq!(info.pid, None);
        assert_eq!(info.backend, "onnx");

        manager.unload("zipvoice");
        let info = manager
            .list()
            .into_iter()
            .find(|engine| engine.id == "zipvoice")
            .expect("listed");
        assert!(!info.loaded);
        assert_eq!(unloads.load(Ordering::SeqCst), 1);

        fs::remove_dir_all(&root).ok();
    }

    /// Only one model's weights are kept resident: a request for one engine
    /// releases every other loaded engine, and never itself.
    #[test]
    fn switching_engines_unloads_the_others() {
        let root = std::env::temp_dir().join(format!("laverock-unload-{}", std::process::id()));
        let manager = EngineManager::open(&root).expect("manager");
        let first_unloads = Arc::new(AtomicUsize::new(0));
        let second_unloads = Arc::new(AtomicUsize::new(0));
        {
            let mut loaded = manager.loaded.lock().expect("engine lock");
            loaded.insert(
                "first".into(),
                Arc::new(MockEngine {
                    id: "first".into(),
                    unloads: first_unloads.clone(),
                }),
            );
            loaded.insert(
                "second".into(),
                Arc::new(MockEngine {
                    id: "second".into(),
                    unloads: second_unloads.clone(),
                }),
            );
        }

        manager.synthesize("first", &request()).expect("first");
        assert_eq!(first_unloads.load(Ordering::SeqCst), 0);
        assert_eq!(second_unloads.load(Ordering::SeqCst), 1);

        manager.synthesize("second", &request()).expect("second");
        assert_eq!(first_unloads.load(Ordering::SeqCst), 1);
        assert_eq!(second_unloads.load(Ordering::SeqCst), 1);
    }
}

#[cfg(all(test, unix))]
mod music_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A `kind: "music"` manifest registers as a music engine and builds
    /// the shared audio.cpp engine with the ACE-Step family.
    #[test]
    fn music_manifests_build_an_ace_step_engine() {
        let root = std::env::temp_dir().join(format!(
            "laverock-music-manager-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("ace-step");
        fs::create_dir_all(dir.join("turbo")).unwrap();
        let cli = dir.join("fake_cli.sh");
        fs::write(&cli, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(&cli).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&cli, perms).unwrap();
        fs::write(dir.join("turbo/ace-step.gguf"), b"x").unwrap();
        fs::write(
            dir.join("engine.json"),
            format!(
                r#"{{
                  "id": "ace-step",
                  "kind": "music",
                  "name": "ACE-Step",
                  "music": {{
                    "binary": "{}",
                    "model": "turbo",
                    "backend": "cpu",
                    "family": "ace_step",
                    "task": "gen"
                  }},
                  "defaults": {{ "speed": 1.0, "num_steps": 8 }}
                }}"#,
                cli.display()
            ),
        )
        .unwrap();

        let manager = EngineManager::open(&root).expect("manager");
        let info = manager
            .list()
            .into_iter()
            .find(|e| e.id == "ace-step")
            .expect("registered");
        assert_eq!(info.kind, "music");
        assert_eq!(info.reference_limit_secs, 0.0);
        let params = manager.parameters("ace-step").unwrap();
        assert!(params.iter().any(|p| p.name == "lyrics"));
        assert!(params.iter().any(|p| p.name == "duration_sec"));
        assert!(params.iter().any(|p| p.name == "task_route"));

        // The model path is a directory (a GGUF package), not a file.
        let engine = manager.engine("ace-step").expect("engine builds");
        assert_eq!(engine.id(), "ace-step");
        let _ = fs::remove_dir_all(&root);
    }
}
