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
    Audio, CoreError, EngineKind, Manifest, ParamSpec, Result,
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

/// A loaded, ready-to-run engine.
pub trait SpeechEngine: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn synthesize(&self, req: &SynthRequest) -> Result<Audio>;
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
    /// Optional progress callback; absent for engines/clients without UI.
    pub progress: Option<ProgressSink>,
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
            .field("progress", &self.progress.is_some())
            .finish()
    }
}

#[derive(Debug, Serialize)]
pub struct EngineInfo {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub languages: Vec<String>,
    pub capabilities: Vec<String>,
    pub loaded: bool,
    pub defaults: serde_json::Value,
    pub parameters: Vec<ParamSpec>,
}

pub struct EngineManager {
    root: PathBuf,
    manifests: RwLock<Vec<Manifest>>,
    loaded: Mutex<HashMap<String, Arc<dyn SpeechEngine>>>,
}

impl EngineManager {
    /// Scan `root`: every immediate subdirectory containing an
    /// `engine.json` is a registered engine.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let manifests = scan(&root)?;
        Ok(Self {
            root,
            manifests: RwLock::new(manifests),
            loaded: Mutex::new(HashMap::new()),
        })
    }

    /// Re-scan the models directory, e.g. after a model was downloaded
    /// from the UI. Already-loaded engines stay loaded as long as their
    /// manifest still exists.
    pub fn reload(&self) -> Result<usize> {
        let manifests = scan(&self.root)?;
        let count = manifests.len();
        let ids: HashSet<String> = manifests.iter().map(|m| m.id.clone()).collect();

        *self.manifests.write().expect("manifest lock poisoned") = manifests;
        self.loaded
            .lock()
            .expect("engine lock poisoned")
            .retain(|id, _| ids.contains(id));
        Ok(count)
    }

    pub fn len(&self) -> usize {
        self.manifests.read().expect("manifest lock poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn list(&self) -> Vec<EngineInfo> {
        let manifests = self.manifests.read().expect("manifest lock poisoned");
        let loaded = self.loaded.lock().expect("engine lock poisoned");
        manifests
            .iter()
            .map(|m| EngineInfo {
                id: m.id.clone(),
                kind: m.kind.as_str().to_string(),
                name: m.name.clone(),
                languages: m.languages.clone(),
                capabilities: m.capabilities.clone(),
                loaded: loaded.contains_key(&m.id),
                defaults: serde_json::to_value(&m.defaults).unwrap_or(serde_json::json!({})),
                parameters: m.parameters_or_defaults(),
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
        self.manifests
            .read()
            .expect("manifest lock poisoned")
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(id.to_string()))
    }

    /// Return the engine as a shared handle, constructing it on first
    /// call. Construction is serialized; inference itself is not.
    pub fn engine(&self, id: &str) -> Result<Arc<dyn SpeechEngine>> {
        if let Some(engine) = self.loaded.lock().expect("engine lock poisoned").get(id) {
            return Ok(engine.clone());
        }

        let manifest = self.manifest(id)?;
        let dir: PathBuf = self.root.join(&manifest.id);
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
            EngineKind::MossTts => Arc::new(MossTtsEngine::build(&dir, &manifest)?),
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
        engine.synthesize(req)
    }

    pub fn manifest_defaults(&self, id: &str) -> Result<crate::manifest::SynthDefaults> {
        Ok(self.manifest(id)?.defaults)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Read every valid manifest under `root`, sorted by engine id.
fn scan(root: &Path) -> Result<Vec<Manifest>> {
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
                        manifests.push(m);
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

    manifests.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(manifests)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{
        "id": "zipvoice",
        "kind": "zipvoice",
        "name": "ZipVoice"
    }"#;

    /// `models/` may hold symlinks to a shared weight store (the dev and
    /// desktop installs do); they must register like real directories.
    #[cfg(unix)]
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
        assert_eq!(found.len(), 1, "found: {:?}", found.iter().map(|m| &m.id));
        assert_eq!(found[0].id, "zipvoice");

        fs::remove_dir_all(&base).ok();
    }
}
