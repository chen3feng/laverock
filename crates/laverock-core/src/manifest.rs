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
}

impl EngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineKind::ZipVoice => "zipvoice",
            EngineKind::VoxCpm2 => "voxcpm2",
            EngineKind::Qwen3Tts => "qwen3tts",
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
