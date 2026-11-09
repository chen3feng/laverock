//! Generation history: every successful synthesis is saved here.
//!
//! Layout (one directory per entry):
//!   <data_dir>/history/<id>/meta.json
//!   <data_dir>/history/<id>/audio.wav

use laverock_core::{decode_wav, CoreError, Result, TaskKind};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

/// Keep at most this many generations.
const MAX_ENTRIES: usize = 100;
/// Keep at most this much audio in total (1 GiB).
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
    pub text: String,
    pub duration_secs: f32,
    pub bytes: u64,
    /// 1 for speech, 2 for music. The WAV in the entry keeps this shape.
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
            if !manifest.is_file() || !entry.path().join("audio.wav").is_file() {
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

    /// Entries for one task, or all of them when `task` is `None`.
    pub fn list_by_task(&self, task: Option<TaskKind>) -> Vec<HistoryMeta> {
        let entries = self.entries.lock().expect("history lock");
        match task {
            Some(task) => entries.iter().filter(|m| m.task == task).cloned().collect(),
            None => entries.clone(),
        }
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
            task,
            route: route.map(str::to_string),
            text: text.chars().take(2000).collect(),
            duration_secs,
            bytes: wav.len() as u64,
            channels: channels.max(1),
            generation: batch.as_ref().map(|(g, _)| g.clone()),
            index: batch.map(|(_, i)| i),
        };

        fs::write(dir.join("audio.wav"), wav)?;
        write_atomic(&dir.join("meta.json"), &serde_json::to_vec_pretty(&meta)?)?;

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
        Ok(meta)
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
        assert_eq!(store.list_by_task(Some(TaskKind::Music)).len(), 1);
        assert_eq!(store.list_by_task(Some(TaskKind::Tts)).len(), 3);
        assert_eq!(store.list_by_task(None).len(), 4);

        // Reload from disk preserves the grouping fields.
        let reopened = HistoryStore::open(&dir).unwrap();
        let metas = reopened.list_by_task(None);
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
            .list_by_task(None)
            .into_iter()
            .find(|m| m.id == entry.id)
            .unwrap();
        assert_eq!(meta.task, TaskKind::Music);

        let _ = fs::remove_dir_all(&dir);
    }
}
