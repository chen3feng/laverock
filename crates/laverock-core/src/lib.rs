//! Laverock core: manifest-driven TTS engines.
//!
//! Adding an engine means dropping a directory with an `engine.json`
//! manifest into the models directory — no code changes required for
//! engine variants that share an already supported kind.

mod audio;
mod dots;
mod longcat;
mod manager;
mod manifest;
mod qwen3tts;
mod voxcpm2;
mod zipvoice;

pub use audio::{decode_wav, encode_wav};
pub use manager::{
    EngineInfo, EngineManager, ProgressEvent, ProgressSink, SpeechEngine, SynthRequest,
};
pub use manifest::{
    DotsManifest, EngineKind, LongCatManifest, Manifest, ParamKind, ParamOption, ParamSpec,
    Qwen3TtsManifest, SynthDefaults, VoxCpm2Manifest, ZipVoiceManifest,
};

/// Re-exported for engines that need the underlying runtime.
pub use sherpa_onnx;

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("engine not found: {0}")]
    NotFound(String),

    #[error("unsupported engine kind: {0}")]
    UnsupportedKind(String),

    #[error("model load failed: {0}")]
    Load(String),

    #[error("synthesis failed: {0}")]
    Synth(String),

    #[error("audio decode failed: {0}")]
    Wav(String),

    #[error("{0}")]
    BadRequest(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// Synthesized mono PCM at `sample_rate`.
#[derive(Debug, Clone)]
pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: i32,
}

/// Status of the running service, surfaced over HTTP.
#[derive(Debug, Serialize)]
pub struct Health {
    pub status: &'static str,
    pub engines: usize,
}
