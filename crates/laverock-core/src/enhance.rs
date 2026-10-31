//! Cleanup for recorded takes before they become voice references.
//!
//! Two stages, both optional and both run on the local machine:
//!
//! - **denoise** — sherpa-onnx's GTCRN model at 16 kHz, for room noise,
//!   fans and traffic.
//! - **vocals** — audio.cpp's BS-RoFormer at 44.1 kHz, which splits a mix
//!   into a vocals stem and an instrumental one; the vocals stem removes
//!   background music.
//!
//! The models are not downloaded with the app; [`Enhancer::discover`] only
//! reports what is present, and the caller asks the user to install the
//! enhancement pack when a requested stage is missing. Processing happens
//! on the stored take, and the untouched upload is kept so the user can
//! compare or revert.

use crate::{
    audio::{decode_wav, encode_wav, resample_mono},
    i18n::{self, Key},
    Audio, CoreError, Result,
};
use sherpa_onnx::{
    OfflineSpeechDenoiser, OfflineSpeechDenoiserConfig, OfflineSpeechDenoiserGtcrnModelConfig,
    OfflineSpeechDenoiserModelConfig,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Denoise checkpoints, in the order they are looked for.
///
/// Only GTCRN is shipped. sherpa-onnx's DPDFNet models sound better on
/// steady noise, but they gate the first half second of speech after a
/// quiet lead-in — on a reference clip that is the first word, so they are
/// deliberately not used. A DPDFNet file left over from an older install
/// is ignored.
pub const DENOISE_MODELS: &[&str] = &["gtcrn_simple.onnx"];
/// BS-RoFormer checkpoint (Q8_0, ~164 MB) used for vocal isolation.
pub const SEPARATION_MODEL: &str = "bs-roformer.gguf";
/// Directory under the models root that holds the enhancement pack.
pub const ENHANCE_DIR: &str = "enhance";
/// Sample rate BS-RoFormer expects; the take is resampled around it.
const SEPARATION_RATE: u32 = 44_100;
/// Separation is roughly twice real time on Metal, so a long take can
/// legitimately run for minutes.
const SEPARATION_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// What to remove from a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnhanceMode {
    #[default]
    Off,
    /// Room noise, fans, traffic.
    Denoise,
    /// Background music, keeping the voice.
    Vocals,
    /// Music first, then noise.
    Both,
}

impl EnhanceMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "off" | "none" => Some(Self::Off),
            "denoise" | "noise" => Some(Self::Denoise),
            "vocals" | "music" => Some(Self::Vocals),
            "both" | "all" => Some(Self::Both),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Denoise => "denoise",
            Self::Vocals => "vocals",
            Self::Both => "both",
        }
    }

    pub fn is_off(self) -> bool {
        self == Self::Off
    }

    pub fn needs_denoise(self) -> bool {
        matches!(self, Self::Denoise | Self::Both)
    }

    pub fn needs_separation(self) -> bool {
        matches!(self, Self::Vocals | Self::Both)
    }
}

struct Separator {
    cli: PathBuf,
    model: PathBuf,
}

/// The enhancement stages whose model files exist on this machine.
pub struct Enhancer {
    denoise_model: Option<PathBuf>,
    separator: Option<Separator>,
}

impl Enhancer {
    /// Look for the pack under `<models>/enhance` and the audio.cpp CLI
    /// under `<tools>/audiocpp`. Missing pieces simply disable a stage.
    pub fn discover(models_dir: &Path, tools_dir: Option<&Path>) -> Self {
        let dir = models_dir.join(ENHANCE_DIR);
        let denoise_model = DENOISE_MODELS
            .iter()
            .map(|name| dir.join(name))
            .find(|path| path.is_file());

        let model = dir.join(SEPARATION_MODEL);
        let cli = tools_dir
            .map(|tools| tools.join("audiocpp").join(cli_file()))
            .filter(|path| path.is_file());
        let separator = cli
            .filter(|_| model.is_file())
            .map(|cli| Separator { cli, model });

        Self {
            denoise_model,
            separator,
        }
    }

    /// Whether every stage `mode` asks for is installed.
    pub fn supports(&self, mode: EnhanceMode) -> bool {
        (!mode.needs_denoise() || self.denoise_model.is_some())
            && (!mode.needs_separation() || self.separator.is_some())
    }

    pub fn has_denoise(&self) -> bool {
        self.denoise_model.is_some()
    }

    pub fn has_separation(&self) -> bool {
        self.separator.is_some()
    }

    /// Run the requested stages on mono `samples` at `rate`, returning
    /// audio at the same rate.
    pub fn apply(&self, mode: EnhanceMode, samples: &[f32], rate: u32) -> Result<Audio> {
        if mode.is_off() || samples.is_empty() {
            return Ok(Audio {
                samples: samples.to_vec(),
                sample_rate: rate as i32,
            });
        }
        if !self.supports(mode) {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::EnhanceModelsMissing,
            )));
        }

        let mut audio = Audio {
            samples: samples.to_vec(),
            sample_rate: rate as i32,
        };
        if mode.needs_separation() {
            audio = self.separate(&audio)?;
        }
        if mode.needs_denoise() {
            audio = self.denoise(&audio)?;
        }
        Ok(audio)
    }

    fn denoise(&self, audio: &Audio) -> Result<Audio> {
        let model = self
            .denoise_model
            .as_ref()
            .ok_or_else(|| CoreError::UserError(i18n::Message::new(Key::EnhanceModelsMissing)))?;
        let model_config = OfflineSpeechDenoiserModelConfig {
            gtcrn: OfflineSpeechDenoiserGtcrnModelConfig {
                model: Some(model.display().to_string()),
            },
            ..Default::default()
        };
        let denoiser = OfflineSpeechDenoiser::create(&OfflineSpeechDenoiserConfig {
            model: model_config,
        })
        .ok_or_else(|| CoreError::Synth(format!("failed to load denoiser {}", model.display())))?;

        let target = u32::try_from(denoiser.sample_rate()).unwrap_or(16_000);
        let input = resample_mono(&audio.samples, audio.sample_rate as u32, target);
        let cleaned = denoiser.run(&input, target as i32);
        if cleaned.samples.is_empty() {
            return Err(CoreError::Synth("denoiser produced no audio".into()));
        }
        let samples = resample_mono(
            &cleaned.samples,
            cleaned.sample_rate.max(1) as u32,
            audio.sample_rate as u32,
        );
        Ok(Audio {
            samples,
            sample_rate: audio.sample_rate,
        })
    }

    fn separate(&self, audio: &Audio) -> Result<Audio> {
        let separator = self
            .separator
            .as_ref()
            .ok_or_else(|| CoreError::UserError(i18n::Message::new(Key::EnhanceModelsMissing)))?;

        let dir = std::env::temp_dir().join(format!(
            "laverock-enhance-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir)?;
        let result = self.run_separation(separator, &dir, audio);
        let _ = fs::remove_dir_all(&dir);
        result
    }

    fn run_separation(&self, separator: &Separator, dir: &Path, audio: &Audio) -> Result<Audio> {
        let input = dir.join("input.wav");
        let rate = SEPARATION_RATE;
        let resampled = resample_mono(&audio.samples, audio.sample_rate as u32, rate);
        fs::write(&input, encode_wav(&resampled, rate as i32)?)?;

        let out_dir = dir.join("stems");
        let log = dir.join("audiocpp.log");
        let log_file = fs::File::create(&log)?;
        let mut child = Command::new(&separator.cli)
            .arg("--task")
            .arg("sep")
            .arg("--family")
            .arg("bs_roformer")
            .arg("--model")
            .arg(&separator.model)
            .arg("--audio")
            .arg(&input)
            .arg("--out-dir")
            .arg(&out_dir)
            // The CLI's default backend is the CPU; ask for the best
            // available one, which is Metal here and CUDA where present.
            .arg("--backend")
            .arg("best")
            .stdout(Stdio::null())
            .stderr(Stdio::from(log_file))
            .spawn()
            .map_err(|e| {
                CoreError::Synth(format!("could not start {}: {e}", separator.cli.display()))
            })?;

        let deadline = Instant::now() + SEPARATION_TIMEOUT;
        loop {
            match child.try_wait()? {
                Some(status) if status.success() => break,
                Some(status) => {
                    let detail = fs::read_to_string(&log).unwrap_or_default();
                    return Err(CoreError::Synth(format!(
                        "vocal separation failed ({status}): {}",
                        detail.lines().last().unwrap_or("").trim()
                    )));
                }
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(CoreError::Synth(
                        "vocal separation timed out after 15 minutes".into(),
                    ));
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }

        let vocals = out_dir.join("vocals.wav");
        let bytes = fs::read(&vocals)
            .map_err(|e| CoreError::Synth(format!("the separator produced no vocals stem: {e}")))?;
        let (samples, stem_rate) = decode_wav(&bytes)?;
        Ok(Audio {
            samples: resample_mono(&samples, stem_rate.max(1) as u32, audio.sample_rate as u32),
            sample_rate: audio.sample_rate,
        })
    }
}

fn cli_file() -> &'static str {
    if cfg!(windows) {
        "audiocpp_cli.exe"
    } else {
        "audiocpp_cli"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes_and_aliases() {
        for (raw, mode) in [
            ("", EnhanceMode::Off),
            (" off ", EnhanceMode::Off),
            ("none", EnhanceMode::Off),
            ("denoise", EnhanceMode::Denoise),
            ("Noise", EnhanceMode::Denoise),
            ("vocals", EnhanceMode::Vocals),
            ("music", EnhanceMode::Vocals),
            ("both", EnhanceMode::Both),
            ("ALL", EnhanceMode::Both),
        ] {
            assert_eq!(EnhanceMode::parse(raw), Some(mode), "{raw:?}");
            assert_eq!(EnhanceMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(EnhanceMode::parse("loud"), None);
    }

    #[test]
    fn stages_per_mode() {
        assert!(EnhanceMode::Off.is_off());
        assert!(EnhanceMode::Denoise.needs_denoise() && !EnhanceMode::Denoise.needs_separation());
        assert!(EnhanceMode::Vocals.needs_separation() && !EnhanceMode::Vocals.needs_denoise());
        assert!(EnhanceMode::Both.needs_denoise() && EnhanceMode::Both.needs_separation());
    }

    #[test]
    fn discovers_only_installed_stages() {
        let root = std::env::temp_dir().join(format!(
            "laverock-enhance-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let models = root.join("models");
        let tools = root.join("tools");
        let pack = models.join(ENHANCE_DIR);
        fs::create_dir_all(&pack).unwrap();

        let empty = Enhancer::discover(&models, Some(&tools));
        assert!(!empty.has_denoise() && !empty.has_separation());
        assert!(empty.supports(EnhanceMode::Off));
        assert!(!empty.supports(EnhanceMode::Denoise));

        fs::write(pack.join(DENOISE_MODELS[0]), b"x").unwrap();
        let denoise_only = Enhancer::discover(&models, Some(&tools));
        assert!(denoise_only.has_denoise() && !denoise_only.has_separation());
        assert!(denoise_only.supports(EnhanceMode::Denoise));
        assert!(!denoise_only.supports(EnhanceMode::Vocals));

        // The separator needs both its model and the audio.cpp CLI.
        fs::write(pack.join(SEPARATION_MODEL), b"x").unwrap();
        let no_cli = Enhancer::discover(&models, Some(&tools));
        assert!(!no_cli.has_separation());

        fs::create_dir_all(tools.join("audiocpp")).unwrap();
        fs::write(tools.join("audiocpp").join(cli_file()), b"x").unwrap();
        let full = Enhancer::discover(&models, Some(&tools));
        assert!(full.has_denoise() && full.has_separation());
        assert!(full.supports(EnhanceMode::Both));

        let _ = fs::remove_dir_all(&root);
    }
}
