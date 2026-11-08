//! Subtitle/dubbing projects.
//!
//! A project owns one source clip and the timed segments edited against it.
//! Layout (one directory per project):
//!   <data_dir>/projects/<id>/project.json
//!   <data_dir>/projects/<id>/source.wav
//!
//! The schema already carries the fields later stages need (translation,
//! per-segment TTS, mix and exports) so adding them does not invalidate
//! projects created by this stage.

use crate::reference::{is_valid_id, new_id, write_atomic};
use laverock_core::{CoreError, Result, TranscriptSegment};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const PROJECT_FILE: &str = "project.json";
const SOURCE_FILE: &str = "source.wav";
/// Generated per-cue audio, relative to the project directory.
const SEGMENT_DIR: &str = "segments";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    /// File name inside the project directory.
    pub file: String,
    /// `audio` for now; `video` once media packaging lands.
    pub kind: String,
    pub duration: f32,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub fps: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub start: f32,
    pub end: f32,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub translated: String,
    #[serde(default)]
    pub tts_path: String,
    #[serde(default)]
    pub tts_duration: f32,
    #[serde(default = "default_status")]
    pub status: String,
}

impl Default for Segment {
    fn default() -> Self {
        Self {
            id: String::new(),
            start: 0.0,
            end: 0.0,
            text: String::new(),
            translated: String::new(),
            tts_path: String::new(),
            tts_duration: 0.0,
            status: default_status(),
        }
    }
}

fn default_status() -> String {
    "pending".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mix {
    #[serde(default = "default_background_volume")]
    pub background_volume: f32,
    #[serde(default = "default_voice_volume")]
    pub voice_volume: f32,
    #[serde(default = "default_ducking")]
    pub ducking: bool,
}

impl Default for Mix {
    fn default() -> Self {
        Self {
            background_volume: default_background_volume(),
            voice_volume: default_voice_volume(),
            ducking: default_ducking(),
        }
    }
}

fn default_background_volume() -> f32 {
    0.35
}

fn default_voice_volume() -> f32 {
    1.0
}

fn default_ducking() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Export {
    pub name: String,
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// `subtitles` for now; `dubbing` once per-segment TTS lands.
    pub scenario: String,
    pub source: Source,
    #[serde(default = "default_source_language")]
    pub source_language: String,
    #[serde(default)]
    pub target_language: String,
    #[serde(default)]
    pub engine_id: String,
    #[serde(default)]
    pub voice_id: String,
    #[serde(default)]
    pub segments: Vec<Segment>,
    #[serde(default)]
    pub mix: Mix,
    #[serde(default)]
    pub exports: Vec<Export>,
    #[serde(default = "default_revision")]
    pub revision: u32,
    pub created_at: u64,
    pub updated_at: u64,
}

fn default_source_language() -> String {
    "auto".to_string()
}

fn default_revision() -> u32 {
    1
}

pub struct ProjectStore {
    root: PathBuf,
}

impl ProjectStore {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join("projects");
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Store a new project and its source clip. `segments` may come from an
    /// imported subtitle file; otherwise the project starts empty and can be
    /// transcribed afterwards.
    pub fn create(
        &self,
        name: &str,
        source_bytes: &[u8],
        duration: f32,
        mut segments: Vec<Segment>,
    ) -> Result<Project> {
        let name = clean_name(name)?;
        normalize_segments(&mut segments, duration)?;
        let id = new_id('j');
        let dir = self.root.join(&id);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(SOURCE_FILE), source_bytes)?;
        let now = now_secs();
        let project = Project {
            id: id.clone(),
            name,
            scenario: "subtitles".to_string(),
            source: Source {
                file: SOURCE_FILE.to_string(),
                kind: "audio".to_string(),
                duration,
                width: 0,
                height: 0,
                fps: 0.0,
            },
            source_language: default_source_language(),
            target_language: String::new(),
            engine_id: String::new(),
            voice_id: String::new(),
            segments,
            mix: Mix::default(),
            exports: Vec::new(),
            revision: 1,
            created_at: now,
            updated_at: now,
        };
        self.save(&project)?;
        Ok(project)
    }

    /// Newest first.
    pub fn list(&self) -> Result<Vec<Project>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Ok(id) = entry.file_name().into_string() else {
                continue;
            };
            if let Ok(project) = self.load(&id) {
                out.push(project);
            }
        }
        out.sort_by_key(|project| std::cmp::Reverse(project.updated_at));
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Project> {
        self.load(id)
    }

    /// Apply the editable fields. `None` leaves a field untouched.
    ///
    /// Per-cue TTS is server-owned: a client cannot set `tts_path` (it would
    /// choose which file to read), and editing a cue's text invalidates the
    /// audio generated for the old wording.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        id: &str,
        name: Option<&str>,
        source_language: Option<&str>,
        target_language: Option<&str>,
        engine_id: Option<&str>,
        voice_id: Option<&str>,
        segments: Option<Vec<Segment>>,
    ) -> Result<Project> {
        let mut project = self.load(id)?;
        if let Some(name) = name {
            project.name = clean_name(name)?;
        }
        if let Some(language) = source_language {
            project.source_language = clean_language(language);
        }
        if let Some(language) = target_language {
            project.target_language = clean_language(language);
        }
        if let Some(engine) = engine_id {
            project.engine_id = clean_id(engine);
        }
        if let Some(voice) = voice_id {
            project.voice_id = clean_id(voice);
        }
        if let Some(mut segments) = segments {
            normalize_segments(&mut segments, project.source.duration)?;
            let dir = self.dir(id)?;
            keep_generated_audio(&dir, &project.segments, &mut segments);
            project.segments = segments;
        }
        project.revision += 1;
        project.updated_at = now_secs();
        self.save(&project)?;
        Ok(project)
    }

    /// Replace the cues, e.g. after transcription. Every old cue's audio is
    /// stale, so the generated segment files are dropped with it.
    pub fn set_segments(&self, id: &str, mut segments: Vec<Segment>) -> Result<Project> {
        let mut project = self.load(id)?;
        normalize_segments(&mut segments, project.source.duration)?;
        project.segments = segments;
        project.revision += 1;
        project.updated_at = now_secs();
        let _ = fs::remove_dir_all(self.dir(id)?.join(SEGMENT_DIR));
        self.save(&project)?;
        Ok(project)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let dir = self.dir(id)?;
        if !dir.is_dir() {
            return Err(CoreError::NotFound(id.to_string()));
        }
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    /// The stored source clip and its file name.
    pub fn source(&self, id: &str) -> Result<(String, Vec<u8>)> {
        let project = self.load(id)?;
        let bytes = fs::read(self.dir(id)?.join(&project.source.file))?;
        Ok((project.source.file, bytes))
    }

    /// Persist a project the worker has been mutating (dubbing writes each
    /// finished cue as it goes).
    pub(crate) fn save(&self, project: &Project) -> Result<()> {
        let path = self.dir(&project.id)?.join(PROJECT_FILE);
        write_atomic(&path, &serde_json::to_vec_pretty(project)?)
    }

    /// Path of a cue's generated audio. Only a valid id becomes a file name,
    /// so a cue id can never escape the project directory.
    pub fn segment_file(&self, project_id: &str, segment_id: &str) -> Result<PathBuf> {
        if !is_valid_id(segment_id) {
            return Err(CoreError::NotFound(segment_id.to_string()));
        }
        Ok(segment_file_in(&self.dir(project_id)?, segment_id))
    }

    /// Store the audio generated for one cue.
    pub fn write_segment_audio(
        &self,
        project_id: &str,
        segment_id: &str,
        wav: &[u8],
    ) -> Result<PathBuf> {
        let path = self.segment_file(project_id, segment_id)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&path, wav)?;
        Ok(path)
    }

    /// The generated audio for one cue. A cue without audio is a 404 rather
    /// than a read of a path supplied by a client.
    pub fn segment_audio(&self, project_id: &str, segment_id: &str) -> Result<Vec<u8>> {
        let project = self.load(project_id)?;
        let segment = project
            .segments
            .iter()
            .find(|segment| segment.id == segment_id)
            .ok_or_else(|| CoreError::NotFound(segment_id.to_string()))?;
        if segment.tts_path.is_empty() {
            return Err(CoreError::NotFound(format!("{project_id}/{segment_id}")));
        }
        Ok(fs::read(self.segment_file(project_id, segment_id)?)?)
    }

    fn load(&self, id: &str) -> Result<Project> {
        let path = self.dir(id)?.join(PROJECT_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CoreError::NotFound(id.to_string()));
            }
            Err(e) => return Err(e.into()),
        };
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn dir(&self, id: &str) -> Result<PathBuf> {
        if !is_valid_id(id) {
            return Err(CoreError::NotFound(id.to_string()));
        }
        Ok(self.root.join(id))
    }
}

/// Cues suitable for the subtitle renderer.
pub fn cues(project: &Project) -> Vec<TranscriptSegment> {
    project
        .segments
        .iter()
        .filter(|segment| !segment.text.trim().is_empty())
        .map(|segment| TranscriptSegment {
            start: segment.start,
            end: segment.end,
            text: segment.text.clone(),
        })
        .collect()
}

/// Turn model output into editable segments, numbering their ids.
pub fn from_transcript(segments: &[TranscriptSegment]) -> Vec<Segment> {
    segments
        .iter()
        .enumerate()
        .map(|(index, segment)| Segment {
            id: format!("s{}", index + 1),
            start: segment.start,
            end: segment.end,
            text: segment.text.clone(),
            ..Segment::default()
        })
        .collect()
}

fn clean_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(CoreError::BadRequest("project name cannot be empty".into()));
    }
    if name.chars().count() > 120 {
        return Err(CoreError::BadRequest(
            "project name is too long (120 characters max)".into(),
        ));
    }
    Ok(name.to_string())
}

fn clean_language(raw: &str) -> String {
    let language = raw.trim().to_ascii_lowercase();
    if language.is_empty() {
        "auto".to_string()
    } else if language.chars().count() > 16 {
        language.chars().take(16).collect()
    } else {
        language
    }
}

/// Engine and voice ids are opaque, but they end up in JSON and logs only;
/// trim and cap them so a stray paste cannot bloat the project file.
fn clean_id(raw: &str) -> String {
    let id = raw.trim();
    id.chars().take(64).collect()
}

fn segment_file_in(dir: &Path, segment_id: &str) -> PathBuf {
    dir.join(SEGMENT_DIR).join(format!("{segment_id}.wav"))
}

/// The project-relative path recorded on a cue after its audio is generated.
pub fn segment_rel_path(segment_id: &str) -> String {
    format!("{SEGMENT_DIR}/{segment_id}.wav")
}

/// Keep generated audio only for cues whose wording did not change, and drop
/// the files of cues that were edited or removed.
fn keep_generated_audio(dir: &Path, old: &[Segment], new: &mut [Segment]) {
    let previous: std::collections::HashMap<&str, &Segment> =
        old.iter().map(|s| (s.id.as_str(), s)).collect();
    let kept: HashSet<String> = new.iter().map(|s| s.id.clone()).collect();
    for segment in new.iter_mut() {
        match previous.get(segment.id.as_str()) {
            Some(prev) if prev.text == segment.text && prev.translated == segment.translated => {
                segment.tts_path = prev.tts_path.clone();
                segment.tts_duration = prev.tts_duration;
                segment.status = prev.status.clone();
            }
            _ => {
                segment.tts_path.clear();
                segment.tts_duration = 0.0;
                segment.status = default_status();
                let _ = fs::remove_file(segment_file_in(dir, &segment.id));
            }
        }
    }
    for id in previous.keys().filter(|id| !kept.contains(**id)) {
        let _ = fs::remove_file(segment_file_in(dir, id));
    }
}

/// Reject impossible cue ranges and give every cue a unique id. Text is
/// trimmed; overlapping cues are allowed because real subtitles do that.
fn normalize_segments(segments: &mut [Segment], duration: f32) -> Result<()> {
    let tolerance = 1.0;
    let mut seen = HashSet::new();
    for (index, segment) in segments.iter_mut().enumerate() {
        segment.text = segment.text.trim().to_string();
        segment.translated = segment.translated.trim().to_string();
        if !segment.start.is_finite()
            || !segment.end.is_finite()
            || segment.start < 0.0
            || segment.end <= segment.start
        {
            return Err(CoreError::BadRequest(format!(
                "segment {}: end must be after start",
                index + 1
            )));
        }
        if duration > 0.0 && segment.start > duration + tolerance {
            return Err(CoreError::BadRequest(format!(
                "segment {}: starts after the clip ends",
                index + 1
            )));
        }
        let mut id = segment.id.trim().to_string();
        if id.is_empty() || !is_valid_id(&id) || seen.contains(&id) {
            let mut n = index + 1;
            loop {
                id = format!("s{n}");
                if !seen.contains(&id) {
                    break;
                }
                n += 1;
            }
        }
        seen.insert(id.clone());
        segment.id = id;
    }
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use laverock_core::encode_wav;

    fn store(name: &str) -> (ProjectStore, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("laverock-projects-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        (ProjectStore::open(&dir).unwrap(), dir)
    }

    fn wav() -> Vec<u8> {
        encode_wav(&vec![0.0; 16_000], 16_000).unwrap()
    }

    #[test]
    fn creates_lists_and_deletes_projects() {
        let (store, dir) = store("crud");
        let project = store.create("First", &wav(), 1.0, Vec::new()).unwrap();
        assert_eq!(project.name, "First");
        assert_eq!(project.source.duration, 1.0);
        assert_eq!(project.revision, 1);

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, project.id);
        assert_eq!(store.get(&project.id).unwrap().name, "First");

        let (name, bytes) = store.source(&project.id).unwrap();
        assert_eq!(name, SOURCE_FILE);
        assert_eq!(bytes, wav());

        store.delete(&project.id).unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(matches!(
            store.get(&project.id),
            Err(CoreError::NotFound(_))
        ));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn updates_segments_and_bumps_the_revision() {
        let (store, dir) = store("update");
        let project = store.create("Edit", &wav(), 1.0, Vec::new()).unwrap();
        let updated = store
            .update(
                &project.id,
                Some("Renamed"),
                None,
                Some("EN"),
                Some("zipvoice"),
                Some("voice-1"),
                Some(vec![Segment {
                    start: 0.1,
                    end: 0.8,
                    text: "  hello  ".to_string(),
                    ..Segment::default()
                }]),
            )
            .unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.target_language, "en");
        assert_eq!(updated.engine_id, "zipvoice");
        assert_eq!(updated.voice_id, "voice-1");
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.segments[0].text, "hello");
        assert_eq!(updated.segments[0].id, "s1");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn editing_a_cue_invalidates_its_audio() {
        let (store, dir) = store("dub");
        let project = store
            .create(
                "Dub",
                &wav(),
                1.0,
                vec![Segment {
                    id: "s1".into(),
                    start: 0.0,
                    end: 0.5,
                    text: "one".into(),
                    ..Segment::default()
                }],
            )
            .unwrap();
        let audio = encode_wav(&[0.1f32, 0.2], 16_000).unwrap();
        store
            .write_segment_audio(&project.id, "s1", &audio)
            .unwrap();
        let mut project = store.get(&project.id).unwrap();
        project.segments[0].tts_path = segment_rel_path("s1");
        project.segments[0].tts_duration = 0.5;
        project.segments[0].status = "done".into();
        store.save(&project).unwrap();
        assert_eq!(store.segment_audio(&project.id, "s1").unwrap(), audio);

        // Same wording keeps the audio, and the server ignores client-sent
        // TTS fields (they choose which file would be read).
        let kept = store
            .update(
                &project.id,
                None,
                None,
                None,
                None,
                None,
                Some(vec![Segment {
                    id: "s1".into(),
                    start: 0.0,
                    end: 0.6,
                    text: "one".into(),
                    tts_path: "../escape.wav".into(),
                    status: "hacked".into(),
                    ..Segment::default()
                }]),
            )
            .unwrap();
        assert_eq!(kept.segments[0].tts_path, segment_rel_path("s1"));
        assert_eq!(kept.segments[0].status, "done");
        assert!(store.segment_audio(&project.id, "s1").is_ok());

        // Editing the text makes the generated audio stale.
        let edited = store
            .update(
                &project.id,
                None,
                None,
                None,
                None,
                None,
                Some(vec![Segment {
                    id: "s1".into(),
                    start: 0.0,
                    end: 0.6,
                    text: "two".into(),
                    ..Segment::default()
                }]),
            )
            .unwrap();
        assert!(edited.segments[0].tts_path.is_empty());
        assert_eq!(edited.segments[0].status, "pending");
        assert!(matches!(
            store.segment_audio(&project.id, "s1"),
            Err(CoreError::NotFound(_))
        ));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_impossible_segment_ranges() {
        let (store, dir) = store("bad-ranges");
        let project = store.create("Bad", &wav(), 1.0, Vec::new()).unwrap();
        for (start, end) in [(-1.0, 0.5), (0.5, 0.5), (0.8, 0.2), (f32::NAN, 0.5)] {
            let result = store.set_segments(
                &project.id,
                vec![Segment {
                    start,
                    end,
                    text: "x".into(),
                    ..Segment::default()
                }],
            );
            assert!(result.is_err(), "{start}..{end} should be rejected");
        }
        // A cue may start slightly past the clip end (rounding in imports).
        assert!(store
            .set_segments(
                &project.id,
                vec![Segment {
                    start: 1.5,
                    end: 1.9,
                    text: "x".into(),
                    ..Segment::default()
                }],
            )
            .is_ok());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn transcript_segments_become_numbered_cues() {
        let segments = [
            TranscriptSegment {
                start: 0.0,
                end: 1.0,
                text: "one".into(),
            },
            TranscriptSegment {
                start: 1.2,
                end: 2.0,
                text: "two".into(),
            },
        ];
        let editable = from_transcript(&segments);
        assert_eq!(editable.len(), 2);
        assert_eq!(editable[0].id, "s1");
        assert_eq!(editable[1].id, "s2");

        let project = Project {
            id: "j1".into(),
            name: "x".into(),
            scenario: "subtitles".into(),
            source: Source {
                file: SOURCE_FILE.into(),
                kind: "audio".into(),
                duration: 2.0,
                width: 0,
                height: 0,
                fps: 0.0,
            },
            source_language: "auto".into(),
            target_language: String::new(),
            engine_id: String::new(),
            voice_id: String::new(),
            segments: editable,
            mix: Mix::default(),
            exports: Vec::new(),
            revision: 1,
            created_at: 0,
            updated_at: 0,
        };
        let cues = cues(&project);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[1].text, "two");
    }

    #[test]
    fn rejects_invalid_ids_and_names() {
        let (store, dir) = store("validation");
        assert!(store.get("../escape").is_err());
        assert!(store.create("", &wav(), 1.0, Vec::new()).is_err());
        assert!(store
            .create("x".repeat(200).as_str(), &wav(), 1.0, Vec::new())
            .is_err());
        let project = store.create("ok", &wav(), 1.0, Vec::new()).unwrap();
        // A cue id becomes a file name, so anything unsafe is replaced.
        let updated = store
            .set_segments(
                &project.id,
                vec![Segment {
                    id: "../escape".into(),
                    start: 0.0,
                    end: 0.5,
                    text: "x".into(),
                    ..Segment::default()
                }],
            )
            .unwrap();
        assert_eq!(updated.segments[0].id, "s1");
        assert!(store.segment_file(&project.id, "../escape").is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
