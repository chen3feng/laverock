//! What the user asked the app to do.
//!
//! `EngineKind` describes how a model is implemented; `TaskKind` describes
//! the operation. They diverge as the app grows: several engines can serve
//! speech synthesis, while voice conversion and transcription produce a
//! different output from the same audio pipeline.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    /// Text to speech, including cloning and voice design modes.
    #[default]
    Tts,
    /// Text (and optional lyrics) to music.
    Music,
    /// Speech to text.
    Asr,
    /// Voice conversion: audio plus a target voice to audio.
    Vc,
    /// Voice design: a description to a voice.
    Design,
    /// Audio cleanup, separation and other processing.
    Process,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::Tts => "tts",
            TaskKind::Music => "music",
            TaskKind::Asr => "asr",
            TaskKind::Vc => "vc",
            TaskKind::Design => "design",
            TaskKind::Process => "process",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "tts" => TaskKind::Tts,
            "music" => TaskKind::Music,
            "asr" => TaskKind::Asr,
            "vc" => TaskKind::Vc,
            "design" => TaskKind::Design,
            "process" => TaskKind::Process,
            _ => return None,
        })
    }

    /// History entries written before `task` existed only carry the output
    /// medium. Keep them readable by inferring the task from it.
    pub fn from_output_kind(kind: &str) -> Self {
        match kind {
            "music" => TaskKind::Music,
            "transcript" | "stt" | "text" => TaskKind::Asr,
            "vc" => TaskKind::Vc,
            _ => TaskKind::Tts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_and_parse_round_trip() {
        for task in [
            TaskKind::Tts,
            TaskKind::Music,
            TaskKind::Asr,
            TaskKind::Vc,
            TaskKind::Design,
            TaskKind::Process,
        ] {
            let raw = serde_json::to_string(&task).unwrap();
            assert_eq!(raw, format!("\"{}\"", task.as_str()));
            assert_eq!(TaskKind::parse(task.as_str()), Some(task));
            assert_eq!(serde_json::from_str::<TaskKind>(&raw).unwrap(), task);
        }
        assert_eq!(TaskKind::parse("nope"), None);
    }

    #[test]
    fn old_output_kinds_map_to_tasks() {
        assert_eq!(TaskKind::from_output_kind("speech"), TaskKind::Tts);
        assert_eq!(TaskKind::from_output_kind("music"), TaskKind::Music);
        assert_eq!(TaskKind::from_output_kind("transcript"), TaskKind::Asr);
    }
}
