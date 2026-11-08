//! SRT and WebVTT rendering for timed transcripts.
//!
//! Both formats share the same cue list; only the header and the decimal
//! separator of the timestamps differ. Empty cues are skipped so numbering
//! stays contiguous.

use laverock_core::TranscriptSegment;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Srt,
    Vtt,
}

impl Format {
    /// `?format=` value; an empty value means SRT.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "srt" => Some(Self::Srt),
            "vtt" => Some(Self::Vtt),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Srt => "application/x-subrip; charset=utf-8",
            Self::Vtt => "text/vtt; charset=utf-8",
        }
    }

    pub fn render(self, segments: &[TranscriptSegment]) -> String {
        match self {
            Self::Srt => to_srt(segments),
            Self::Vtt => to_vtt(segments),
        }
    }
}

pub fn to_srt(segments: &[TranscriptSegment]) -> String {
    let mut out = String::new();
    for (index, segment) in cues(segments).enumerate() {
        let (start, end) = bounds(segment);
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            index + 1,
            stamp(start, ','),
            stamp(end, ','),
            segment.text.trim()
        ));
    }
    out
}

pub fn to_vtt(segments: &[TranscriptSegment]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for segment in cues(segments) {
        let (start, end) = bounds(segment);
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            stamp(start, '.'),
            stamp(end, '.'),
            segment.text.trim()
        ));
    }
    out
}

fn cues(segments: &[TranscriptSegment]) -> impl Iterator<Item = &TranscriptSegment> {
    segments
        .iter()
        .filter(|segment| !segment.text.trim().is_empty())
}

/// Parse SRT or WebVTT text into cues.
///
/// The two formats share their cue syntax closely enough that one tolerant
/// parser handles both: an optional identifier/index line, a timestamp line
/// with `-->`, then the text. Unparseable blocks (the `WEBVTT` header,
/// notes, styling) are skipped rather than failing the import.
pub fn parse_cues(text: &str) -> Vec<TranscriptSegment> {
    let text = text
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        let Some((index, arrow)) = lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains("-->"))
            .map(|(index, line)| (index, *line))
        else {
            continue;
        };
        let Some((start_raw, rest)) = arrow.split_once("-->") else {
            continue;
        };
        // WebVTT allows cue settings after the end timestamp.
        let end_raw = rest.split_whitespace().next().unwrap_or("");
        let (Some(start), Some(end)) = (parse_stamp(start_raw), parse_stamp(end_raw)) else {
            continue;
        };
        if end < start {
            continue;
        }
        let body = lines[index + 1..].join("\n");
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        out.push(TranscriptSegment {
            start: start as f32,
            end: end as f32,
            text: body.to_string(),
        });
    }
    out
}

/// `HH:MM:SS,mmm`, `HH:MM:SS.mmm` or `MM:SS.mmm` → seconds.
fn parse_stamp(raw: &str) -> Option<f64> {
    let parts: Vec<&str> = raw.trim().split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [hours, minutes, seconds] => (
            hours.parse::<u32>().ok()?,
            minutes.parse::<u32>().ok()?,
            *seconds,
        ),
        [minutes, seconds] => (0, minutes.parse::<u32>().ok()?, *seconds),
        _ => return None,
    };
    let seconds: f64 = seconds.replace(',', ".").parse().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some(hours as f64 * 3600.0 + minutes as f64 * 60.0 + seconds)
}

/// Clamp a cue to sane bounds: no negatives, never backwards.
fn bounds(segment: &TranscriptSegment) -> (f32, f32) {
    let start = segment.start.max(0.0);
    (start, segment.end.max(start))
}

/// `HH:MM:SS<separator>mmm`; hours are not capped at 99.
fn stamp(seconds: f32, separator: char) -> String {
    let total_ms = (f64::from(seconds.max(0.0)) * 1000.0).round() as u64;
    let millis = total_ms % 1000;
    let total_secs = total_ms / 1000;
    format!(
        "{:02}:{:02}:{:02}{separator}{:03}",
        total_secs / 3600,
        (total_secs / 60) % 60,
        total_secs % 60,
        millis
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(start: f32, end: f32, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start,
            end,
            text: text.to_string(),
        }
    }

    #[test]
    fn timestamps_use_srt_and_vtt_separators() {
        assert_eq!(stamp(0.0, ','), "00:00:00,000");
        assert_eq!(stamp(1.5, ','), "00:00:01,500");
        assert_eq!(stamp(3661.234, '.'), "01:01:01.234");
        // Rounding carries into the next second, minute and hour.
        assert_eq!(stamp(59.9996, ','), "00:01:00,000");
        assert_eq!(stamp(3599.9999, '.'), "01:00:00.000");
        assert_eq!(stamp(3661.5, '.'), "01:01:01.500");
        assert_eq!(stamp(-3.0, ','), "00:00:00,000");
    }

    #[test]
    fn srt_numbers_cues_and_skips_empty_text() {
        let segments = [
            segment(0.0, 1.0, "你好，世界。"),
            segment(1.1, 1.4, "   "),
            segment(1.5, 2.5, "Second line"),
        ];
        let srt = to_srt(&segments);
        assert_eq!(
            srt,
            "1\n00:00:00,000 --> 00:00:01,000\n你好，世界。\n\n\
             2\n00:00:01,500 --> 00:00:02,500\nSecond line\n\n"
        );
        assert!(!srt.contains("   "));
    }

    #[test]
    fn vtt_starts_with_the_required_header() {
        let segments = [segment(0.25, 1.75, "hello")];
        let vtt = to_vtt(&segments);
        assert!(vtt.starts_with("WEBVTT\n\n"));
        assert!(vtt.contains("00:00:00.250 --> 00:00:01.750\nhello\n\n"));
    }

    #[test]
    fn backwards_cues_render_as_zero_length() {
        let segments = [segment(2.0, 1.0, "x")];
        assert_eq!(to_srt(&segments), "1\n00:00:02,000 --> 00:00:02,000\nx\n\n");
    }

    #[test]
    fn format_parsing_is_forgiving() {
        assert_eq!(Format::parse(""), Some(Format::Srt));
        assert_eq!(Format::parse("SRT"), Some(Format::Srt));
        assert_eq!(Format::parse(" VTT "), Some(Format::Vtt));
        assert_eq!(Format::parse("ass"), None);
    }

    #[test]
    fn parses_srt_cues_and_keeps_multiline_text() {
        let srt = "\u{feff}1\r\n00:00:00,000 --> 00:00:01,500\r\n你好\r\n世界\r\n\r\n\
                   2\r\n00:00:02,250 --> 00:00:04,000\r\nSecond\r\n\r\n";
        let cues = parse_cues(srt);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start, 0.0);
        assert_eq!(cues[0].end, 1.5);
        assert_eq!(cues[0].text, "你好\n世界");
        assert_eq!(cues[1].start, 2.25);
        assert_eq!(cues[1].end, 4.0);
        assert_eq!(cues[1].text, "Second");
    }

    #[test]
    fn parses_vtt_cue_ids_settings_and_short_stamps() {
        let vtt = "WEBVTT\n\nNOTE this is ignored\n\n\
                   intro\n00:01.000 --> 00:02.500 align:start\nhello\n\n\
                   00:00:03.000 --> 00:00:04.000\nbye\n";
        let cues = parse_cues(vtt);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start, 1.0);
        assert_eq!(cues[0].end, 2.5);
        assert_eq!(cues[0].text, "hello");
        assert_eq!(cues[1].text, "bye");
    }

    #[test]
    fn skips_unparseable_blocks_and_backwards_cues() {
        let cues = parse_cues(
            "1\nnot a timestamp\nhello\n\n2\n00:00:05,000 --> 00:00:01,000\nbackwards\n\n",
        );
        assert!(cues.is_empty());
    }

    #[test]
    fn rendering_round_trips_through_the_parser() {
        let original = [segment(0.0, 1.0, "one"), segment(1.5, 2.25, "two\nlines")];
        for format in [Format::Srt, Format::Vtt] {
            let parsed = parse_cues(&format.render(&original));
            assert_eq!(parsed.len(), original.len(), "{format:?}");
            for (got, want) in parsed.iter().zip(&original) {
                assert!((got.start - want.start).abs() < 0.001, "{format:?}");
                assert!((got.end - want.end).abs() < 0.001, "{format:?}");
                assert_eq!(got.text, want.text, "{format:?}");
            }
        }
    }
}
