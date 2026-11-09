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
/// Streaming decode reports its cues in chunks of roughly this length.
const STREAM_CHUNK_SECS: f32 = 12.0;
/// Hard cap for one streaming decode when speech never pauses.
const STREAM_MAX_CHUNK_SECS: f32 = 20.0;
/// A pause at least this long ends a streaming chunk early.
const STREAM_GAP_SECS: f32 = 0.35;
/// Quiet window used to cut continuous speech without slicing a word.
const STREAM_CUT_WINDOW_SECS: f32 = 0.03;
/// How far back from the hard cap the quiet cut may search.
const STREAM_CUT_SEARCH_SECS: f32 = 2.0;

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

    /// Transcribe a clip in speech-sized chunks, reporting each finished
    /// cue as soon as it is decoded. The first cues therefore appear after
    /// a few seconds even for a long recording, instead of after one full
    /// pass. The returned transcript has the same shape as
    /// [`Self::transcribe_detailed`]; its text is assembled from the cues
    /// because every chunk is decoded on its own.
    pub fn transcribe_progressive<F>(&self, wav: &[u8], mut on_segment: F) -> Result<Transcript>
    where
        F: FnMut(&TranscriptSegment),
    {
        let (samples, sample_rate) = decode_clip(wav)?;
        self.with_recognizer(|recognizer| {
            let rate = sample_rate.max(1);
            let duration = samples.len() as f32 / rate as f32;
            let mut segments = Vec::new();
            for (start, end) in stream_chunks(&samples, sample_rate.max(1) as u32) {
                let offset = start as f32 / rate as f32;
                let chunk_duration = (end - start) as f32 / rate as f32;
                let chunk = &samples[start..end];
                let (text, tokens, times) = match recognizer.run(chunk, rate) {
                    Some(result) => (
                        result.text.trim().to_string(),
                        result.tokens,
                        result.timestamps,
                    ),
                    None => (String::new(), Vec::new(), None),
                };
                let mut cues = match times {
                    Some(times) if !tokens.is_empty() && times.len() == tokens.len() => {
                        segments_from_tokens(&tokens, &times, chunk_duration)
                    }
                    _ => recognizer.segments_by_speech(chunk, rate, &text, chunk_duration),
                };
                for cue in &mut cues {
                    cue.start = (cue.start + offset).min(duration);
                    cue.end = (cue.end + offset).min(duration);
                }
                for cue in cues {
                    if cue.text.trim().is_empty() {
                        continue;
                    }
                    on_segment(&cue);
                    segments.push(cue);
                }
            }
            let text = join_text(segments.iter().map(|segment| segment.text.as_str()));
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

/// Split a clip into streaming decode chunks at pauses, merging short
/// speech runs up to the target size. Continuous speech is cut near the
/// hard cap at its quietest window so a word is not sliced.
fn stream_chunks(samples: &[f32], sample_rate: u32) -> Vec<(usize, usize)> {
    let rate = sample_rate.max(1) as f32;
    let target = (STREAM_CHUNK_SECS * rate) as usize;
    let hard = (STREAM_MAX_CHUNK_SECS * rate) as usize;
    let ranges = crate::audio::speech_chunks(samples, sample_rate, STREAM_GAP_SECS);
    if ranges.is_empty() {
        return (!samples.is_empty())
            .then_some((0, samples.len()))
            .into_iter()
            .collect();
    }

    let mut chunks = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    for (start, end) in ranges {
        match open {
            Some((chunk_start, _)) if end - chunk_start <= target => {
                open = Some((chunk_start, end));
            }
            Some((chunk_start, chunk_end)) => {
                push_chunk(&mut chunks, samples, chunk_start, chunk_end, hard, rate);
                open = Some((start, end));
            }
            None => open = Some((start, end)),
        }
    }
    if let Some((start, end)) = open {
        push_chunk(&mut chunks, samples, start, end, hard, rate);
    }
    chunks
}

/// Append one chunk, splitting it at quiet windows until it fits `hard`.
fn push_chunk(
    out: &mut Vec<(usize, usize)>,
    samples: &[f32],
    start: usize,
    end: usize,
    hard: usize,
    rate: f32,
) {
    let mut pos = start;
    while end.saturating_sub(pos) > hard {
        let cut = quiet_cut(samples, pos, pos + hard, rate);
        out.push((pos, cut));
        pos = cut;
    }
    if end > pos {
        out.push((pos, end));
    }
}

/// The quietest 30 ms window in the last two seconds before `cap`, so a
/// forced cut lands in a gap between words when one exists.
fn quiet_cut(samples: &[f32], start: usize, cap: usize, rate: f32) -> usize {
    let cap = cap.min(samples.len());
    let window = ((STREAM_CUT_WINDOW_SECS * rate) as usize).max(1);
    let search = (STREAM_CUT_SEARCH_SECS * rate) as usize;
    let from = cap.saturating_sub(search).max(start + window);
    let mut best = cap;
    let mut best_energy = f32::INFINITY;
    let mut pos = from;
    while pos + window <= cap {
        let energy: f32 = samples[pos..pos + window]
            .iter()
            .map(|s| s.abs())
            .sum::<f32>()
            / window as f32;
        // `<=` keeps the latest window on a tie, maximizing retained length.
        if energy <= best_energy {
            best_energy = energy;
            best = pos + window / 2;
        }
        pos += window;
    }
    best.clamp(start + 1, cap)
}

/// Join per-cue text. CJK cues need no separator; Latin ones do.
fn join_text<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let (Some(previous), Some(next)) = (out.chars().last(), part.chars().next()) {
            if !is_cjk(previous) && !is_cjk(next) && !previous.is_whitespace() {
                out.push(' ');
            }
        }
        out.push_str(part);
    }
    out
}

/// Whether a character belongs to a script written without word spaces.
fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3000..=0x303F // CJK punctuation
            | 0x3040..=0x30FF // kana
            | 0x3400..=0x4DBF // CJK extension A
            | 0x4E00..=0x9FFF // CJK unified ideographs
            | 0xF900..=0xFAFF // compatibility ideographs
            | 0xFF00..=0xFFEF // full-width forms
    )
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

    fn tone(secs: f32, rate: u32) -> Vec<f32> {
        (0..(secs * rate as f32) as usize)
            .map(|i| (i as f32 * 0.05).sin() * 0.3)
            .collect()
    }

    #[test]
    fn streaming_chunks_split_on_pauses_and_cap_continuous_speech() {
        let rate = 16_000u32;

        // Three utterances separated by a second of silence.
        let mut samples = Vec::new();
        for _ in 0..3 {
            samples.extend(tone(4.0, rate));
            samples.extend(vec![0.0f32; rate as usize]);
        }
        let chunks = stream_chunks(&samples, rate);
        assert!(chunks.len() >= 2, "pauses should end chunks: {chunks:?}");
        for pair in chunks.windows(2) {
            assert!(pair[0].1 <= pair[1].0, "chunks overlap: {chunks:?}");
        }

        // Continuous speech has no pause to cut at, so the hard cap applies
        // and the quiet cut still keeps every chunk within it.
        let long = stream_chunks(&tone(45.0, rate), rate);
        let hard = (STREAM_MAX_CHUNK_SECS * rate as f32) as usize;
        assert_eq!(long.len(), 3);
        for (start, end) in &long {
            assert!(end > start);
            assert!(end - start <= hard);
            assert!(*end <= (45.0 * rate as f32) as usize);
        }
        assert!(long.last().unwrap().1 > (44.0 * rate as f32) as usize);
    }

    #[test]
    fn streaming_text_joins_cjk_without_spaces() {
        assert_eq!(join_text(["你好。", "世界。"].into_iter()), "你好。世界。");
        assert_eq!(join_text(["Hello.", "World."].into_iter()), "Hello. World.");
        assert_eq!(join_text(["", "  "].into_iter()), "");
    }
}
