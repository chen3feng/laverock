//! ZipVoice engine (flow-matching zero-shot cloning, zh/en, ONNX).
//!
//! Runtime: the official `sherpa-onnx` Rust crate, which links a
//! prebuilt static sherpa-onnx + onnxruntime at build time.

use crate::{Audio, CoreError, Manifest, Result, SpeechEngine, SynthRequest};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsModelConfig,
    OfflineTtsZipvoiceModelConfig,
};
use std::path::{Path, PathBuf};

pub struct ZipVoiceEngine {
    id: String,
    name: String,
    tts: OfflineTts,
}

impl ZipVoiceEngine {
    pub fn build(dir: &Path, manifest: &Manifest) -> Result<Self> {
        let z = manifest.zipvoice.as_ref().ok_or_else(|| {
            CoreError::Load(format!("{}: missing [zipvoice] section", manifest.id))
        })?;

        // Model-relative paths -> absolute, since the server CWD is unrelated.
        let resolve = |rel: &str| -> String {
            let p: PathBuf = dir.join(rel);
            p.to_string_lossy().into_owned()
        };
        let lexicon = z.lexicon.as_ref().map(|list| {
            list.split(',')
                .map(|s| resolve(s.trim()))
                .collect::<Vec<_>>()
                .join(",")
        });

        // sherpa-onnx calls exit() on configuration errors, which would
        // take the whole server down. Validate every referenced file
        // (and the espeak data dir) before handing control to C++.
        let mut required: Vec<PathBuf> = vec![
            resolve(&z.tokens).into(),
            resolve(&z.encoder).into(),
            resolve(&z.decoder).into(),
            resolve(&z.vocoder).into(),
        ];
        if let Some(n) = z.data_dir.as_ref() {
            required.push(resolve(n).into());
        }
        if let Some(list) = z.lexicon.as_ref() {
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
                zipvoice: OfflineTtsZipvoiceModelConfig {
                    tokens: Some(resolve(&z.tokens)),
                    encoder: Some(resolve(&z.encoder)),
                    decoder: Some(resolve(&z.decoder)),
                    vocoder: Some(resolve(&z.vocoder)),
                    data_dir: z.data_dir.as_ref().map(|d| resolve(d)),
                    lexicon,
                    feat_scale: z.feat_scale,
                    t_shift: z.t_shift,
                    target_rms: z.target_rms,
                    guidance_scale: z.guidance_scale,
                },
                num_threads: z.num_threads,
                debug: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let tts = OfflineTts::create(&config).ok_or_else(|| {
            CoreError::Load(format!(
                "{}: sherpa-onnx refused the model (check that all files in engine.json exist)",
                manifest.id
            ))
        })?;

        tracing::info!(
            engine = %manifest.id,
            sample_rate = tts.sample_rate(),
            "ZipVoice engine loaded"
        );

        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            tts,
        })
    }
}

impl SpeechEngine for ZipVoiceEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        let reference_text = req
            .prompt_text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                CoreError::Synth(
                    "reference transcript is required: ZipVoice cannot align the voice without it"
                        .into(),
                )
            })?;

        let gen_config = GenerationConfig {
            speed: req.speed,
            reference_audio: Some(req.prompt_samples.clone()),
            reference_sample_rate: req.prompt_sr,
            reference_text: Some(reference_text.to_string()),
            num_steps: req.num_steps,
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
