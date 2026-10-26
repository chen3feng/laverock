//! Kokoro engine (hexgrad Kokoro-82M, ONNX, preset voices, no cloning).
//!
//! Runtime: the official `sherpa-onnx` Rust crate, which links a
//! prebuilt static sherpa-onnx + onnxruntime at build time.
//!
//! Kokoro only ships fixed style vectors in `voices.bin`, addressed by an
//! integer speaker id (`sid`). The manifest's parameter list exposes the
//! sid-to-name choices for a particular export; a request may also pass
//! the chosen sid directly as a `voice` parameter.

use crate::{Audio, CoreError, Manifest, Result, SpeechEngine, SynthRequest};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig,
    OfflineTtsModelConfig,
};
use std::path::{Path, PathBuf};

pub struct KokoroEngine {
    id: String,
    name: String,
    tts: OfflineTts,
    default_sid: i32,
}

impl KokoroEngine {
    pub fn build(dir: &Path, manifest: &Manifest) -> Result<Self> {
        let k = manifest
            .kokoro
            .as_ref()
            .ok_or_else(|| CoreError::Load(format!("{}: missing [kokoro] section", manifest.id)))?;

        // Model-relative paths -> absolute, since the server CWD is unrelated.
        let resolve = |rel: &str| -> String {
            let p: PathBuf = dir.join(rel);
            p.to_string_lossy().into_owned()
        };
        let lexicon = k.lexicon.as_ref().map(|list| {
            list.split(',')
                .map(|s| resolve(s.trim()))
                .collect::<Vec<_>>()
                .join(",")
        });
        let rule_fsts = k.rule_fsts.as_ref().map(|list| {
            list.split(',')
                .map(|s| resolve(s.trim()))
                .collect::<Vec<_>>()
                .join(",")
        });

        // sherpa-onnx calls exit() on configuration errors, which would
        // take the whole server down. Validate every referenced file
        // (and the espeak data dir) before handing control to C++.
        let mut required: Vec<PathBuf> = vec![
            resolve(&k.model).into(),
            resolve(&k.voices).into(),
            resolve(&k.tokens).into(),
            resolve(&k.data_dir).into(),
        ];
        if let Some(d) = k.dict_dir.as_ref() {
            required.push(resolve(d).into());
        }
        if let Some(list) = k.lexicon.as_ref() {
            required.extend(list.split(',').map(|s| resolve(s.trim()).into()));
        }
        if let Some(list) = k.rule_fsts.as_ref() {
            required.extend(list.split(',').map(|s| resolve(s.trim()).into()));
        }
        for path in required {
            if !path.exists() {
                return Err(CoreError::Load(format!(
                    "{}: missing required file {}",
                    manifest.id,
                    path.display()
                )));
            }
        }

        let config = OfflineTtsConfig {
            model: OfflineTtsModelConfig {
                kokoro: OfflineTtsKokoroModelConfig {
                    model: Some(resolve(&k.model)),
                    voices: Some(resolve(&k.voices)),
                    tokens: Some(resolve(&k.tokens)),
                    data_dir: Some(resolve(&k.data_dir)),
                    length_scale: k.length_scale,
                    dict_dir: k.dict_dir.as_ref().map(|d| resolve(d)),
                    lexicon,
                    lang: k.lang.clone(),
                },
                num_threads: k.num_threads,
                debug: false,
                ..Default::default()
            },
            rule_fsts,
            ..Default::default()
        };

        let tts = OfflineTts::create(&config).ok_or_else(|| {
            CoreError::Load(format!(
                "{}: sherpa-onnx refused the model (check that all files in engine.json exist)",
                manifest.id
            ))
        })?;

        let num_speakers = tts.num_speakers();
        if k.default_sid < 0 || k.default_sid >= num_speakers {
            return Err(CoreError::Load(format!(
                "{}: default_sid {} out of range (model has {} speakers)",
                manifest.id, k.default_sid, num_speakers
            )));
        }

        tracing::info!(
            engine = %manifest.id,
            sample_rate = tts.sample_rate(),
            speakers = num_speakers,
            "Kokoro engine loaded"
        );

        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            tts,
            default_sid: k.default_sid,
        })
    }

    /// Resolve the speaker id for a request. The manifest's parameter
    /// list sends the sid as a string under `voice`; absent/invalid input
    /// falls back to the manifest default.
    fn sid_for(&self, req: &SynthRequest) -> Result<i32> {
        let sid = match req.params.get("voice").and_then(|v| v.as_str()) {
            Some(raw) => raw.parse::<i32>().unwrap_or(self.default_sid),
            None => self.default_sid,
        };
        let num_speakers = self.tts.num_speakers();
        if sid < 0 || sid >= num_speakers {
            return Err(CoreError::BadRequest(format!(
                "音色 id {sid} 超出范围（该模型有 {num_speakers} 个预置音色，sid 0–{}）",
                num_speakers - 1
            )));
        }
        Ok(sid)
    }
}

impl SpeechEngine for KokoroEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        let sid = self.sid_for(req)?;

        let gen_config = GenerationConfig {
            sid,
            speed: req.speed,
            ..Default::default()
        };

        let audio = self
            .tts
            .generate_with_config(&req.text, &gen_config, None::<fn(&[f32], f32) -> bool>)
            .ok_or_else(|| CoreError::Synth("sherpa-onnx returned no audio".into()))?;

        Ok(Audio {
            samples: audio.samples().to_vec(),
            sample_rate: audio.sample_rate(),
        })
    }
}
