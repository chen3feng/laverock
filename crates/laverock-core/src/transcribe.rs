//! On-device speech-to-text for reference clips and subtitles.
//!
//! A recorder clip is normally transcribed by hand, which is the most
//! tedious step of the cloning flow. The transcriber wraps sherpa-onnx's
//! offline recognizer (SenseVoice: zh/en/ja/ko/yue, with inverse text
//! normalization), loading the model lazily on first use from a directory
//! whose manifest is `kind = "stt"`.
//!
//! [`Transcriber::transcribe`] keeps the original text-only contract used
//! for prompt texts. [`Transcriber::transcribe_detailed`] returns timed
//! segments for subtitles: SenseVoice's CTC decoder reports one timestamp
//! per token, which is aggregated into sentence-sized segments here. When
//! a model reports no token timestamps, the audio is split on pauses and
//! each chunk is recognized on its own instead.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineRecognizerResult,
    OfflineSenseVoiceModelConfig,
};

use crate::i18n::{self, Key};
use crate::{audio::decode_wav, manifest::Manifest, CoreError, Result};

/// One subtitle cue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Seconds from the start of the clip.
    pub start: f32,
    /// Seconds from the start of the clip; never before `start`.
    pub end: f32,
    pub text: String,
}

/// A transcript with timings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// Full transcript, same shape as the text-only endpoint.
    pub text: String,
    /// Sentence-sized cues, in order and without overlaps.
    pub segments: Vec<TranscriptSegment>,
    /// Model language, when the manifest pins one (`auto` reports none).
    pub language: Option<String>,
    /// Length of the decoded clip, in seconds.
    pub duration: f32,
}

/// Holds an optional loaded recognizer; one model at a time.
pub struct Transcriber {
    models_roots: Vec<PathBuf>,
    inner: Mutex<Option<Recognizer>>,
}

struct Recognizer {
    recognizer: OfflineRecognizer,
    /// `None` when the manifest uses `auto`.
    language: Option<String>,
}

/// Longest cue before it is force-closed, even across no punctuation.
const MAX_SEGMENT_SECS: f32 = 8.0;
/// Soft character budget for one cue (CJK characters count as one).
const MAX_SEGMENT_CHARS: usize = 48;
/// A pause between tokens at least this long starts a new cue.
const PAUSE_BREAK_SECS: f32 = 0.8;
/// Estimated speech after a cue's last token, when the next token is far.
const TOKEN_TAIL_SECS: f32 = 0.35;
/// A comma closes a cue once it already runs this long.
const COMMA_BREAK_SECS: f32 = 3.0;
/// Pauses at least this long split speech chunks in the fallback path.
const FALLBACK_GAP_SECS: f32 = 0.45;

impl Transcriber {
    pub fn new(models_root: impl AsRef<Path>) -> Self {
        Self::new_all(vec![models_root.as_ref().to_path_buf()])
    }

    /// Search several models directories for the STT model, highest
    /// priority first (the same layering as the engine registry).
    pub fn new_all(models_roots: Vec<PathBuf>) -> Self {
        Self {
            models_roots,
            inner: Mutex::new(None),
        }
    }

    /// Whether an STT model directory exists (cheap, no model load).
    pub fn available(&self) -> bool {
        model_manifest_in(&self.models_roots).is_ok()
    }

    /// Manifest id and display name of the installed recognizer, for
    /// labelling results in history.
    pub fn model_info(&self) -> Option<(String, String)> {
        let (manifest, _) = model_manifest_in(&self.models_roots).ok()?;
        Some((manifest.id, manifest.name))
    }

    /// Transcribe a WAV clip to text, loading the model on first call.
    pub fn transcribe(&self, wav: &[u8]) -> Result<String> {
        let (samples, sample_rate) = decode_clip(wav)?;
        self.with_recognizer(|recognizer| Ok(recognizer.text(&samples, sample_rate)))
    }

    /// Transcribe with per-segment timings, for subtitles.
    pub fn transcribe_detailed(&self, wav: &[u8]) -> Result<Transcript> {
        let (samples, sample_rate) = decode_clip(wav)?;
        self.with_recognizer(|recognizer| {
            let duration = samples.len() as f32 / sample_rate.max(1) as f32;
            let result = recognizer.run(&samples, sample_rate);
            let (text, tokens, times) = match result {
                Some(result) => (
                    result.text.trim().to_string(),
                    result.tokens,
                    result.timestamps,
                ),
                None => (String::new(), Vec::new(), None),
            };

            let segments = match times {
                Some(times) if !tokens.is_empty() && times.len() == tokens.len() => {
                    segments_from_tokens(&tokens, &times, duration)
                }
                _ => recognizer.segments_by_speech(&samples, sample_rate, &text, duration),
            };

            Ok(Transcript {
                text,
                segments,
                language: recognizer.language.clone(),
                duration,
            })
        })
    }

    /// Run `f` with the model loaded, keeping load and decode under the
    /// same lock so two requests never load twice.
    fn with_recognizer<T>(&self, f: impl FnOnce(&Recognizer) -> Result<T>) -> Result<T> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| CoreError::Load("transcriber lock poisoned".into()))?;
        if guard.is_none() {
            *guard = Some(Recognizer::load(&self.models_roots)?);
        }
        f(guard.as_ref().expect("loaded above"))
    }
}

impl Recognizer {
    fn load(models_roots: &[PathBuf]) -> Result<Self> {
        let (manifest, dir) = model_manifest_in(models_roots)?;
        let stt = manifest
            .stt
            .as_ref()
            .ok_or_else(|| CoreError::Load("stt manifest missing an [stt] section".into()))?;
        if stt.engine != "sense_voice" {
            return Err(CoreError::Load(format!(
                "unsupported STT engine: {}",
                stt.engine
            )));
        }

        let model = dir.join(&stt.model);
        let tokens = dir.join(&stt.tokens);
        for path in [&model, &tokens] {
            if !path.exists() {
                return Err(CoreError::Load(format!(
                    "missing STT file {}",
                    path.display()
                )));
            }
        }

        let mut config = OfflineRecognizerConfig::default();
        config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(model.to_string_lossy().into_owned()),
            language: Some(stt.language.clone()),
            use_itn: stt.use_itn,
        };
        config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
        config.model_config.num_threads = stt.num_threads.max(1);
        config.model_config.debug = false;

        let recognizer = crate::onnx::with_fallback("sensevoice", |provider| {
            config.model_config.provider = Some(provider.to_string());
            OfflineRecognizer::create(&config)
        })
        .ok_or_else(|| CoreError::Load("failed to create the SenseVoice recognizer".into()))?;
        let language =
            (stt.language != "auto" && !stt.language.is_empty()).then(|| stt.language.clone());
        Ok(Self {
            recognizer,
            language,
        })
    }

    fn run(&self, samples: &[f32], sample_rate: i32) -> Option<OfflineRecognizerResult> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(sample_rate, samples);
        self.recognizer.decode(&stream);
        stream.get_result()
    }

    fn text(&self, samples: &[f32], sample_rate: i32) -> String {
        self.run(samples, sample_rate)
            .map(|result| result.text.trim().to_string())
            .unwrap_or_default()
    }

    /// Fallback for models without token timestamps: split the clip on
    /// pauses, recognize each chunk, and use the chunk bounds as its cue.
    fn segments_by_speech(
        &self,
        samples: &[f32],
        sample_rate: i32,
        full_text: &str,
        duration: f32,
    ) -> Vec<TranscriptSegment> {
        let rate = sample_rate.max(1) as f32;
        let chunks =
            crate::audio::speech_chunks(samples, sample_rate.max(1) as u32, FALLBACK_GAP_SECS);
        let mut segments = Vec::new();
        if chunks.len() > 1 {
            for (start, end) in chunks {
                let text = self.text(&samples[start..end], sample_rate);
                if text.is_empty() {
                    continue;
                }
                segments.push(TranscriptSegment {
                    start: start as f32 / rate,
                    end: (end as f32 / rate).min(duration),
                    text,
                });
            }
        }
        if segments.is_empty() && !full_text.is_empty() {
            let (start, end) = speech_bounds(samples, sample_rate.max(1) as u32, duration);
            segments.push(TranscriptSegment {
                start,
                end: end.max(start + 0.05).min(duration),
                text: full_text.to_string(),
            });
        }
        monotonic(segments, duration)
    }
}

/// Decode and reject empty clips, the one error every caller reports.
fn decode_clip(wav: &[u8]) -> Result<(Vec<f32>, i32)> {
    let (samples, sample_rate) = decode_wav(wav)?;
    if samples.is_empty() {
        return Err(CoreError::UserError(i18n::Message::new(Key::EmptyClip)));
    }
    Ok((samples, sample_rate))
}

/// Leading/trailing speech bounds in seconds, from energy alone.
fn speech_bounds(samples: &[f32], sample_rate: u32, duration: f32) -> (f32, f32) {
    let profile = crate::audio::silence_profile(samples, sample_rate);
    let start = profile.leading_secs.clamp(0.0, duration);
    let end = (duration - profile.trailing_secs).max(start);
    (start, end)
}

/// Aggregate token timestamps into sentence-sized cues.
fn segments_from_tokens(tokens: &[String], times: &[f32], duration: f32) -> Vec<TranscriptSegment> {
    let at = |i: usize| times[i].clamp(0.0, duration);
    let mut out = Vec::new();
    let mut text = String::new();
    let mut start = 0.0f32;
    let mut last = 0.0f32;
    let mut open = false;

    for (i, token) in tokens.iter().enumerate() {
        let piece = token.replace('\u{2581}', " "); // SentencePiece word marker
        if !open {
            if piece.trim().is_empty() {
                continue;
            }
            start = at(i);
            open = true;
        }
        text.push_str(&piece);
        last = at(i);

        let next = times.get(i + 1).map(|_| at(i + 1));
        let gap_break = next.is_some_and(|next| next - last >= PAUSE_BREAK_SECS);
        let soft_break = last - start >= COMMA_BREAK_SECS && ends_with_comma(&text);
        let too_long =
            last - start >= MAX_SEGMENT_SECS || text.trim().chars().count() >= MAX_SEGMENT_CHARS;
        if closes_sentence(&text) || gap_break || soft_break || too_long {
            let end = match next {
                Some(next) if next - last <= 0.6 => next,
                _ => last + TOKEN_TAIL_SECS,
            };
            out.push(TranscriptSegment {
                start,
                end: end.min(duration),
                text: clean(&text),
            });
            text.clear();
            open = false;
        }
    }
    if open && !text.trim().is_empty() {
        out.push(TranscriptSegment {
            start,
            end: (last + TOKEN_TAIL_SECS).min(duration),
            text: clean(&text),
        });
    }
    out.retain(|segment| !segment.text.is_empty());
    monotonic(out, duration)
}

/// Normalize spacing left by SentencePiece markers.
fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether the accumulated text ends a sentence. A period between digits
/// is a decimal point, not a stop.
fn closes_sentence(text: &str) -> bool {
    let trimmed = text.trim_end();
    let Some(last) = trimmed.chars().last() else {
        return false;
    };
    match last {
        '。' | '！' | '？' | '!' | '?' | '…' | '；' | ';' => true,
        '.' => !trimmed
            .chars()
            .rev()
            .nth(1)
            .is_some_and(|previous| previous.is_ascii_digit()),
        _ => false,
    }
}

fn ends_with_comma(text: &str) -> bool {
    matches!(text.trim_end().chars().last(), Some('，' | ','))
}

/// Enforce non-decreasing starts and `end >= start`, clamped to the clip.
fn monotonic(mut segments: Vec<TranscriptSegment>, duration: f32) -> Vec<TranscriptSegment> {
    let mut previous = 0.0f32;
    for segment in &mut segments {
        segment.start = segment.start.clamp(previous, duration);
        segment.end = segment
            .end
            .clamp(segment.start, duration.max(segment.start));
        previous = segment.start;
    }
    segments
}

/// Find the newest STT manifest across several models directories. The
/// highest-priority directory wins; within it, the most recently installed
/// STT model wins.
pub fn model_manifest_in(models_roots: &[PathBuf]) -> Result<(Manifest, PathBuf)> {
    for root in models_roots {
        if let Ok(found) = model_manifest_one(root) {
            return Ok(found);
        }
    }
    Err(CoreError::Load(
        "no speech-to-text model installed; download SenseVoice from the Models page".into(),
    ))
}

fn model_manifest_one(models_root: &Path) -> Result<(Manifest, PathBuf)> {
    let mut found: Vec<(PathBuf, Manifest)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(models_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("engine.json");
            if let Ok(raw) = std::fs::read_to_string(&manifest_path) {
                if let Ok(manifest) = serde_json::from_str::<Manifest>(&raw) {
                    if manifest.kind.as_str() == "stt" {
                        found.push((path, manifest));
                    }
                }
            }
        }
    }
    match found.into_iter().max_by_key(|(p, _)| {
        std::fs::metadata(p.join("engine.json"))
            .and_then(|m| m.modified())
            .ok()
    }) {
        Some((dir, manifest)) => Ok((manifest, dir)),
        None => Err(CoreError::Load(
            "no speech-to-text model installed; download SenseVoice from the Models page".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|token| token.to_string()).collect()
    }

    #[test]
    fn empty_dir_has_no_model() {
        let dir = std::env::temp_dir().join(format!("laverock-stt-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(model_manifest_in(std::slice::from_ref(&dir)).is_err());
        assert!(!Transcriber::new(&dir).available());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_stt_manifest_and_skips_tts() {
        let dir = std::env::temp_dir().join(format!("laverock-stt-find-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("tts-model")).unwrap();
        std::fs::write(
            dir.join("tts-model/engine.json"),
            r#"{"id":"t","kind":"kokoro","name":"t","kokoro":{}}"#,
        )
        .unwrap();
        let stt = dir.join("stt-model");
        std::fs::create_dir_all(&stt).unwrap();
        std::fs::write(
            stt.join("engine.json"),
            r#"{"id":"s","kind":"stt","name":"s",
               "stt":{"model":"model.int8.onnx","tokens":"tokens.txt"}}"#,
        )
        .unwrap();

        let (manifest, path) = model_manifest_in(std::slice::from_ref(&dir)).unwrap();
        assert_eq!(manifest.kind, crate::manifest::EngineKind::Stt);
        assert_eq!(path, stt);
        assert!(Transcriber::new(&dir).available());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn splits_token_timestamps_at_punctuation() {
        let tokens = tokens(&["你", "好", "！", "世", "界", "。"]);
        let times = [0.0, 0.2, 0.4, 1.0, 1.2, 1.4];
        let segments = segments_from_tokens(&tokens, &times, 2.0);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "你好！");
        assert_eq!(segments[0].start, 0.0);
        assert!((segments[0].end - 1.0).abs() < 1e-6);
        assert_eq!(segments[1].text, "世界。");
        assert!((segments[1].start - 1.0).abs() < 1e-6);
    }

    #[test]
    fn splits_on_long_pauses_and_keeps_times_monotonic() {
        // No punctuation at all: the silence carries the split.
        let tokens = tokens(&["a", "b", "c", "d"]);
        let times = [0.1, 0.3, 2.0, 2.2];
        let segments = segments_from_tokens(&tokens, &times, 3.0);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "ab");
        // The long pause is dead air, so the cue ends shortly after the
        // last token instead of lingering until the next one starts.
        assert!((segments[0].end - 0.65).abs() < 1e-6);
        assert_eq!(segments[1].text, "cd");
        assert!(segments[0].start <= segments[1].start);
        assert!(segments[0].end <= segments[1].end);
        for segment in &segments {
            assert!(segment.end >= segment.start);
            assert!(segment.end <= 3.0);
        }
    }

    #[test]
    fn word_markers_become_spaces() {
        let tokens = tokens(&["\u{2581}Hello", "\u{2581}world", "."]);
        let times = [0.0, 0.4, 0.8];
        let segments = segments_from_tokens(&tokens, &times, 1.5);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello world.");
    }

    #[test]
    fn a_decimal_point_does_not_end_a_sentence() {
        assert!(!closes_sentence("3."));
        assert!(closes_sentence("done."));
        assert!(closes_sentence("好。"));
    }

    #[test]
    fn long_text_is_force_split() {
        let pieces: Vec<String> = (0..30).map(|_| "字".to_string()).collect();
        let times: Vec<f32> = (0..30).map(|i| i as f32 * 0.4).collect();
        let segments = segments_from_tokens(&pieces, &times, 12.5);
        assert!(segments.len() >= 2);
        for segment in &segments {
            assert!(segment.text.chars().count() <= MAX_SEGMENT_CHARS);
            // The cue may extend to the next token, just past the budget.
            assert!(segment.end - segment.start <= MAX_SEGMENT_SECS + 1.0);
        }
    }

    #[test]
    fn empty_token_stream_yields_no_segments() {
        assert!(segments_from_tokens(&[], &[], 1.0).is_empty());
    }
}
