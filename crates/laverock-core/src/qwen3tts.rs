//! Qwen3-TTS engine, backed by the official `qwen-tts` Python package.
//!
//! Qwen3-TTS ships as a Python/transformers package, so it follows the
//! "isolated runner" model: a small script in a dedicated virtualenv is
//! invoked as a subprocess. The core never imports Python, and Qwen's
//! torch/transformers pins cannot collide with other engines.
//!
//! Modes map to the three released families:
//!   clone         Base model (zero-shot cloning; transcript optional)
//!   custom_voice  preset speakers, optional natural-language instruction
//!   voice_design  voice described in natural language

use crate::{
    decode_wav, encode_wav, Audio, CoreError, Manifest, ProgressEvent, Result, SpeechEngine,
    SynthRequest,
};
use std::{
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};

pub struct Qwen3TtsEngine {
    id: String,
    name: String,
    python: PathBuf,
    script: PathBuf,
    model: String,
    mode: String,
    speaker: Option<String>,
    language: Option<String>,
    instruct: Option<String>,
    device: String,
    dtype: String,
    attn: String,
    env: Vec<(String, String)>,
    run_lock: Mutex<()>,
}

impl Qwen3TtsEngine {
    pub fn build(dir: &Path, manifest: &Manifest) -> Result<Self> {
        let q = manifest.qwen3tts.as_ref().ok_or_else(|| {
            CoreError::Load(format!("{}: missing [qwen3tts] section", manifest.id))
        })?;

        if !matches!(q.mode.as_str(), "clone" | "custom_voice" | "voice_design") {
            return Err(CoreError::Load(format!(
                "{}: unknown qwen3tts mode '{}' (expected clone, custom_voice or voice_design)",
                manifest.id, q.mode
            )));
        }

        let script = resolve_path(dir, &q.script);
        if !script.is_file() {
            return Err(CoreError::Load(format!(
                "{}: runner script not found at {}",
                manifest.id,
                script.display()
            )));
        }

        // A repo id stays as-is; anything that exists on disk becomes absolute.
        let model = {
            let candidate = resolve_path(dir, &q.model);
            if candidate.exists() {
                candidate.to_string_lossy().into_owned()
            } else {
                q.model.clone()
            }
        };

        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            python: PathBuf::from(&q.python),
            script,
            model,
            mode: q.mode.clone(),
            speaker: non_empty(&q.speaker),
            language: non_empty(&q.language),
            instruct: non_empty(&q.instruct),
            device: q.device.clone(),
            dtype: q.dtype.clone(),
            attn: q.attn.clone(),
            env: q.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            run_lock: Mutex::new(()),
        })
    }
}

impl SpeechEngine for Qwen3TtsEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        if self.mode == "clone" && req.prompt_samples.is_empty() {
            return Err(CoreError::BadRequest(
                "Qwen3-TTS 克隆模式需要一段参考音频（3 秒以上效果更好）".into(),
            ));
        }

        let _guard = self.run_lock.lock().expect("qwen3tts run lock");

        let tmp = std::env::temp_dir().join(format!(
            "laverock-qwen3tts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).map_err(CoreError::from)?;
        let out_wav = tmp.join("out.wav");

        let mut cmd = Command::new(&self.python);
        cmd.arg(&self.script)
            .arg("--model")
            .arg(&self.model)
            .arg("--mode")
            .arg(&self.mode)
            .arg("--text")
            .arg(&req.text)
            .arg("--out")
            .arg(&out_wav)
            .arg("--device")
            .arg(&self.device)
            .arg("--dtype")
            .arg(&self.dtype)
            .arg("--attn")
            .arg(&self.attn);

        if let Some(language) = &self.language {
            cmd.arg("--language").arg(language);
        }
        if let Some(speaker) = &self.speaker {
            cmd.arg("--speaker").arg(speaker);
        }
        if let Some(instruct) = &self.instruct {
            cmd.arg("--instruct").arg(instruct);
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        // Clone mode: write the reference clip. The transcript is
        // deliberately not passed: qwen-tts' ICL mode is very sensitive
        // to transcript accuracy, and saved profiles often carry an
        // approximate/empty transcript (VoxCPM2 needs none). A mismatch
        // produces garbled, truncated or runaway audio. Speaker
        // embedding-only mode clones from the voice alone and is robust.
        if !req.prompt_samples.is_empty() {
            let path = tmp.join("reference.wav");
            let bytes = encode_wav(&req.prompt_samples, req.prompt_sr)?;
            std::fs::write(&path, bytes).map_err(CoreError::from)?;
            cmd.arg("--ref-audio").arg(&path);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            CoreError::Synth(format!(
                "failed to run {} {}: {e} (is the qwen-tts venv set up?)",
                self.python.display(),
                self.script.display()
            ))
        })?;

        // The runner prints `@@LV stage/gen …` markers on stdout; forward
        // them to the per-request progress sink while generation runs.
        let stdout = child.stdout.take().expect("piped stdout");
        let sink = req.progress.clone();
        let stdout_handle = std::thread::spawn(move || {
            let mut unexpected = Vec::new();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(rest) = line.strip_prefix("@@LV ") {
                    let mut parts = rest.splitn(2, ' ');
                    match (parts.next(), parts.next()) {
                        (Some("stage"), Some(label)) => {
                            if let Some(cb) = &sink {
                                cb(ProgressEvent::Stage(label.trim().to_string()));
                            }
                        }
                        (Some("gen"), Some(value)) => {
                            if let Ok(steps) = value.trim().parse::<u32>() {
                                if let Some(cb) = &sink {
                                    cb(ProgressEvent::Steps(steps));
                                }
                            }
                        }
                        _ => {}
                    }
                } else if !line.trim().is_empty() {
                    unexpected.push(line);
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
            // Runner diagnostics that landed on stdout for some reason.
            let extra: String = stdout_lines
                .into_iter()
                .rev()
                .take(6)
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            if !extra.trim().is_empty() {
                tail = format!("{tail}\n{extra}");
            }
            return Err(CoreError::Synth(format!(
                "qwen3-tts runner exited {:?}:\n{}",
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

fn non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
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
