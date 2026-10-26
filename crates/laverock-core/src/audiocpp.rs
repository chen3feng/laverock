//! Engine family backed by audio.cpp's native runtime.
//!
//! Several modern TTS models ship a GGUF export that audio.cpp executes
//! directly (Metal on macOS, CUDA/Vulkan/CPU elsewhere), which avoids a
//! multi-gigabyte Python/PyTorch environment inside the app. This module
//! implements the shared engine and the per-family request shape:
//!
//! * `dots` — dots.tts: 2B continuous AR, 48 kHz, x-vector or
//!   continuation cloning, APG/CFG guidance.
//! * `cosyvoice` — CosyVoice3: 0.5B streaming LLM TTS with zero-shot,
//!   cross-lingual and instruction templates.

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

/// Which audio.cpp model family an engine wraps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Dots,
    CosyVoice,
}

impl Family {
    /// audio.cpp's `--family` value.
    fn cli_name(self) -> &'static str {
        match self {
            Family::Dots => "dots_tts",
            Family::CosyVoice => "cosyvoice3",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Family::Dots => "dots.tts",
            Family::CosyVoice => "CosyVoice3",
        }
    }
}

pub struct AudioCppEngine {
    id: String,
    name: String,
    family: Family,
    binary: PathBuf,
    model: PathBuf,
    backend: String,
    language: String,
    guidance: Option<f32>,
    seed: i64,
    reference_duration: Option<f32>,
    env: Vec<(String, String)>,
    run_lock: Mutex<()>,
}

impl AudioCppEngine {
    pub fn build(dir: &Path, manifest: &Manifest, family: Family) -> Result<Self> {
        let section = match family {
            Family::Dots => manifest.dots.as_ref(),
            Family::CosyVoice => manifest.cosyvoice.as_ref(),
        };
        let cfg = section.ok_or_else(|| {
            CoreError::Load(format!(
                "{}: missing [{}] section",
                manifest.id,
                family.cli_name()
            ))
        })?;

        let binary = resolve_path(dir, &cfg.binary);
        if !binary.is_file() {
            return Err(CoreError::Load(format!(
                "{}: audiocpp_cli not found at {}",
                manifest.id,
                binary.display()
            )));
        }
        let model = resolve_path(dir, &cfg.model);
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
            family,
            binary,
            model,
            backend: cfg.backend.clone(),
            language: cfg.language.clone().unwrap_or_default(),
            guidance: cfg.guidance,
            seed: cfg.seed,
            reference_duration: cfg.reference_duration_sec,
            env: cfg
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            run_lock: Mutex::new(()),
        })
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

    fn param_i64(&self, req: &SynthRequest, name: &str, fallback: i64) -> i64 {
        req.params
            .get(name)
            .and_then(Value::as_i64)
            .unwrap_or(fallback)
    }

    fn prompt_text<'a>(&self, req: &'a SynthRequest) -> Option<&'a str> {
        req.prompt_text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
    }
}

impl SpeechEngine for AudioCppEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        if req.prompt_samples.is_empty() {
            return Err(CoreError::BadRequest(format!(
                "{} 需要一段参考音频（3–10 秒效果最好）",
                self.family.label()
            )));
        }
        match self.family {
            Family::Dots => (),
            Family::CosyVoice => {
                let template = self.param_str(req, "template", "zero_shot");
                if template == "zero_shot" && self.prompt_text(req).is_none() {
                    return Err(CoreError::BadRequest(
                        "零样本克隆需要参考音频对应的文字；不确定时可改用「跨语种」模板".into(),
                    ));
                }
                if template == "instruct" && self.param_str(req, "instruct", "").is_empty() {
                    return Err(CoreError::BadRequest(
                        "「指令」模板需要填写指令内容，例如「用温和的语气朗读」".into(),
                    ));
                }
            }
        }

        let _guard = self.run_lock.lock().expect("audiocpp run lock");

        let tmp = std::env::temp_dir().join(format!(
            "laverock-{}-{}-{}",
            self.family.cli_name(),
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

        let seed = self.param_i64(req, "seed", self.seed);
        let mut cmd = Command::new(&self.binary);
        cmd.arg("--task")
            .arg(self.task(req))
            .arg("--family")
            .arg(self.family.cli_name())
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
            .arg("--num-inference-steps")
            .arg(req.num_steps.to_string())
            .arg("--out")
            .arg(&out_wav)
            .arg("--metrics");

        self.add_family_options(req, &mut cmd)?;

        if let Some(seconds) = self.reference_duration {
            cmd.arg("--reference-duration-seconds")
                .arg(format!("{seconds}"));
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        if let Some(cb) = &req.progress {
            cb(ProgressEvent::Stage(format!(
                "正在加载模型并合成（{} 首次加载约数秒）",
                self.family.label()
            )));
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            CoreError::Synth(format!(
                "failed to run {}: {e} (is the audio.cpp runtime installed?)",
                self.binary.display()
            ))
        })?;

        let family = self.family;
        let stdout = child.stdout.take().expect("piped stdout");
        let stdout_handle = std::thread::spawn(move || {
            let mut unexpected = Vec::new();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.starts_with("@@LV ") {
                    // Reserved for future step markers.
                } else if !line.trim().is_empty() {
                    if line.starts_with("metrics.") {
                        tracing::debug!(target: "audiocpp", family = family.cli_name(), "{line}");
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
                "{} 推理失败（退出码 {:?}）：\n{}",
                self.family.label(),
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

impl AudioCppEngine {
    /// audio.cpp task name for this request.
    fn task(&self, req: &SynthRequest) -> &'static str {
        match self.family {
            Family::Dots => "tts",
            Family::CosyVoice => {
                let template = self.param_str(req, "template", "zero_shot");
                if template == "instruct" {
                    "tts"
                } else {
                    "clon"
                }
            }
        }
    }

    fn add_family_options(&self, req: &SynthRequest, cmd: &mut Command) -> Result<()> {
        match self.family {
            Family::Dots => {
                // x-vector (audio only) or continuation (audio + text).
                let clone_mode = self.param_str(req, "clone_mode", "xvector");
                if clone_mode == "continuation" {
                    match self.prompt_text(req) {
                        Some(text) => {
                            cmd.arg("--reference-text").arg(text);
                        }
                        None => {
                            return Err(CoreError::BadRequest(
                                "「声纹 + 文字」模式需要参考音频对应的文字；文字不准确时请改用「仅声纹」"
                                    .into(),
                            ))
                        }
                    }
                }
                let guidance = req
                    .params
                    .get("guidance")
                    .and_then(Value::as_f64)
                    .map(|v| v as f32)
                    .or(self.guidance);
                if let Some(guidance) = guidance {
                    cmd.arg("--guidance-scale").arg(format!("{guidance}"));
                }
                if let Some(code) = self.language_tag(req) {
                    cmd.arg("--language").arg(code);
                }
            }
            Family::CosyVoice => {
                let template = self.param_str(req, "template", "zero_shot");
                cmd.arg("--request-option")
                    .arg(format!("template_name={template}"));
                let top_k = self.param_i64(req, "top_k", 25);
                cmd.arg("--request-option").arg(format!("top_k={top_k}"));
                if template == "zero_shot" {
                    if let Some(text) = self.prompt_text(req) {
                        cmd.arg("--reference-text").arg(text);
                    }
                }
                if template == "instruct" {
                    let instruct = self.param_str(req, "instruct", "");
                    cmd.arg("--request-option")
                        .arg(format!("instruct={instruct}"));
                }
            }
        }
        Ok(())
    }

    fn language_tag(&self, req: &SynthRequest) -> Option<String> {
        let from_request = req
            .params
            .get("language")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty());
        from_request
            .map(str::to_string)
            .or_else(|| (!self.language.is_empty()).then(|| self.language.clone()))
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

    fn manifest(kind: &str) -> Manifest {
        let (section, id) = match kind {
            "dots" => ("dots", "dots"),
            _ => ("cosyvoice", "cosyvoice"),
        };
        let json = serde_json::json!({
            "id": id,
            "kind": kind,
            "name": "test",
            "defaults": { "speed": 1.0, "num_steps": 10 },
            section: {
                "binary": "fake_cli.sh",
                "model": "fake.gguf",
                "backend": "cpu"
            }
        });
        serde_json::from_value(json).expect("manifest")
    }

    fn fixture(dir: &Path, kind: &str) -> AudioCppEngine {
        std::fs::write(dir.join("fake.gguf"), b"x").unwrap();
        std::fs::write(
            dir.join("fake_cli.sh"),
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/args.txt\"\n\
             out=\"\"\nwhile [ $# -gt 0 ]; do [ \"$1\" = \"--out\" ] && out=$2; shift; done\n\
             cp \"$(dirname \"$0\")/fx.wav\" \"$out\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join("fake_cli.sh");
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        let wav = crate::encode_wav(&vec![0.0; 2400], 24_000).unwrap();
        std::fs::write(dir.join("fx.wav"), wav).unwrap();
        let m = manifest(kind);
        AudioCppEngine::build(dir, &m, super::Family::Dots)
            .unwrap_or_else(|_| AudioCppEngine::build(dir, &m, super::Family::CosyVoice).unwrap())
    }

    fn request(params: &[(&str, serde_json::Value)]) -> SynthRequest {
        let mut map = std::collections::BTreeMap::new();
        for (k, v) in params {
            map.insert((*k).to_string(), v.clone());
        }
        SynthRequest {
            text: "hello".into(),
            prompt_text: Some("ref words".into()),
            prompt_samples: vec![0.1; 2400],
            prompt_sr: 24_000,
            speed: 1.0,
            num_steps: 10,
            params: map,
            progress: None,
        }
    }

    fn run(dir: &Path, kind: &str, params: &[(&str, serde_json::Value)]) -> Vec<String> {
        let engine = fixture(dir, kind);
        let audio = engine.synthesize(&request(params)).expect("synthesis");
        assert_eq!(audio.sample_rate, 24_000);
        std::fs::read_to_string(dir.join("args.txt"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn args_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "laverock-audiocpp-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn dots_maps_clone_and_guidance() {
        let dir = args_dir("dots");
        let args = run(
            &dir,
            "dots",
            &[
                ("clone_mode", serde_json::json!("continuation")),
                ("guidance", serde_json::json!(1.5)),
            ],
        );
        assert!(args.windows(2).any(|w| w == ["--family", "dots_tts"]));
        assert!(args.windows(2).any(|w| w == ["--task", "tts"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--reference-text", "ref words"]));
        assert!(args.windows(2).any(|w| w == ["--guidance-scale", "1.5"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cosyvoice_maps_templates() {
        let dir = args_dir("cosy");
        let args = run(
            &dir,
            "cosyvoice",
            &[("template", serde_json::json!("zero_shot"))],
        );
        assert!(args.windows(2).any(|w| w == ["--family", "cosyvoice3"]));
        assert!(args.windows(2).any(|w| w == ["--task", "clon"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--request-option", "template_name=zero_shot"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--reference-text", "ref words"]));

        let args = run(
            &dir,
            "cosyvoice",
            &[
                ("template", serde_json::json!("instruct")),
                ("instruct", serde_json::json!("speak warmly")),
            ],
        );
        assert!(args.windows(2).any(|w| w == ["--task", "tts"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--request-option", "instruct=speak warmly"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cosyvoice_validation() {
        let dir = args_dir("cosy-validation");
        let engine = fixture(&dir, "cosyvoice");
        let mut req = request(&[("template", serde_json::json!("zero_shot"))]);
        req.prompt_text = None;
        let err = engine.synthesize(&req).unwrap_err().to_string();
        assert!(err.contains("零样本"), "{err}");

        let req = request(&[("template", serde_json::json!("instruct"))]);
        let err = engine.synthesize(&req).unwrap_err().to_string();
        assert!(err.contains("指令"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_prompt_is_rejected() {
        let dir = args_dir("no-prompt");
        let engine = fixture(&dir, "dots");
        let mut req = request(&[]);
        req.prompt_samples.clear();
        assert!(engine.synthesize(&req).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_reaches_the_sink() {
        let dir = args_dir("stage");
        let engine = fixture(&dir, "dots");
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let mut req = request(&[]);
        req.progress = Some(Arc::new(move |e| {
            if let ProgressEvent::Stage(s) = e {
                sink.lock().unwrap().push(s);
            }
        }));
        engine.synthesize(&req).expect("synthesis");
        assert_eq!(seen.lock().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
