//! dots.tts engine (Studio Dots), backed by audio.cpp's native runtime.
//!
//! dots.tts is a 2B continuous-AR TTS model whose flow-matching head
//! produces 48 kHz audio. The released GGUF export is executed by
//! `audiocpp_cli` (Metal on macOS, CUDA/Vulkan/CPU elsewhere), so no
//! Python environment is involved.
//!
//! Cloning has two modes: x-vector (reference audio only, robust to an
//! inaccurate transcript and used by default) and continuation (audio +
//! transcript, better speaker similarity when the text is correct).

use crate::{
    decode_wav, encode_wav, Audio, CoreError, Manifest, ProgressEvent, Result, SpeechEngine,
    SynthRequest,
};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};

pub struct DotsEngine {
    id: String,
    name: String,
    binary: PathBuf,
    model: PathBuf,
    backend: String,
    language: String,
    guidance: f32,
    seed: i64,
    reference_duration: Option<f32>,
    env: Vec<(String, String)>,
    run_lock: Mutex<()>,
}

impl DotsEngine {
    pub fn build(dir: &Path, manifest: &Manifest) -> Result<Self> {
        let d = manifest
            .dots
            .as_ref()
            .ok_or_else(|| CoreError::Load(format!("{}: missing [dots] section", manifest.id)))?;

        let binary = resolve_path(dir, &d.binary);
        if !binary.is_file() {
            return Err(CoreError::Load(format!(
                "{}: audiocpp_cli not found at {}",
                manifest.id,
                binary.display()
            )));
        }
        let model = resolve_path(dir, &d.model);
        if !model.is_file() {
            return Err(CoreError::Load(format!(
                "{}: GGUF weights not found at {}",
                manifest.id,
                model.display()
            )));
        }

        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            binary,
            model,
            backend: d.backend.clone(),
            language: d.language.clone().unwrap_or_default(),
            guidance: d.guidance,
            seed: d.seed,
            reference_duration: d.reference_duration_sec,
            env: d.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            run_lock: Mutex::new(()),
        })
    }

    fn param_f32(&self, req: &SynthRequest, name: &str, fallback: f32) -> f32 {
        req.params
            .get(name)
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .unwrap_or(fallback)
    }

    fn param_str(&self, req: &SynthRequest, name: &str, fallback: &str) -> String {
        req.params
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(fallback)
            .to_string()
    }
}

impl SpeechEngine for DotsEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        let clone_mode = self.param_str(req, "clone_mode", "xvector");
        let has_prompt = !req.prompt_samples.is_empty();
        if !has_prompt {
            return Err(CoreError::BadRequest(
                "dots.tts 需要一段参考音频（3–10 秒效果最好）".into(),
            ));
        }
        if clone_mode == "continuation" {
            let has_text = req
                .prompt_text
                .as_deref()
                .map(str::trim)
                .is_some_and(|t| !t.is_empty());
            if !has_text {
                return Err(CoreError::BadRequest(
                    "「声纹 + 文字」模式需要参考音频对应的文字；文字不准确时请改用「仅声纹」"
                        .into(),
                ));
            }
        }

        let _guard = self.run_lock.lock().expect("dots run lock");

        let tmp = std::env::temp_dir().join(format!(
            "laverock-dots-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).map_err(CoreError::from)?;
        let out_wav = tmp.join("out.wav");
        let ref_wav = tmp.join("reference.wav");
        let bytes = encode_wav(&req.prompt_samples, req.prompt_sr)?;
        std::fs::write(&ref_wav, bytes).map_err(CoreError::from)?;

        let guidance = self.param_f32(req, "guidance", self.guidance);
        let seed = req
            .params
            .get("seed")
            .and_then(Value::as_i64)
            .unwrap_or(self.seed);
        let language = {
            let from_request = req
                .params
                .get("language")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty());
            from_request
                .map(str::to_string)
                .or_else(|| (!self.language.is_empty()).then(|| self.language.clone()))
        };

        let mut cmd = Command::new(&self.binary);
        cmd.arg("--task")
            .arg("tts")
            .arg("--family")
            .arg("dots_tts")
            .arg("--model")
            .arg(&self.model)
            .arg("--backend")
            .arg(&self.backend)
            .arg("--text")
            .arg(&req.text)
            .arg("--voice-ref")
            .arg(&ref_wav)
            .arg("--seed")
            .arg(seed.to_string())
            .arg("--guidance-scale")
            .arg(format!("{guidance}"))
            .arg("--num-inference-steps")
            .arg(req.num_steps.to_string())
            .arg("--out")
            .arg(&out_wav)
            .arg("--metrics");

        if let Some(code) = &language {
            cmd.arg("--language").arg(code);
        }
        if clone_mode == "continuation" {
            if let Some(text) = req.prompt_text.as_deref().map(str::trim) {
                if !text.is_empty() {
                    cmd.arg("--reference-text").arg(text);
                }
            }
        }
        if let Some(seconds) = self.reference_duration {
            cmd.arg("--reference-duration-seconds")
                .arg(format!("{seconds}"));
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        if let Some(cb) = &req.progress {
            cb(ProgressEvent::Stage(
                "正在加载模型并合成（dots.tts 首次加载约数秒）".to_string(),
            ));
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            CoreError::Synth(format!(
                "failed to run {}: {e} (is the dots.tts runtime installed?)",
                self.binary.display()
            ))
        })?;

        let stdout = child.stdout.take().expect("piped stdout");
        let stdout_handle = std::thread::spawn(move || {
            let mut unexpected = Vec::new();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(rest) = line.strip_prefix("@@LV ") {
                    // Reserved for future step markers; ignored for now.
                    let _ = rest;
                } else if !line.trim().is_empty() {
                    if line.starts_with("metrics.") {
                        tracing::debug!(target: "dots", "{line}");
                    } else {
                        unexpected.push(line);
                    }
                }
            }
            unexpected
        });
        let stderr = child.stderr.take().expect("piped stderr");
        let stderr_handle = std::thread::spawn(move || {
            let mut buf = Vec::new();
            BufReader::new(stderr).read_to_end(&mut buf).ok();
            buf
        });

        let status = child.wait().map_err(CoreError::from)?;
        let stdout_lines = stdout_handle.join().unwrap_or_default();
        let stderr_bytes = stderr_handle.join().unwrap_or_default();

        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr_bytes);
            let mut tail: String = stderr
                .lines()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            let extra: String = stdout_lines
                .into_iter()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            if !extra.trim().is_empty() {
                tail = format!("{tail}\n{extra}");
            }
            return Err(CoreError::Synth(format!(
                "dots.tts runner exited {:?}:\n{}",
                status.code(),
                tail.trim()
            )));
        }

        let bytes = std::fs::read(&out_wav)
            .map_err(|e| CoreError::Synth(format!("runner produced no audio: {e}")))?;
        let (samples, sample_rate) = decode_wav(&bytes)?;

        let _ = std::fs::remove_dir_all(&tmp);

        Ok(Audio {
            samples,
            sample_rate,
        })
    }
}

/// Join relative paths onto the model directory; absolute paths pass through.
fn resolve_path(dir: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        dir.join(path)
    }
}
