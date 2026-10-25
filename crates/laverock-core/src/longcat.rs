//! LongCat-AudioDiT engine (Meituan), backed by the official `audiodit`
//! package in an isolated Python virtualenv.
//!
//! The model is a flow-matching diffusion TTS that operates directly in a
//! waveform latent space: a WAV-VAE plus a DiT backbone. Zero-shot cloning
//! takes the reference clip and its transcript; `guidance_method` selects
//! APG (recommended) or classic CFG.
//!
//! Like Qwen3-TTS, the core never imports Python: a runner script in the
//! model directory is invoked as a subprocess and streams `@@LV` progress
//! markers on stdout.

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

pub struct LongCatEngine {
    id: String,
    name: String,
    python: PathBuf,
    script: PathBuf,
    model: String,
    tokenizer: Option<String>,
    device: String,
    dtype: String,
    guidance: String,
    cfg_strength: f32,
    seed: i64,
    env: Vec<(String, String)>,
    run_lock: Mutex<()>,
}

impl LongCatEngine {
    pub fn build(dir: &Path, manifest: &Manifest) -> Result<Self> {
        let lc = manifest.longcat.as_ref().ok_or_else(|| {
            CoreError::Load(format!("{}: missing [longcat] section", manifest.id))
        })?;

        let script = resolve_path(dir, &lc.script);
        if !script.is_file() {
            return Err(CoreError::Load(format!(
                "{}: runner script not found at {}",
                manifest.id,
                script.display()
            )));
        }

        // A repo id stays as-is; anything on disk becomes absolute.
        let model = {
            let candidate = resolve_path(dir, &lc.model);
            if candidate.exists() {
                candidate.to_string_lossy().into_owned()
            } else {
                lc.model.clone()
            }
        };
        let tokenizer = lc.tokenizer.as_ref().map(|t| {
            let candidate = resolve_path(dir, t);
            if candidate.exists() {
                candidate.to_string_lossy().into_owned()
            } else {
                t.clone()
            }
        });

        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            python: PathBuf::from(&lc.python),
            script,
            model,
            tokenizer,
            device: lc.device.clone(),
            dtype: lc.dtype.clone(),
            guidance: lc.guidance.clone(),
            cfg_strength: lc.cfg_strength,
            seed: lc.seed,
            env: lc.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
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
}

impl SpeechEngine for LongCatEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        if !req.prompt_samples.is_empty() {
            let transcript = req
                .prompt_text
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty());
            if transcript.is_none() {
                return Err(CoreError::BadRequest(
                    "LongCat-AudioDiT 克隆需要参考音频对应的文字".into(),
                ));
            }
        }

        let _guard = self.run_lock.lock().expect("longcat run lock");

        let tmp = std::env::temp_dir().join(format!(
            "laverock-longcat-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).map_err(CoreError::from)?;
        let out_wav = tmp.join("out.wav");

        let guidance = req
            .params
            .get("guidance_method")
            .and_then(Value::as_str)
            .filter(|v| matches!(*v, "apg" | "cfg"))
            .unwrap_or(self.guidance.as_str())
            .to_string();
        let cfg = self.param_f32(req, "cfg", self.cfg_strength);
        let seed = req
            .params
            .get("seed")
            .and_then(Value::as_i64)
            .unwrap_or(self.seed);

        let mut cmd = Command::new(&self.python);
        cmd.arg(&self.script)
            .arg("--model")
            .arg(&self.model)
            .arg("--text")
            .arg(&req.text)
            .arg("--out")
            .arg(&out_wav)
            .arg("--device")
            .arg(&self.device)
            .arg("--dtype")
            .arg(&self.dtype)
            .arg("--steps")
            .arg(req.num_steps.to_string())
            .arg("--cfg")
            .arg(format!("{cfg}"))
            .arg("--guidance")
            .arg(&guidance)
            .arg("--seed")
            .arg(seed.to_string());
        if let Some(tokenizer) = &self.tokenizer {
            cmd.arg("--tokenizer").arg(tokenizer);
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        if !req.prompt_samples.is_empty() {
            let path = tmp.join("reference.wav");
            let bytes = encode_wav(&req.prompt_samples, req.prompt_sr)?;
            std::fs::write(&path, bytes).map_err(CoreError::from)?;
            cmd.arg("--ref-audio").arg(&path);
            if let Some(text) = req.prompt_text.as_deref().map(str::trim) {
                if !text.is_empty() {
                    cmd.arg("--ref-text").arg(text);
                }
            }
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            CoreError::Synth(format!(
                "failed to run {} {}: {e} (is the longcat venv set up?)",
                self.python.display(),
                self.script.display()
            ))
        })?;

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
                        (Some("step"), Some(value)) => {
                            if let Ok(step) = value.trim().parse::<u32>() {
                                if let Some(cb) = &sink {
                                    cb(ProgressEvent::Steps(step));
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
                "longcat-tts runner exited {:?}:\n{}",
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn passes_params_to_the_runner() {
        let base = std::env::temp_dir().join(format!(
            "laverock-longcat-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("fake_runner.py"),
            r#"import sys, wave, struct
args = sys.argv[1:]
open(sys.argv[0] + ".args", "w").write(" ".join(args))
out = args[args.index("--out") + 1]
with wave.open(out, "wb") as w:
    w.setnchannels(1); w.setsampwidth(2); w.setframerate(24000)
    w.writeframes(b"".join(struct.pack("<h", 1000) for _ in range(24000)))
print("@@LV stage done", flush=True)
"#,
        )
        .unwrap();

        let manifest_json = serde_json::json!({
            "id": "longcat",
            "kind": "longcat",
            "name": "LongCat test",
            "longcat": {
                "python": "python3",
                "script": "fake_runner.py",
                "model": "/nonexistent/weights",
                "tokenizer": "/nonexistent/tokenizer"
            },
            "defaults": { "speed": 1.0, "num_steps": 16 }
        });
        let manifest: Manifest = serde_json::from_value(manifest_json).unwrap();
        let engine = LongCatEngine::build(&base, &manifest).unwrap();

        let mut params = std::collections::BTreeMap::new();
        params.insert("cfg".to_string(), serde_json::json!(2.5));
        params.insert("guidance_method".to_string(), serde_json::json!("cfg"));
        params.insert("seed".to_string(), serde_json::json!(7));
        let stages: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_stages = stages.clone();
        let progress: crate::ProgressSink = Arc::new(move |e| {
            if let ProgressEvent::Stage(s) = e {
                sink_stages.lock().unwrap().push(s);
            }
        });

        let audio = engine
            .synthesize(&SynthRequest {
                text: "hello".into(),
                prompt_text: Some("reference words".into()),
                prompt_samples: vec![0.1; 2400],
                prompt_sr: 24000,
                speed: 1.0,
                num_steps: 8,
                params,
                progress: Some(progress),
            })
            .expect("synthesis");

        assert_eq!(audio.samples.len(), 24000);
        assert_eq!(audio.sample_rate, 24000);
        let args = std::fs::read_to_string(base.join("fake_runner.py.args")).unwrap();
        for expected in [
            "--steps 8",
            "--cfg 2.5",
            "--guidance cfg",
            "--seed 7",
            "--ref-text reference words",
        ] {
            assert!(args.contains(expected), "missing {expected} in {args}");
        }
        assert_eq!(stages.lock().unwrap().as_slice(), ["done"]);

        let _ = std::fs::remove_dir_all(&base);
    }
}
