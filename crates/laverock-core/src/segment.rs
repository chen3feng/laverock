//! Shared long-text segmentation.
//!
//! Engines that synthesize one utterance per call have a length at which
//! their decoder starts dropping words, repeating phrases or running out of
//! token budget. This module finds sentence boundaries (CJK full stops,
//! ASCII `. ? !` at a word end, semicolons and paragraph breaks), packs whole
//! sentences into pieces no longer than the engine's budget, and drives the
//! per-piece synthesis and the final join.
//!
//! Packing is deliberately greedy: two sentences share a piece whenever they
//! fit, so a paragraph is split as few times as its sentences allow. Only a
//! single sentence longer than the budget is cut further, first at commas,
//! colons or spaces and finally at a character boundary.

use crate::{Audio, CoreError, ProgressEvent, Result, SpeechEngine, SynthRequest};
use std::time::Duration;

/// How an engine wants long text broken up before synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LongTextPolicy {
    /// Longest piece handed to one engine call, in characters.
    pub max_chars: usize,
    /// Silence inserted between pieces, in milliseconds.
    pub pause_ms: u32,
    /// Extra delay before a follow-up call. Runtimes that crash when
    /// launched back to back (MOSS-TTS's llama.cpp build) need this.
    pub launch_pause_ms: u32,
}

/// Characters that end a sentence. `.` is handled separately so decimals and
/// abbreviations survive.
const ENDERS: &[char] = &['。', '！', '？', '；', '…', '!', '?', ';'];

/// Closing marks that belong to the sentence before them.
const TRAILERS: &[char] = &[
    '”', '’', '」', '』', '）', ')', '》', '】', '"', '\'', ']', '〕', '〉',
];

/// Punctuation that can end a clause when a single sentence is too long.
const SOFT_CUTS: &[char] = &['，', ',', '、', '：', ':', ' ', '\t', '—', '–'];

/// Common abbreviations whose trailing dot does not end a sentence.
const ABBREVIATIONS: &[&str] = &[
    "mr.", "mrs.", "ms.", "dr.", "prof.", "sr.", "jr.", "st.", "vs.", "etc.", "e.g.", "i.e.",
    "a.m.", "p.m.",
];

/// Split `text` into pieces of at most `max_chars` characters. Whitespace
/// around a cut is dropped; everything else is preserved in order.
///
/// Returns an empty vector for blank input.
pub fn split(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let mut pieces = Vec::new();
    let mut current = String::new();
    // Whether the previous sentence ended with a line break. A paragraph
    // break is kept inside the piece as a newline instead of forcing a cut,
    // so a run of short lines still needs only one engine call.
    let mut after_line_break = false;

    for (start, end) in sentence_ranges(text) {
        let raw = &text[start..end];
        let sentence = raw.trim();
        if sentence.is_empty() {
            continue;
        }
        let breaks_line = raw.ends_with('\n');
        let chars = sentence.chars().count();
        if chars > max_chars {
            flush(&mut pieces, &mut current);
            for part in split_overlong(sentence, max_chars) {
                pieces.push(part);
            }
            after_line_break = breaks_line;
            continue;
        }
        let separator = if current.is_empty() {
            0
        } else if after_line_break {
            1
        } else {
            spacer(&current, sentence)
        };
        if !current.is_empty() && current.chars().count() + separator + chars > max_chars {
            flush(&mut pieces, &mut current);
        }
        if !current.is_empty() {
            if after_line_break {
                current.push('\n');
            } else if spacer(&current, sentence) > 0 {
                current.push(' ');
            }
        }
        current.push_str(sentence);
        after_line_break = breaks_line;
    }
    flush(&mut pieces, &mut current);
    pieces
}

/// Synthesize a request piece by piece when the engine declares a
/// [`LongTextPolicy`] and the text is over its budget. Returns the engine's
/// single call unchanged otherwise.
pub fn synthesize_long(
    engine: &dyn SpeechEngine,
    req: &SynthRequest,
    policy: LongTextPolicy,
) -> Result<Audio> {
    let segments = split(&req.text, policy.max_chars);
    if segments.len() < 2 {
        // The engine may still take tens of seconds to load its weights;
        // keep the UI from showing a bare spinner.
        if let Some(progress) = &req.progress {
            progress(ProgressEvent::Stage(format!(
                "Synthesizing {}",
                engine.name()
            )));
        }
        return engine.synthesize(req);
    }

    let total = segments.len();
    let mut merged: Option<Audio> = None;
    for (index, segment) in segments.into_iter().enumerate() {
        if index > 0 && policy.launch_pause_ms > 0 {
            // Some runtimes corrupt their heap when started back to back.
            std::thread::sleep(Duration::from_millis(u64::from(policy.launch_pause_ms)));
        }
        if let Some(progress) = &req.progress {
            progress(ProgressEvent::Stage(format!(
                "Synthesizing segment {}/{}",
                index + 1,
                total
            )));
        }
        let mut piece_req = req.clone();
        piece_req.text = segment;
        let audio = engine.synthesize(&piece_req)?;
        match &mut merged {
            None => merged = Some(audio),
            Some(merged) => join(merged, audio, policy.pause_ms)?,
        }
    }
    merged.ok_or_else(|| CoreError::Synth("long-text split produced no segments".into()))
}

/// Append `part` to `merged`, leaving `pause_ms` of silence between them.
fn join(merged: &mut Audio, part: Audio, pause_ms: u32) -> Result<()> {
    if merged.sample_rate != part.sample_rate {
        return Err(CoreError::Synth(format!(
            "engine returned mixed sample rates ({} Hz then {} Hz)",
            merged.sample_rate, part.sample_rate
        )));
    }
    if pause_ms > 0 {
        let samples =
            (merged.sample_rate.max(0) as f64 * f64::from(pause_ms) / 1000.0).round() as usize;
        merged.samples.extend(std::iter::repeat_n(0.0, samples));
    }
    merged.samples.extend_from_slice(&part.samples);
    Ok(())
}

fn flush(pieces: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        pieces.push(trimmed.to_string());
    }
    current.clear();
}

/// Whether concatenating two sentences needs a space between them. CJK text
/// reads without one; Latin words would collide (`end.Start`).
fn spacer(left: &str, right: &str) -> usize {
    let last = left.chars().last();
    let first = right.chars().next();
    let latin_start = first.is_some_and(|c| c.is_ascii_alphanumeric());
    let last_has_space = last.is_some_and(char::is_whitespace);
    if latin_start && !last_has_space {
        1
    } else {
        0
    }
}

/// Byte ranges of sentences in `text`. The range ends after any closing
/// quotes, brackets or repeated punctuation attached to the sentence.
fn sentence_ranges(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        let (byte, ch) = chars[i];
        let ends =
            ch == '\n' || ENDERS.contains(&ch) || (ch == '.' && ascii_period_ends(text, &chars, i));
        if !ends {
            i += 1;
            continue;
        }
        let mut end = byte + ch.len_utf8();
        // Attach closing marks and any further sentence punctuation.
        let mut j = i + 1;
        while j < chars.len() {
            let (next_byte, next) = chars[j];
            let attached =
                TRAILERS.contains(&next) || ENDERS.contains(&next) || (next == '.' && ch == '.');
            if !attached {
                break;
            }
            end = next_byte + next.len_utf8();
            j += 1;
        }
        if !text[start..end].trim().is_empty() {
            ranges.push((start, end));
        }
        start = end;
        i = j;
    }
    if start < text.len() && !text[start..].trim().is_empty() {
        ranges.push((start, text.len()));
    }
    ranges
}

/// Whether the `.` at `chars[i]` closes a sentence. A dot inside a number
/// (`3.14`) or directly followed by more text is a decimal or an initial;
/// common abbreviations (`Mr.`) stay with the following word.
fn ascii_period_ends(text: &str, chars: &[(usize, char)], i: usize) -> bool {
    match chars.get(i + 1).map(|(_, c)| *c) {
        None => !is_abbreviation(text, chars[i].0),
        Some(next) if next.is_whitespace() => !is_abbreviation(text, chars[i].0),
        _ => false,
    }
}

fn is_abbreviation(text: &str, byte: usize) -> bool {
    let head = text[..byte + 1].to_ascii_lowercase();
    ABBREVIATIONS.iter().any(|abbr| head.ends_with(abbr))
}

/// Break a sentence that is longer than the budget, preferring a comma,
/// colon or space near the limit and falling back to a hard character cut.
fn split_overlong(sentence: &str, max_chars: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = sentence;
    while rest.chars().count() > max_chars {
        let cut = choose_cut(rest, max_chars);
        if cut == 0 {
            break;
        }
        let (head, tail) = rest.split_at(cut);
        if !head.trim().is_empty() {
            parts.push(head.trim().to_string());
        }
        rest = tail.trim_start();
    }
    if !rest.trim().is_empty() {
        parts.push(rest.trim().to_string());
    }
    parts
}

fn choose_cut(text: &str, max_chars: usize) -> usize {
    let limit = text
        .char_indices()
        .nth(max_chars)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len());
    let half = text
        .char_indices()
        .nth(max_chars / 2)
        .map(|(byte, _)| byte)
        .unwrap_or(0);
    text.char_indices()
        .filter(|(byte, ch)| *byte >= half && *byte < limit && SOFT_CUTS.contains(ch))
        .map(|(byte, ch)| byte + ch.len_utf8())
        .next_back()
        .unwrap_or(limit)
}

#[cfg(test)]
mod tests {
    use super::{split, synthesize_long, LongTextPolicy};
    use crate::{Audio, Result, SpeechEngine, SynthRequest};
    use std::sync::{Arc, Mutex};

    #[test]
    fn packs_whole_sentences_up_to_the_limit() {
        // Eight characters is "句子一。句子二。", so both sentences share a
        // piece instead of each sentence starting a call of its own.
        assert_eq!(
            split("句子一。句子二。句子三。", 8),
            vec!["句子一。句子二。", "句子三。"]
        );
    }

    #[test]
    fn keeps_closing_quotes_with_their_sentence() {
        assert_eq!(
            split("他说：“好。”下一句。", 7),
            vec!["他说：“好。”", "下一句。"]
        );
    }

    #[test]
    fn hard_splits_text_without_punctuation() {
        let text = "字".repeat(450);
        let chunks = split(&text, 200);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 200));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn prefers_spaces_for_english() {
        let text = "word ".repeat(60);
        let chunks = split(&text, 200);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.chars().count() <= 200));
        for chunk in &chunks {
            assert!(chunk.starts_with("word"), "{chunk:?}");
            assert!(chunk.ends_with("word"), "{chunk:?}");
        }
        assert_eq!(
            chunks.join(" ").split_whitespace().collect::<Vec<_>>(),
            text.split_whitespace().collect::<Vec<_>>()
        );
    }

    #[test]
    fn english_sentences_keep_their_separating_space() {
        assert_eq!(
            split("Hello there. How are you? I am fine.", 200),
            vec!["Hello there. How are you? I am fine."]
        );
    }

    #[test]
    fn decimals_and_abbreviations_do_not_split() {
        assert_eq!(
            split("The value is 3.14159 today, Dr. Smith said.", 200),
            vec!["The value is 3.14159 today, Dr. Smith said."]
        );
    }

    #[test]
    fn paragraph_breaks_stay_inside_a_piece() {
        assert_eq!(split("第一段\n第二段", 200), vec!["第一段\n第二段"]);
    }

    #[test]
    fn blank_input_produces_nothing() {
        assert!(split("  \n ", 200).is_empty());
    }

    #[test]
    fn short_text_stays_in_one_piece() {
        assert_eq!(split("一句话。", 200), vec!["一句话。"]);
    }

    #[test]
    fn long_sentence_cuts_at_a_comma_when_it_can() {
        let text = format!("{}，{}。", "甲".repeat(12), "乙".repeat(12));
        let chunks = split(&text, 16);
        assert_eq!(chunks, vec!["甲".repeat(12) + "，", "乙".repeat(12) + "。"]);
    }

    /// Records every text it was asked to synthesize and returns one sample.
    struct Recorder {
        texts: Arc<Mutex<Vec<String>>>,
        sample_rate: i32,
    }

    impl SpeechEngine for Recorder {
        fn id(&self) -> &str {
            "recorder"
        }
        fn name(&self) -> &str {
            "Recorder"
        }
        fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
            self.texts.lock().unwrap().push(req.text.clone());
            Ok(Audio {
                samples: vec![1.0],
                sample_rate: self.sample_rate,
            })
        }
    }

    fn request(text: &str) -> SynthRequest {
        SynthRequest {
            text: text.to_string(),
            prompt_text: None,
            prompt_samples: Vec::new(),
            prompt_sr: 0,
            speed: 1.0,
            num_steps: 1,
            params: Default::default(),
            progress: None,
        }
    }

    #[test]
    fn segmented_synthesis_joins_with_a_pause() {
        let texts = Arc::new(Mutex::new(Vec::new()));
        let engine = Recorder {
            texts: texts.clone(),
            sample_rate: 1000,
        };
        let policy = LongTextPolicy {
            max_chars: 8,
            pause_ms: 100,
            launch_pause_ms: 0,
        };
        let audio = synthesize_long(&engine, &request("句子一。句子二。句子三。"), policy).unwrap();
        assert_eq!(*texts.lock().unwrap(), vec!["句子一。句子二。", "句子三。"]);
        // 1 + 100 samples of pause + 1.
        assert_eq!(audio.samples.len(), 102);
        assert_eq!(audio.samples[0], 1.0);
        assert_eq!(audio.samples[100], 0.0);
        assert_eq!(audio.samples[101], 1.0);
    }

    #[test]
    fn short_text_is_passed_through_once() {
        let texts = Arc::new(Mutex::new(Vec::new()));
        let engine = Recorder {
            texts: texts.clone(),
            sample_rate: 1000,
        };
        let policy = LongTextPolicy {
            max_chars: 200,
            pause_ms: 100,
            launch_pause_ms: 0,
        };
        let audio = synthesize_long(&engine, &request("一句话。"), policy).unwrap();
        assert_eq!(*texts.lock().unwrap(), vec!["一句话。"]);
        assert_eq!(audio.samples.len(), 1);
    }
}
