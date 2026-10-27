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
    pub voxcpm2: Option<VoxCpm2Manifest>,
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
    /// Declared parameters, or the built-in set for this engine kind.
    pub fn parameters_or_defaults(&self) -> Vec<ParamSpec> {
        if self.parameters.is_empty() {
            Self::default_parameters(self.kind)
        } else {
            self.parameters.clone()
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
                    "语速",
                    1.0,
                    0.5,
                    2.0,
                    0.1,
                    "越大语速越快，不改变音色",
                ),
                ParamSpec::int_slider("num_steps", "扩散步数", 4, 1, 16, 1, "越多越稳但越慢"),
            ],
            EngineKind::VoxCpm2 => vec![
                ParamSpec::slider(
                    "speed",
                    "语速",
                    1.0,
                    0.5,
                    2.0,
                    0.1,
                    "越大语速越快，不改变音色",
                ),
                ParamSpec::int_slider("num_steps", "扩散步数", 10, 1, 30, 1, "越多越稳但越慢"),
                ParamSpec::slider("cfg", "提示词强度 (CFG)", 2.0, 0.0, 5.0, 0.1, "过高会失真"),
                ParamSpec::slider(
                    "temperature",
                    "随机性",
                    1.0,
                    0.1,
                    2.0,
                    0.05,
                    "越高变化越大，越低越确定",
                ),
                ParamSpec {
                    name: "seed".into(),
                    label: "随机种子".into(),
                    kind: ParamKind::Int,
                    min: Some(json!(-1)),
                    max: Some(json!(2_147_483_647)),
                    step: None,
                    options: Vec::new(),
                    default: json!(42),
                    hint: "相同种子 + 文本得到可复现的声音；-1 每次随机".into(),
                },
            ],
            // Non-diffusion autoregressive models expose no step control.
            EngineKind::Qwen3Tts => Vec::new(),
            // Flow-matching ODE: NFE steps, guidance strength/method, seed.
            EngineKind::LongCat => vec![
                ParamSpec::int_slider(
                    "num_steps",
                    "采样步数 (NFE)",
                    16,
                    1,
                    32,
                    1,
                    "扩散 ODE 步数；16 已足够，越多越慢",
                ),
                ParamSpec::slider(
                    "cfg",
                    "引导强度",
                    4.0,
                    0.0,
                    10.0,
                    0.1,
                    "越高越贴近参考音色，过高可能失真",
                ),
                ParamSpec {
                    name: "guidance_method".into(),
                    label: "引导方式".into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: "APG（自适应投影，推荐）".into(),
                            value: "apg".into(),
                        },
                        ParamOption {
                            label: "CFG（传统无分类器引导）".into(),
                            value: "cfg".into(),
                        },
                    ],
                    default: serde_json::json!("apg"),
                    hint: "APG 通常更自然、伪影更少".into(),
                },
                ParamSpec {
                    name: "seed".into(),
                    label: "随机种子".into(),
                    kind: ParamKind::Int,
                    min: Some(serde_json::json!(0)),
                    max: Some(serde_json::json!(2_147_483_647)),
                    step: None,
                    options: Vec::new(),
                    default: serde_json::json!(1024),
                    hint: "相同种子 + 文本得到可复现的声音".into(),
                },
            ],
            // Continuous-AR flow matching; 48 kHz output.
            EngineKind::Dots => vec![
                ParamSpec::int_slider(
                    "num_steps",
                    "采样步数",
                    10,
                    1,
                    32,
                    1,
                    "SOAR 推荐 10 步；变多会更慢",
                ),
                ParamSpec::slider(
                    "guidance",
                    "引导强度",
                    1.2,
                    0.0,
                    3.0,
                    0.1,
                    "越高越贴近参考音色，过高可能失真",
                ),
                ParamSpec {
                    name: "clone_mode".into(),
                    label: "克隆方式".into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: "仅声纹（只凭声音，推荐）".into(),
                            value: "xvector".into(),
                        },
                        ParamOption {
                            label: "声纹 + 文字（文字稿准确时更相似）".into(),
                            value: "continuation".into(),
                        },
                    ],
                    default: serde_json::json!("xvector"),
                    hint: "文字稿不准确时选「仅声纹」更稳".into(),
                },
                ParamSpec {
                    name: "language".into(),
                    label: "语言标签".into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: "自动".into(),
                            value: "".into(),
                        },
                        ParamOption {
                            label: "中文".into(),
                            value: "zh".into(),
                        },
                        ParamOption {
                            label: "English".into(),
                            value: "en".into(),
                        },
                        ParamOption {
                            label: "日本語".into(),
                            value: "ja".into(),
                        },
                        ParamOption {
                            label: "한국어".into(),
                            value: "ko".into(),
                        },
                        ParamOption {
                            label: "Français".into(),
                            value: "fr".into(),
                        },
                        ParamOption {
                            label: "Deutsch".into(),
                            value: "de".into(),
                        },
                        ParamOption {
                            label: "Español".into(),
                            value: "es".into(),
                        },
                    ],
                    default: serde_json::json!(""),
                    hint: "多语言或混合语言时固定标签有帮助".into(),
                },
                ParamSpec {
                    name: "seed".into(),
                    label: "随机种子".into(),
                    kind: ParamKind::Int,
                    min: Some(serde_json::json!(0)),
                    max: Some(serde_json::json!(2_147_483_647)),
                    step: None,
                    options: Vec::new(),
                    default: serde_json::json!(42),
                    hint: "相同种子 + 文本得到可复现的声音".into(),
                },
            ],
            // CosyVoice3: streaming LLM TTS with three request templates.
            EngineKind::CosyVoice => vec![
                ParamSpec {
                    name: "template".into(),
                    label: "合成模板".into(),
                    kind: ParamKind::Choice,
                    min: None,
                    max: None,
                    step: None,
                    options: vec![
                        ParamOption {
                            label: "零样本克隆（需文字稿，相似度最高）".into(),
                            value: "zero_shot".into(),
                        },
                        ParamOption {
                            label: "跨语种克隆（只凭声音）".into(),
                            value: "cross_lingual".into(),
                        },
                        ParamOption {
                            label: "指令（用文字描述语气）".into(),
                            value: "instruct".into(),
                        },
                    ],
                    default: serde_json::json!("zero_shot"),
                    hint: "零样本最像；文字稿不准时选跨语种".into(),
                },
                ParamSpec {
                    name: "instruct".into(),
                    label: "指令（仅「指令」模板使用）".into(),
                    kind: ParamKind::TextArea,
                    min: None,
                    max: None,
                    step: None,
                    options: Vec::new(),
                    default: serde_json::json!(""),
                    hint: "例如：用温和亲切的语气，语速稍慢".into(),
                },
                ParamSpec::int_slider(
                    "num_steps",
                    "流式解码步数",
                    10,
                    1,
                    32,
                    1,
                    "声学流匹配的 Euler 步数；越多越稳越慢",
                ),
                ParamSpec {
                    name: "top_k".into(),
                    label: "Top-K 采样".into(),
                    kind: ParamKind::Int,
                    min: Some(serde_json::json!(1)),
                    max: Some(serde_json::json!(100)),
                    step: None,
                    options: Vec::new(),
                    default: serde_json::json!(25),
                    hint: "越小越稳定，越大越有变化".into(),
                },
                ParamSpec {
                    name: "seed".into(),
                    label: "随机种子".into(),
                    kind: ParamKind::Int,
                    min: Some(serde_json::json!(0)),
                    max: Some(serde_json::json!(2_147_483_647)),
                    step: None,
                    options: Vec::new(),
                    default: serde_json::json!(1986),
                    hint: "相同种子 + 文本得到可复现的声音".into(),
                },
            ],
            // Preset-voices model: speed is the only built-in control;
            // the speaker (`voice`) choice is model-specific, so manifests
            // declare their own parameter list with every sid they ship.
            EngineKind::Kokoro => vec![ParamSpec::slider(
                "speed",
                "语速",
                1.0,
                0.5,
                2.0,
                0.1,
                "越大语速越快，不改变音色",
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
    /// UI label.
    pub label: String,
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
    pub hint: String,
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
    pub label: String,
    pub value: String,
}

impl ParamSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        label: &str,
        kind: ParamKind,
        default: serde_json::Value,
        hint: &str,
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
        label: &str,
        default: f64,
        min: f64,
        max: f64,
        step: f64,
        hint: &str,
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
        label: &str,
        default: i64,
        min: i64,
        max: i64,
        step: i64,
        hint: &str,
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

/// A VoxCPM2 model: a llama.cpp `voxcpm2-cli` binary plus two GGUF files.
///
/// All paths are relative to the model directory. The runner binary can be
/// provided either by the `binary` field or, when unset, is looked up as
/// `voxcpm2-cli` next to the GGUF files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoxCpm2Manifest {
    pub base_lm: String,
    pub acoustic: String,
    #[serde(default)]
    pub binary: Option<String>,
    #[serde(default = "default_voxcpm2_cfg")]
    pub cfg: f32,
    #[serde(default = "default_voxcpm2_temperature")]
    pub temperature: f32,
    #[serde(default = "default_voxcpm2_seed")]
    pub seed: i32,
    #[serde(default)]
    pub cpu: bool,
}

fn default_voxcpm2_cfg() -> f32 {
    2.0
}
fn default_voxcpm2_temperature() -> f32 {
    1.0
}
fn default_voxcpm2_seed() -> i32 {
    42
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
    pub binary: String,
    /// GGUF weights, absolute or relative to the model directory.
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
}
