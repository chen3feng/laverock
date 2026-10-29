//! MOSS-TTS-Delay (OpenMOSS) engine, backed by the native `llama-moss-tts`
//! binary.
//!
//! The installer builds that binary from the OpenMOSS llama.cpp fork and
//! converts the audio tokenizer to GGUF, so synthesis is fully offline and
//! torch-free: one executable drives the Qwen3 backbone plus the audio
//! encoder/decoder GGUFs. A reference clip is optional — without one the
//! model speaks in a default voice; with one it clones that voice. The
//! runtime only accepts 24 kHz mono references, so the engine resamples
//! before invoking the binary.

use crate::{
    decode_wav, encode_wav, resample_mono, Audio, CoreError, Manifest, ProgressEvent, Result,
    SpeechEngine, SynthRequest,
};
use std::{
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};

/// The native runtime requires reference audio at this exact rate.
const REFERENCE_SAMPLE_RATE: u32 = 24_000;

pub struct MossTtsEngine {
    id: String,
    name: String,
    binary: PathBuf,
    backbone: PathBuf,
    audio_encoder: PathBuf,
    audio_decoder: PathBuf,
    language: String,
    n_gpu_layers: i64,
    max_new_tokens: i64,
    env: Vec<(String, String)>,
    run_lock: Mutex<()>,
}

impl MossTtsEngine {
    pub fn build(dir: &Path, manifest: &Manifest) -> Result<Self> {
        let c = manifest.mosstts.as_ref().ok_or_else(|| {
            CoreError::Load(format!("{}: missing [mosstts] section", manifest.id))
        })?;

        let binary = resolve_path(dir, &c.binary);
        if !binary.is_file() {
            return Err(CoreError::Load(format!(
                "{}: llama-moss-tts not found at {}",
                manifest.id,
                binary.display()
            )));
        }
        let require = |label: &str, value: &str| -> Result<PathBuf> {
            let path = resolve_path(dir, value);
            if !path.is_file() {
                return Err(CoreError::Load(format!(
                    "{}: {label} GGUF not found at {}",
                    manifest.id,
                    path.display()
                )));
            }
            Ok(path)
        };
        let backbone = require("backbone", &c.backbone)?;
        let audio_encoder = require("audio encoder", &c.audio_encoder)?;
        let audio_decoder = require("audio decoder", &c.audio_decoder)?;

        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            binary,
            backbone,
            audio_encoder,
            audio_decoder,
            language: c.language.clone(),
            n_gpu_layers: c.n_gpu_layers,
            max_new_tokens: c.max_new_tokens,
            env: c.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            run_lock: Mutex::new(()),
        })
    }
}

impl SpeechEngine for MossTtsEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        let _guard = self.run_lock.lock().expect("mosstts run lock");

        let tmp = std::env::temp_dir().join(format!(
            "laverock-mosstts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).map_err(CoreError::from)?;
        let out_wav = tmp.join("out.wav");

        let mut cmd = Command::new(&self.binary);
        cmd.arg("-m")
            .arg(&self.backbone)
            .arg("--audio-decoder-model")
            .arg(&self.audio_decoder)
            .arg("--text")
            .arg(&req.text)
            .arg("--wav-out")
            .arg(&out_wav)
            .arg("-ngl")
            .arg(self.n_gpu_layers.to_string())
            .arg("--max-new-tokens")
            .arg(self.max_new_tokens.to_string());

        // Per-request language choice overrides the manifest default.
        let language = req
            .params
            .get("language")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.language);
        if !language.is_empty() {
            cmd.arg("--language").arg(language);
        }

        // Optional voice cloning: the runtime wants a 24 kHz mono WAV.
        if !req.prompt_samples.is_empty() {
            let pcm = resample_mono(
                &req.prompt_samples,
                req.prompt_sr.max(1) as u32,
                REFERENCE_SAMPLE_RATE,
            );
            let path = tmp.join("reference.wav");
            let bytes = encode_wav(&pcm, REFERENCE_SAMPLE_RATE as i32)?;
            std::fs::write(&path, bytes).map_err(CoreError::from)?;
            cmd.arg("--audio-encoder-model")
                .arg(&self.audio_encoder)
                .arg("--reference-audio")
                .arg(&path);
        }

        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        if let Some(cb) = &req.progress {
            cb(ProgressEvent::Stage(
                "正在加载 MOSS-TTS 并合成（首次加载约数十秒）".to_string(),
            ));
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            CoreError::Synth(format!(
                "failed to run {}: {e} (is the llama-moss-tts runtime built?)",
                self.binary.display()
            ))
        })?;

        // llama.cpp logs to stderr; keep a bounded tail for error reporting
        // instead of buffering a long generation's output in memory.
        let stdout = child.stdout.take().expect("piped stdout");
        let stdout_handle = std::thread::spawn(move || {
            let mut tail: Vec<String> = Vec::new();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                tail.push(line);
                if tail.len() > 12 {
                    tail.remove(0);
                }
            }
            tail
        });
        let stderr = child.stderr.take().expect("piped stderr");
        let stderr_handle = std::thread::spawn(move || {
            let mut buf = Vec::new();
            BufReader::new(stderr).read_to_end(&mut buf).ok();
            buf
        });

        let status = child.wait().map_err(CoreError::from)?;
        let stdout_tail = stdout_handle.join().unwrap_or_default();
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
            if !stdout_tail.is_empty() {
                tail = format!("{tail}\n{}", stdout_tail.join("\n"));
            }
            return Err(CoreError::Synth(format!(
                "MOSS-TTS 推理失败（退出码 {:?}）：\n{}",
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
