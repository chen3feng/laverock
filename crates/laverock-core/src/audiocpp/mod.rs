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
//! * `indextts2` — IndexTTS-2: GPT-style token model + flow-matching
//!   vocoder, cloned from a reference clip (zh/en).
//! * `ace_step` — ACE-Step 1.5: text/lyrics-to-music at 48 kHz stereo,
//!   the only family here that is not speech.
//!
//! This file owns the shared CLI/resident-server plumbing; each family's
//! flags and resident options live in `families/<family>.rs`.

use crate::i18n::{self, Key};
use crate::{
    decode_wav, decode_wav_channels, encode_wav, trim_reference, Audio, AudioCppManifest,
    AudioCppTaskManifest, ConvertRequest, CoreError, Manifest, ModelSpec, MusicManifest,
    ProgressEvent, ProgressSink, Result, SpeechEngine, SynthRequest, VOXCPM2_MAX_REFERENCE_SECS,
};
use base64::Engine as _;
use serde_json::{json, Map, Value};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    io::{BufRead, BufReader, Read},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

mod families;

/// How long a resident `audiocpp_server` keeps its weights after the last
/// request. The process itself stays up, so a follow-up request is warm.
const RESIDENT_IDLE_UNLOAD_MS: u64 = 15 * 60 * 1000;
/// Startup budget for the resident server: process spawn plus listener.
const RESIDENT_START_TIMEOUT: Duration = Duration::from_secs(30);
/// A generation can legitimately run for minutes on long text.
const RESIDENT_READ_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Which audio.cpp model family an engine wraps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Dots,
    CosyVoice,
    VoxCpm2,
    IndexTts2,
    /// Music generation (`--task gen`), currently ACE-Step 1.5.
    AceStep,
    /// Any family driven by a `model_spec.json`; the manifest supplies
    /// the family and task names.
    Generic,
}

impl Family {
    /// audio.cpp's `--family` value.
    fn cli_name(self) -> &'static str {
        match self {
            Family::Dots => "dots_tts",
            Family::CosyVoice => "cosyvoice3",
            Family::VoxCpm2 => "voxcpm2",
            Family::IndexTts2 => "index_tts2",
            Family::AceStep => "ace_step",
            Family::Generic => "audio_cpp",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Family::Dots => "dots.tts",
            Family::CosyVoice => "CosyVoice3",
            Family::VoxCpm2 => "VoxCPM2",
            Family::IndexTts2 => "IndexTTS-2",
            Family::AceStep => "ACE-Step",
            Family::Generic => "audio.cpp",
        }
    }
}

pub struct AudioCppEngine {
    id: String,
    name: String,
    family: Family,
    /// The audio.cpp `--family` value; a generic install names its own.
    family_name: String,
    /// Request contract of a generic install (`kind: "audio_cpp"`).
    spec: Option<ModelSpec>,
    /// Request field that carries source audio, when the task consumes one.
    input_field: Option<String>,
    /// Whether the task needs a target voice reference.
    voice_ref: bool,
    binary: PathBuf,
    model: PathBuf,
    backend: String,
    language: String,
    guidance: Option<f32>,
    seed: i64,
    reference_duration: Option<f32>,
    /// Fixed task for families that only run one (`gen` for music); the
    /// speech families pick their task per request.
    task: String,
    load_options: Vec<(String, String)>,
    session_options: Vec<(String, String)>,
    env: Vec<(String, String)>,
    run_lock: Mutex<()>,
    /// `audiocpp_server` beside the CLI; `None` keeps the one-shot path.
    server_bin: Option<PathBuf>,
    /// The server started for this model, kept across requests.
    resident: Mutex<Option<ResidentServer>>,
}

/// The manifest section a family reads, normalized across speech and music.
struct Section {
    family_name: String,
    spec: Option<ModelSpec>,
    input_field: Option<String>,
    voice_ref: bool,
    binary: String,
    model: String,
    backend: String,
    language: Option<String>,
    guidance: Option<f32>,
    seed: i64,
    reference_duration: Option<f32>,
    task: String,
    load_options: BTreeMap<String, String>,
    session_options: BTreeMap<String, String>,
    env: BTreeMap<String, String>,
    /// Speech weights are a single GGUF file; a music install may be a
    /// directory holding a split checkpoint.
    model_is_file: bool,
}

impl Section {
    fn speech(cfg: &AudioCppManifest, family: Family) -> Self {
        Self {
            family_name: family.cli_name().to_string(),
            spec: None,
            input_field: None,
            voice_ref: true,
            binary: cfg.binary.clone(),
            model: cfg.model.clone(),
            backend: cfg.backend.clone(),
            language: cfg.language.clone(),
            guidance: cfg.guidance,
            seed: cfg.seed,
            reference_duration: cfg.reference_duration_sec,
            task: String::new(),
            load_options: BTreeMap::new(),
            session_options: BTreeMap::new(),
            env: cfg.env.clone(),
            model_is_file: true,
        }
    }

    fn music(cfg: &MusicManifest) -> Self {
        Self {
            family_name: Family::AceStep.cli_name().to_string(),
            spec: None,
            input_field: None,
            voice_ref: false,
            binary: cfg.binary.clone(),
            model: cfg.model.clone(),
            backend: cfg.backend.clone(),
            language: cfg.language.clone(),
            guidance: cfg.guidance,
            seed: cfg.seed,
            reference_duration: None,
            task: cfg.task.clone(),
            load_options: cfg.load_options.clone(),
            session_options: cfg.session_options.clone(),
            env: cfg.env.clone(),
            model_is_file: false,
        }
    }

    fn generic(cfg: &AudioCppTaskManifest, spec: Option<ModelSpec>) -> Self {
        // A voice-conversion task always consumes source audio, even when
        // the manifest forgets to say so.
        let input_field = cfg.input.clone().or_else(|| {
            matches!(cfg.task.as_str(), "vc" | "svc" | "s2s").then(|| "audio".to_string())
        });
        Self {
            family_name: cfg.family.clone(),
            spec,
            input_field,
            voice_ref: cfg.voice_ref,
            binary: cfg.binary.clone(),
            model: cfg.model.clone(),
            backend: cfg.backend.clone(),
            language: cfg.language.clone(),
            guidance: None,
            seed: cfg.seed,
            reference_duration: None,
            task: cfg.task.clone(),
            load_options: cfg.load_options.clone(),
            session_options: cfg.session_options.clone(),
            env: cfg.env.clone(),
            model_is_file: false,
        }
    }
}

/// A running `audiocpp_server` and the config file it was started with.
struct ResidentServer {
    child: Child,
    port: u16,
    dir: PathBuf,
}

/// Per-family options for a resident-server request: the OpenAI-style
/// `options` object plus the separate `reference_text` field.
pub(crate) struct FamilyResident {
    pub(crate) options: Map<String, Value>,
    pub(crate) reference_text: Option<String>,
}

impl FamilyResident {
    pub(crate) fn new() -> Self {
        Self {
            options: Map::new(),
            reference_text: None,
        }
    }
}

impl Drop for AudioCppEngine {
    fn drop(&mut self) {
        self.stop_resident();
    }
}

impl AudioCppEngine {
    pub fn build(dir: &Path, manifest: &Manifest, family: Family) -> Result<Self> {
        let section = match family {
            Family::Dots => manifest.dots.as_ref().map(|c| Section::speech(c, family)),
            Family::CosyVoice => manifest
                .cosyvoice
                .as_ref()
                .map(|c| Section::speech(c, family)),
            Family::VoxCpm2 => manifest
                .voxcpm2
                .as_ref()
                .map(|c| Section::speech(c, family)),
            Family::IndexTts2 => manifest
                .indextts2
                .as_ref()
                .map(|c| Section::speech(c, family)),
            Family::AceStep => manifest.music.as_ref().map(Section::music),
            Family::Generic => manifest.audio_cpp.as_ref().map(|cfg| {
                let spec = manifest
                    .resolved_spec
                    .clone()
                    .or_else(|| cfg.model_spec.clone())
                    .or_else(|| read_spec(dir, cfg));
                Section::generic(cfg, spec)
            }),
        };
        let cfg = section.ok_or_else(|| {
            CoreError::Load(format!(
                "{}: missing [{}] section",
                manifest.id,
                family.cli_name()
            ))
        })?;

        // A manifest from an older runner (VoxCPM2 used llama.cpp-omni
        // with `base_lm`/`acoustic`) has no audio.cpp fields at all.
        if cfg.binary.is_empty() && cfg.model.is_empty() {
            return Err(CoreError::Load(match family {
                Family::VoxCpm2 => format!(
                    "{}: 该安装来自旧的运行器，请重新运行 scripts/download-voxcpm2.sh \
                     （VoxCPM2 已改为 audio.cpp 运行时）",
                    manifest.id
                ),
                _ => format!(
                    "{}: missing binary/model in [{}]",
                    manifest.id,
                    family.cli_name()
                ),
            }));
        }

        let binary = resolve_path(dir, &cfg.binary);
        if !binary.is_file() {
            return Err(CoreError::Load(format!(
                "{}: audiocpp_cli not found at {}",
                manifest.id,
                binary.display()
            )));
        }
        let model = resolve_path(dir, &cfg.model);
        let model_present = if cfg.model_is_file {
            model.is_file()
        } else {
            model.exists()
        };
        if !model_present {
            return Err(CoreError::Load(format!(
                "{}: model not found at {}",
                manifest.id,
                model.display()
            )));
        }

        // The resident server ships beside the CLI in the same release
        // archive. Installers from before it was copied simply lack it,
        // and the engine keeps working through the one-shot CLI.
        let server_name = if cfg!(windows) {
            "audiocpp_server.exe"
        } else {
            "audiocpp_server"
        };
        let server_bin = binary
            .with_file_name(server_name)
            .is_file()
            .then(|| binary.with_file_name(server_name));

        // The server rejects the CLI's `best`; resolve it once here.
        let backend = match &server_bin {
            Some(bin) => resolve_server_backend(bin, &cfg.backend),
            None => cfg.backend.clone(),
        };

        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            family,
            family_name: cfg.family_name,
            spec: cfg.spec,
            input_field: cfg.input_field,
            voice_ref: cfg.voice_ref,
            binary,
            model,
            backend,
            language: cfg.language.clone().unwrap_or_default(),
            guidance: cfg.guidance,
            seed: cfg.seed,
            reference_duration: cfg.reference_duration,
            task: cfg.task,
            load_options: cfg.load_options.into_iter().collect(),
            session_options: cfg.session_options.into_iter().collect(),
            env: cfg.env.into_iter().collect(),
            run_lock: Mutex::new(()),
            server_bin,
            resident: Mutex::new(None),
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

    /// Numeric parameter that may be declared as a float slider. JSON
    /// numbers carry the same value as either integer or float, so
    /// `as_f64` covers both.
    fn param_f64(&self, req: &SynthRequest, name: &str, fallback: f64) -> f64 {
        req.params
            .get(name)
            .and_then(Value::as_f64)
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

    /// Stop this engine's resident server, freeing the model's VRAM.
    ///
    /// Skipped while a request holds `run_lock`: the manager calls this from
    /// another engine's request path, and killing a server mid-synthesis
    /// would fail a call that was already in flight.
    fn unload(&self) {
        let Ok(_guard) = self.run_lock.try_lock() else {
            return;
        };
        self.stop_resident();
    }

    /// PID of the resident server, so the service can report which engine
    /// currently holds the model in memory (and let the UI watch its RSS).
    fn resident_pid(&self) -> Option<u32> {
        self.resident
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|server| server.child.id()))
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        // A generic install is driven entirely by its model spec and may
        // be a conversion model rather than a TTS one.
        if self.family == Family::Generic {
            return self.synthesize_generic(req);
        }
        let is_music = self.family == Family::AceStep;
        // Music never clones a voice; a stray `samples` upload must not turn
        // into a `--voice-ref` the music family does not understand.
        let has_prompt = !is_music && !req.prompt_samples.is_empty();
        if !has_prompt
            && matches!(
                self.family,
                Family::Dots | Family::CosyVoice | Family::IndexTts2
            )
        {
            return Err(CoreError::UserError(
                i18n::Message::new(Key::ReferenceAudioRequired).arg("engine", self.family.label()),
            ));
        }
        self.validate(req)?;

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
        let ref_wav = if has_prompt {
            let path = tmp.join("reference.wav");
            // VoxCPM2's AudioVAE encoder tensor holds 240000 samples at 16 kHz
            // (15 s, with patch padding) and the runtime ignores
            // --reference-duration-seconds for this family, so an overlong
            // voice reference must be trimmed on our side before encoding.
            let reference = if self.family == Family::VoxCpm2 {
                trim_reference(
                    &req.prompt_samples,
                    req.prompt_sr as u32,
                    VOXCPM2_MAX_REFERENCE_SECS,
                )
            } else {
                Cow::Borrowed(req.prompt_samples.as_slice())
            };
            if reference.len() < req.prompt_samples.len() {
                if let Some(cb) = &req.progress {
                    let kept = reference.len() as f32 / req.prompt_sr.max(1) as f32;
                    cb(ProgressEvent::Stage(format!(
                        "参考音频过长，VoxCPM2 已自动截取开头约 {kept:.0} 秒"
                    )));
                }
            }
            let bytes = encode_wav(&reference, req.prompt_sr)?;
            std::fs::write(&path, bytes).map_err(CoreError::from)?;
            Some(path)
        } else {
            None
        };

        let seed = self.param_i64(req, "seed", self.seed);

        // The resident server keeps the weights loaded between requests,
        // which saves the model load on every call. A failed request never
        // falls back to the one-shot CLI: that would load the model a second
        // time and, on a card that holds one model, is exactly the memory
        // pressure the resident server exists to avoid. Instead the server is
        // stopped and started again for the next attempt, which is also how a
        // switch to another engine's model works.
        if self.server_bin.is_some() {
            let audio = match self.synthesize_resident(req, ref_wav.as_deref(), seed) {
                Ok(audio) => audio,
                Err(failure) if !failure.retryable => return Err(failure.error),
                Err(failure) => {
                    tracing::warn!(
                        target: "audiocpp",
                        family = self.family.cli_name(),
                        "resident server failed, restarting it: {}",
                        failure.error
                    );
                    self.stop_resident();
                    self.synthesize_resident(req, ref_wav.as_deref(), seed)
                        .map_err(|failure| failure.error)?
                }
            };
            let _ = std::fs::remove_dir_all(&tmp);
            return Ok(audio);
        }

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
            .arg(&req.text);
        if let Some(ref_wav) = &ref_wav {
            cmd.arg("--voice-ref").arg(ref_wav);
        }
        cmd.arg("--seed")
            .arg(seed.to_string())
            .arg("--num-inference-steps")
            .arg(req.num_steps.to_string())
            .arg("--out")
            .arg(&out_wav)
            .arg("--metrics");

        self.add_family_options(req, &mut cmd)?;

        for (key, value) in &self.load_options {
            cmd.arg("--load-option").arg(format!("{key}={value}"));
        }
        for (key, value) in &self.session_options {
            cmd.arg("--session-option").arg(format!("{key}={value}"));
        }

        if let Some(seconds) = self.reference_duration {
            cmd.arg("--reference-duration-seconds")
                .arg(format!("{seconds}"));
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        if let Some(cb) = &req.progress {
            cb(ProgressEvent::Stage(if is_music {
                format!(
                    "正在加载模型并生成音乐（{}，首次加载约数秒）",
                    self.family.label()
                )
            } else {
                format!(
                    "正在加载模型并合成（{} 首次加载约数秒）",
                    self.family.label()
                )
            }));
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
        let (samples, sample_rate, channels) = if is_music {
            let (samples, channels, rate) = decode_wav_channels(&bytes)?;
            (samples, rate as i32, channels)
        } else {
            let (samples, rate) = decode_wav(&bytes)?;
            (samples, rate, 1)
        };

        let _ = std::fs::remove_dir_all(&tmp);

        Ok(Audio {
            samples,
            sample_rate,
            channels,
        })
    }

    /// Convert an existing clip into the engine's target voice.
    ///
    /// Only generic audio.cpp families that declare a conversion task
    /// implement this; the spec supplies the options.
    fn convert(&self, req: &ConvertRequest) -> Result<Audio> {
        let Some(spec) = &self.spec else {
            return Err(CoreError::BadRequest(format!(
                "{} does not support voice conversion",
                self.name
            )));
        };
        if req.input_samples.is_empty() {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::SourceAudioRequired,
            )));
        }
        if self.voice_ref && req.prompt_samples.is_empty() {
            return Err(CoreError::UserError(
                i18n::Message::new(Key::ReferenceAudioRequired).arg("engine", &self.name),
            ));
        }
        let options = spec.resolve(&req.params)?;
        let reference = (!req.prompt_samples.is_empty())
            .then_some((req.prompt_samples.as_slice(), req.prompt_sr));
        self.run_generic(
            "",
            Some((&req.input_samples, req.input_sr)),
            reference,
            options,
            req.progress.as_ref(),
        )
    }
}

impl AudioCppEngine {
    /// Family-specific request validation that must run before any work.
    fn validate(&self, req: &SynthRequest) -> Result<()> {
        match self.family {
            Family::CosyVoice => self.cosyvoice_validate(req),
            _ => Ok(()),
        }
    }

    /// audio.cpp task name for this request.
    fn task(&self, req: &SynthRequest) -> &str {
        match self.family {
            Family::Dots => self.dots_task(req),
            Family::VoxCpm2 => self.voxcpm2_task(req),
            Family::CosyVoice => self.cosyvoice_task(req),
            Family::IndexTts2 => self.indextts2_task(req),
            Family::AceStep => self.ace_step_task(req),
            Family::Generic => self.task.as_str(),
        }
    }

    fn add_family_options(&self, req: &SynthRequest, cmd: &mut Command) -> Result<()> {
        match self.family {
            Family::Dots => self.dots_options(req, cmd),
            Family::VoxCpm2 => self.voxcpm2_options(req, cmd),
            Family::CosyVoice => self.cosyvoice_options(req, cmd),
            Family::IndexTts2 => self.indextts2_options(req, cmd),
            Family::AceStep => self.ace_step_options(req, cmd),
            Family::Generic => Ok(()),
        }
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

    /// Generic synthesis: a text task driven by the model spec.
    fn synthesize_generic(&self, req: &SynthRequest) -> Result<Audio> {
        let Some(spec) = &self.spec else {
            return Err(CoreError::Load(format!(
                "{}: kind audio_cpp needs a model_spec.json",
                self.id
            )));
        };
        if self.input_field.is_some() {
            return Err(CoreError::BadRequest(format!(
                "{} converts source audio; use the convert endpoint",
                self.name
            )));
        }
        if self.voice_ref && req.prompt_samples.is_empty() {
            return Err(CoreError::UserError(
                i18n::Message::new(Key::ReferenceAudioRequired).arg("engine", &self.name),
            ));
        }
        let options = spec.resolve(&req.params)?;
        let reference = (!req.prompt_samples.is_empty())
            .then_some((req.prompt_samples.as_slice(), req.prompt_sr));
        self.run_generic(&req.text, None, reference, options, req.progress.as_ref())
    }

    /// Run one generic audio.cpp task, resident server first and the CLI
    /// as a fallback.
    fn run_generic(
        &self,
        text: &str,
        input: Option<(&[f32], i32)>,
        reference: Option<(&[f32], i32)>,
        options: BTreeMap<String, Value>,
        progress: Option<&ProgressSink>,
    ) -> Result<Audio> {
        let _guard = self.run_lock.lock().expect("audiocpp run lock");

        let tmp = std::env::temp_dir().join(format!(
            "laverock-{}-{}-{}",
            self.family_name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).map_err(CoreError::from)?;
        let out_wav = tmp.join("out.wav");

        let input_wav = match input {
            Some((samples, rate)) => {
                let path = tmp.join("input.wav");
                std::fs::write(&path, encode_wav(samples, rate)?).map_err(CoreError::from)?;
                Some(path)
            }
            None => None,
        };
        let ref_wav = match reference {
            Some((samples, rate)) => {
                let path = tmp.join("reference.wav");
                std::fs::write(&path, encode_wav(samples, rate)?).map_err(CoreError::from)?;
                Some(path)
            }
            None => None,
        };

        let audio = if self.server_bin.is_some() {
            let attempt = || {
                self.run_generic_resident(
                    text,
                    input_wav.as_deref(),
                    ref_wav.as_deref(),
                    &options,
                    progress,
                )
            };
            match attempt() {
                Ok(audio) => audio,
                Err(failure) if !failure.retryable => return Err(failure.error),
                Err(failure) => {
                    tracing::warn!(
                        target: "audiocpp",
                        family = %self.family_name,
                        "resident server failed, restarting it: {}",
                        failure.error
                    );
                    self.stop_resident();
                    attempt().map_err(|failure| failure.error)?
                }
            }
        } else {
            self.run_generic_cli(
                text,
                input_wav.as_deref(),
                ref_wav.as_deref(),
                &options,
                &out_wav,
                progress,
            )?
        };
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(audio)
    }

    fn run_generic_cli(
        &self,
        text: &str,
        input_wav: Option<&Path>,
        ref_wav: Option<&Path>,
        options: &BTreeMap<String, Value>,
        out_wav: &Path,
        progress: Option<&ProgressSink>,
    ) -> Result<Audio> {
        let mut cmd = Command::new(&self.binary);
        cmd.arg("--task")
            .arg(&self.task)
            .arg("--family")
            .arg(&self.family_name)
            .arg("--model")
            .arg(&self.model)
            .arg("--backend")
            .arg(&self.backend)
            .arg("--out")
            .arg(out_wav)
            .arg("--metrics");
        if !text.is_empty() {
            cmd.arg("--text").arg(text);
        }
        if let Some(path) = input_wav {
            cmd.arg("--audio").arg(path);
        }
        if let Some(path) = ref_wav {
            cmd.arg("--voice-ref").arg(path);
        }
        for (key, value) in options {
            cmd.arg("--request-option")
                .arg(format!("{key}={}", option_text(value)));
        }
        for (key, value) in &self.load_options {
            cmd.arg("--load-option").arg(format!("{key}={value}"));
        }
        for (key, value) in &self.session_options {
            cmd.arg("--session-option").arg(format!("{key}={value}"));
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        if let Some(cb) = progress {
            cb(ProgressEvent::Stage(self.stage_label(input_wav.is_some())));
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            CoreError::Synth(format!(
                "failed to run {}: {e} (is the audio.cpp runtime installed?)",
                self.binary.display()
            ))
        })?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stdout_handle = std::thread::spawn(move || {
            let mut lines = Vec::new();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if !line.trim().is_empty() {
                    lines.push(line);
                }
            }
            lines
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
            let mut tail: Vec<String> = stderr.lines().rev().take(12).map(str::to_string).collect();
            tail.reverse();
            tail.extend(stdout_lines.into_iter().rev().take(6).rev());
            return Err(CoreError::Synth(format!(
                "{} 推理失败（退出码 {:?}）：\n{}",
                self.name,
                status.code(),
                tail.join("\n").trim()
            )));
        }

        let bytes = std::fs::read(out_wav)
            .map_err(|e| CoreError::Synth(format!("runner produced no audio: {e}")))?;
        decode_audio(&bytes)
    }

    fn run_generic_resident(
        &self,
        text: &str,
        input_wav: Option<&Path>,
        ref_wav: Option<&Path>,
        options: &BTreeMap<String, Value>,
        progress: Option<&ProgressSink>,
    ) -> std::result::Result<Audio, ResidentFailure> {
        let retry = |error: CoreError| ResidentFailure {
            error,
            retryable: true,
        };
        let port = self.ensure_server().map_err(retry)?;
        if let Some(cb) = progress {
            cb(ProgressEvent::Stage(self.stage_label(input_wav.is_some())));
        }

        let mut request = Map::new();
        request.insert(
            "options".into(),
            Value::Object(options.clone().into_iter().collect()),
        );
        if !text.is_empty() {
            request.insert("text".into(), json!(text));
        }
        if let Some(path) = input_wav {
            request.insert("audio".into(), json!(path.to_string_lossy()));
        }
        if let Some(path) = ref_wav {
            request.insert("voice_ref".into(), json!(path.to_string_lossy()));
        }
        let body = json!({
            "model": self.resident_id(&self.task),
            "request": Value::Object(request),
        });
        let bytes = self.post_resident(port, "/v1/tasks/run", &body)?;

        // `/v1/tasks/run` answers with a JSON envelope carrying the WAV
        // as base64, unlike the speech endpoint's raw audio body.
        let envelope: Value = serde_json::from_slice(&bytes).map_err(|e| ResidentFailure {
            error: CoreError::Synth(format!("audiocpp_server returned invalid JSON: {e}")),
            retryable: true,
        })?;
        let encoded = envelope
            .get("audio")
            .and_then(Value::as_str)
            .ok_or_else(|| ResidentFailure {
                error: CoreError::Synth(
                    "audiocpp_server returned no audio for this task".to_string(),
                ),
                retryable: false,
            })?;
        let wav = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| ResidentFailure {
                error: CoreError::Synth(format!("audiocpp_server returned bad audio: {e}")),
                retryable: false,
            })?;
        decode_audio(&wav).map_err(|error| ResidentFailure {
            error,
            retryable: true,
        })
    }

    fn stage_label(&self, converting: bool) -> String {
        let action = if converting { "转换声音" } else { "合成" };
        format!("正在{action}（{} 常驻运行时）", self.name)
    }
}

/// Render an option value the way audio.cpp's CLI parses it.
fn option_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// Decode a runner's WAV, keeping its channel layout (music and some
/// conversion models return stereo).
fn decode_audio(bytes: &[u8]) -> Result<Audio> {
    let (samples, channels, rate) = decode_wav_channels(bytes)?;
    Ok(Audio {
        samples,
        sample_rate: rate as i32,
        channels,
    })
}

/// Pick a concrete backend when the manifest says `best`, which only the
/// CLI understands. `audiocpp_server --version` lists the compiled-in
/// backends, so this follows the same binary the server will run.
fn resolve_server_backend(bin: &Path, backend: &str) -> String {
    if backend != "best" {
        return backend.to_string();
    }
    let output = Command::new(bin).arg("--version").output();
    let text = output
        .map(|o| {
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            text
        })
        .unwrap_or_default();
    let available: Vec<String> = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("backends:"))
        .map(|list| list.split(',').map(|b| b.trim().to_string()).collect())
        .unwrap_or_default();
    for candidate in ["cuda", "metal", "vulkan", "hip", "cpu"] {
        if available.iter().any(|b| b == candidate) {
            return candidate.to_string();
        }
    }
    // No parsed list: the platform's usual accelerator, else CPU.
    if cfg!(target_os = "macos") {
        "metal".to_string()
    } else {
        "cpu".to_string()
    }
}

impl AudioCppEngine {
    /// Tasks this family can run. Each one becomes a separate entry in the
    /// server config, because a session is bound to its task; VoxCPM2 and
    /// CosyVoice3 switch between cloning and plain/instruction TTS.
    fn server_tasks(&self) -> Vec<&str> {
        match self.family {
            Family::Dots => self.dots_server_tasks(),
            Family::VoxCpm2 => self.voxcpm2_server_tasks(),
            Family::CosyVoice => self.cosyvoice_server_tasks(),
            Family::IndexTts2 => self.indextts2_server_tasks(),
            Family::AceStep => self.ace_step_server_tasks(),
            Family::Generic => vec![self.task.as_str()],
        }
    }

    /// The config entry (and request `model` value) for one task.
    fn resident_id(&self, task: &str) -> String {
        format!("laverock-{task}")
    }

    /// `server.json` for a server hosting this one model. Keeping
    /// `max_loaded_models` at 1 bounds memory when a family alternates
    /// between tasks: the other session is evicted and reloaded.
    fn server_config(&self, port: u16) -> Value {
        // The server resolves a relative path against its own working
        // directory, not ours, so a relative LAVEROCK_MODELS_DIR made it fail
        // to find the weights. Resolve here, where the path is known to be
        // right.
        let model = std::path::absolute(&self.model)
            .unwrap_or_else(|_| self.model.clone())
            .to_string_lossy()
            .to_string();
        let models: Vec<Value> = self
            .server_tasks()
            .iter()
            .map(|task| {
                let mut entry = json!({
                    "id": self.resident_id(task),
                    "family": self.family_name.as_str(),
                    "path": model,
                    "task": task,
                    "mode": "offline",
                });
                if !self.load_options.is_empty() {
                    entry["load_options"] = Value::Object(
                        self.load_options
                            .iter()
                            .map(|(key, value)| (key.clone(), json!(value)))
                            .collect(),
                    );
                }
                if !self.session_options.is_empty() {
                    entry["session_options"] = Value::Object(
                        self.session_options
                            .iter()
                            .map(|(key, value)| (key.clone(), json!(value)))
                            .collect(),
                    );
                }
                entry
            })
            .collect();
        json!({
            "host": "127.0.0.1",
            "port": port,
            "backend": self.backend,
            "lazy_load": true,
            "max_loaded_models": 1,
            "models": models,
        })
    }

    /// The OpenAI-shaped speech request for this call. The option names are
    /// the CLI's long flags without the dashes, so both paths stay in sync.
    fn resident_body(
        &self,
        req: &SynthRequest,
        ref_wav: Option<&Path>,
        seed: i64,
    ) -> Result<Value> {
        let family = match self.family {
            Family::Dots => self.dots_resident(req)?,
            Family::VoxCpm2 => self.voxcpm2_resident(req)?,
            Family::CosyVoice => self.cosyvoice_resident(req)?,
            Family::IndexTts2 => self.indextts2_resident(req)?,
            Family::AceStep => self.ace_step_resident(req)?,
            Family::Generic => {
                return Err(CoreError::Load(
                    "generic audio.cpp tasks use /v1/tasks/run".into(),
                ))
            }
        };

        let mut body = json!({
            "model": self.resident_id(self.task(req)),
            "input": req.text,
            "seed": seed,
            "options": Value::Object(family.options),
        });
        if let Some(path) = ref_wav {
            body["voice_ref"] = json!(path.to_string_lossy());
        }
        if let Some(text) = family.reference_text {
            body["reference_text"] = json!(text);
        }
        Ok(body)
    }

    /// Port for a new server: bind and release, so the kernel hands out an
    /// unused one. A concurrent process could take it in between; the
    /// spawn/health failure then falls back to the CLI.
    fn free_port() -> Result<u16> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| CoreError::Synth(format!("cannot pick a server port: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| CoreError::Synth(format!("cannot read the server port: {e}")))?
            .port();
        drop(listener);
        Ok(port)
    }

    /// Start the server if it is not running and wait until it answers.
    fn ensure_server(&self) -> Result<u16> {
        let mut guard = self.resident.lock().expect("resident lock poisoned");
        if let Some(server) = guard.as_mut() {
            match server.child.try_wait() {
                Ok(None) => return Ok(server.port),
                _ => {
                    // Exited or unreadable: drop the handle and start over.
                    *guard = None;
                }
            }
        }

        let bin = self
            .server_bin
            .as_ref()
            .ok_or_else(|| CoreError::Synth("audiocpp_server is not installed".into()))?;
        let port = Self::free_port()?;
        let dir = std::env::temp_dir().join(format!(
            "laverock-acsrv-{}-{}",
            self.family.cli_name(),
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).map_err(CoreError::from)?;
        let config = dir.join("server.json");
        std::fs::write(&config, self.server_config(port).to_string()).map_err(CoreError::from)?;

        let mut cmd = Command::new(bin);
        cmd.arg("--config")
            .arg(&config)
            .arg("--no-ui")
            .arg("--backend")
            .arg(&self.backend)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--max-loaded-models")
            .arg("1")
            .arg("--idle-unload-ms")
            .arg(RESIDENT_IDLE_UNLOAD_MS.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| CoreError::Synth(format!("cannot start audiocpp_server: {e}")))?;

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_millis(300))
            .timeout_read(Duration::from_millis(500))
            .build();
        let url = format!("http://127.0.0.1:{port}/health");
        let deadline = Instant::now() + RESIDENT_START_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(CoreError::Synth(format!(
                        "audiocpp_server exited during startup ({status})"
                    )));
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = child.kill();
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(CoreError::Synth(format!(
                        "cannot check audiocpp_server: {e}"
                    )));
                }
            }
            if agent.get(&url).call().is_ok() {
                *guard = Some(ResidentServer { child, port, dir });
                return Ok(port);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_dir_all(&dir);
                return Err(CoreError::Synth(
                    "audiocpp_server did not become ready in time".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    fn stop_resident(&self) {
        let Ok(mut guard) = self.resident.lock() else {
            return;
        };
        if let Some(mut server) = guard.take() {
            let _ = server.child.kill();
            let _ = server.child.wait();
            let _ = std::fs::remove_dir_all(&server.dir);
        }
    }

    fn synthesize_resident(
        &self,
        req: &SynthRequest,
        ref_wav: Option<&Path>,
        seed: i64,
    ) -> std::result::Result<Audio, ResidentFailure> {
        let retry = |error: CoreError| ResidentFailure {
            error,
            retryable: true,
        };
        let port = self.ensure_server().map_err(retry)?;
        if let Some(cb) = &req.progress {
            cb(ProgressEvent::Stage(if self.family == Family::AceStep {
                format!("正在生成音乐（{} 常驻运行时）", self.family.label())
            } else {
                format!("正在合成（{} 常驻运行时）", self.family.label())
            }));
        }
        // A malformed request fails the same way on a fresh server.
        let body = self
            .resident_body(req, ref_wav, seed)
            .map_err(|error| ResidentFailure {
                error,
                retryable: false,
            })?;
        let bytes = self.post_resident(port, "/v1/audio/speech", &body)?;
        let (samples, sample_rate, channels) = if self.family == Family::AceStep {
            let (samples, channels, rate) = decode_wav_channels(&bytes).map_err(retry)?;
            (samples, rate as i32, channels)
        } else {
            let (samples, rate) = decode_wav(&bytes).map_err(retry)?;
            (samples, rate, 1)
        };
        Ok(Audio {
            samples,
            sample_rate,
            channels,
        })
    }

    /// POST one JSON body to the resident server and return the raw
    /// response. 4xx is the caller's mistake (no restart); transport
    /// failures and 5xx are retried against a fresh server.
    fn post_resident(
        &self,
        port: u16,
        path: &str,
        body: &Value,
    ) -> std::result::Result<Vec<u8>, ResidentFailure> {
        let retry = |error: CoreError| ResidentFailure {
            error,
            retryable: true,
        };
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(3))
            .timeout_read(RESIDENT_READ_TIMEOUT)
            .build();
        let url = format!("http://127.0.0.1:{port}{path}");
        let response = agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body.to_string());
        let mut bytes = Vec::new();
        match response {
            Ok(resp) => {
                resp.into_reader().read_to_end(&mut bytes).map_err(|e| {
                    retry(CoreError::Synth(format!(
                        "cannot read the server response: {e}"
                    )))
                })?;
            }
            Err(ureq::Error::Status(code, resp)) => {
                let mut text = String::new();
                let _ = resp.into_reader().read_to_string(&mut text);
                return Err(ResidentFailure {
                    error: CoreError::Synth(format!(
                        "audiocpp_server HTTP {code}: {}",
                        text.trim()
                    )),
                    retryable: code >= 500,
                });
            }
            Err(e) => {
                return Err(retry(CoreError::Synth(format!(
                    "audiocpp_server request failed: {e}"
                ))))
            }
        }
        Ok(bytes)
    }
}

/// A failed resident request. `retryable` separates a broken server (process
/// gone, port taken, 5xx) from a request the server correctly rejected.
struct ResidentFailure {
    error: CoreError,
    retryable: bool,
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

/// Read the spec a generic install names, or its conventional
/// `model_spec.json`. A failure only costs the generated parameter
/// controls; the engine still runs with the manifest's own options.
fn read_spec(dir: &Path, cfg: &AudioCppTaskManifest) -> Option<ModelSpec> {
    let path = dir.join(cfg.spec.as_deref().unwrap_or("model_spec.json"));
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            if cfg.spec.is_some() {
                tracing::warn!(spec = %path.display(), "cannot read model spec: {e}");
            }
            return None;
        }
    };
    match ModelSpec::parse(&raw) {
        Ok(spec) => Some(spec),
        Err(e) => {
            tracing::warn!(spec = %path.display(), "invalid model spec: {e}");
            None
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn manifest(kind: &str) -> Manifest {
        let (section, id) = match kind {
            "dots" => ("dots", "dots"),
            "voxcpm2" => ("voxcpm2", "voxcpm2"),
            "indextts2" => ("indextts2", "indextts2"),
            "music" => ("music", "ace-step"),
            "audio_cpp" => ("audio_cpp", "seed-vc"),
            _ => ("cosyvoice", "cosyvoice"),
        };
        let json = if kind == "audio_cpp" {
            serde_json::json!({
                "id": id,
                "kind": kind,
                "name": "test",
                "defaults": { "speed": 1.0, "num_steps": 30 },
                section: {
                    "binary": "fake_cli.sh",
                    "model": "fake.gguf",
                    "backend": "cpu",
                    "family": "seed_vc",
                    "task": "vc",
                    "model_spec": {
                        "family": "seed_vc",
                        "options": { "request": [
                            { "name": "route", "type": "enum",
                              "values": ["v2_vc", "v1_svc"], "default": "v2_vc" },
                            { "name": "num_inference_steps", "type": "int",
                              "default": 30, "min": 1, "max": 100 }
                        ] }
                    }
                }
            })
        } else {
            serde_json::json!({
                "id": id,
                "kind": kind,
                "name": "test",
                "defaults": { "speed": 1.0, "num_steps": 10 },
                section: {
                    "binary": "fake_cli.sh",
                    "model": "fake.gguf",
                    "backend": "cpu"
                }
            })
        };
        serde_json::from_value(json).expect("manifest")
    }

    fn fixture(dir: &Path, kind: &str) -> AudioCppEngine {
        std::fs::write(dir.join("fake.gguf"), b"x").unwrap();
        std::fs::write(
            dir.join("fake_cli.sh"),
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/args.txt\"\n\
             out=\"\"; ref=\"\"; input=\"\"\nwhile [ $# -gt 0 ]; do [ \"$1\" = \"--out\" ] && out=$2; \
             [ \"$1\" = \"--voice-ref\" ] && ref=$2; [ \"$1\" = \"--audio\" ] && input=$2; shift; done\n\
             d=\"$(dirname \"$0\")\"; cp \"$d/fx.wav\" \"$out\"\n\
             [ -n \"$ref\" ] && cp \"$ref\" \"$d/ref.wav\"\n\
             [ -n \"$input\" ] && cp \"$input\" \"$d/input.wav\"\nexit 0\n",
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
        let family = match kind {
            "dots" => super::Family::Dots,
            "voxcpm2" => super::Family::VoxCpm2,
            "indextts2" => super::Family::IndexTts2,
            "music" => super::Family::AceStep,
            "audio_cpp" => super::Family::Generic,
            _ => super::Family::CosyVoice,
        };
        AudioCppEngine::build(dir, &m, family).expect("engine")
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
            input_samples: Vec::new(),
            input_sr: 0,
            progress: None,
            audio: None,
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
    fn indextts2_clones_with_language() {
        let dir = args_dir("indextts2");
        let args = run(&dir, "indextts2", &[]);
        assert!(args.windows(2).any(|w| w == ["--family", "index_tts2"]));
        assert!(args.windows(2).any(|w| w == ["--task", "clon"]));
        assert!(args.iter().any(|a| a == "--voice-ref"));
        // No transcript: IndexTTS-2 clones from the audio alone.
        assert!(!args.iter().any(|a| a == "--reference-text"));
        // Beam search stays the default; the fast path is opt-in.
        assert!(args
            .windows(2)
            .any(|w| w == ["--request-option", "num_beams=3"]));
        // Segment controls keep audio.cpp's chunking but give full stops an
        // audible gap between segments.
        assert!(args.windows(2).any(|w| w == ["--text-chunk-size", "600"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--request-option", "interval_silence_ms=400"]));
        let _ = std::fs::remove_dir_all(&dir);

        let dir = args_dir("indextts2-pauses");
        let args = run(
            &dir,
            "indextts2",
            &[
                ("text_chunk_size", serde_json::json!(40)),
                ("interval_silence_ms", serde_json::json!(300)),
            ],
        );
        assert!(args.windows(2).any(|w| w == ["--text-chunk-size", "40"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--request-option", "interval_silence_ms=300"]));
        let _ = std::fs::remove_dir_all(&dir);

        let dir = args_dir("indextts2-fast");
        let args = run(&dir, "indextts2", &[("num_beams", serde_json::json!("1"))]);
        assert!(args
            .windows(2)
            .any(|w| w == ["--request-option", "num_beams=1"]));
        let _ = std::fs::remove_dir_all(&dir);

        // A per-request language overrides the manifest default.
        let dir = args_dir("indextts2-lang");
        let args = run(&dir, "indextts2", &[("language", serde_json::json!("en"))]);
        assert!(args.windows(2).any(|w| w == ["--language", "en"]));
        let _ = std::fs::remove_dir_all(&dir);

        // "auto" is audio.cpp's own default; passing it would be redundant.
        let dir = args_dir("indextts2-auto");
        let args = run(
            &dir,
            "indextts2",
            &[("language", serde_json::json!("auto"))],
        );
        assert!(!args.iter().any(|a| a == "--language"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn indextts2_requires_reference() {
        let dir = args_dir("indextts2-ref");
        let engine = fixture(&dir, "indextts2");
        let mut req = request(&[]);
        req.prompt_samples.clear();
        let err = engine.synthesize(&req).unwrap_err();
        assert!(err.to_string().contains("reference clip"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn best_backend_follows_the_server_build() {
        let dir = args_dir("backend-probe");
        let probe = dir.join("probe.sh");
        std::fs::write(&probe, "#!/bin/sh\necho 'backends: cpu,metal'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&probe).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&probe, perms).unwrap();
        }
        assert_eq!(resolve_server_backend(&probe, "best"), "metal");
        assert_eq!(resolve_server_backend(&probe, "cpu"), "cpu");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resident_server_is_detected_beside_the_cli() {
        let dir = args_dir("resident-bin");
        let engine = fixture(&dir, "cosyvoice");
        assert!(engine.server_bin.is_none(), "fixture has no server");
        std::fs::write(dir.join("audiocpp_server"), b"x").unwrap();
        let engine = fixture(&dir, "cosyvoice");
        assert_eq!(
            engine.server_bin.as_deref(),
            Some(dir.join("audiocpp_server").as_path())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resident_server_config_and_body() {
        let dir = args_dir("resident");
        let engine = fixture(&dir, "indextts2");
        let config = engine.server_config(1234);
        assert_eq!(config["port"], 1234);
        assert_eq!(config["max_loaded_models"], 1);
        let models = config["models"].as_array().expect("models");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["id"], "laverock-clon");
        assert_eq!(models[0]["family"], "index_tts2");
        assert_eq!(models[0]["task"], "clon");

        let body = engine
            .resident_body(
                &request(&[
                    ("num_beams", serde_json::json!("1")),
                    ("language", serde_json::json!("en")),
                ]),
                Some(Path::new("/tmp/ref.wav")),
                7,
            )
            .expect("body");
        assert_eq!(body["model"], "laverock-clon");
        assert_eq!(body["voice_ref"], "/tmp/ref.wav");
        assert_eq!(body["seed"], 7);
        assert_eq!(body["options"]["num_beams"], 1);
        assert_eq!(body["options"]["language"], "en");
        // The transcript is not part of IndexTTS-2's contract.
        assert!(body.get("reference_text").is_none());

        // Families that switch tasks get one config entry per task.
        let engine = fixture(&dir, "cosyvoice");
        let config = engine.server_config(1);
        let tasks: Vec<&str> = config["models"]
            .as_array()
            .expect("models")
            .iter()
            .map(|m| m["task"].as_str().expect("task"))
            .collect();
        assert_eq!(tasks, ["clon", "tts"]);
        let body = engine
            .resident_body(
                &request(&[
                    ("template", serde_json::json!("instruct")),
                    ("instruct", serde_json::json!("warm")),
                ]),
                None,
                1,
            )
            .expect("body");
        assert_eq!(body["model"], "laverock-tts");
        assert_eq!(body["options"]["template_name"], "instruct");
        assert_eq!(body["options"]["instruct"], "warm");
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
        assert!(args.windows(2).any(|w| w == ["--text-chunk-size", "100"]));

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
        // Display is English; the Chinese text is available via localized().
        let err = engine.synthesize(&req).unwrap_err();
        assert!(err.to_string().contains("Zero-shot"), "{err}");
        assert!(err.localized(crate::i18n::Lang::Zh).contains("零样本"));

        let req = request(&[("template", serde_json::json!("instruct"))]);
        let err = engine.synthesize(&req).unwrap_err();
        assert!(err.to_string().contains("instruct template"), "{err}");
        assert!(err.localized(crate::i18n::Lang::Zh).contains("指令"));
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
    fn ace_step_maps_music_options_and_keeps_stereo() {
        let dir = args_dir("ace-step");
        let engine = fixture(&dir, "music");
        // 48 kHz stereo, the shape ACE-Step returns.
        let interleaved: Vec<f32> = (0..48_000)
            .map(|i| if i % 2 == 0 { 0.1 } else { -0.1 })
            .collect();
        std::fs::write(
            dir.join("fx.wav"),
            crate::encode_wav_channels(&interleaved, 48_000, 2).unwrap(),
        )
        .unwrap();

        let audio = engine
            .synthesize(&request(&[
                ("lyrics", serde_json::json!("[verse] hello")),
                ("duration_sec", serde_json::json!(90)),
                ("task_route", serde_json::json!("text2music")),
                ("guidance_scale", serde_json::json!(2.5)),
            ]))
            .expect("synthesis");
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.sample_rate, 48_000);
        assert_eq!(audio.samples.len(), 48_000);

        let args: Vec<String> = std::fs::read_to_string(dir.join("args.txt"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert!(args.windows(2).any(|w| w == ["--task", "gen"]));
        assert!(args.windows(2).any(|w| w == ["--family", "ace_step"]));
        assert!(args.windows(2).any(|w| w == ["--lyrics", "[verse] hello"]));
        assert!(args.windows(2).any(|w| w == ["--duration-seconds", "90"]));
        assert!(args.windows(2).any(|w| w == ["--task-route", "text2music"]));
        assert!(args.windows(2).any(|w| w == ["--guidance-scale", "2.5"]));
        // The request carried samples, but music must not clone a voice.
        assert!(!args.iter().any(|a| a == "--voice-ref"));

        // Instrumental: no lyrics, and still no reference clip required.
        let mut req = request(&[("duration_sec", serde_json::json!(30))]);
        req.prompt_samples.clear();
        engine.synthesize(&req).expect("instrumental");
        let args: Vec<String> = std::fs::read_to_string(dir.join("args.txt"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert!(!args.iter().any(|a| a == "--lyrics"));
        assert!(args.windows(2).any(|w| w == ["--duration-seconds", "30"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ace_step_resident_config_and_body() {
        let dir = args_dir("ace-step-resident");
        let engine = fixture(&dir, "music");
        let config = engine.server_config(1234);
        let models = config["models"].as_array().expect("models");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["id"], "laverock-gen");
        assert_eq!(models[0]["family"], "ace_step");
        assert_eq!(models[0]["task"], "gen");

        let body = engine
            .resident_body(
                &request(&[
                    ("lyrics", serde_json::json!("la la")),
                    ("duration_sec", serde_json::json!(45)),
                    ("guidance_scale", serde_json::json!(3.0)),
                ]),
                None,
                9,
            )
            .expect("body");
        assert_eq!(body["model"], "laverock-gen");
        assert_eq!(body["input"], "hello");
        assert_eq!(body["seed"], 9);
        assert_eq!(body["options"]["lyrics"], "la la");
        assert_eq!(body["options"]["duration_sec"].as_f64(), Some(45.0));
        assert_eq!(body["options"]["route"], "text2music");
        assert_eq!(body["options"]["guidance_scale"], 3.0);
        assert_eq!(body["options"]["num_inference_steps"], 10);
        assert!(body.get("voice_ref").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn voxcpm2_trims_overlong_reference() {
        let dir = args_dir("vox-trim");
        let engine = fixture(&dir, "voxcpm2");
        let mut req = request(&[]);
        // 25 s @ 24 kHz — well beyond the 14 s trim ceiling.
        req.prompt_samples = vec![0.2f32; 24_000 * 25];
        engine.synthesize(&req).expect("synthesis");
        let ref_bytes = std::fs::read(dir.join("ref.wav")).expect("voice-ref copied out");
        let (trimmed, rate) = crate::decode_wav(&ref_bytes).expect("ref is a wav");
        assert_eq!(rate, 24_000);
        // At most 14 s, and the cut landed within the final 2 s window
        // (>= 11.9 s retained) even with no silence to target.
        assert!(trimmed.len() <= 14 * 24_000, "kept {}", trimmed.len());
        assert!(
            trimmed.len() >= (11.9 * 24_000.0) as usize,
            "kept {}",
            trimmed.len()
        );

        // A short reference passes through untouched.
        let dir2 = args_dir("vox-trim2");
        let engine2 = fixture(&dir2, "voxcpm2");
        let mut req2 = request(&[]);
        req2.prompt_samples = vec![0.2f32; 24_000 * 5];
        engine2.synthesize(&req2).expect("synthesis");
        let ref_bytes2 = std::fs::read(dir2.join("ref.wav")).expect("voice-ref copied out");
        let (short, _) = crate::decode_wav(&ref_bytes2).expect("ref is a wav");
        assert_eq!(short.len(), 24_000 * 5);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
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

    fn convert_request() -> ConvertRequest {
        ConvertRequest {
            input_samples: vec![0.3; 2400],
            input_sr: 24_000,
            prompt_samples: vec![0.2; 2400],
            prompt_sr: 24_000,
            params: std::collections::BTreeMap::new(),
            progress: None,
        }
    }

    #[test]
    fn generic_vc_runs_the_cli_with_audio_and_spec_options() {
        let dir = args_dir("generic-vc");
        let engine = fixture(&dir, "audio_cpp");
        let audio = engine.convert(&convert_request()).expect("convert");
        assert_eq!(audio.sample_rate, 24_000);
        assert_eq!(audio.channels, 1);

        let args: Vec<String> = std::fs::read_to_string(dir.join("args.txt"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert!(args.windows(2).any(|w| w == ["--task", "vc"]));
        assert!(args.windows(2).any(|w| w == ["--family", "seed_vc"]));
        assert!(args.iter().any(|a| a == "--audio"));
        assert!(args.iter().any(|a| a == "--voice-ref"));
        // Defaults from the spec are passed as request options.
        assert!(args
            .windows(2)
            .any(|w| w == ["--request-option", "route=v2_vc"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--request-option", "num_inference_steps=30"]));

        // The source clip reached the runner unchanged.
        let bytes = std::fs::read(dir.join("input.wav")).expect("source audio copied");
        let (samples, rate) = crate::decode_wav(&bytes).expect("wav");
        assert_eq!(rate, 24_000);
        assert_eq!(samples.len(), 2400);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generic_vc_requires_source_audio() {
        let dir = args_dir("generic-vc-empty");
        let engine = fixture(&dir, "audio_cpp");
        let mut req = convert_request();
        req.input_samples.clear();
        let err = engine.convert(&req).unwrap_err();
        assert!(err.to_string().contains("source clip"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generic_synth_refuses_a_conversion_task() {
        let dir = args_dir("generic-synth");
        let engine = fixture(&dir, "audio_cpp");
        let err = engine.synthesize(&request(&[])).unwrap_err();
        assert!(err.to_string().contains("convert endpoint"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generic_rejects_unknown_options() {
        let dir = args_dir("generic-unknown");
        let engine = fixture(&dir, "audio_cpp");
        let mut req = convert_request();
        req.params.insert("surprise".into(), serde_json::json!(1));
        let err = engine.convert(&req).unwrap_err();
        assert!(err.to_string().contains("unknown option"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generic_resident_config_and_body() {
        let dir = args_dir("generic-resident");
        let engine = fixture(&dir, "audio_cpp");
        let config = engine.server_config(1234);
        let models = config["models"].as_array().expect("models");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["family"], "seed_vc");
        assert_eq!(models[0]["task"], "vc");
        assert_eq!(engine.resident_id(&engine.task), "laverock-vc");
    }
}
