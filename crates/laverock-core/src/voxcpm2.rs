//! VoxCPM2 engine, backed by llama.cpp's `voxcpm2-cli` (a subprocess).
//!
//! VoxCPM2 is a 2-billion-parameter, 30-language diffusion-AR TTS model.
//! Unlike ZipVoice it runs from GGUF weights and does zero-shot voice
//! cloning from reference audio alone — no transcript required.
//!
//! The runner binary is built from `tc-mb/llama.cpp-omni` (see
//! `scripts/download-voxcpm2.sh`); the core just invokes it, so no heavy
//! C++ or GGUF code is dragged into this crate.

use crate::{
    decode_wav, encode_wav, Audio, CoreError, Manifest, Result, SpeechEngine, SynthRequest,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

pub struct VoxCpm2Engine {
    id: String,
    name: String,
    binary: PathBuf,
    base_lm: PathBuf,
    acoustic: PathBuf,
    cfg: f32,
    temperature: f32,
    seed: i32,
    cpu: bool,
    // One subprocess at a time: the model is big and GPU contention would
    // just thrash.
    run_lock: Mutex<()>,
}

impl VoxCpm2Engine {
    pub fn build(dir: &Path, manifest: &Manifest) -> Result<Self> {
        let v = manifest.voxcpm2.as_ref().ok_or_else(|| {
            CoreError::Load(format!("{}: missing [voxcpm2] section", manifest.id))
        })?;

        let resolve = |rel: &str| -> PathBuf { dir.join(rel) };
        let base_lm = resolve(&v.base_lm);
        let acoustic = resolve(&v.acoustic);

        let binary = match &v.binary {
            Some(rel) => resolve(rel),
            // The download script places the prebuilt/source-built runner
            // next to the GGUFs; on Windows it keeps the .exe suffix.
            None => {
                let candidate = dir.join(if cfg!(windows) {
                    "voxcpm2-cli.exe"
                } else {
                    "voxcpm2-cli"
                });
                if candidate.is_file() {
                    candidate
                } else {
                    // Older installs named it without the suffix.
                    dir.join("voxcpm2-cli")
                }
            }
        };
        if !binary.is_file() {
            return Err(CoreError::Load(format!(
                "{}: runner binary not found at {} (install it with scripts/download-voxcpm2.sh)",
                manifest.id,
                binary.display()
            )));
        }
        if !base_lm.is_file() || !acoustic.is_file() {
            return Err(CoreError::Load(format!(
                "{}: missing GGUF weights ({}, {})",
                manifest.id,
                base_lm.display(),
                acoustic.display()
            )));
        }

        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            binary,
            base_lm,
            acoustic,
            cfg: v.cfg,
            temperature: v.temperature,
            seed: v.seed,
            cpu: v.cpu,
            run_lock: Mutex::new(()),
        })
    }
}

impl SpeechEngine for VoxCpm2Engine {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        let _guard = self.run_lock.lock().expect("voxcpm2 run lock");

        let tmp = std::env::temp_dir().join(format!(
            "laverock-voxcpm2-{}-{}",
            std::process::id(),
            // Cheap per-invocation uniqueness.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).map_err(CoreError::from)?;
        let out_wav = tmp.join("out.wav");

        // Per-request overrides from the manifest-driven parameter list,
        // falling back to the values baked into the engine manifest.
        let p_f32 = |name: &str, built_in: f32| -> f32 {
            req.params
                .get(name)
                .and_then(|v| v.as_f64())
                .map(|v| v as f32)
                .unwrap_or(built_in)
        };
        let cfg = p_f32("cfg", self.cfg);
        let temperature = p_f32("temperature", self.temperature);
        let seed = req
            .params
            .get("seed")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32)
            .unwrap_or(self.seed);
        // -1 means "random per request" (the CLI itself treats -1 oddly).
        let seed = if seed < 0 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| (d.subsec_nanos() & 0x7fff_ffff) as i32)
                .unwrap_or(self.seed)
        } else {
            seed
        };

        let mut cmd = Command::new(&self.binary);
        cmd.arg("-t")
            .arg(&req.text)
            // `--steps` caps decoding at ~0.16 s of audio per step and
            // defaults to 200, silently truncating anything past ~32 s.
            // Scale it with the text; the model still stops at EOS.
            .arg("--steps")
            .arg(decode_steps(&req.text).to_string())
            .arg("--timesteps")
            .arg(req.num_steps.to_string())
            .arg("--cfg")
            .arg(format!("{cfg}"))
            .arg("--temperature")
            .arg(format!("{temperature}"))
            .arg("--seed")
            .arg(seed.to_string())
            .arg("-o")
            .arg(&out_wav);

        if self.cpu {
            cmd.arg("--cpu");
        }

        // Zero-shot cloning: reference audio only, no transcript.
        let mut ref_wav = None;
        if !req.prompt_samples.is_empty() {
            let path = tmp.join("reference.wav");
            let bytes = encode_wav(&req.prompt_samples, req.prompt_sr)?;
            std::fs::write(&path, bytes).map_err(CoreError::from)?;
            cmd.arg("-r").arg(&path);
            ref_wav = Some(path);
        }

        cmd.arg(&self.base_lm).arg(&self.acoustic);

        let output = cmd.output().map_err(|e| {
            CoreError::Synth(format!("failed to run {}: {e}", self.binary.display()))
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CoreError::Synth(format!(
                "voxcpm2-cli exited {:?}: {}",
                output.status.code(),
                stderr.trim()
            )));
        }

        let bytes = std::fs::read(&out_wav)
            .map_err(|e| CoreError::Synth(format!("runner produced no output: {e}")))?;
        let (samples, sample_rate) = decode_wav(&bytes)?;

        // Best-effort cleanup; a stray temp dir is harmless.
        if ref_wav.is_some() {
            let _ = std::fs::remove_dir_all(&tmp);
        }

        Ok(Audio {
            samples,
            sample_rate,
        })
    }
}

/// Decode-step cap for a text.
///
/// voxcpm2-cli emits roughly 0.16 s of audio per step and defaults to 200
/// steps (≈32 s), which is what truncated long paragraphs. Estimate the
/// spoken length with a margin; EOS still ends generation earlier.
fn decode_steps(text: &str) -> i64 {
    let cjk = text
        .chars()
        .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
        .count() as f32;
    let words = text.split_whitespace().filter(|w| !w.is_empty()).count() as f32;
    let seconds = cjk * 0.25 + words * 0.45 + 3.0;
    let steps = (seconds / 0.16 * 1.15).ceil() as i64;
    steps.clamp(200, 4000)
}

#[cfg(test)]
mod tests {
    use super::decode_steps;

    #[test]
    fn scales_steps_with_text_length() {
        // Short text keeps the CLI default (200 steps ≈ 32 s).
        assert_eq!(decode_steps("你好"), 200);
        // A ~200 character paragraph needs well over the default.
        let long = "人工智能技术正在快速发展。".repeat(14);
        assert!(decode_steps(&long) > 300, "{}", decode_steps(&long));
        // Very long input is bounded.
        let huge = "很长的一段文字。".repeat(2000);
        assert_eq!(decode_steps(&huge), 4000);
    }
}
