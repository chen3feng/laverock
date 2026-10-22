//! Engine registry: scans model directories for manifests and lazily
//! builds engines on first use.
//!
//! The manager is executor-agnostic. Inference is blocking ONNX work;
//! the embedding application should call it from a blocking thread.

use crate::{zipvoice::ZipVoiceEngine, Audio, CoreError, EngineKind, Manifest, Result};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// A loaded, ready-to-run engine.
pub trait SpeechEngine: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn synthesize(&self, req: &SynthRequest) -> Result<Audio>;
}

/// A synthesis request. Reference audio is required for cloning engines.
#[derive(Debug, Clone)]
pub struct SynthRequest {
    pub text: String,
    pub prompt_text: Option<String>,
    pub prompt_samples: Vec<f32>,
    pub prompt_sr: i32,
    pub speed: f32,
    pub num_steps: i32,
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
}

pub struct EngineManager {
    root: PathBuf,
    manifests: Vec<Manifest>,
    loaded: Mutex<HashMap<String, Arc<dyn SpeechEngine>>>,
}

impl EngineManager {
    /// Scan `root`: every immediate subdirectory containing an
    /// `engine.json` is a registered engine.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let mut manifests = Vec::new();

        if root.is_dir() {
            for entry in fs::read_dir(&root)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let manifest_path = entry.path().join("engine.json");
                if !manifest_path.is_file() {
                    continue;
                }
                match fs::read_to_string(&manifest_path) {
                    Ok(raw) => match serde_json::from_str::<Manifest>(&raw) {
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

        Ok(Self {
            root,
            manifests,
            loaded: Mutex::new(HashMap::new()),
        })
    }

    pub fn len(&self) -> usize {
        self.manifests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }

    pub fn list(&self) -> Vec<EngineInfo> {
        let loaded = self.loaded.lock().expect("engine lock poisoned");
        self.manifests
            .iter()
            .map(|m| EngineInfo {
                id: m.id.clone(),
                kind: m.kind.as_str().to_string(),
                name: m.name.clone(),
                languages: m.languages.clone(),
                capabilities: m.capabilities.clone(),
                loaded: loaded.contains_key(&m.id),
                defaults: serde_json::to_value(&m.defaults).unwrap_or(serde_json::json!({})),
            })
            .collect()
    }

    fn manifest(&self, id: &str) -> Result<&Manifest> {
        self.manifests
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| CoreError::NotFound(id.to_string()))
    }

    /// Return the engine as a shared handle, constructing it on first
    /// call. Construction is serialized; inference itself is not.
    pub fn engine(&self, id: &str) -> Result<Arc<dyn SpeechEngine>> {
        let mut loaded = self.loaded.lock().expect("engine lock poisoned");
        if let Some(engine) = loaded.get(id) {
            return Ok(engine.clone());
        }

        let manifest = self.manifest(id)?.clone();
        let dir: PathBuf = self.root.join(&manifest.id);
        let engine: Arc<dyn SpeechEngine> = match manifest.kind {
            EngineKind::ZipVoice => Arc::new(ZipVoiceEngine::build(&dir, &manifest)?),
        };
        loaded.insert(id.to_string(), engine.clone());
        Ok(engine)
    }

    /// Convenience: load if needed and run one synthesis request.
    pub fn synthesize(&self, id: &str, req: &SynthRequest) -> Result<Audio> {
        let engine = self.engine(id)?;
        engine.synthesize(req)
    }

    pub fn manifest_defaults(&self, id: &str) -> Result<crate::manifest::SynthDefaults> {
        Ok(self.manifest(id)?.defaults.clone())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
