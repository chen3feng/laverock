//! On-device speech-to-text for reference clips.
//!
//! A recorder clip is normally transcribed by hand, which is the most
//! tedious step of the cloning flow. The transcriber wraps sherpa-onnx's
//! offline recognizer (SenseVoice: zh/en/ja/ko/yue, with inverse text
//! normalization), loading the model lazily on first use from a directory
//! whose manifest is `kind = "stt"`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig};

use crate::i18n::{self, Key};
use crate::{audio::decode_wav, manifest::Manifest, CoreError, Result};

/// Holds an optional loaded recognizer; one model at a time.
pub struct Transcriber {
    models_root: PathBuf,
    inner: Mutex<Option<Recognizer>>,
}

struct Recognizer {
    recognizer: OfflineRecognizer,
}

impl Transcriber {
    pub fn new(models_root: impl AsRef<Path>) -> Self {
        Self {
            models_root: models_root.as_ref().to_path_buf(),
            inner: Mutex::new(None),
        }
    }

    /// Whether an STT model directory exists (cheap, no model load).
    pub fn available(&self) -> bool {
        model_manifest(&self.models_root).is_ok()
    }

    /// Transcribe a WAV clip to text, loading the model on first call.
    pub fn transcribe(&self, wav: &[u8]) -> Result<String> {
        let (samples, sample_rate) = decode_wav(wav)?;
        if samples.is_empty() {
            return Err(CoreError::UserError(i18n::Message::new(Key::EmptyClip)));
        }

        let mut guard = self
            .inner
            .lock()
            .map_err(|_| CoreError::Load("transcriber lock poisoned".into()))?;
        if guard.is_none() {
            *guard = Some(Recognizer::load(&self.models_root)?);
        }
        let recognizer = guard.as_ref().unwrap();

        let stream = recognizer.recognizer.create_stream();
        stream.accept_waveform(sample_rate, &samples);
        recognizer.recognizer.decode(&stream);
        let text = stream
            .get_result()
            .map(|r| r.text.trim().to_string())
            .unwrap_or_default();
        Ok(text)
    }
}

impl Recognizer {
    fn load(models_root: &Path) -> Result<Self> {
        let (manifest, dir) = model_manifest(models_root)?;
        let stt = manifest
            .stt
            .as_ref()
            .ok_or_else(|| CoreError::Load("stt manifest missing an [stt] section".into()))?;
        if stt.engine != "sense_voice" {
            return Err(CoreError::Load(format!(
                "unsupported STT engine: {}",
                stt.engine
            )));
        }

        let model = dir.join(&stt.model);
        let tokens = dir.join(&stt.tokens);
        for path in [&model, &tokens] {
            if !path.exists() {
                return Err(CoreError::Load(format!(
                    "missing STT file {}",
                    path.display()
                )));
            }
        }

        let mut config = OfflineRecognizerConfig::default();
        config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(model.to_string_lossy().into_owned()),
            language: Some(stt.language.clone()),
            use_itn: stt.use_itn,
        };
        config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
        config.model_config.num_threads = stt.num_threads.max(1);
        config.model_config.debug = false;

        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| CoreError::Load("failed to create the SenseVoice recognizer".into()))?;
        Ok(Self { recognizer })
    }
}

/// Find the newest STT model directory: returns its manifest and directory.
pub fn model_manifest(models_root: &Path) -> Result<(Manifest, PathBuf)> {
    let mut found: Vec<(PathBuf, Manifest)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(models_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("engine.json");
            if let Ok(raw) = std::fs::read_to_string(&manifest_path) {
                if let Ok(manifest) = serde_json::from_str::<Manifest>(&raw) {
                    if manifest.kind.as_str() == "stt" {
                        found.push((path, manifest));
                    }
                }
            }
        }
    }
    match found.into_iter().max_by_key(|(p, _)| {
        std::fs::metadata(p.join("engine.json"))
            .and_then(|m| m.modified())
            .ok()
    }) {
        Some((dir, manifest)) => Ok((manifest, dir)),
        None => Err(CoreError::Load(
            "no speech-to-text model installed; download SenseVoice from the Models page".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dir_has_no_model() {
        let dir = std::env::temp_dir().join(format!("laverock-stt-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(model_manifest(&dir).is_err());
        assert!(!Transcriber::new(&dir).available());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_stt_manifest_and_skips_tts() {
        let dir = std::env::temp_dir().join(format!("laverock-stt-find-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("tts-model")).unwrap();
        std::fs::write(
            dir.join("tts-model/engine.json"),
            r#"{"id":"t","kind":"kokoro","name":"t","kokoro":{}}"#,
        )
        .unwrap();
        let stt = dir.join("stt-model");
        std::fs::create_dir_all(&stt).unwrap();
        std::fs::write(
            stt.join("engine.json"),
            r#"{"id":"s","kind":"stt","name":"s",
               "stt":{"model":"model.int8.onnx","tokens":"tokens.txt"}}"#,
        )
        .unwrap();

        let (manifest, path) = model_manifest(&dir).unwrap();
        assert_eq!(manifest.kind, crate::manifest::EngineKind::Stt);
        assert_eq!(path, stt);
        assert!(Transcriber::new(&dir).available());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
