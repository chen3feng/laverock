//! Ming-3.0-tts engine, backed by the repo's Python runtime.
//!
//! Ming-3.0-tts ships as a Python/transformers program (torch 2.6,
//! transformers 4.46.3), so it follows the same "isolated runner" model as
//! Qwen3-TTS: a small script in a dedicated virtualenv is invoked as a
//! subprocess. The core never imports Python.
//!
//! The single 0.5B dense checkpoint handles every task; the manifest's
//! `mode` selects which one an install exposes:
//!   clone         task TTS (zero-shot cloning; transcript optional)
//!   multilingual  task MULTILINGUAL (clone across German/Spanish/...)
//!   voice_design  task VOICE_DESIGN (voice described in natural language)
//!   ip            task IP (built-in preset voices, no reference needed)

use crate::i18n::{self, Key};
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

pub struct Ming3TtsEngine {
    id: String,
    name: String,
    python: PathBuf,
    script: PathBuf,
    model: String,
    runtime: PathBuf,
    mode: String,
    language: Option<String>,
    instruct: Option<String>,
    speaker: Option<String>,
    device: String,
    env: Vec<(String, String)>,
    run_lock: Mutex<()>,
}

impl Ming3TtsEngine {
    pub fn build(dir: &Path, manifest: &Manifest) -> Result<Self> {
        let m = manifest.ming3tts.as_ref().ok_or_else(|| {
            CoreError::Load(format!("{}: missing [ming3tts] section", manifest.id))
        })?;

        if !matches!(
            m.mode.as_str(),
            "clone" | "voice_design" | "multilingual" | "ip"
        ) {
            return Err(CoreError::Load(format!(
                "{}: unknown ming3tts mode '{}' \
                 (expected clone, voice_design, multilingual or ip)",
                manifest.id, m.mode
            )));
        }

        let script = resolve_path(dir, &m.script);
        if !script.is_file() {
            return Err(CoreError::Load(format!(
                "{}: runner script not found at {}",
                manifest.id,
                script.display()
            )));
        }

        let runtime = {
            let candidate = resolve_path(dir, &m.runtime);
            if candidate.is_dir() {
                candidate
            } else {
                dir.to_path_buf()
            }
        };

        // A repo id stays as-is; anything that exists on disk becomes absolute.
        let model = {
            let candidate = resolve_path(dir, &m.model);
            if candidate.exists() {
                candidate.to_string_lossy().into_owned()
            } else {
                m.model.clone()
            }
        };

        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            python: PathBuf::from(&m.python),
            script,
            model,
            runtime,
            mode: m.mode.clone(),
            language: non_empty(&m.language),
            instruct: non_empty(&m.instruct),
            speaker: non_empty(&m.speaker),
            device: m.device.clone(),
            env: m.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            run_lock: Mutex::new(()),
        })
    }
}

impl SpeechEngine for Ming3TtsEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        // Reference-dependent modes need a voice; the others build one from
        // a description or a built-in preset.
        if matches!(self.mode.as_str(), "clone" | "multilingual") && req.prompt_samples.is_empty() {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::MingReferenceRequired,
            )));
        }
        if self.mode == "voice_design" {
            let has_instruct = req
                .params
                .get("instruct")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.trim().is_empty())
                || self
                    .instruct
                    .as_deref()
                    .is_some_and(|s| !s.trim().is_empty());
            if !has_instruct {
                return Err(CoreError::UserError(i18n::Message::new(
                    Key::VoiceDesignDescriptionRequired,
                )));
            }
        }

        let _guard = self.run_lock.lock().expect("ming3tts run lock");

        let tmp = std::env::temp_dir().join(format!(
            "laverock-ming3tts-{}-{}",
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
            .arg("--runtime")
            .arg(&self.runtime)
            .arg("--mode")
            .arg(&self.mode)
            .arg("--text")
            .arg(&req.text)
            .arg("--out")
            .arg(&out_wav)
            .arg("--device")
            .arg(&self.device);

        // Per-request overrides win over the manifest's defaults.
        if let Some(language) = req
            .params
            .get("language")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .or(self.language.as_deref())
        {
            cmd.arg("--language").arg(language);
        }
        let instruct = req
            .params
            .get("instruct")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| self.instruct.clone());
        if let Some(instruct) = instruct {
            cmd.arg("--instruct").arg(instruct);
        }
        let speaker = req
            .params
            .get("speaker")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| self.speaker.clone());
        if let Some(speaker) = speaker {
            cmd.arg("--speaker").arg(speaker);
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        // Clone/multilingual: write the reference clip. The transcript is
        // passed only when the saved profile carries one; a wrong transcript
        // is worse than none (which falls back to speaker-embedding cloning).
        if !req.prompt_samples.is_empty() {
            let path = tmp.join("reference.wav");
            let bytes = encode_wav(&req.prompt_samples, req.prompt_sr)?;
            std::fs::write(&path, bytes).map_err(CoreError::from)?;
            cmd.arg("--ref-audio").arg(&path);
            if let Some(text) = req.prompt_text.as_deref().filter(|t| !t.trim().is_empty()) {
                cmd.arg("--ref-text").arg(text);
            }
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            CoreError::Synth(format!(
                "failed to run {} {}: {e} (is the ming3tts venv set up?)",
                self.python.display(),
                self.script.display()
            ))
        })?;

        // The runner prints `@@LV stage …` markers on stdout; forward them
        // to the per-request progress sink while generation runs.
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
                "ming3tts runner exited {:?}:\n{}",
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
