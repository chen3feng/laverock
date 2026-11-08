//! Per-cue dubbing: synthesize every cue with the project's engine and voice
//! and store one WAV per cue.
//!
//! This is the first half of the dubbing stage; timeline assembly and mixing
//! come next and reuse the same files. The worker is blocking (engines are),
//! so the HTTP layer runs it on the blocking pool and reports through a job.

use crate::{
    profiles::ProfileStore,
    projects::{self, Project, ProjectStore, Segment},
};
use laverock_core::{
    decode_wav, encode_wav, trim_silence, Audio, CoreError, EngineManager, Result, SynthRequest,
};

/// The reference clip handed to the engine, prepared the same way the
/// synthesize endpoint prepares an uploaded prompt.
struct Voice {
    samples: Vec<f32>,
    sample_rate: i32,
    text: Option<String>,
}

impl Voice {
    fn none() -> Self {
        Self {
            samples: Vec::new(),
            sample_rate: 0,
            text: None,
        }
    }
}

fn prepare_voice(
    profiles: &ProfileStore,
    manager: &EngineManager,
    engine_id: &str,
    voice_id: &str,
) -> Result<Voice> {
    if voice_id.trim().is_empty() {
        // Preset engines (Kokoro) need no reference at all.
        return Ok(Voice::none());
    }
    let limit = manager.reference_limit_secs(engine_id).unwrap_or(15.0);
    let combined = profiles.reference(voice_id, limit)?;
    let (samples, rate) = decode_wav(&combined.wav)?;
    let trimmed = trim_silence(&samples, rate.max(1) as u32, 0.10);
    if trimmed.is_empty() {
        return Err(CoreError::UserError(laverock_core::i18n::Message::new(
            laverock_core::i18n::Key::ReferenceSilent,
        )));
    }
    Ok(Voice {
        samples: trimmed.into_owned(),
        sample_rate: rate,
        text: Some(combined.text).filter(|text| !text.trim().is_empty()),
    })
}

/// The line to speak: the translation when there is one, else the source.
pub fn speech_text(segment: &Segment) -> &str {
    let translated = segment.translated.trim();
    if translated.is_empty() {
        segment.text.trim()
    } else {
        translated
    }
}

/// Synthesize the cues into `segments/<id>.wav`. `only` limits the run to
/// those cue ids (empty means every cue with text). Finished cues are saved
/// as they go, so cancelling or failing keeps the work already done.
///
/// Returns the number of cues synthesized.
pub fn synthesize(
    store: &ProjectStore,
    manager: &EngineManager,
    profiles: &ProfileStore,
    project: &mut Project,
    only: &[String],
    progress: &dyn Fn(usize, usize),
    cancelled: &dyn Fn() -> bool,
) -> Result<usize> {
    let engine_id = project.engine_id.trim().to_string();
    if engine_id.is_empty() {
        return Err(CoreError::BadRequest(
            "choose a synthesis engine first".into(),
        ));
    }
    let defaults = manager.manifest_defaults(&engine_id)?;
    let voice = prepare_voice(profiles, manager, &engine_id, &project.voice_id)?;
    let targets: Vec<usize> = project
        .segments
        .iter()
        .enumerate()
        .filter(|(_, segment)| !speech_text(segment).is_empty())
        .filter(|(_, segment)| only.is_empty() || only.iter().any(|id| id == &segment.id))
        .map(|(index, _)| index)
        .collect();
    let total = targets.len();
    let mut done = 0;

    for (step, index) in targets.into_iter().enumerate() {
        if cancelled() {
            break;
        }
        progress(step, total);
        let request = SynthRequest {
            text: speech_text(&project.segments[index]).to_string(),
            prompt_text: voice.text.clone(),
            prompt_samples: voice.samples.clone(),
            prompt_sr: voice.sample_rate,
            speed: defaults.speed,
            num_steps: defaults.num_steps,
            params: Default::default(),
            input_samples: Vec::new(),
            input_sr: 0,
            progress: None,
            audio: None,
        };
        let segment = &mut project.segments[index];
        let Audio {
            samples,
            sample_rate,
            ..
        } = match manager.synthesize(&engine_id, &request) {
            Ok(audio) => audio,
            Err(e) => {
                // Keep the failure on the cue and everything generated so far.
                segment.status = "failed".to_string();
                let _ = store.save(project);
                return Err(e);
            }
        };
        let wav = encode_wav(&samples, sample_rate)?;
        store.write_segment_audio(&project.id, &segment.id, &wav)?;
        segment.tts_path = projects::segment_rel_path(&segment.id);
        segment.tts_duration = samples.len() as f32 / sample_rate.max(1) as f32;
        segment.status = "done".to_string();
        done += 1;
    }

    store.save(project)?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dubs_the_translation_when_present() {
        let segment = Segment {
            id: "s1".into(),
            start: 0.0,
            end: 1.0,
            text: "hello".into(),
            translated: "你好".into(),
            ..Segment::default()
        };
        assert_eq!(speech_text(&segment), "你好");
        let source_only = Segment {
            translated: "  ".into(),
            ..segment
        };
        assert_eq!(speech_text(&source_only), "hello");
    }

    #[test]
    fn blank_cues_are_not_synthesized() {
        let segment = Segment {
            id: "s1".into(),
            start: 0.0,
            end: 1.0,
            text: "   ".into(),
            ..Segment::default()
        };
        assert_eq!(speech_text(&segment), "");
    }
}
