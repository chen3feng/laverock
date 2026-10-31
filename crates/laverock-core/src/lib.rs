//! Laverock core: manifest-driven TTS engines.
//!
//! Adding an engine means dropping a directory with an `engine.json`
//! manifest into the models directory — no code changes required for
//! engine variants that share an already supported kind.

mod audio;
mod audiocpp;
mod chatterbox;
pub mod i18n;
mod kokoro;
mod legacy_i18n;
mod longcat;
mod manager;
mod manifest;
mod ming3tts;
mod mosstts;
mod onnx;
mod qwen3tts;
mod transcribe;
mod zipvoice;

pub use audio::{
    cap_silences, decode_wav, decode_wav_channels, encode_wav, resample_mono, silence_profile,
    trim_reference, trim_silence, SilenceProfile, VOXCPM2_MAX_REFERENCE_SECS,
};
pub use manager::{
    EngineInfo, EngineManager, ProgressEvent, ProgressSink, SpeechEngine, SynthRequest,
};
pub use manifest::{
    AudioCppManifest, ChatterboxManifest, EngineKind, KokoroManifest, Localized, LongCatManifest,
    Manifest, Ming3TtsManifest, MossTtsManifest, ParamKind, ParamOption, ParamSpec,
    Qwen3TtsManifest, SttManifest, SynthDefaults, ZipVoiceManifest,
};
pub use transcribe::Transcriber;

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

    /// A user-facing validation error with translations (see [`i18n`]).
    #[error("{0}")]
    UserError(i18n::Message),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl CoreError {
    /// User-facing text in `lang`. Engine internals (subprocess output,
    /// file paths) stay as they are; validation errors translate.
    pub fn localized(&self, lang: i18n::Lang) -> String {
        match self {
            CoreError::UserError(message) => message.render(lang),
            other => other.to_string(),
        }
    }
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
