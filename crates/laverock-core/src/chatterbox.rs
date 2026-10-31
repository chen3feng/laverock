//! Chatterbox engine (Resemble AI), backed by the official `chatterbox-tts`
//! Python package.
//!
//! A 0.5B multilingual TTS that clones a voice from a reference clip and
//! speaks any supported language (default Chinese). Like Ming-3.0-tts it
//! follows the "isolated runner" model: the core spawns a small script in a
//! dedicated venv and never imports Python. Weights are pre-downloaded to a
//! local directory and loaded via `from_local`, so synthesis is fully
//! offline.

use crate::i18n::{self, Key};
use crate::{
    decode_wav, encode_wav, trim_trailing_low_level, Audio, CoreError, Manifest, ProgressEvent,
    Result, SpeechEngine, SynthRequest,
};
use std::{
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};

pub struct ChatterboxEngine {
    id: String,
    name: String,
    python: PathBuf,
    script: PathBuf,
    model: String,
    t3: String,
    device: String,
    env: Vec<(String, String)>,
    run_lock: Mutex<()>,
}

impl ChatterboxEngine {
    pub fn build(dir: &Path, manifest: &Manifest) -> Result<Self> {
        let c = manifest.chatterbox.as_ref().ok_or_else(|| {
            CoreError::Load(format!("{}: missing [chatterbox] section", manifest.id))
        })?;

        let script = resolve_path(dir, &c.script);
        if !script.is_file() {
            return Err(CoreError::Load(format!(
                "{}: runner script not found at {}",
                manifest.id,
                script.display()
            )));
        }

        let model = {
            let candidate = resolve_path(dir, &c.model);
            if candidate.exists() {
                candidate.to_string_lossy().into_owned()
            } else {
                c.model.clone()
            }
        };

        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            python: PathBuf::from(&c.python),
            script,
            model,
            t3: c.t3.clone(),
            device: c.device.clone(),
            env: c.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            run_lock: Mutex::new(()),
        })
    }
}

impl SpeechEngine for ChatterboxEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        if req.prompt_samples.is_empty() {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::ChatterboxReferenceRequired,
            )));
        }

        let _guard = self.run_lock.lock().expect("chatterbox run lock");

        let tmp = std::env::temp_dir().join(format!(
            "laverock-chatterbox-{}-{}",
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
            .arg("--t3")
            .arg(&self.t3)
            .arg("--text")
            .arg(&req.text)
            .arg("--out")
            .arg(&out_wav)
            .arg("--device")
            .arg(&self.device);

        // Per-request override wins over the manifest default.
        if let Some(language) = req
            .params
            .get("language")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            cmd.arg("--language").arg(language);
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        // Clone mode: write the reference clip.
        let path = tmp.join("reference.wav");
        let bytes = encode_wav(&req.prompt_samples, req.prompt_sr)?;
        std::fs::write(&path, bytes).map_err(CoreError::from)?;
        cmd.arg("--ref-audio").arg(&path);

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            CoreError::Synth(format!(
                "failed to run {} {}: {e} (is the chatterbox-tts venv set up?)",
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
                    if let Some(label) = rest.strip_prefix("stage ") {
                        if let Some(cb) = &sink {
                            cb(ProgressEvent::Stage(label.trim().to_string()));
                        }
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
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            if !extra.trim().is_empty() {
                tail = format!("{tail}\n{extra}");
            }
            return Err(CoreError::Synth(format!(
                "chatterbox runner exited {:?}:\n{}",
                status.code(),
                tail.trim()
            )));
        }

        let bytes = std::fs::read(&out_wav)
            .map_err(|e| CoreError::Synth(format!("runner produced no audio: {e}")))?;
        let (samples, sample_rate) = decode_wav(&bytes)?;
        // Chatterbox occasionally keeps emitting a quiet babble tail after
        // the text ends (several seconds in the worst case). It is above
        // digital silence, so trim it against the clip's own level.
        let samples = trim_trailing_low_level(&samples, sample_rate.max(1) as u32).into_owned();

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
