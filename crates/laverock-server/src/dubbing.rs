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
    decode_wav, encode_wav, resample_mono, trim_silence, Audio, CoreError, EngineManager, Result,
    SynthRequest,
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

/// Lay the dubbed cues over the source clip.
///
/// The source becomes the background at `background_volume`; each cue's WAV
/// is placed at its start time and mixed in at `voice_volume`. With ducking
/// on, the background dips under the voice with 100 ms ramps. The result is
/// long enough for the last cue even if it runs past the source, so a long
/// dub is never cut off.
pub fn mix(store: &ProjectStore, project: &Project) -> Result<Audio> {
    let (_, source_bytes) = store.source(&project.id)?;
    let (source, rate) = decode_wav(&source_bytes)?;
    let rate = rate.max(1);
    let sample_rate = rate as u32;

    let mut voices: Vec<(usize, Vec<f32>)> = Vec::new();
    for segment in &project.segments {
        if segment.tts_path.is_empty() {
            continue;
        }
        let bytes = store.segment_audio(&project.id, &segment.id)?;
        let (samples, segment_rate) = decode_wav(&bytes)?;
        let samples = if segment_rate.max(1) as u32 != sample_rate {
            resample_mono(&samples, segment_rate.max(1) as u32, sample_rate)
        } else {
            samples
        };
        if samples.is_empty() {
            continue;
        }
        let start = (segment.start.max(0.0) * rate as f32).round() as usize;
        voices.push((start, samples));
    }
    if voices.is_empty() {
        return Err(CoreError::UserError(laverock_core::i18n::Message::new(
            laverock_core::i18n::Key::ProjectMixNoAudio,
        )));
    }

    let source_len = source
        .len()
        .max((project.source.duration * rate as f32).round() as usize);
    let voice_end = voices
        .iter()
        .map(|(start, samples)| start + samples.len())
        .max()
        .unwrap_or(0);
    let len = source_len.max(voice_end);
    let mut output = vec![0.0f32; len];
    let copy = source.len().min(len);
    output[..copy].copy_from_slice(&source[..copy]);

    let mut duck = vec![1.0f32; len];
    if project.mix.ducking {
        let ramp = ((rate as f32 * 0.1).round() as usize).max(1);
        let level = 0.35f32;
        for (start, samples) in &voices {
            let end = (start + samples.len()).min(len);
            let from = start.saturating_sub(ramp);
            let to = end.saturating_add(ramp).min(len);
            for (i, gain) in duck.iter_mut().enumerate().take(to).skip(from) {
                let value = if i < *start {
                    let progress = (i - from) as f32 / ramp as f32;
                    1.0 - (1.0 - level) * progress
                } else if i >= end {
                    let progress = (i - end) as f32 / ramp as f32;
                    level + (1.0 - level) * progress
                } else {
                    level
                };
                *gain = gain.min(value);
            }
        }
    }

    let background = project.mix.background_volume.clamp(0.0, 4.0);
    let voice = project.mix.voice_volume.clamp(0.0, 4.0);
    for (i, sample) in output.iter_mut().enumerate() {
        *sample *= background * duck[i];
    }
    for (start, samples) in &voices {
        for (i, sample) in samples.iter().enumerate() {
            let index = start + i;
            if index >= len {
                break;
            }
            output[index] += sample * voice;
        }
    }
    for sample in &mut output {
        *sample = sample.clamp(-1.0, 1.0);
    }
    Ok(Audio::mono(output, rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn temp_store(name: &str) -> (ProjectStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("laverock-mix-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        (ProjectStore::open(&dir).unwrap(), dir)
    }

    fn dubbed_project(store: &ProjectStore) -> Project {
        let source = encode_wav(&vec![0.5f32; 8_000], 8_000).unwrap();
        let project = store
            .create(
                "Mix",
                &source,
                1.0,
                vec![Segment {
                    id: "s1".into(),
                    start: 0.1,
                    end: 0.6,
                    text: "x".into(),
                    ..Segment::default()
                }],
            )
            .unwrap();
        let cue = encode_wav(&vec![0.2f32; 4_000], 8_000).unwrap();
        store.write_segment_audio(&project.id, "s1", &cue).unwrap();
        let mut project = store.get(&project.id).unwrap();
        project.segments[0].tts_path = projects::segment_rel_path("s1");
        project.segments[0].status = "done".into();
        project.mix.background_volume = 0.4;
        project.mix.voice_volume = 1.0;
        project.mix.ducking = false;
        store.save(&project).unwrap();
        project
    }

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

    #[test]
    fn mix_places_voice_over_the_background() {
        let (store, dir) = temp_store("levels");
        let project = dubbed_project(&store);
        let mixed = mix(&store, &project).unwrap();
        assert_eq!(mixed.sample_rate, 8_000);
        // Outside the cue the background is scaled to 0.5 * 0.4.
        assert!((mixed.samples[0] - 0.2).abs() < 0.01);
        // 0.35 s is inside the cue: 0.2 background + 0.2 voice.
        assert!((mixed.samples[2_800] - 0.4).abs() < 0.01);

        // Ducking lowers the background under the voice only.
        let mut ducked = project.clone();
        ducked.mix.ducking = true;
        let mixed = mix(&store, &ducked).unwrap();
        assert!((mixed.samples[0] - 0.2).abs() < 0.01);
        assert!((mixed.samples[2_800] - 0.27).abs() < 0.01);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mix_extends_past_the_source_for_a_long_cue() {
        let (store, dir) = temp_store("overflow");
        let mut project = dubbed_project(&store);
        project.segments[0].start = 0.9;
        let cue = encode_wav(&vec![0.2f32; 4_000], 8_000).unwrap();
        store.write_segment_audio(&project.id, "s1", &cue).unwrap();
        store.save(&project).unwrap();
        let mixed = mix(&store, &project).unwrap();
        assert!(mixed.samples.len() >= 11_200);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mix_needs_at_least_one_dub() {
        let (store, dir) = temp_store("empty");
        let source = encode_wav(&vec![0.5f32; 8_000], 8_000).unwrap();
        let project = store.create("Mix", &source, 1.0, Vec::new()).unwrap();
        assert!(matches!(
            mix(&store, &project),
            Err(CoreError::UserError(_))
        ));
        let _ = fs::remove_dir_all(dir);
    }
}
