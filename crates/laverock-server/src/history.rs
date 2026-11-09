//! Generation history: every successful synthesis or transcription is saved
//! here.
//!
//! Layout (one directory per entry):
//!   <data_dir>/history/<id>/meta.json
//!   <data_dir>/history/<id>/audio.wav      (audio results)
//!   <data_dir>/history/<id>/text.txt       (text results, untruncated)
//!   <data_dir>/history/<id>/segments.json  (timed transcript cues, optional)
//!   <data_dir>/history/<id>/input.wav      (voice conversion source, optional)

use laverock_core::{decode_wav, CoreError, Result, TaskKind, TranscriptSegment};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

/// Keep at most this many entries.
const MAX_ENTRIES: usize = 100;
/// Keep at most this much result data in total (1 GiB), including source
/// clips attached to conversions.
const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMeta {
    pub id: String,
    /// Unix seconds.
    pub created_at: u64,
    pub engine_id: String,
    pub engine_name: String,
    /// `"music"` for music generation, `"speech"` otherwise (older entries
    /// predate the field and default to speech).
    #[serde(default = "default_kind")]
    pub kind: String,
    /// What the entry produced: `"audio"` for a rendered take, `"text"` for
    /// a transcript. Older entries predate the field and are audio.
    #[serde(default = "default_output")]
    pub output: String,
    /// The operation that produced the entry. `kind` only says what the
    /// output is; future transcription and voice-conversion entries need
    /// the task to pick a card renderer and a filter.
    #[serde(default)]
    pub task: TaskKind,
    /// Music route that produced the entry (`"text2music"`, `"cover"`,
    /// `"repaint"`, or Stable Audio's `"init_audio"`/`"inpaint_audio"`).
    /// `None` for speech and for music engines without routes.
    #[serde(default)]
    pub route: Option<String>,
    /// Where the result came from, when `text` is not the input: the source
    /// clip or project name for transcription, the source file for voice
    /// conversion.
    #[serde(default)]
    pub input: Option<HistoryInput>,
    /// Whether the entry directory also holds timed transcript cues.
    #[serde(default)]
    pub has_segments: bool,
    pub text: String,
    pub duration_secs: f32,
    pub bytes: u64,
    /// 1 for speech, 2 for music, 0 for text-only results. The WAV in the
    /// entry keeps this shape.
    #[serde(default = "default_channels")]
    pub channels: u16,
    /// Shared id for a batch of variants produced by one request.
    /// `None` for a single standalone entry.
    #[serde(default)]
    pub generation: Option<String>,
    /// Position within the generation batch (0-based). `None` for single.
    #[serde(default)]
    pub index: Option<u32>,
}

fn default_kind() -> String {
    "speech".to_string()
}

fn default_output() -> String {
    "audio".to_string()
}

/// Where a task's input came from, for the history card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryInput {
    /// Source clip filename, project name or other human label.
    #[serde(default)]
    pub name: Option<String>,
    /// The entry directory also holds `input.wav`.
    #[serde(default)]
    pub audio: bool,
}

fn default_channels() -> u16 {
    1
}

pub struct HistoryStore {
    root: PathBuf,
    /// Entry id -> meta, kept in memory for fast listing.
    entries: Mutex<Vec<HistoryMeta>>,
}

impl HistoryStore {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let root = data_dir.as_ref().join("history");
        fs::create_dir_all(&root)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let manifest = entry.path().join("meta.json");
            let has_audio = entry.path().join("audio.wav").is_file();
            let has_text = entry.path().join("text.txt").is_file();
            // A directory needs its manifest and at least one result file;
            // text-only entries are valid.
            if !manifest.is_file() || (!has_audio && !has_text) {
                continue;
            }
            match fs::read_to_string(&manifest)
                .map_err(CoreError::from)
                .and_then(|raw| serde_json::from_str::<HistoryMeta>(&raw).map_err(CoreError::from))
            {
                Ok(mut meta) => {
                    // Entries written before `task` existed: infer it from
                    // the output kind instead of leaving everything as Tts.
                    if meta.task == TaskKind::Tts {
                        meta.task = TaskKind::from_output_kind(&meta.kind);
                    }
                    // An entry whose audio was pruned by hand but whose
                    // transcript survived is still a text result.
                    if !has_audio && has_text {
                        meta.output = "text".to_string();
                    }
                    meta.has_segments = entry.path().join("segments.json").is_file();
                    entries.push(meta);
                }
                Err(e) => tracing::warn!(
                    "skipping unreadable history entry {}: {e}",
                    entry.path().display()
                ),
            }
        }
        entries.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        Ok(Self {
            root,
            entries: Mutex::new(entries),
        })
    }

    /// Entries matching a task and a free-text query. The query is matched
    /// case-insensitively against the preview, engine name, input name and,
    /// for transcripts, the full text file.
    pub fn list(&self, task: Option<TaskKind>, query: Option<&str>) -> Vec<HistoryMeta> {
        let needle = query
            .map(|q| q.trim().to_lowercase())
            .filter(|q| !q.is_empty());
        let entries = self.entries.lock().expect("history lock");
        entries
            .iter()
            .filter(|m| task.is_none_or(|t| m.task == t))
            .filter(|m| needle.as_deref().is_none_or(|q| self.matches(m, q)))
            .cloned()
            .collect()
    }

    fn matches(&self, meta: &HistoryMeta, needle: &str) -> bool {
        if meta.text.to_lowercase().contains(needle)
            || meta.engine_name.to_lowercase().contains(needle)
        {
            return true;
        }
        if let Some(name) = meta.input.as_ref().and_then(|i| i.name.as_deref()) {
            if name.to_lowercase().contains(needle) {
                return true;
            }
        }
        if meta.output == "text" {
            let path = self.root.join(&meta.id).join("text.txt");
            if let Ok(full) = fs::read_to_string(path) {
                return full.to_lowercase().contains(needle);
            }
        }
        false
    }

    /// A fresh group id so several `add_variant` calls from one request can
    /// be tied together as a single multi-sample generation.
    pub fn new_generation_id(&self) -> String {
        format!("g{}-{:04x}", now_secs(), random_suffix())
    }

    /// Add one entry. `batch` marks the entry as the Nth take of a
    /// multi-sample generation; single standalone entries pass `None`.
    /// `task` is the operation, `kind` the output medium (`"music"`,
    /// `"speech"`, later `"transcript"`), `route` the music route that was
    /// used, and `channels` the WAV's spatial shape.
    #[allow(clippy::too_many_arguments)]
    pub fn add_typed(
        &self,
        engine_id: &str,
        engine_name: &str,
        task: TaskKind,
        kind: &str,
        route: Option<&str>,
        text: &str,
        wav: &[u8],
        sample_rate: i32,
        channels: u16,
        batch: Option<(String, u32)>,
    ) -> Result<HistoryMeta> {
        let duration_secs = match decode_wav_bytes(wav, sample_rate) {
            Ok(secs) => secs,
            Err(e) => {
                tracing::warn!("history: skipping undecodable output: {e}");
                return Err(e);
            }
        };

        let created_at = now_secs();
        let id = format!("h{created_at}-{:04x}", random_suffix());
        let dir = self.root.join(&id);
        fs::create_dir_all(&dir)?;

        let meta = HistoryMeta {
            id: id.clone(),
            created_at,
            engine_id: engine_id.to_string(),
            engine_name: engine_name.to_string(),
            kind: kind.to_string(),
            output: "audio".to_string(),
            task,
            route: route.map(str::to_string),
            input: None,
            has_segments: false,
            text: text.chars().take(2000).collect(),
            duration_secs,
            bytes: wav.len() as u64,
            channels: channels.max(1),
            generation: batch.as_ref().map(|(g, _)| g.clone()),
            index: batch.map(|(_, i)| i),
        };

        fs::write(dir.join("audio.wav"), wav)?;
        write_atomic(&dir.join("meta.json"), &serde_json::to_vec_pretty(&meta)?)?;
        self.remember(&meta);
        Ok(meta)
    }

    /// Add a text-only result (a transcript). The full text is kept in
    /// `text.txt`; `meta.text` is a preview for the history list. Timed cues,
    /// when the engine reports them, go to `segments.json` for SRT/VTT
    /// export.
    pub fn add_text(
        &self,
        engine_id: &str,
        engine_name: &str,
        text: &str,
        segments: &[TranscriptSegment],
        duration_secs: f32,
        input_name: Option<&str>,
    ) -> Result<HistoryMeta> {
        let created_at = now_secs();
        let id = format!("h{created_at}-{:04x}", random_suffix());
        let dir = self.root.join(&id);
        fs::create_dir_all(&dir)?;

        let segments_bytes = if segments.is_empty() {
            Vec::new()
        } else {
            serde_json::to_vec(segments)?
        };
        let meta = HistoryMeta {
            id: id.clone(),
            created_at,
            engine_id: engine_id.to_string(),
            engine_name: engine_name.to_string(),
            kind: "transcript".to_string(),
            output: "text".to_string(),
            task: TaskKind::Asr,
            route: None,
            input: input_name.map(|name| HistoryInput {
                name: Some(name.to_string()),
                audio: false,
            }),
            has_segments: !segments_bytes.is_empty(),
            text: text.chars().take(2000).collect(),
            duration_secs,
            bytes: (text.len() + segments_bytes.len()) as u64,
            channels: 0,
            generation: None,
            index: None,
        };

        fs::write(dir.join("text.txt"), text.as_bytes())?;
        if !segments_bytes.is_empty() {
            fs::write(dir.join("segments.json"), &segments_bytes)?;
        }
        write_atomic(&dir.join("meta.json"), &serde_json::to_vec_pretty(&meta)?)?;
        self.remember(&meta);
        Ok(meta)
    }

    /// Keep the source clip of a voice conversion next to its result, so the
    /// history card can play both. `bytes` grows to include the extra file.
    pub fn attach_input(&self, id: &str, wav: &[u8], name: Option<&str>) -> Result<()> {
        if !is_valid_id(id) {
            return Err(CoreError::NotFound(format!("history {id}")));
        }
        let dir = self.root.join(id);
        if !dir.is_dir() {
            return Err(CoreError::NotFound(format!("history {id}")));
        }
        fs::write(dir.join("input.wav"), wav)?;
        let mut entries = self.entries.lock().expect("history lock");
        let meta = entries
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| CoreError::NotFound(format!("history {id}")))?;
        meta.input = Some(HistoryInput {
            name: name.map(str::to_string),
            audio: true,
        });
        meta.bytes += wav.len() as u64;
        write_atomic(&dir.join("meta.json"), &serde_json::to_vec_pretty(&meta)?)
    }

    /// Insert into the in-memory list and drop entries over the budget.
    fn remember(&self, meta: &HistoryMeta) {
        let prune = {
            let mut entries = self.entries.lock().expect("history lock");
            entries.insert(0, meta.clone());
            Self::prune_targets(&entries)
        };
        for old_id in prune {
            let _ = fs::remove_dir_all(self.root.join(&old_id));
            let mut entries = self.entries.lock().expect("history lock");
            entries.retain(|m| m.id != old_id);
        }
    }

    /// Choose oldest entries that exceed the count or size budgets.
    fn prune_targets(entries: &[HistoryMeta]) -> Vec<String> {
        let mut ordered: Vec<&HistoryMeta> = entries.iter().collect();
        ordered.sort_by_key(|m| m.created_at);
        let mut by_size: HashMap<String, u64> =
            ordered.iter().map(|m| (m.id.clone(), m.bytes)).collect();
        let mut total: u64 = by_size.values().sum();
        let mut drop = Vec::new();
        let mut i = 0;
        while ordered.len() - drop.len() > MAX_ENTRIES || total > MAX_TOTAL_BYTES {
            if i >= ordered.len() {
                break;
            }
            let id = ordered[i].id.clone();
            total -= by_size.remove(&id).unwrap_or(0);
            drop.push(id);
            i += 1;
        }
        drop
    }

    pub fn audio_path(&self, id: &str) -> Result<PathBuf> {
        if !is_valid_id(id) {
            return Err(CoreError::NotFound(format!("history {id}")));
        }
        let path = self.root.join(id).join("audio.wav");
        if !path.is_file() {
            return Err(CoreError::NotFound(format!("history {id}")));
        }
        Ok(path)
    }

    /// The untruncated result text: `text.txt` for a transcript, or the
    /// prompt stored in the metadata for an audio entry.
    pub fn text(&self, id: &str) -> Result<String> {
        if !is_valid_id(id) {
            return Err(CoreError::NotFound(format!("history {id}")));
        }
        let path = self.root.join(id).join("text.txt");
        if path.is_file() {
            return Ok(fs::read_to_string(path)?);
        }
        let entries = self.entries.lock().expect("history lock");
        entries
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.text.clone())
            .ok_or_else(|| CoreError::NotFound(format!("history {id}")))
    }

    /// Timed cues for subtitle export; empty for entries without them.
    pub fn segments(&self, id: &str) -> Result<Vec<TranscriptSegment>> {
        if !is_valid_id(id) {
            return Err(CoreError::NotFound(format!("history {id}")));
        }
        let path = self.root.join(id).join("segments.json");
        if !path.is_file() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    /// The source clip of a voice conversion, when one was attached.
    pub fn input_path(&self, id: &str) -> Result<PathBuf> {
        if !is_valid_id(id) {
            return Err(CoreError::NotFound(format!("history {id}")));
        }
        let path = self.root.join(id).join("input.wav");
        if !path.is_file() {
            return Err(CoreError::NotFound(format!("history {id}")));
        }
        Ok(path)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        if !is_valid_id(id) {
            return Err(CoreError::NotFound(format!("history {id}")));
        }
        let dir = self.root.join(id);
        if !dir.is_dir() {
            return Err(CoreError::NotFound(format!("history {id}")));
        }
        fs::remove_dir_all(dir)?;
        self.entries
            .lock()
            .expect("history lock")
            .retain(|m| m.id != id);
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        let mut entries = self.entries.lock().expect("history lock");
        for meta in entries.drain(..) {
            let _ = fs::remove_dir_all(self.root.join(meta.id));
        }
        Ok(())
    }
}

fn decode_wav_bytes(wav: &[u8], expected_rate: i32) -> Result<f32> {
    let (samples, rate) = decode_wav(wav)?;
    let rate = if rate > 0 { rate } else { expected_rate };
    if rate <= 0 || samples.is_empty() {
        return Err(CoreError::Wav("empty synthesis output".into()));
    }
    Ok(samples.len() as f32 / rate as f32)
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_suffix() -> u16 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    h.write_u64(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0),
    );
    (h.finish() & 0xffff) as u16
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_bytes() -> Vec<u8> {
        // A tiny valid 16-bit mono 24kHz WAV (~1 s of silence).
        let sr = 24_000u32;
        let frames = 1_000u32;
        let data_len = frames * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36u32 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&sr.to_le_bytes());
        v.extend_from_slice(&(sr * 2).to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        v.extend(vec![0u8; data_len as usize]);
        v
    }

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "laverock-hist-test-{}-{:x}",
            std::process::id(),
            random_suffix()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn variant_batch_shares_a_generation() {
        let dir = temp_dir();
        let store = HistoryStore::open(&dir).unwrap();
        let gen = store.new_generation_id();

        let a = store
            .add_typed(
                "zipvoice",
                "ZipVoice",
                TaskKind::Tts,
                "speech",
                None,
                "你好",
                &wav_bytes(),
                24_000,
                1,
                Some((gen.clone(), 0)),
            )
            .unwrap();
        let b = store
            .add_typed(
                "zipvoice",
                "ZipVoice",
                TaskKind::Tts,
                "speech",
                None,
                "你好",
                &wav_bytes(),
                24_000,
                1,
                Some((gen.clone(), 1)),
            )
            .unwrap();

        assert_eq!(a.generation.as_deref(), Some(gen.as_str()));
        assert_eq!(a.index, Some(0));
        assert_eq!(b.generation, a.generation);
        assert_eq!(b.index, Some(1));
        assert_ne!(a.id, b.id);

        // A standalone entry is not part of the batch.
        let single = store
            .add_typed(
                "zipvoice",
                "ZipVoice",
                TaskKind::Tts,
                "speech",
                None,
                "再见",
                &wav_bytes(),
                24_000,
                1,
                None,
            )
            .unwrap();
        assert_eq!(single.generation, None);
        assert_eq!(single.index, None);

        // Music keeps its kind and channel count so the UI can label it.
        let stereo = laverock_core::encode_wav_channels(&[0.0f32; 4_000], 48_000, 2).unwrap();
        let music = store
            .add_typed(
                "ace-step",
                "ACE-Step",
                TaskKind::Music,
                "music",
                Some("text2music"),
                "calm piano",
                &stereo,
                48_000,
                2,
                None,
            )
            .unwrap();
        assert_eq!(music.kind, "music");
        assert_eq!(music.task, TaskKind::Music);
        assert_eq!(music.route.as_deref(), Some("text2music"));
        assert_eq!(music.channels, 2);
        assert_eq!(a.task, TaskKind::Tts);
        assert_eq!(store.list(Some(TaskKind::Music), None).len(), 1);
        assert_eq!(store.list(Some(TaskKind::Tts), None).len(), 3);
        assert_eq!(store.list(None, None).len(), 4);

        // Reload from disk preserves the grouping fields.
        let reopened = HistoryStore::open(&dir).unwrap();
        let metas = reopened.list(None, None);
        assert!(metas
            .iter()
            .any(|m| m.id == a.id && m.generation.as_deref() == Some(gen.as_str())));
        assert!(metas
            .iter()
            .any(|m| m.id == music.id && m.route.as_deref() == Some("text2music")));
        assert!(metas
            .iter()
            .any(|m| m.id == single.id && m.generation.is_none()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_matches_preview_engine_source_and_full_text() {
        let dir = temp_dir();
        let store = HistoryStore::open(&dir).unwrap();
        let speech = store
            .add_typed(
                "zipvoice",
                "ZipVoice",
                TaskKind::Tts,
                "speech",
                None,
                "Morning light",
                &wav_bytes(),
                24_000,
                1,
                None,
            )
            .unwrap();
        let music = store
            .add_typed(
                "ace-step",
                "ACE-Step",
                TaskKind::Music,
                "music",
                None,
                "Calm piano",
                &wav_bytes(),
                48_000,
                1,
                None,
            )
            .unwrap();
        let asr = store
            .add_text(
                "sensevoice",
                "SenseVoice",
                "the quick brown fox",
                &[],
                1.0,
                Some("Interview.WAV"),
            )
            .unwrap();

        let ids = |metas: Vec<HistoryMeta>| metas.into_iter().map(|m| m.id).collect::<Vec<_>>();

        // Case-insensitive preview and engine name.
        assert_eq!(
            ids(store.list(None, Some("morning"))),
            vec![speech.id.clone()]
        );
        assert_eq!(ids(store.list(None, Some("ace"))), vec![music.id.clone()]);
        // Source file name.
        assert_eq!(
            ids(store.list(None, Some("interview"))),
            vec![asr.id.clone()]
        );

        // The query also reaches text beyond the 2,000-char list preview.
        let long = format!("{}needle", "a".repeat(2_500));
        let long_entry = store
            .add_text("sensevoice", "SenseVoice", &long, &[], 1.0, None)
            .unwrap();
        assert_eq!(
            ids(store.list(None, Some("needle"))),
            vec![long_entry.id.clone()]
        );

        // Task and query filters combine; a blank query lists everything.
        assert!(store.list(Some(TaskKind::Tts), Some("needle")).is_empty());
        assert_eq!(store.list(Some(TaskKind::Asr), Some("needle")).len(), 1);
        assert_eq!(store.list(None, Some("   ")).len(), 4);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_entries_infer_task_from_kind() {
        let dir = temp_dir();
        let store = HistoryStore::open(&dir).unwrap();
        let stereo = laverock_core::encode_wav_channels(&[0.0f32; 4_000], 48_000, 2).unwrap();
        let entry = store
            .add_typed(
                "ace-step",
                "ACE-Step",
                TaskKind::Music,
                "music",
                Some("text2music"),
                "calm piano",
                &stereo,
                48_000,
                2,
                None,
            )
            .unwrap();

        // Simulate an entry written before `task` existed.
        let path = dir.join("history").join(&entry.id).join("meta.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("task");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        let reopened = HistoryStore::open(&dir).unwrap();
        let meta = reopened
            .list(None, None)
            .into_iter()
            .find(|m| m.id == entry.id)
            .unwrap();
        assert_eq!(meta.task, TaskKind::Music);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_entries_keep_the_full_transcript_and_cues() {
        let dir = temp_dir();
        let store = HistoryStore::open(&dir).unwrap();
        let segments = vec![
            TranscriptSegment {
                start: 0.0,
                end: 1.5,
                text: "hello".into(),
            },
            TranscriptSegment {
                start: 1.5,
                end: 3.0,
                text: "world".into(),
            },
        ];
        // Longer than the list preview, so the file must hold the full text.
        let full = "a".repeat(2_500);
        let entry = store
            .add_text(
                "sensevoice",
                "SenseVoice",
                &full,
                &segments,
                3.0,
                Some("talk.wav"),
            )
            .unwrap();
        assert_eq!(entry.output, "text");
        assert_eq!(entry.task, TaskKind::Asr);
        assert_eq!(entry.kind, "transcript");
        assert!(entry.has_segments);
        assert_eq!(
            entry.input.as_ref().and_then(|i| i.name.as_deref()),
            Some("talk.wav")
        );
        assert_eq!(entry.text.len(), 2_000);
        assert_eq!(store.text(&entry.id).unwrap(), full);
        assert_eq!(store.segments(&entry.id).unwrap().len(), 2);

        // Reopening accepts the text-only directory and keeps the cues.
        let reopened = HistoryStore::open(&dir).unwrap();
        let meta = reopened
            .list(Some(TaskKind::Asr), None)
            .into_iter()
            .find(|m| m.id == entry.id)
            .unwrap();
        assert_eq!(meta.output, "text");
        assert!(meta.has_segments);
        assert_eq!(reopened.text(&entry.id).unwrap(), full);
        assert_eq!(reopened.segments(&entry.id).unwrap().len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn conversion_entries_can_keep_the_source_clip() {
        let dir = temp_dir();
        let store = HistoryStore::open(&dir).unwrap();
        let entry = store
            .add_typed(
                "seed-vc",
                "Seed-VC",
                TaskKind::Vc,
                "speech",
                None,
                "source.wav",
                &wav_bytes(),
                24_000,
                1,
                None,
            )
            .unwrap();
        assert!(entry.input.is_none());
        store
            .attach_input(&entry.id, &wav_bytes(), Some("source.wav"))
            .unwrap();

        let reopened = HistoryStore::open(&dir).unwrap();
        let meta = reopened.list(Some(TaskKind::Vc), None).pop().unwrap();
        let input = meta.input.expect("source recorded");
        assert!(input.audio);
        assert_eq!(input.name.as_deref(), Some("source.wav"));
        assert!(reopened.input_path(&entry.id).unwrap().is_file());
        assert!(reopened.audio_path(&entry.id).unwrap().is_file());

        let _ = fs::remove_dir_all(&dir);
    }
}
