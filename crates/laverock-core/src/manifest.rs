//! Engine manifest format.
//!
//! Every model directory under the models root must contain an
//! `engine.json`: it is the single source of truth for which files an
//! engine uses and what its default synthesis parameters are.

use crate::task::TaskKind;
use serde::{Deserialize, Serialize};

/// Supported backend kinds. Adding a new kind is a code change; adding a
/// new model of an existing kind is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineKind {
    #[serde(rename = "zipvoice")]
    ZipVoice,
    #[serde(rename = "voxcpm2")]
    VoxCpm2,
    #[serde(rename = "qwen3tts")]
    Qwen3Tts,
    #[serde(rename = "longcat")]
    LongCat,
    #[serde(rename = "dots")]
    Dots,
    #[serde(rename = "cosyvoice")]
    CosyVoice,
    #[serde(rename = "kokoro")]
    Kokoro,
    #[serde(rename = "ming3tts")]
    Ming3Tts,
    #[serde(rename = "chatterbox")]
    Chatterbox,
    /// IndexTTS-2 (bilibili) run by audio.cpp's `index_tts2` family.
    #[serde(rename = "indextts2")]
    IndexTts2,
    /// MOSS-TTS-Delay (OpenMOSS) run by a native llama.cpp binary.
    #[serde(rename = "mosstts")]
    MossTts,
    /// Music generation (audio.cpp `--task gen`). Not speech synthesis:
    /// requests carry a caption, optional lyrics and a duration.
    #[serde(rename = "music")]
    Music,
    /// Any audio.cpp family driven by a `model_spec.json` instead of a
    /// hand-written Rust mapping. Voice conversion (Seed-VC) is the first
    /// user; the `[audio_cpp]` section names the family and task.
    #[serde(rename = "audio_cpp")]
    AudioCpp,
    /// Speech-to-text models (SenseVoice via sherpa-onnx). Not a
    /// synthesis engine; discovered by the transcriber instead.
    #[serde(rename = "stt")]
    Stt,
}

/// Backend configuration for a speech-to-text model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttManifest {
    /// Recognizer family, currently only `sense_voice`.
    #[serde(default = "default_stt_engine")]
    pub engine: String,
    pub model: String,
    pub tokens: String,
    /// SenseVoice language hint: `auto`, `zh`, `en`, `ja`, `ko`, `yue`.
    #[serde(default = "default_stt_language")]
    pub language: String,
    /// Inverse text normalization (digits, dates, punctuation).
    #[serde(default = "default_true")]
    pub use_itn: bool,
    #[serde(default = "default_threads")]
    pub num_threads: i32,
}

fn default_stt_engine() -> String {
    "sense_voice".to_string()
}
fn default_stt_language() -> String {
    "auto".to_string()
}
fn default_true() -> bool {
    true
}

impl EngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineKind::ZipVoice => "zipvoice",
            EngineKind::VoxCpm2 => "voxcpm2",
            EngineKind::Qwen3Tts => "qwen3tts",
            EngineKind::LongCat => "longcat",
            EngineKind::Dots => "dots",
            EngineKind::CosyVoice => "cosyvoice",
            EngineKind::Kokoro => "kokoro",
            EngineKind::Ming3Tts => "ming3tts",
            EngineKind::Chatterbox => "chatterbox",
            EngineKind::IndexTts2 => "indextts2",
            EngineKind::MossTts => "mosstts",
            EngineKind::Music => "music",
            EngineKind::AudioCpp => "audio_cpp",
            EngineKind::Stt => "stt",
        }
    }

    /// Runtime that executes this engine, shown in the resource panel.
    pub fn backend(self) -> &'static str {
        match self {
            EngineKind::VoxCpm2
            | EngineKind::Dots
            | EngineKind::CosyVoice
            | EngineKind::IndexTts2
            | EngineKind::Music
            | EngineKind::AudioCpp => "audiocpp",
            EngineKind::MossTts => "llama.cpp",
            EngineKind::Ming3Tts | EngineKind::Chatterbox => "python",
            _ => "onnx",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct License {
    /// Short name shown in the UI, e.g. `MIT` or `CC BY-NC 4.0`.
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    /// The licence forbids commercial use. The music picker avoids
    /// defaulting to such a model when a commercial alternative exists.
    #[serde(default)]
    pub noncommercial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Stable identifier, also the directory name in practice.
    pub id: String,
    pub kind: EngineKind,
    /// The operation this model performs. When absent it follows `kind`
    /// (`music` → music, `stt` → transcription, everything else → TTS),
    /// so existing manifests need no change. Set it explicitly for a model
    /// whose backend kind serves a different task, such as a future
    /// voice-conversion engine.
    #[serde(default)]
    pub task: Option<TaskKind>,
    pub name: String,
    /// One-line blurb for model pickers. Built-in installs inherit the
    /// catalog text through the server; hand-written manifests may set it.
    #[serde(default)]
    pub description: Localized,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Terms the weights are distributed under, copied from the catalog
    /// by the installer. Absent for installs written before this field.
    #[serde(default)]
    pub license: Option<License>,
    #[serde(default)]
    pub zipvoice: Option<ZipVoiceManifest>,
    #[serde(default)]
    pub voxcpm2: Option<AudioCppManifest>,
    #[serde(default)]
    pub qwen3tts: Option<Qwen3TtsManifest>,
    #[serde(default)]
    pub longcat: Option<LongCatManifest>,
    #[serde(default)]
    pub dots: Option<AudioCppManifest>,
    #[serde(default)]
    pub cosyvoice: Option<AudioCppManifest>,
    #[serde(default)]
    pub kokoro: Option<KokoroManifest>,
    #[serde(default)]
    pub ming3tts: Option<Ming3TtsManifest>,
    #[serde(default)]
    pub chatterbox: Option<ChatterboxManifest>,
    #[serde(default)]
    pub indextts2: Option<AudioCppManifest>,
    #[serde(default)]
    pub mosstts: Option<MossTtsManifest>,
    /// Music-generation configuration when `kind == "music"`.
    #[serde(default)]
    pub music: Option<MusicManifest>,
    /// Generic audio.cpp configuration when `kind == "audio_cpp"`.
    #[serde(default)]
    pub audio_cpp: Option<AudioCppTaskManifest>,
    /// STT configuration when `kind == "stt"`.
    #[serde(default)]
    pub stt: Option<SttManifest>,
    #[serde(default)]
    pub defaults: SynthDefaults,
    /// User-tunable synthesis parameters shown by the UI. When absent,
    /// [`Manifest::default_parameters`] supplies the built-in set per kind.
    #[serde(default)]
    pub parameters: Vec<ParamSpec>,
    /// Parsed `model_spec.json`, filled in by the scanner for
    /// `kind: "audio_cpp"` installs. Not part of `engine.json`.
    #[serde(skip)]
    pub resolved_spec: Option<crate::modelspec::ModelSpec>,
}

impl Manifest {
    /// The task this model serves. Falls back to the backend kind so
    /// manifests written before the field existed keep working.
    pub fn task(&self) -> TaskKind {
        self.task.unwrap_or(match self.kind {
            EngineKind::Music => TaskKind::Music,
            EngineKind::Stt => TaskKind::Asr,
            // A generic audio.cpp install declares its task in the
            // section (`vc`, `svc`, `tts`, `clon`, `gen`, …).
            EngineKind::AudioCpp => self
                .audio_cpp
                .as_ref()
                .and_then(|cfg| TaskKind::parse(&cfg.task))
                .unwrap_or(TaskKind::Tts),
            _ => TaskKind::Tts,
        })
    }

    /// Load the `model_spec.json` of a generic audio.cpp install. A missing
    /// optional file is not an error: the manifest's own `parameters` list
    /// then drives the UI, and the engine still runs with no extra options.
    pub fn load_model_spec(&mut self, dir: &std::path::Path) {
        if self.kind != EngineKind::AudioCpp {
            return;
        }
        let Some(cfg) = &self.audio_cpp else {
            return;
        };
        if let Some(spec) = &cfg.model_spec {
            self.resolved_spec = Some(spec.clone());
            return;
        }
        let path = dir.join(cfg.spec.as_deref().unwrap_or("model_spec.json"));
        match std::fs::read_to_string(&path) {
            Ok(raw) => match crate::modelspec::ModelSpec::parse(&raw) {
                Ok(spec) => self.resolved_spec = Some(spec),
                Err(e) => tracing::warn!(spec = %path.display(), "invalid model spec: {e}"),
            },
            Err(e) => {
                // Only an explicitly named spec is worth a warning; the
                // default name is optional.
                if cfg.spec.is_some() {
                    tracing::warn!(spec = %path.display(), "cannot read model spec: {e}");
                }
            }
        }
    }

    /// Structural problems that make this install unusable, e.g. a
    /// VoxCPM2 directory written by the retired llama.cpp-omni runner.
    /// Cheap to compute (no model load), so the model manager can flag
    /// an installed directory as "needs reinstall" before the user hits
    /// a synthesis error.
    pub fn problem(&self) -> Option<Localized> {
        let legacy = || {
            Some(
                (
                    "This install is from a retired runner and must be reinstalled.",
                    "该安装来自旧的运行器，需要重新安装",
                )
                    .into(),
            )
        };
        let legacy_vox = || {
            Some(
                (
                    "This install is from the retired llama.cpp runner; reinstall it.",
                    "该安装来自旧的 llama.cpp 运行器，需要重新安装",
                )
                    .into(),
            )
        };
        let missing = |section: &str| Some(Localized::Text(format!("missing [{section}] section")));
        let empty_audio_cpp =
            |cfg: &AudioCppManifest| cfg.binary.trim().is_empty() && cfg.model.trim().is_empty();
        let structural = match self.kind {
            EngineKind::Dots => match &self.dots {
                None => missing("dots"),
                Some(c) if empty_audio_cpp(c) => legacy(),
                Some(_) => None,
            },
            EngineKind::CosyVoice => match &self.cosyvoice {
                None => missing("cosyvoice"),
                Some(c) if empty_audio_cpp(c) => legacy(),
                Some(_) => None,
            },
            EngineKind::VoxCpm2 => match &self.voxcpm2 {
                None => missing("voxcpm2"),
                Some(c) if empty_audio_cpp(c) => legacy_vox(),
                Some(_) => None,
            },
            EngineKind::ZipVoice => self
                .zipvoice
                .is_none()
                .then(|| missing("zipvoice"))
                .flatten(),
            EngineKind::Qwen3Tts => self
                .qwen3tts
                .is_none()
                .then(|| missing("qwen3tts"))
                .flatten(),
            EngineKind::LongCat => self.longcat.is_none().then(|| missing("longcat")).flatten(),
            EngineKind::Kokoro => self.kokoro.is_none().then(|| missing("kokoro")).flatten(),
            EngineKind::Ming3Tts => self
                .ming3tts
                .is_none()
                .then(|| missing("ming3tts"))
                .flatten(),
            EngineKind::Chatterbox => self
                .chatterbox
                .is_none()
                .then(|| missing("chatterbox"))
                .flatten(),
            EngineKind::IndexTts2 => self
                .indextts2
                .is_none()
                .then(|| missing("indextts2"))
                .flatten(),
            EngineKind::MossTts => self.mosstts.is_none().then(|| missing("mosstts")).flatten(),
            EngineKind::Music => match &self.music {
                None => missing("music"),
                Some(c) if c.binary.trim().is_empty() && c.model.trim().is_empty() => {
                    Some(Localized::Text("missing [music] binary/model".to_string()))
                }
                Some(_) => None,
            },
            EngineKind::AudioCpp => match &self.audio_cpp {
                None => missing("audio_cpp"),
                Some(c) if c.binary.trim().is_empty() && c.model.trim().is_empty() => Some(
                    Localized::Text("missing [audio_cpp] binary/model".to_string()),
                ),
                Some(c) if c.family.trim().is_empty() || c.task.trim().is_empty() => Some(
                    Localized::Text("missing [audio_cpp] family/task".to_string()),
                ),
                Some(_) => None,
            },
            EngineKind::Stt => self.stt.is_none().then(|| missing("stt")).flatten(),
        };
        if structural.is_some() {
            return structural;
        }
        // A manifest written before the bilingual pass whose labels we cannot
        // translate is not broken, but its English UI would show Chinese.
        if crate::legacy_i18n::any_untranslated(self.kind, &self.parameters) {
            return Some(
                (
                    "Some parameter labels have no English text; reinstall from the catalog to get bilingual labels.",
                    "部分参数标签没有英文，从模型目录重新安装即可获得双语标签",
                )
                    .into(),
            );
        }
        None
    }

    /// Declared parameters, or the built-in set for this engine kind.
    ///
    /// Declared parameters from an install predating the bilingual pass get
    /// their English text filled in on the way out; see `legacy_i18n`.
    pub fn parameters_or_defaults(&self) -> Vec<ParamSpec> {
        if self.parameters.is_empty() {
            // A generic audio.cpp install describes its controls in
            // `model_spec.json`; they become ordinary manifest parameters.
            if let Some(spec) = &self.resolved_spec {
                let params = spec.parameters();
                if !params.is_empty() {
                    return params;
                }
            }
            self.kind_default_parameters()
        } else {
            let mut params = self.parameters.clone();
            // New built-in controls must reach installs written before they
            // existed; the manifest's own entries win by name.
            for spec in self.kind_default_parameters() {
                if !params.iter().any(|p| p.name == spec.name) {
                    params.push(spec);
                }
            }
            crate::legacy_i18n::localize(self.kind, &mut params);
            params
        }
    }

    /// Built-in controls for this manifest's kind and, for music, family.
    ///
    /// ACE-Step's editing routes and Stable Audio's conditioning controls
    /// are different enough that one music list cannot describe both.
    fn kind_default_parameters(&self) -> Vec<ParamSpec> {
        match self.music.as_ref().map(|m| m.family.as_str()) {
            Some("stable_audio") if self.kind == EngineKind::Music => {
                Self::stable_audio_parameters()
            }
            _ => Self::default_parameters(self.kind),
        }
    }

    /// Stable Audio 3 controls: text-to-music and sound effects, plus the
    /// audio-conditioned init/inpaint modes.
    fn stable_audio_parameters() -> Vec<ParamSpec> {
        use serde_json::json;
        vec![
            ParamSpec::slider(
                "duration_sec",
                ("Duration (seconds)", "时长（秒）"),
                30.0,
                1.0,
                300.0,
                1.0,
                (
                    "Requested clip length; longer clips take proportionally longer.",
                    "期望的音频长度；越长耗时越久。",
                ),
            ),
            ParamSpec::int_slider(
                "num_steps",
                ("Diffusion steps", "扩散步数"),
                8,
                1,
                50,
                1,
                (
                    "8 is the model default; more steps are slower.",
                    "8 是模型默认值；越多越慢。",
                ),
            ),
            ParamSpec::slider(
                "guidance_scale",
                ("Guidance strength", "引导强度"),
                1.0,
                0.0,
                10.0,
                0.1,
                (
                    "Classifier-free guidance; 1 keeps the model default.",
                    "无分类器引导；1 表示使用模型默认值。",
                ),
            ),
            ParamSpec {
                name: "negative_prompt".into(),
                label: ("Negative prompt", "反向提示").into(),
                kind: ParamKind::Text,
                min: None,
                max: None,
                step: None,
                options: Vec::new(),
                default: json!(""),
                hint: (
                    "What to avoid; leave empty to use the model default.",
                    "希望避免的内容；留空使用模型默认值。",
                )
                    .into(),
            },
            ParamSpec {
                name: "seed".into(),
                label: ("Random seed", "随机种子").into(),
                kind: ParamKind::Int,
                min: Some(json!(0)),
                max: Some(json!(4_294_967_295i64)),
                step: None,
                options: Vec::new(),
                default: json!(42),
                hint: (
                    "Same seed + prompt gives a reproducible clip.",
                    "相同种子 + 提示词得到可复现的音频。",
                )
                    .into(),
            },
            ParamSpec {
                name: "audio_input_kind".into(),
                label: ("Source audio use", "源音频用法").into(),
                kind: ParamKind::Choice,
                min: None,
                max: None,
                step: None,
                options: vec![
                    ParamOption {
                        label: ("None", "不使用").into(),
                        value: "none".into(),
                    },
                    ParamOption {
                        label: ("Init audio", "初始音频").into(),
                        value: "init_audio".into(),
                    },
                    ParamOption {
                        label: ("Inpaint", "局部重绘").into(),
                        value: "inpaint_audio".into(),
                    },
                ],
                default: json!("none"),
                hint: (
                    "Condition generation on an uploaded song, or inpaint masked regions.",
                    "用上传的歌曲作为生成条件，或对掩码区间做局部重绘。",
                )
                    .into(),
            },
            ParamSpec::slider(
                "init_noise_level",
                ("Init noise", "初始噪声"),
                0.45,
                0.0,
                1.0,
                0.05,
                (
                    "How far audio-conditioned generation may move from the source.",
                    "音频条件生成允许偏离源音频的程度。",
                ),
            ),
            ParamSpec {
                name: "inpaint_mask_start_sec".into(),
                label: ("Inpaint starts (s)", "重绘起点（秒）").into(),
                kind: ParamKind::Text,
                min: None,
                max: None,
                step: None,
                options: Vec::new(),
                default: json!(""),
                hint: (
                    "Comma-separated start times, e.g. 2.5,6.5.",
                    "逗号分隔的起点，例如 2.5,6.5。",
                )
                    .into(),
            },
            ParamSpec {
                name: "inpaint_mask_end_sec".into(),
                label: ("Inpaint ends (s)", "重绘终点（秒）").into(),
                kind: ParamKind::Text,
                min: None,
                max: None,
                step: None,
                options: Vec::new(),
                default: json!(""),
                hint: (
                    "Comma-separated end times, e.g. 3.5,8.0.",
                    "逗号分隔的终点，例如 3.5,8.0。",
                )
                    .into(),
            },
        ]
    }

    /// Longest reference clip this engine will use, in seconds.
    ///
    /// A voice can be built from several takes; joining them stops here,
    /// because that is the part of the reference the engine actually
    /// conditions on. The numbers come from each engine's own limit:
    /// VoxCPM2's encoder input is a fixed 15 s tensor (see
    /// [`crate::VOXCPM2_MAX_REFERENCE_SECS`]), and LongCat budgets the
    /// reference inside a ~30 s request. Everything else gets the common
    /// "3–10 s works best, more is ignored" ceiling with room to spare.
    pub fn reference_limit_secs(&self) -> f32 {
        match self.kind {
            EngineKind::VoxCpm2 => crate::VOXCPM2_MAX_REFERENCE_SECS,
            EngineKind::LongCat => 10.0,
            EngineKind::Music | EngineKind::Stt => 0.0,
            _ => 15.0,
        }
    }

    /// Parameters a manifest ships unless it declares its own list.
    pub fn default_parameters(kind: EngineKind) -> Vec<ParamSpec> {
        use serde_json::json;
        match kind {
            // STT is not a synthesis engine; it has no per-request params.
            EngineKind::Stt => Vec::new(),
            // Generic audio.cpp installs declare their controls in the
            // model spec (or the manifest's own `parameters` list).
            EngineKind::AudioCpp => Vec::new(),
            EngineKind::ZipVoice => vec![
                ParamSpec::slider(
                    "speed",
                    ("Speed", "语速"),
                    1.0,
                    0.5,
                    2.0,
                    0.1,
                    (
                        "Higher is faster; the voice itself is unchanged.",
                        "越大语速越快，不改变音色",
                    ),
                ),
                ParamSpec::int_slider(
                    "num_steps",
                    ("Diffusion steps", "扩散步数"),
                    4,
                    1,
                    16,
                    1,
                    ("More steps are steadier but slower.", "越多越稳但越慢"),
                ),
            ],
            EngineKind::VoxCpm2 => vec![
                ParamSpec::slider(
                    "speed",
                    ("Speed", "语速"),
                    1.0,
                    0.5,
                    2.0,
                    0.1,
                    (
                        "Higher is faster; the voice itself is unchanged.",
                        "越大语速越快，不改变音色",
                    ),
                ),
                ParamSpec::int_slider(
                    "num_steps",
                    ("Diffusion steps", "扩散步数"),
                    10,
                    1,
                    30,
                    1,
                    ("More steps are steadier but slower.", "越多越稳但越慢"),
                ),
                ParamSpec::slider(
                    "guidance",
                    ("Guidance strength (CFG)", "引导强度 (CFG)"),
                    2.0,
                    0.0,
                    5.0,
                    0.1,
                    (
                        "Too high distorts; too low drifts from the reference.",
                        "过高会失真，过低音色偏离参考",
                    ),
                ),
                ParamSpec {
                    name: "seed".into(),
                    label: ("Random seed", "随机种子").into(),
                    kind: ParamKind::Int,
                    min: Some(json!(-1)),
                    max: Some(json!(2_147_483_647)),
                    step: None,
                    options: Vec::new(),
                    default: json!(42),
                    hint: (
                        "Same seed + text gives reproducible speech; -1 randomizes each run.",
                        "相同种子 + 文本得到可复现的声音；-1 每次随机",
                    )
                        .into(),
                },
            ],
            // Non-diffusion autoregressive models expose no step control.
            EngineKind::Qwen3Tts => Vec::new(),
            // Every mode ships its own parameter lists via the manifest.
            EngineKind::Ming3Tts => Vec::new(),
            EngineKind::Chatterbox => Vec::new(),
            // Autoregressive GPT + flow-matching vocoder; text is split
            // into segments internally, and emotion guidance needs an
            // emotion reference clip we do not collect. The language,
            // decoding and segment controls below are passed straight
            // through to audio.cpp.
            EngineKind::IndexTts2 => vec![
                ParamSpec {
                    name: "language".into(),
                    label: ("Text language", "文本语言").into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: ("Detect from text", "按文本自动判断").into(),
                            value: "auto".into(),
                        },
                        ParamOption {
                            label: ("Chinese 中文", "中文 Chinese").into(),
                            value: "zh".into(),
                        },
                        ParamOption {
                            label: ("English 英语", "英语 English").into(),
                            value: "en".into(),
                        },
                    ],
                    default: serde_json::json!("auto"),
                    hint: (
                        "Guides the text front-end; automatic picks Chinese for Han text and English otherwise.",
                        "引导文本前端；自动时含汉字按中文读，否则按英文读。",
                    )
                        .into(),
                },
                ParamSpec {
                    name: "num_beams".into(),
                    label: ("Decoding", "解码方式").into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: ("Beam search", "束搜索（质量优先）").into(),
                            value: "3".into(),
                        },
                        ParamOption {
                            label: ("Greedy, about 25% faster", "贪心（快约 25%）").into(),
                            value: "1".into(),
                        },
                    ],
                    default: serde_json::json!("3"),
                    hint: (
                        "Beam search reads slightly better; greedy skips the extra beams.",
                        "束搜索韵律更稳；贪心省掉多余搜索，明显更快。",
                    )
                        .into(),
                },
                // The model's own pause at a full stop is shorter than at a
                // comma inside one segment. audio.cpp inserts silence only
                // between segments, so these two knobs are how a long take
                // gets audible sentence breaks without re-running the model.
                ParamSpec::int_slider(
                    "text_chunk_size",
                    ("Chunk size (characters)", "分段长度（字）"),
                    600,
                    40,
                    600,
                    10,
                    (
                        "Lower splits long text into more segments: 40 is roughly one sentence per segment, with clearer full stops but 10–20% slower synthesis.",
                        "越小分段越多：约 40 字接近每句一段，句号更清楚，但合成慢 10–20%。",
                    ),
                ),
                ParamSpec::int_slider(
                    "interval_silence_ms",
                    ("Pause between segments (ms)", "分段间停顿（毫秒）"),
                    400,
                    0,
                    2000,
                    50,
                    (
                        "Silence audio.cpp inserts between segments; the model's own pause inside a segment is not affected.",
                        "audio.cpp 在分段之间插入的静音；段内句号停顿不受此影响。",
                    ),
                ),
            ],
            // MOSS-TTS ships a language choice via the manifest.
            EngineKind::MossTts => Vec::new(),
            // Flow-matching ODE: NFE steps, guidance strength/method, seed.
            EngineKind::LongCat => vec![
                ParamSpec::int_slider(
                    "num_steps",
                    ("Sampling steps (NFE)", "采样步数 (NFE)"),
                    16,
                    1,
                    32,
                    1,
                    (
                        "Diffusion ODE steps; 16 is enough, more is slower.",
                        "扩散 ODE 步数；16 已足够，越多越慢",
                    ),
                ),
                ParamSpec::slider(
                    "cfg",
                    ("Guidance strength", "引导强度"),
                    4.0,
                    0.0,
                    10.0,
                    0.1,
                    (
                        "Higher follows the reference voice more closely; too high may distort.",
                        "越高越贴近参考音色，过高可能失真",
                    ),
                ),
                ParamSpec {
                    name: "guidance_method".into(),
                    label: ("Guidance method", "引导方式").into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: (
                                "APG (adaptive projection, recommended)",
                                "APG（自适应投影，推荐）",
                            )
                                .into(),
                            value: "apg".into(),
                        },
                        ParamOption {
                            label: (
                                "CFG (classic classifier-free guidance)",
                                "CFG（传统无分类器引导）",
                            )
                                .into(),
                            value: "cfg".into(),
                        },
                    ],
                    default: serde_json::json!("apg"),
                    hint: (
                        "APG is usually more natural with fewer artifacts.",
                        "APG 通常更自然、伪影更少",
                    )
                        .into(),
                },
                ParamSpec {
                    name: "seed".into(),
                    label: ("Random seed", "随机种子").into(),
                    kind: ParamKind::Int,
                    min: Some(serde_json::json!(0)),
                    max: Some(serde_json::json!(2_147_483_647)),
                    step: None,
                    options: Vec::new(),
                    default: serde_json::json!(1024),
                    hint: (
                        "Same seed + text gives reproducible speech.",
                        "相同种子 + 文本得到可复现的声音",
                    )
                        .into(),
                },
            ],
            // Continuous-AR flow matching; 48 kHz output.
            EngineKind::Dots => vec![
                ParamSpec::int_slider(
                    "num_steps",
                    ("Sampling steps", "采样步数"),
                    10,
                    1,
                    32,
                    1,
                    (
                        "SOAR recommends 10 steps; more is slower.",
                        "SOAR 推荐 10 步；变多会更慢",
                    ),
                ),
                ParamSpec::slider(
                    "guidance",
                    ("Guidance strength", "引导强度"),
                    1.2,
                    0.0,
                    3.0,
                    0.1,
                    (
                        "Higher follows the reference voice more closely; too high may distort.",
                        "越高越贴近参考音色，过高可能失真",
                    ),
                ),
                ParamSpec {
                    name: "clone_mode".into(),
                    label: ("Clone mode", "克隆方式").into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: (
                                "Voice only (sound alone, recommended)",
                                "仅声纹（只凭声音，推荐）",
                            )
                                .into(),
                            value: "xvector".into(),
                        },
                        ParamOption {
                            label: (
                                "Voice + transcript (more alike with an accurate transcript)",
                                "声纹 + 文字（文字稿准确时更相似）",
                            )
                                .into(),
                            value: "continuation".into(),
                        },
                    ],
                    default: serde_json::json!("xvector"),
                    hint: (
                        "Use voice-only when the transcript may be inaccurate.",
                        "文字稿不准确时选「仅声纹」更稳",
                    )
                        .into(),
                },
                ParamSpec {
                    name: "language".into(),
                    label: ("Language tag", "语言标签").into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: ("Auto", "自动").into(),
                            value: "".into(),
                        },
                        ParamOption {
                            label: ("Chinese", "中文").into(),
                            value: "zh".into(),
                        },
                        ParamOption {
                            label: "English".into(),
                            value: "en".into(),
                        },
                        ParamOption {
                            label: ("Japanese", "日本語").into(),
                            value: "ja".into(),
                        },
                        ParamOption {
                            label: ("Korean", "한국어").into(),
                            value: "ko".into(),
                        },
                        ParamOption {
                            label: ("French", "Français").into(),
                            value: "fr".into(),
                        },
                        ParamOption {
                            label: ("German", "Deutsch").into(),
                            value: "de".into(),
                        },
                        ParamOption {
                            label: ("Spanish", "Español").into(),
                            value: "es".into(),
                        },
                    ],
                    default: serde_json::json!(""),
                    hint: (
                        "A fixed tag helps with mixed-language text.",
                        "多语言或混合语言时固定标签有帮助",
                    )
                        .into(),
                },
                ParamSpec {
                    name: "seed".into(),
                    label: ("Random seed", "随机种子").into(),
                    kind: ParamKind::Int,
                    min: Some(serde_json::json!(0)),
                    max: Some(serde_json::json!(2_147_483_647)),
                    step: None,
                    options: Vec::new(),
                    default: serde_json::json!(42),
                    hint: (
                        "Same seed + text gives reproducible speech.",
                        "相同种子 + 文本得到可复现的声音",
                    )
                        .into(),
                },
            ],
            // CosyVoice3: streaming LLM TTS with three request templates.
            EngineKind::CosyVoice => vec![
                ParamSpec {
                    name: "template".into(),
                    label: ("Synthesis template", "合成模板").into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: (
                                "Zero-shot clone (needs transcript, closest match)",
                                "零样本克隆（需文字稿，相似度最高）",
                            )
                                .into(),
                            value: "zero_shot".into(),
                        },
                        ParamOption {
                            label: ("Cross-lingual clone (voice only)", "跨语种克隆（只凭声音）")
                                .into(),
                            value: "cross_lingual".into(),
                        },
                        ParamOption {
                            label: (
                                "Instruct (describe the tone in words)",
                                "指令（用文字描述语气）",
                            )
                                .into(),
                            value: "instruct".into(),
                        },
                    ],
                    default: serde_json::json!("zero_shot"),
                    hint: (
                        "Zero-shot is the closest; use cross-lingual with no transcript.",
                        "零样本最像；文字稿不准时选跨语种",
                    )
                        .into(),
                },
                ParamSpec {
                    name: "instruct".into(),
                    label: (
                        "Instruction (used only by the Instruct template)",
                        "指令（仅「指令」模板使用）",
                    )
                        .into(),
                    kind: ParamKind::TextArea,
                    min: None,
                    max: None,
                    step: None,
                    options: Vec::new(),
                    default: serde_json::json!(""),
                    hint: (
                        "e.g. Read in a gentle, warm tone at a slightly slower pace.",
                        "例如：用温和亲切的语气，语速稍慢",
                    )
                        .into(),
                },
                ParamSpec::int_slider(
                    "num_steps",
                    ("Streaming decode steps", "流式解码步数"),
                    10,
                    1,
                    32,
                    1,
                    (
                        "Euler steps for the acoustic flow matching; more is steadier and slower.",
                        "声学流匹配的 Euler 步数；越多越稳越慢",
                    ),
                ),
                ParamSpec {
                    name: "top_k".into(),
                    label: ("Top-K sampling", "Top-K 采样").into(),
                    kind: ParamKind::Int,
                    min: Some(serde_json::json!(1)),
                    max: Some(serde_json::json!(100)),
                    step: None,
                    options: Vec::new(),
                    default: serde_json::json!(25),
                    hint: (
                        "Lower is steadier; higher adds variation.",
                        "越小越稳定，越大越有变化",
                    )
                        .into(),
                },
                ParamSpec {
                    name: "seed".into(),
                    label: ("Random seed", "随机种子").into(),
                    kind: ParamKind::Int,
                    min: Some(serde_json::json!(0)),
                    max: Some(serde_json::json!(2_147_483_647)),
                    step: None,
                    options: Vec::new(),
                    default: serde_json::json!(1986),
                    hint: (
                        "Same seed + text gives reproducible speech.",
                        "相同种子 + 文本得到可复现的声音",
                    )
                        .into(),
                },
            ],
            // Music: the caption is the request's `text`; these are the
            // per-request controls audio.cpp's `gen` task understands.
            EngineKind::Music => vec![
                ParamSpec {
                    name: "lyrics".into(),
                    label: ("Lyrics", "歌词").into(),
                    kind: ParamKind::TextArea,
                    min: None,
                    max: None,
                    step: None,
                    options: Vec::new(),
                    default: json!(""),
                    hint: (
                        "Leave empty for an instrumental; section tags like [verse] and [chorus] are understood.",
                        "留空生成纯音乐；支持 [verse]、[chorus] 等段落标记。",
                    )
                        .into(),
                },
                ParamSpec::slider(
                    "duration_sec",
                    ("Duration (seconds)", "时长（秒）"),
                    60.0,
                    10.0,
                    600.0,
                    1.0,
                    (
                        "Requested song length; the planner may stop earlier.",
                        "期望的歌曲长度；规划器可能提前结束。",
                    ),
                ),
                ParamSpec::int_slider(
                    "num_steps",
                    ("Diffusion steps", "扩散步数"),
                    8,
                    1,
                    32,
                    1,
                    (
                        "ACE-Step Turbo uses 8 steps; more is slower.",
                        "ACE-Step Turbo 使用 8 步；越多越慢。",
                    ),
                ),
                ParamSpec::slider(
                    "guidance_scale",
                    ("Guidance strength", "引导强度"),
                    1.0,
                    0.0,
                    10.0,
                    0.1,
                    (
                        "Turbo checkpoints ignore this; base/SFT follow it.",
                        "Turbo 检查点会忽略此值；base/SFT 会使用。",
                    ),
                ),
                ParamSpec {
                    name: "seed".into(),
                    label: ("Random seed", "随机种子").into(),
                    kind: ParamKind::Int,
                    min: Some(json!(0)),
                    max: Some(json!(4_294_967_295i64)),
                    step: None,
                    options: Vec::new(),
                    default: json!(42),
                    hint: (
                        "Same seed + prompt gives a reproducible song.",
                        "相同种子 + 提示词得到可复现的歌曲。",
                    )
                        .into(),
                },
                ParamSpec {
                    name: "task_route".into(),
                    label: ("Task route", "任务路由").into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: ("Text to music", "文本生成音乐").into(),
                            value: "text2music".into(),
                        },
                        ParamOption {
                            label: ("Cover", "翻唱").into(),
                            value: "cover".into(),
                        },
                        ParamOption {
                            label: ("Cover (no FSQ)", "翻唱（无 FSQ）").into(),
                            value: "cover-nofsq".into(),
                        },
                        ParamOption {
                            label: ("Repaint", "局部重绘").into(),
                            value: "repaint".into(),
                        },
                        ParamOption {
                            label: ("Complete", "补全").into(),
                            value: "complete".into(),
                        },                        ParamOption {
                            label: ("Lego (track)", "分轨生成").into(),
                            value: "lego".into(),
                        },
                        ParamOption {
                            label: ("Extract (track)", "分轨提取").into(),
                            value: "extract".into(),
                        },
                    ],
                    default: json!("text2music"),
                    hint: (
                        "Every route except text2music edits a source song and needs an upload.",
                        "除文本生成音乐外，其余路由都会编辑源音乐，需要先上传。",
                    )
                        .into(),
                },
                ParamSpec {
                    name: "negative_prompt".into(),
                    label: ("Negative prompt", "反向提示").into(),
                    kind: ParamKind::Text,
                    min: None,
                    max: None,
                    step: None,
                    options: Vec::new(),
                    default: json!(""),
                    hint: (
                        "What to avoid; leave empty to use the model default.",
                        "希望避免的内容；留空使用模型默认值。",
                    )
                        .into(),
                },
                ParamSpec {
                    name: "track_name".into(),
                    label: ("Track", "音轨").into(),
                    kind: ParamKind::Text,
                    min: None,
                    max: None,
                    step: None,
                    options: Vec::new(),
                    default: json!(""),
                    hint: (
                        "Target track for the lego and extract routes, e.g. drums or bass.",
                        "分轨生成 / 提取的目标音轨，例如 drums 或 bass。",
                    )
                        .into(),
                },
                ParamSpec::slider(
                    "repainting_start",
                    ("Repaint start (s)", "重绘起点（秒）"),
                    0.0,
                    0.0,
                    600.0,
                    0.1,
                    (
                        "Start of the region to repaint, in source seconds.",
                        "要重绘的区间起点，按源音频秒数计。",
                    ),
                ),
                ParamSpec::slider(
                    "repainting_end",
                    ("Repaint end (s)", "重绘终点（秒）"),
                    10.0,
                    0.0,
                    600.0,
                    0.1,
                    (
                        "End of the region to repaint, in source seconds.",
                        "要重绘的区间终点，按源音频秒数计。",
                    ),
                ),
                ParamSpec {
                    name: "repaint_mode".into(),
                    label: ("Repaint mode", "重绘模式").into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: ("Balanced", "均衡").into(),
                            value: "balanced".into(),
                        },
                        ParamOption {
                            label: ("Conservative", "保守").into(),
                            value: "conservative".into(),
                        },
                        ParamOption {
                            label: ("Aggressive", "激进").into(),
                            value: "aggressive".into(),
                        },
                    ],
                    default: json!("balanced"),
                    hint: (
                        "How far the repainted region may drift from the source.",
                        "重绘区间允许偏离源音频的程度。",
                    )
                        .into(),
                },
                ParamSpec::slider(
                    "repaint_strength",
                    ("Repaint strength", "重绘强度"),
                    0.5,
                    0.0,
                    1.0,
                    0.05,
                    (
                        "Balanced-mode blend strength; lower stays closer to the source.",
                        "均衡模式下的混合强度；越低越接近源音频。",
                    ),
                ),
                ParamSpec::slider(
                    "audio_cover_strength",
                    ("Cover strength", "翻唱强度"),
                    1.0,
                    0.0,
                    1.0,
                    0.05,
                    (
                        "How strongly cover conditioning follows the source song.",
                        "翻唱条件跟随源音乐的程度。",
                    ),
                ),
                ParamSpec::slider(
                    "cover_noise_strength",
                    ("Cover noise", "翻唱噪声"),
                    0.0,
                    0.0,
                    1.0,
                    0.05,
                    (
                        "Extra noise mixed into cover conditioning.",
                        "混入翻唱条件的额外噪声。",
                    ),
                ),
            ],
            // Preset-voices model: speed is the only built-in control;
            // the speaker (`voice`) choice is model-specific, so manifests
            // declare their own parameter list with every sid they ship.
            EngineKind::Kokoro => vec![ParamSpec::slider(
                "speed",
                ("Speed", "语速"),
                1.0,
                0.5,
                2.0,
                0.1,
                (
                    "Higher is faster; the voice itself is unchanged.",
                    "越大语速越快，不改变音色",
                ),
            )],
        }
    }
}

/// A user-tunable synthesis parameter declared by an engine manifest.
///
/// The list drives both the generated UI (control type, range, hint) and
/// request validation; unknown names are ignored by the engines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSpec {
    /// Form field / engine option name, e.g. `num_steps` or `cfg`.
    pub name: String,
    /// UI label. Manifests we generate carry both languages; a plain
    /// string (third-party manifests) is shown as-is.
    pub label: Localized,
    #[serde(rename = "type")]
    pub kind: ParamKind,
    #[serde(default)]
    pub min: Option<serde_json::Value>,
    #[serde(default)]
    pub max: Option<serde_json::Value>,
    #[serde(default)]
    pub step: Option<serde_json::Value>,
    /// Radio/select options for `choice`; `[label, value]` pairs.
    #[serde(default)]
    pub options: Vec<ParamOption>,
    pub default: serde_json::Value,
    #[serde(default)]
    pub hint: Localized,
}

/// Display text supplied by a manifest or catalog.
///
/// Engine-provided identifiers (Kokoro's `af_heart`, Qwen's `Vivian`)
/// are data and are shown as-is; anything *we* author — descriptions,
/// hints, parameter labels — must be translatable, so it is stored as
/// `{"en": …, "zh": …}` and resolved by the UI. A bare string keeps
/// hand-written third-party manifests working.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Localized {
    Text(String),
    Translations {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        en: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zh: Option<String>,
    },
}

impl Localized {
    /// Text for `lang` ("en" or "zh"), falling back to the other
    /// language and finally to an empty string.
    pub fn for_lang(&self, lang: &str) -> &str {
        match self {
            Localized::Text(text) => text,
            Localized::Translations { en, zh } => {
                // Unknown languages fall back to English rather than Chinese.
                let (first, second) = if lang == "zh" { (zh, en) } else { (en, zh) };
                first.as_deref().or(second.as_deref()).unwrap_or("")
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.for_lang("en").is_empty() && self.for_lang("zh").is_empty()
    }

    /// Whether any language's text equals `value` (used to match a
    /// requested voice against an option's label).
    pub fn matches(&self, value: &str) -> bool {
        match self {
            Localized::Text(text) => text == value,
            Localized::Translations { en, zh } => {
                en.as_deref() == Some(value) || zh.as_deref() == Some(value)
            }
        }
    }
}

impl From<&str> for Localized {
    fn from(value: &str) -> Self {
        Localized::Text(value.to_string())
    }
}

impl From<(&str, &str)> for Localized {
    /// `(English, Chinese)` shorthand used by the built-in parameter sets.
    fn from((en, zh): (&str, &str)) -> Self {
        Localized::Translations {
            en: Some(en.to_string()),
            zh: Some(zh.to_string()),
        }
    }
}

impl From<String> for Localized {
    fn from(value: String) -> Self {
        Localized::Text(value)
    }
}

impl Default for Localized {
    fn default() -> Self {
        Localized::Text(String::new())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    Float,
    Int,
    Bool,
    Choice,
    /// Free-form single-line text.
    Text,
    /// Free-form multi-line text.
    TextArea,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamOption {
    pub label: Localized,
    pub value: String,
}

impl ParamSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        label: impl Into<Localized>,
        kind: ParamKind,
        default: serde_json::Value,
        hint: impl Into<Localized>,
    ) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            kind,
            min: None,
            max: None,
            step: None,
            options: Vec::new(),
            default,
            hint: hint.into(),
        }
    }

    pub fn slider(
        name: &str,
        label: impl Into<Localized>,
        default: f64,
        min: f64,
        max: f64,
        step: f64,
        hint: impl Into<Localized>,
    ) -> Self {
        use serde_json::json;
        Self {
            min: Some(json!(min)),
            max: Some(json!(max)),
            step: Some(json!(step)),
            ..Self::new(name, label, ParamKind::Float, json!(default), hint)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn int_slider(
        name: &str,
        label: impl Into<Localized>,
        default: i64,
        min: i64,
        max: i64,
        step: i64,
        hint: impl Into<Localized>,
    ) -> Self {
        use serde_json::json;
        Self {
            min: Some(json!(min)),
            max: Some(json!(max)),
            step: Some(json!(step)),
            ..Self::new(name, label, ParamKind::Int, json!(default), hint)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthDefaults {
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default = "default_steps")]
    pub num_steps: i32,
}

fn default_speed() -> f32 {
    1.0
}

fn default_steps() -> i32 {
    4
}

impl Default for SynthDefaults {
    fn default() -> Self {
        Self {
            speed: default_speed(),
            num_steps: default_steps(),
        }
    }
}

/// File layout and tuning constants for a ZipVoice model.
///
/// All file paths are relative to the model directory.
/// `lexicon` accepts several comma-separated files, as sherpa-onnx does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZipVoiceManifest {
    pub tokens: String,
    pub encoder: String,
    pub decoder: String,
    pub vocoder: String,
    /// espeak-ng data directory (required by the lexicon frontend).
    #[serde(default)]
    pub data_dir: Option<String>,
    #[serde(default)]
    pub lexicon: Option<String>,
    #[serde(default = "default_feat_scale")]
    pub feat_scale: f32,
    #[serde(default = "default_t_shift")]
    pub t_shift: f32,
    #[serde(default = "default_target_rms")]
    pub target_rms: f32,
    #[serde(default = "default_guidance")]
    pub guidance_scale: f32,
    #[serde(default = "default_threads")]
    pub num_threads: i32,
}

fn default_feat_scale() -> f32 {
    0.1
}
fn default_t_shift() -> f32 {
    0.5
}
fn default_target_rms() -> f32 {
    0.1
}
fn default_guidance() -> f32 {
    1.0
}
fn default_threads() -> i32 {
    2
}

/// Qwen3-TTS runs through the official `qwen-tts` Python package in its
/// own isolated environment; the core only invokes a small runner script.
///
/// `model` may be a Hugging Face repo id or a path relative to the model
/// directory. `mode` selects the model family:
///   - `clone`          Base model, needs a reference clip (transcript optional)
///   - `custom_voice`   preset speakers + optional instruction
///   - `voice_design`   a voice described in natural language
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Qwen3TtsManifest {
    #[serde(default = "default_python")]
    pub python: String,
    pub script: String,
    pub model: String,
    #[serde(default = "default_qwen_mode")]
    pub mode: String,
    #[serde(default)]
    pub speaker: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub instruct: Option<String>,
    #[serde(default = "default_device")]
    pub device: String,
    #[serde(default = "default_dtype")]
    pub dtype: String,
    #[serde(default = "default_attn")]
    pub attn: String,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

/// Ming-3.0-tts: an isolated Python runner around the single 0.5B dense
/// checkpoint, which handles every task (zero-shot cloning, voice design,
/// multilingual, built-in IP voices, free-form control). The `mode` field
/// selects which task the manifest exposes; all modes share one model dir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ming3TtsManifest {
    #[serde(default = "default_python")]
    pub python: String,
    pub script: String,
    pub model: String,
    /// Directory holding the repo's Python sources (ming_tts.py + the
    /// tts_tasks/frontend/modeling_bailingmm modules it imports).
    #[serde(default)]
    pub runtime: String,
    #[serde(default = "default_ming_mode")]
    pub mode: String,
    /// Multilingual task language (`Chinese`, `English`, `German`, ...).
    #[serde(default)]
    pub language: Option<String>,
    /// Built-in IP voice name for `mode == "ip"` and -design style fallback.
    #[serde(default)]
    pub instruct: Option<String>,
    #[serde(default)]
    pub speaker: Option<String>,
    #[serde(default = "default_device")]
    pub device: String,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

fn default_ming_mode() -> String {
    "clone".to_string()
}

/// Chatterbox (Resemble AI): a 0.5B multilingual TTS that clones a voice
/// from a reference clip, with a per-request output language. Runs through
/// the official `chatterbox-tts` Python package in its own venv.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatterboxManifest {
    #[serde(default = "default_python")]
    pub python: String,
    pub script: String,
    pub model: String,
    /// Multilingual T3 checkpoint filename; `v3` is the latest and default.
    #[serde(default = "default_chatterbox_t3")]
    pub t3: String,
    #[serde(default = "default_device")]
    pub device: String,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

fn default_chatterbox_t3() -> String {
    "v3".to_string()
}

/// MOSS-TTS-Delay (OpenMOSS) run by the native `llama-moss-tts` binary.
///
/// The installer builds that binary from the OpenMOSS llama.cpp fork and
/// converts the audio tokenizer to GGUF, so synthesis needs no Python at
/// run time: one executable drives the Qwen3 backbone plus the audio
/// encoder/decoder GGUFs. A reference clip is optional (text-only TTS
/// works too); when present it must be 24 kHz mono, which the engine
/// guarantees before invoking the binary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MossTtsManifest {
    /// `llama-moss-tts` path, absolute or relative to the model directory.
    pub binary: String,
    /// First-class MOSS-TTS-Delay backbone GGUF.
    pub backbone: String,
    /// Native audio-tokenizer encoder GGUF.
    pub audio_encoder: String,
    /// Native audio-tokenizer decoder GGUF.
    pub audio_decoder: String,
    /// Default language tag passed to the prompt builder (`zh`, `en`, …).
    #[serde(default = "default_mosstts_language")]
    pub language: String,
    /// GPU layers to offload; `-1` all, `0` CPU only.
    #[serde(default = "default_mosstts_ngl")]
    pub n_gpu_layers: i64,
    /// Upper bound on generated tokens; guards against runaway generation.
    #[serde(default = "default_mosstts_max_tokens")]
    pub max_new_tokens: i64,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

fn default_mosstts_language() -> String {
    "zh".to_string()
}
fn default_mosstts_ngl() -> i64 {
    -1
}
fn default_mosstts_max_tokens() -> i64 {
    3072
}

/// LongCat-AudioDiT (Meituan): a waveform-latent diffusion TTS model with
/// zero-shot cloning. Like Qwen3-TTS it runs through a small Python
/// runner in its own venv; the `audiodit` package is unpacked next to the
/// runner inside the model directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongCatManifest {
    #[serde(default = "default_python")]
    pub python: String,
    pub script: String,
    /// Local weights directory or a Hugging Face repo id.
    pub model: String,
    /// Local tokenizer directory (google/umt5-base files).
    #[serde(default)]
    pub tokenizer: Option<String>,
    #[serde(default = "default_device")]
    pub device: String,
    /// `auto` picks fp16 on CUDA and fp32 elsewhere (MPS fp16 is unsafe
    /// for the WAV-VAE decode).
    #[serde(default = "default_dtype")]
    pub dtype: String,
    /// Default guidance: `apg` or `cfg`.
    #[serde(default = "default_longcat_guidance")]
    pub guidance: String,
    #[serde(default = "default_longcat_cfg")]
    pub cfg_strength: f32,
    #[serde(default = "default_longcat_seed")]
    pub seed: i64,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

fn default_longcat_guidance() -> String {
    "apg".to_string()
}
fn default_longcat_cfg() -> f32 {
    4.0
}
fn default_longcat_seed() -> i64 {
    1024
}

/// A model executed by audio.cpp's native runtime.
///
/// Used by `dots` (dots.tts: 2B continuous AR, 48 kHz) and `cosyvoice`
/// (CosyVoice3: 0.5B streaming LLM TTS). Both ship a GGUF export that the
/// shared runtime runs on Metal/CUDA/Vulkan/CPU, so neither needs a
/// Python environment inside the app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioCppManifest {
    /// `audiocpp_cli` path, absolute or relative to the model directory.
    ///
    /// Defaulted so manifests written for a previous runner (VoxCPM2 used
    /// llama.cpp-omni and carried `base_lm`/`acoustic` instead) still
    /// parse: the engine then fails at load time with an actionable
    /// "reinstall" message instead of silently disappearing from the
    /// engine list.
    #[serde(default)]
    pub binary: String,
    /// GGUF weights, absolute or relative to the model directory.
    #[serde(default)]
    pub model: String,
    /// audio.cpp backend: `best`, `metal`, `cuda`, `vulkan`, `cpu`…
    #[serde(default = "default_dots_backend")]
    pub backend: String,
    /// Default language tag for the model (empty = none/auto).
    #[serde(default)]
    pub language: Option<String>,
    /// Classifier-free guidance; `None` leaves the family default alone.
    #[serde(default)]
    pub guidance: Option<f32>,
    #[serde(default = "default_dots_seed")]
    pub seed: i64,
    /// Optional prompt trim in seconds (audio.cpp's own trimmer).
    #[serde(default)]
    pub reference_duration_sec: Option<f32>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

fn default_dots_backend() -> String {
    "best".to_string()
}
fn default_dots_seed() -> i64 {
    42
}

/// Generic audio.cpp configuration (`kind: "audio_cpp"`).
///
/// The family and task name the audio.cpp operation (`seed_vc` / `vc`,
/// `ace_step` / `gen`, …); `model_spec.json` describes the request options
/// so a new family needs no Rust change. `input` and `voice_ref` say which
/// audio the task consumes: a voice-conversion model needs both, a
/// text-to-speech model only the reference clip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioCppTaskManifest {
    /// `audiocpp_cli` path, absolute or relative to the model directory.
    #[serde(default)]
    pub binary: String,
    /// GGUF file or a package directory, absolute or relative to the
    /// model directory.
    #[serde(default)]
    pub model: String,
    /// audio.cpp backend: `best`, `metal`, `cuda`, `vulkan`, `cpu`…
    #[serde(default = "default_dots_backend")]
    pub backend: String,
    /// audio.cpp family, e.g. `seed_vc`.
    #[serde(default)]
    pub family: String,
    /// audio.cpp task, e.g. `vc`, `svc`, `tts`, `clon`, `gen`.
    #[serde(default)]
    pub task: String,
    /// `model_spec.json` path relative to the model directory.
    #[serde(default)]
    pub spec: Option<String>,
    /// Inline spec, for hand-written manifests; wins over `spec`.
    #[serde(default)]
    pub model_spec: Option<crate::modelspec::ModelSpec>,
    /// Request field carrying the source audio (`audio` by default).
    #[serde(default)]
    pub input: Option<String>,
    /// Whether the task needs a target voice reference.
    #[serde(default = "default_true")]
    pub voice_ref: bool,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default = "default_dots_seed")]
    pub seed: i64,
    /// Passed as `--load-option k=v` and in the resident server config.
    #[serde(default)]
    pub load_options: std::collections::BTreeMap<String, String>,
    /// Passed as `--session-option k=v` and in the resident server config.
    #[serde(default)]
    pub session_options: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

/// Music-generation configuration (audio.cpp `--task gen`).
///
/// The request shape differs from speech: `text` is a caption/style
/// prompt, `lyrics` is optional (empty = instrumental), and the duration
/// is an explicit request option rather than a reference-audio limit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicManifest {
    /// `audiocpp_cli` path, absolute or relative to the model directory.
    #[serde(default)]
    pub binary: String,
    /// GGUF weights or a model package directory.
    #[serde(default)]
    pub model: String,
    /// audio.cpp family, e.g. `ace_step`, `stable_audio`.
    #[serde(default = "default_music_family")]
    pub family: String,
    /// audio.cpp backend: `best`, `metal`, `cuda`, `vulkan`, `cpu`…
    #[serde(default = "default_music_backend")]
    pub backend: String,
    /// audio.cpp task, normally `gen`.
    #[serde(default = "default_music_task")]
    pub task: String,
    /// Optional language hint for lyric parsing.
    #[serde(default)]
    pub language: Option<String>,
    /// Default guidance strength; `None` leaves the family default alone.
    #[serde(default)]
    pub guidance: Option<f32>,
    #[serde(default = "default_music_seed")]
    pub seed: i64,
    /// Passed as `--load-option k=v` and in the resident server config.
    #[serde(default)]
    pub load_options: std::collections::BTreeMap<String, String>,
    /// Passed as `--session-option k=v` and in the resident server config.
    #[serde(default)]
    pub session_options: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

fn default_music_family() -> String {
    "ace_step".to_string()
}
fn default_music_backend() -> String {
    "best".to_string()
}
fn default_music_task() -> String {
    "gen".to_string()
}
fn default_music_seed() -> i64 {
    42
}

/// Kokoro (hexgrad Kokoro-82M, sherpa-onnx ONNX export).
///
/// A preset-voices model: `voices.bin` holds one style vector per speaker
/// and a request selects one by integer speaker id (`sid`). There is no
/// reference-audio cloning, so engines built from this manifest never
/// inspect the synthesis request's prompt fields.
///
/// All file paths are relative to the model directory. The multi-language
/// export needs `dict_dir` (jieba segmentation) and both lexicon files;
/// English-only exports can leave them unset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KokoroManifest {
    pub model: String,
    /// Higher-precision export of the same model. Used when the runtime can
    /// make it worthwhile: the CUDA provider has no INT8 kernels, and on an
    /// x86 CPU without AVX2 the INT8 ones fall back to scalar code.
    #[serde(default)]
    pub model_fp32: Option<String>,
    pub voices: String,
    pub tokens: String,
    /// espeak-ng data directory.
    pub data_dir: String,
    #[serde(default)]
    pub dict_dir: Option<String>,
    /// Comma-separated lexicon files, same syntax as sherpa-onnx.
    #[serde(default)]
    pub lexicon: Option<String>,
    /// Comma-separated text-normalization FSTs (date/number/phone-zh…).
    #[serde(default)]
    pub rule_fsts: Option<String>,
    /// Fixed language tag; unset lets the multi-language frontend decide.
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default = "default_kokoro_length_scale")]
    pub length_scale: f32,
    #[serde(default = "default_kokoro_threads")]
    pub num_threads: i32,
    /// Speaker id used when the request carries no `voice` parameter.
    #[serde(default)]
    pub default_sid: i32,
}

fn default_kokoro_length_scale() -> f32 {
    1.0
}
fn default_kokoro_threads() -> i32 {
    2
}

fn default_python() -> String {
    "python3".to_string()
}
fn default_qwen_mode() -> String {
    "clone".to_string()
}
fn default_device() -> String {
    "auto".to_string()
}
fn default_dtype() -> String {
    "auto".to_string()
}
fn default_attn() -> String {
    "sdpa".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_is_optional_and_localized() {
        let without: Manifest = serde_json::from_str(
            r#"{"id":"dots","kind":"dots","name":"d",
                "dots":{"binary":"/b","model":"/m"}}"#,
        )
        .expect("manifest without description");
        assert!(without.description.is_empty());

        let with: Manifest = serde_json::from_str(
            r#"{"id":"dots","kind":"dots","name":"d",
                "description":{"en":"Fast cloning","zh":"快速克隆"},
                "dots":{"binary":"/b","model":"/m"}}"#,
        )
        .expect("manifest with description");
        assert_eq!(with.description.for_lang("en"), "Fast cloning");
        assert_eq!(with.description.for_lang("zh"), "快速克隆");
    }

    #[test]
    fn task_follows_kind_unless_declared() {
        let parse = |raw: &str| serde_json::from_str::<Manifest>(raw).expect("manifest");
        assert_eq!(
            parse(r#"{"id":"s","kind":"kokoro","name":"s"}"#).task(),
            TaskKind::Tts
        );
        assert_eq!(
            parse(r#"{"id":"m","kind":"music","name":"m"}"#).task(),
            TaskKind::Music
        );
        assert_eq!(
            parse(r#"{"id":"a","kind":"stt","name":"a"}"#).task(),
            TaskKind::Asr
        );
        // A kind can serve another task when the manifest says so.
        assert_eq!(
            parse(r#"{"id":"v","kind":"zipvoice","name":"v","task":"vc"}"#).task(),
            TaskKind::Vc
        );
    }

    #[test]
    fn builtin_parameters_are_appended_to_old_manifests() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"id":"indextts2","kind":"indextts2","name":"i",
                "indextts2":{"binary":"/b","model":"/m"},
                "parameters":[{
                    "name":"num_beams","label":"Decoding","type":"choice",
                    "options":[{"label":"Beam","value":"3"}],"default":"3"
                }]}"#,
        )
        .expect("manifest with an older parameter list");
        let params = manifest.parameters_or_defaults();
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "num_beams",
                "language",
                "text_chunk_size",
                "interval_silence_ms"
            ],
            "manifest entries stay first, new built-ins are appended"
        );
        // The manifest's own spec wins; the built-in one only fills gaps.
        assert_eq!(params[0].label.for_lang("en"), "Decoding");
    }

    #[test]
    fn music_defaults_follow_the_family() {
        let ace: Manifest = serde_json::from_str(
            r#"{"id":"ace","kind":"music","name":"a",
                "music":{"binary":"/b","model":"/m"}}"#,
        )
        .expect("ACE-Step manifest");
        let ace_params = ace.parameters_or_defaults();
        let names: Vec<&str> = ace_params.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"task_route"), "ACE-Step has editing routes");
        assert!(!names.contains(&"audio_input_kind"));

        let stable: Manifest = serde_json::from_str(
            r#"{"id":"sa","kind":"music","name":"s",
                "music":{"binary":"/b","model":"/m","family":"stable_audio"}}"#,
        )
        .expect("Stable Audio manifest");
        let params = stable.parameters_or_defaults();
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "duration_sec",
                "num_steps",
                "guidance_scale",
                "negative_prompt",
                "seed",
                "audio_input_kind",
                "init_noise_level",
                "inpaint_mask_start_sec",
                "inpaint_mask_end_sec",
            ]
        );
        assert_eq!(params[0].default, serde_json::json!(30.0));
        assert!(!names.contains(&"task_route"), "no ACE-Step routes");
        assert!(!names.contains(&"lyrics"), "Stable Audio takes no lyrics");
    }

    #[test]
    fn manifests_carry_a_licence() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"id":"x","kind":"music","name":"x",
                "license":{"name":"CC BY-NC 4.0","noncommercial":true},
                "music":{"binary":"/b","model":"/m"}}"#,
        )
        .expect("manifest with a licence");
        let license = manifest.license.expect("licence");
        assert_eq!(license.name, "CC BY-NC 4.0");
        assert!(license.noncommercial);
        assert_eq!(license.url, None);

        let plain: Manifest = serde_json::from_str(
            r#"{"id":"y","kind":"music","name":"y",
                "music":{"binary":"/b","model":"/m"}}"#,
        )
        .expect("manifest without a licence");
        assert!(plain.license.is_none());
    }

    #[test]
    fn longcat_manifest_parses_with_defaults() {
        let raw = r#"{
            "id": "longcat",
            "kind": "longcat",
            "name": "LongCat AudioDiT",
            "longcat": {
                "script": "longcat_runner.py",
                "model": "/tmp/weights",
                "tokenizer": "/tmp/tokenizer"
            }
        }"#;
        let manifest: Manifest = serde_json::from_str(raw).expect("valid manifest");
        let lc = manifest.longcat.as_ref().expect("longcat section");
        assert_eq!(lc.device, "auto");
        assert_eq!(lc.dtype, "auto");
        assert_eq!(lc.guidance, "apg");
        assert_eq!(lc.cfg_strength, 4.0);
        assert_eq!(lc.seed, 1024);

        let params = manifest.parameters_or_defaults();
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["num_steps", "cfg", "guidance_method", "seed"]);
        assert_eq!(params[0].default, serde_json::json!(16));
        assert_eq!(params[2].kind, ParamKind::Choice);
    }

    #[test]
    fn audiocpp_families_parse() {
        let dots: Manifest = serde_json::from_str(
            r#"{"id":"dots","kind":"dots","name":"d",
                "dots":{"binary":"/b","model":"/m"}}"#,
        )
        .expect("dots manifest");
        let cfg = dots.dots.as_ref().expect("dots section");
        assert_eq!(cfg.backend, "best");
        assert_eq!(cfg.guidance, None);
        assert_eq!(cfg.seed, 42);

        let cosy: Manifest = serde_json::from_str(
            r#"{"id":"cosyvoice","kind":"cosyvoice","name":"c",
                "cosyvoice":{"binary":"/b","model":"/m","guidance":1.2}}"#,
        )
        .expect("cosyvoice manifest");
        let cfg = cosy.cosyvoice.as_ref().expect("cosyvoice section");
        assert_eq!(cfg.guidance, Some(1.2));

        let params = cosy.parameters_or_defaults();
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["template", "instruct", "num_steps", "top_k", "seed"]
        );
        assert_eq!(params[1].kind, ParamKind::TextArea);

        // A VoxCPM2 install from the llama.cpp-omni era must still parse
        // so the engine stays visible and reports "reinstall"; dropping it
        // from the scan would look like the model vanished.
        let legacy: Manifest = serde_json::from_str(
            r#"{"id":"voxcpm2","kind":"voxcpm2","name":"v",
                "voxcpm2":{"base_lm":"a.gguf","acoustic":"b.gguf","cfg":2.0}}"#,
        )
        .expect("legacy voxcpm2 manifest still parses");
        let cfg = legacy.voxcpm2.as_ref().expect("voxcpm2 section");
        assert!(cfg.binary.is_empty() && cfg.model.is_empty());
        assert_eq!(params[4].default, serde_json::json!(1986));
    }

    #[test]
    fn kokoro_manifest_parses_with_defaults() {
        let raw = r#"{
            "id": "kokoro",
            "kind": "kokoro",
            "name": "Kokoro 82M",
            "kokoro": {
                "model": "model.int8.onnx",
                "voices": "voices.bin",
                "tokens": "tokens.txt",
                "data_dir": "espeak-ng-data",
                "dict_dir": "dict",
                "lexicon": "lexicon-us-en.txt,lexicon-zh.txt",
                "default_sid": 3
            }
        }"#;
        let manifest: Manifest = serde_json::from_str(raw).expect("valid manifest");
        let k = manifest.kokoro.as_ref().expect("kokoro section");
        assert_eq!(k.length_scale, 1.0);
        assert_eq!(k.num_threads, 2);
        assert_eq!(k.default_sid, 3);
        assert_eq!(k.lang, None);
        // Installs from before the fp32 export existed have no such field.
        assert_eq!(k.model_fp32, None);

        let with_fp32 = raw.replace(
            r#""model": "model.int8.onnx","#,
            r#""model": "model.int8.onnx", "model_fp32": "model.onnx","#,
        );
        let manifest: Manifest = serde_json::from_str(&with_fp32).expect("fp32 manifest");
        let k = manifest.kokoro.as_ref().expect("kokoro section");
        assert_eq!(k.model_fp32.as_deref(), Some("model.onnx"));

        // Without a declared parameter list the kind exposes speed only;
        // the voice choice is expected from the model's own manifest.
        let params = manifest.parameters_or_defaults();
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["speed"]);
    }

    #[test]
    fn indextts2_manifest_parses_with_defaults() {
        let raw = r#"{
            "id": "indextts2",
            "kind": "indextts2",
            "name": "IndexTTS-2",
            "indextts2": {
                "binary": "/tools/audiocpp/audiocpp_cli",
                "model": "index-tts2-q8_0.gguf"
            }
        }"#;
        let manifest: Manifest = serde_json::from_str(raw).expect("valid manifest");
        assert_eq!(manifest.kind, EngineKind::IndexTts2);
        let c = manifest.indextts2.as_ref().expect("indextts2 section");
        assert_eq!(c.backend, "best");
        assert_eq!(c.seed, 42);
        assert_eq!(c.language, None);
        assert_eq!(manifest.reference_limit_secs(), 15.0);
        assert_eq!(manifest.problem(), None);
        // Segment controls are built in, so manifests from before they
        // existed still get them.
        let params = manifest.parameters_or_defaults();
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "language",
                "num_beams",
                "text_chunk_size",
                "interval_silence_ms"
            ]
        );

        // A section without the audio.cpp paths is a broken install.
        let broken: Manifest = serde_json::from_str(
            r#"{"id":"indextts2","kind":"indextts2","name":"i","indextts2":{}}"#,
        )
        .expect("manifest with an empty section");
        assert!(broken.problem().is_none()); // present, so structurally fine
        assert!(broken
            .indextts2
            .as_ref()
            .is_some_and(|c| c.binary.is_empty() && c.model.is_empty()));
    }
}
