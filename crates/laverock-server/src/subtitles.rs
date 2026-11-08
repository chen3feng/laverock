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
}
