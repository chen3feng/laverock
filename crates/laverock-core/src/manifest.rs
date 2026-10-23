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
}

impl EngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineKind::ZipVoice => "zipvoice",
            EngineKind::VoxCpm2 => "voxcpm2",
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
    pub defaults: SynthDefaults,
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
