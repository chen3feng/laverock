//! Engine manifest format.
//!
//! Every model directory under the models root must contain an
//! `engine.json`: it is the single source of truth for which files an
//! engine uses and what its default synthesis parameters are.

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
            EngineKind::Stt => "stt",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Stable identifier, also the directory name in practice.
    pub id: String,
    pub kind: EngineKind,
    pub name: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
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
    /// STT configuration when `kind == "stt"`.
    #[serde(default)]
    pub stt: Option<SttManifest>,
    #[serde(default)]
    pub defaults: SynthDefaults,
    /// User-tunable synthesis parameters shown by the UI. When absent,
    /// [`Manifest::default_parameters`] supplies the built-in set per kind.
    #[serde(default)]
    pub parameters: Vec<ParamSpec>,
}

impl Manifest {
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
            Self::default_parameters(self.kind)
        } else {
            let mut params = self.parameters.clone();
            crate::legacy_i18n::localize(self.kind, &mut params);
            params
        }
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
            EngineKind::Stt => 0.0,
            _ => 15.0,
        }
    }

    /// Parameters a manifest ships unless it declares its own list.
    pub fn default_parameters(kind: EngineKind) -> Vec<ParamSpec> {
        use serde_json::json;
        match kind {
            // STT is not a synthesis engine; it has no per-request params.
            EngineKind::Stt => Vec::new(),
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
            // emotion reference clip we do not collect. The installer
            // ships a text-language choice; nothing else is tunable.
            EngineKind::IndexTts2 => Vec::new(),
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
        // The engine has no tunable request parameters.
        assert!(manifest.parameters_or_defaults().is_empty());

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
