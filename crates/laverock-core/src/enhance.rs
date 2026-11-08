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
//!
//! [`Enhancer::apply`] is the capture-library entry point, which always
//! returns one clip for use as a voice reference. [`Enhancer::apply_stems`]
//! is the standalone-tools entry point: it can also hand back the
//! instrumental stem the separator already produced.

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

/// What a standalone audio tool should produce from an uploaded clip.
///
/// Unlike [`EnhanceMode`], which is about preparing one reference clip,
/// this can also keep the instrumental stem and never runs both stages at
/// once: the tools page runs exactly the stage the user picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StemMode {
    /// Remove room noise; the rest of the clip is unchanged.
    Denoise,
    /// Keep the voice and drop the background music.
    Vocals,
    /// Keep the background music and drop the voice.
    Instrumental,
    /// Both separated stems.
    Both,
}

impl StemMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "denoise" | "noise" => Some(Self::Denoise),
            "vocals" | "voice" => Some(Self::Vocals),
            "instrumental" | "music" | "accompaniment" => Some(Self::Instrumental),
            "both" | "stems" | "all" => Some(Self::Both),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Denoise => "denoise",
            Self::Vocals => "vocals",
            Self::Instrumental => "instrumental",
            Self::Both => "both",
        }
    }

    /// Separation is the slow stage; only denoising skips it.
    pub fn needs_separation(self) -> bool {
        !matches!(self, Self::Denoise)
    }

    /// Whether the vocal stem is part of the output.
    pub fn wants_vocals(self) -> bool {
        matches!(self, Self::Vocals | Self::Both)
    }

    /// Whether the instrumental stem is part of the output.
    pub fn wants_instrumental(self) -> bool {
        matches!(self, Self::Instrumental | Self::Both)
    }
}

/// The output of [`Enhancer::apply_stems`].
#[derive(Debug, Clone)]
pub struct Stems {
    /// The vocal stem, or the denoised input when separation did not run.
    pub vocals: Audio,
    /// The background-music stem, present whenever separation ran. The
    /// `Vocals` mode returns it too: separation produces both at once, and
    /// the caller can discard what it does not need.
    pub instrumental: Option<Audio>,
}

fn no_stage(_: &str) {}
fn never_cancelled() -> bool {
    false
}

/// Progress and cancellation for a stem run.
///
/// Separation drives an external process, so the caller can stop it: the
/// worker polls `cancelled` while the child runs and kills it, after which
/// the temporary directory is removed as usual.
pub struct StemRun<'a> {
    /// Reports a stage id: `"separating"` or `"denoising"`.
    pub stage: &'a dyn Fn(&str),
    /// Polled while the separator runs; `true` stops it.
    pub cancelled: &'a dyn Fn() -> bool,
}

impl<'a> StemRun<'a> {
    /// A run that reports nothing and cannot be cancelled.
    pub fn quiet() -> Self {
        Self {
            stage: &no_stage,
            cancelled: &never_cancelled,
        }
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

    /// Whether every stage `mode` asks for is installed.
    pub fn supports_stems(&self, mode: StemMode) -> bool {
        if mode.needs_separation() {
            self.separator.is_some()
        } else {
            self.denoise_model.is_some()
        }
    }

    /// Run the requested tool stage on mono `samples` at `rate` and return
    /// the stems. Unlike [`Enhancer::apply`], the instrumental stem is kept
    /// whenever separation ran.
    pub fn apply_stems(&self, mode: StemMode, samples: &[f32], rate: u32) -> Result<Stems> {
        self.apply_stems_with(mode, samples, rate, &StemRun::quiet())
    }

    /// [`Enhancer::apply_stems`] with stage reporting and cancellation.
    pub fn apply_stems_with(
        &self,
        mode: StemMode,
        samples: &[f32],
        rate: u32,
        run: &StemRun,
    ) -> Result<Stems> {
        let input = Audio::mono(samples.to_vec(), rate.max(1) as i32);
        if samples.is_empty() {
            return Ok(Stems {
                vocals: input,
                instrumental: None,
            });
        }
        if !self.supports_stems(mode) {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::EnhanceModelsMissing,
            )));
        }
        if (run.cancelled)() {
            return Err(CoreError::Synth("vocal separation cancelled".into()));
        }
        if !mode.needs_separation() {
            (run.stage)("denoising");
            return Ok(Stems {
                vocals: self.denoise(&input)?,
                instrumental: None,
            });
        }
        (run.stage)("separating");
        let (vocals, instrumental) = self.separate_stems(&input, run)?;
        Ok(Stems {
            vocals,
            instrumental: Some(instrumental),
        })
    }

    /// Run the requested stages on mono `samples` at `rate`, returning
    /// audio at the same rate.
    pub fn apply(&self, mode: EnhanceMode, samples: &[f32], rate: u32) -> Result<Audio> {
        if mode.is_off() || samples.is_empty() {
            return Ok(Audio::mono(samples.to_vec(), rate as i32));
        }
        if !self.supports(mode) {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::EnhanceModelsMissing,
            )));
        }

        let mut audio = Audio::mono(samples.to_vec(), rate as i32);
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
        Ok(Audio::mono(samples, audio.sample_rate))
    }

    fn separate(&self, audio: &Audio) -> Result<Audio> {
        let (vocals, _) = self.separate_stems(audio, &StemRun::quiet())?;
        Ok(vocals)
    }

    fn separate_stems(&self, audio: &Audio, run: &StemRun) -> Result<(Audio, Audio)> {
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
        let result = self.run_separation(separator, &dir, audio, run);
        // Cancellation kills the child but leaves whatever it wrote behind;
        // the directory goes away on every path, success or not.
        let _ = fs::remove_dir_all(&dir);
        result
    }

    fn run_separation(
        &self,
        separator: &Separator,
        dir: &Path,
        audio: &Audio,
        run: &StemRun,
    ) -> Result<(Audio, Audio)> {
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
                None if (run.cancelled)() => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(CoreError::Synth("vocal separation cancelled".into()));
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

        let vocals = read_stem(&out_dir, "vocals.wav", audio.sample_rate)?;
        let instrumental = read_stem(&out_dir, "instrumental.wav", audio.sample_rate)?;
        Ok((vocals, instrumental))
    }
}

/// Read one separator output and bring it back to the clip's sample rate.
fn read_stem(dir: &Path, name: &str, target_rate: i32) -> Result<Audio> {
    let path = dir.join(name);
    let bytes = fs::read(&path)
        .map_err(|e| CoreError::Synth(format!("the separator produced no {name}: {e}")))?;
    let (samples, stem_rate) = decode_wav(&bytes)?;
    Ok(Audio::mono(
        resample_mono(&samples, stem_rate.max(1) as u32, target_rate.max(1) as u32),
        target_rate,
    ))
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
    fn parses_stem_modes_and_routes_outputs() {
        for (raw, mode) in [
            ("denoise", StemMode::Denoise),
            ("Noise", StemMode::Denoise),
            ("vocals", StemMode::Vocals),
            ("voice", StemMode::Vocals),
            ("instrumental", StemMode::Instrumental),
            ("music", StemMode::Instrumental),
            ("both", StemMode::Both),
            ("stems", StemMode::Both),
        ] {
            assert_eq!(StemMode::parse(raw), Some(mode), "{raw:?}");
            assert_eq!(StemMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(StemMode::parse("off"), None);

        assert!(!StemMode::Denoise.needs_separation());
        assert!(!StemMode::Denoise.wants_vocals());
        assert!(StemMode::Vocals.needs_separation() && StemMode::Vocals.wants_vocals());
        assert!(!StemMode::Vocals.wants_instrumental());
        assert!(StemMode::Instrumental.needs_separation());
        assert!(StemMode::Instrumental.wants_instrumental());
        assert!(StemMode::Both.wants_vocals() && StemMode::Both.wants_instrumental());
    }

    #[test]
    fn apply_stems_passes_empty_audio_through() {
        let enhancer = Enhancer::discover(Path::new("/nonexistent"), None);
        let stems = enhancer
            .apply_stems(StemMode::Denoise, &[], 16_000)
            .unwrap();
        assert!(stems.vocals.samples.is_empty());
        assert_eq!(stems.vocals.sample_rate, 16_000);
        assert!(stems.instrumental.is_none());
    }

    #[test]
    fn apply_stems_reports_missing_models() {
        let enhancer = Enhancer::discover(Path::new("/nonexistent"), None);
        for mode in [StemMode::Denoise, StemMode::Both] {
            let err = enhancer.apply_stems(mode, &[0.1, 0.2], 16_000).unwrap_err();
            assert!(matches!(err, CoreError::UserError(_)), "{mode:?}: {err:?}");
        }
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
