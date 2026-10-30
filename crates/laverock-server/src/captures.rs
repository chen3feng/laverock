//! Recorded takes — the raw material a voice is built from.
//!
//! Layout (one directory per capture):
//!   <data_dir>/captures/<id>/capture.json
//!   <data_dir>/captures/<id>/capture.wav
//!   <data_dir>/captures/<id>/capture.original.wav   (only when trimmed)
//!
//! A capture is independent of the voices it feeds. It can be shared by
//! several voices, outlives a deleted voice, and is managed on its own:
//! preview, re-transcribe, de-duplicate, delete. Voices keep working the
//! way they always did — a voice with no captures still uses its own
//! `reference.wav`, which is what every voice created before this store
//! existed is migrated to.

use crate::reference::{
    self, clip_facts, is_valid_id, new_id, sha256_hex, write_atomic, PreparedReference,
    ReferenceFacts,
};
use laverock_core::{decode_wav, encode_wav, resample_mono, trim_reference, CoreError, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

const CAPTURE_FILE: &str = "capture.wav";
pub(crate) const ORIGINAL_CAPTURE_FILE: &str = "capture.original.wav";

/// Silence inserted between takes when several captures are joined into
/// one reference clip. Long enough that an engine does not splice the end
/// of one take onto the start of the next, short enough to stay natural.
const COMBINE_GAP_SECS: f32 = 0.12;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureMeta {
    pub id: String,
    pub name: String,
    /// Transcript of this take. Empty when it has not been transcribed
    /// yet, which is a normal intermediate state.
    pub prompt_text: String,
    /// Unix seconds.
    pub created_at: u64,
    /// Missing on captures written before analysis existed.
    #[serde(default)]
    pub reference: Option<ReferenceFacts>,
    /// Content hash of `capture.wav`; the identity used for de-duplication.
    #[serde(default)]
    pub sha256: String,
}

/// Something about a capture that will condition synthesis badly, or that
/// makes it not ready to be used in a voice. The UI localizes `code`.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureIssue {
    /// `no_transcript`, `short`, `long`, `clipped`, `pause` or `quiet`.
    pub code: &'static str,
    /// The measured value behind the issue (seconds, or a fraction).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f32>,
}

impl CaptureIssue {
    fn new(code: &'static str, value: Option<f32>) -> Self {
        Self { code, value }
    }
}

/// A capture plus the facts the library view needs without downloading
/// the audio first.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureView {
    #[serde(flatten)]
    pub meta: CaptureMeta,
    pub duration_secs: f32,
    pub bytes: u64,
    /// Warnings for the library card; empty means usable as-is.
    pub issues: Vec<CaptureIssue>,
    /// Older capture with byte-identical audio, if this is a duplicate.
    pub duplicate_of: Option<String>,
}

/// Readiness and quality warnings for one stored take.
pub fn issues_for(meta: &CaptureMeta) -> Vec<CaptureIssue> {
    let mut out = Vec::new();
    if meta.prompt_text.trim().is_empty() {
        out.push(CaptureIssue::new("no_transcript", None));
    }
    let Some(r) = &meta.reference else {
        return out;
    };
    if r.clipped_fraction > 0.001 {
        out.push(CaptureIssue::new("clipped", None));
    }
    if r.longest_pause_secs >= 0.8 {
        out.push(CaptureIssue::new("pause", Some(r.longest_pause_secs)));
    }
    if r.duration_secs > 0.0 && r.duration_secs < 1.5 {
        out.push(CaptureIssue::new("short", Some(r.duration_secs)));
    } else if r.duration_secs > 15.0 {
        out.push(CaptureIssue::new("long", Some(r.duration_secs)));
    }
    if r.peak > 0.0 && r.peak < 0.05 && r.clipped_fraction <= 0.001 {
        out.push(CaptureIssue::new("quiet", None));
    }
    out
}

pub struct CaptureStore {
    root: PathBuf,
}

impl CaptureStore {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let root = data_dir.as_ref().join("captures");
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Every capture, newest first, with issues and duplicate pointers.
    pub fn list(&self) -> Result<Vec<CaptureView>> {
        let mut metas = self.all_meta()?;
        // Oldest capture of each content hash is the canonical copy; a
        // newer one with identical audio can be deleted without loss.
        let mut canonical: HashMap<String, String> = HashMap::new();
        for meta in &metas {
            if !meta.sha256.is_empty() {
                canonical
                    .entry(meta.sha256.clone())
                    .or_insert_with(|| meta.id.clone());
            }
        }
        metas.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        let mut out = Vec::with_capacity(metas.len());
        for meta in metas {
            // The facts are stored; the file size is not, so it comes from
            // disk. A capture whose facts predate analysis falls back to
            // measuring the clip.
            let clip = self.root.join(&meta.id).join(CAPTURE_FILE);
            let (measured_secs, bytes) = clip_facts(&clip);
            let duration_secs = meta
                .reference
                .as_ref()
                .map(|r| r.duration_secs)
                .filter(|secs| *secs > 0.0)
                .unwrap_or(measured_secs);
            let duplicate_of = canonical
                .get(&meta.sha256)
                .filter(|kept| **kept != meta.id)
                .cloned();
            out.push(CaptureView {
                issues: issues_for(&meta),
                duplicate_of,
                meta,
                duration_secs,
                bytes,
            });
        }
        Ok(out)
    }

    /// Metadata of every capture, oldest first. Unreadable directories are
    /// skipped with a warning rather than failing the whole listing.
    fn all_meta(&self) -> Result<Vec<CaptureMeta>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let manifest = entry.path().join("capture.json");
            if !manifest.is_file() {
                continue;
            }
            match fs::read_to_string(&manifest)
                .map_err(CoreError::from)
                .and_then(|raw| serde_json::from_str::<CaptureMeta>(&raw).map_err(CoreError::from))
            {
                Ok(meta) => out.push(meta),
                Err(e) => tracing::warn!(
                    "skipping unreadable capture {}: {e}",
                    entry.path().display()
                ),
            }
        }
        // Ties happen: several takes recorded in the same second. The id
        // breaks them so "the oldest copy" is the same answer every time.
        out.sort_by_key(|m| (m.created_at, m.id.clone()));
        Ok(out)
    }

    fn dir(&self, id: &str) -> Result<PathBuf> {
        if !is_valid_id(id) {
            return Err(CoreError::NotFound(format!("capture {id}")));
        }
        let dir = self.root.join(id);
        if !dir.is_dir() {
            return Err(CoreError::NotFound(format!("capture {id}")));
        }
        Ok(dir)
    }

    pub fn meta(&self, id: &str) -> Result<CaptureMeta> {
        let dir = self.dir(id)?;
        let raw = fs::read_to_string(dir.join("capture.json"))?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn get(&self, id: &str) -> Result<(CaptureMeta, Vec<u8>)> {
        let dir = self.dir(id)?;
        let raw = fs::read_to_string(dir.join("capture.json"))?;
        let meta: CaptureMeta = serde_json::from_str(&raw)?;
        let audio = fs::read(dir.join(CAPTURE_FILE))?;
        Ok((meta, audio))
    }

    /// Duration (seconds) and size of a stored take, measured from disk.
    pub fn facts(&self, id: &str) -> (f32, u64) {
        clip_facts(&self.root.join(id).join(CAPTURE_FILE))
    }

    /// Store a new take. `clean` trims the dead air at its ends (see
    /// [`reference::prepare`]); imports and migrations pass `false` so the
    /// bytes — and therefore the content hash — stay untouched.
    ///
    /// Returns the stored bytes alongside, so a voice can use exactly the
    /// clip that was ingested instead of processing the upload twice.
    pub fn ingest(
        &self,
        name: &str,
        prompt_text: &str,
        wav: &[u8],
        clean: bool,
    ) -> Result<(CaptureMeta, Vec<u8>)> {
        reference::check_size(wav)?;
        let prepared = reference::prepare(wav, clean)?;
        let meta = self.write_new(name, prompt_text, &prepared)?;
        Ok((meta, prepared.wav))
    }

    pub fn create(
        &self,
        name: &str,
        prompt_text: &str,
        wav: &[u8],
        clean: bool,
    ) -> Result<CaptureMeta> {
        self.ingest(name, prompt_text, wav, clean).map(|(m, _)| m)
    }

    fn write_new(
        &self,
        name: &str,
        prompt_text: &str,
        prepared: &PreparedReference,
    ) -> Result<CaptureMeta> {
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let id = new_id('c');
        let dir = self.root.join(&id);
        fs::create_dir_all(&dir)?;
        let meta = CaptureMeta {
            id: id.clone(),
            name: name.trim().to_string(),
            prompt_text: prompt_text.trim().to_string(),
            created_at,
            reference: Some(prepared.facts.clone()),
            sha256: sha256_hex(&prepared.wav),
        };
        // Manifest last and atomically, like the voice store: a directory
        // without capture.json is ignored by list().
        fs::write(dir.join(CAPTURE_FILE), &prepared.wav)?;
        if let Some(original) = &prepared.original {
            fs::write(dir.join(ORIGINAL_CAPTURE_FILE), original)?;
        }
        write_atomic(
            &dir.join("capture.json"),
            &serde_json::to_vec_pretty(&meta)?,
        )?;
        Ok(meta)
    }

    /// Rename a take and/or set its transcript. Both are optional; an
    /// empty transcript is allowed (the take simply is not transcribed).
    pub fn update(
        &self,
        id: &str,
        name: Option<&str>,
        prompt_text: Option<&str>,
    ) -> Result<CaptureMeta> {
        let dir = self.dir(id)?;
        let raw = fs::read_to_string(dir.join("capture.json"))?;
        let mut meta: CaptureMeta = serde_json::from_str(&raw)?;
        if let Some(name) = name {
            meta.name = name.trim().to_string();
        }
        if let Some(text) = prompt_text {
            meta.prompt_text = text.trim().to_string();
        }
        write_atomic(
            &dir.join("capture.json"),
            &serde_json::to_vec_pretty(&meta)?,
        )?;
        Ok(meta)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let dir = self.dir(id)?;
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    /// Delete takes whose audio is byte-identical to an older one,
    /// keeping the oldest of each group. Returns `(removed, kept)` pairs
    /// so callers can repoint voices at the copy that survived.
    pub fn dedupe(&self) -> Result<Vec<(String, String)>> {
        let mut seen: HashMap<String, String> = HashMap::new();
        let mut removed = Vec::new();
        for meta in self.all_meta()? {
            if meta.sha256.is_empty() {
                continue;
            }
            match seen.get(&meta.sha256) {
                Some(kept) => {
                    self.delete(&meta.id)?;
                    removed.push((meta.id, kept.clone()));
                }
                None => {
                    seen.insert(meta.sha256.clone(), meta.id);
                }
            }
        }
        Ok(removed)
    }
}

/// Several captures joined into the single clip an engine is given.
#[derive(Debug)]
pub struct CombinedReference {
    pub wav: Vec<u8>,
    pub text: String,
    /// Captures that made it into `wav`.
    pub used: usize,
    /// Captures left out because the clip already reached the limit.
    pub dropped: usize,
}

/// Join captures, in the order given, into one reference clip of at most
/// `max_secs`.
///
/// Engines take one prompt clip, and they are sensitive to its length —
/// VoxCPM2's encoder input is a fixed 15 s tensor, LongCat budgets the
/// reference inside a ~30 s request — so the list is consumed in order
/// and cut at the engine's limit rather than concatenated blindly. Every
/// take is resampled to the first one's rate; takes are separated by
/// [`COMBINE_GAP_SECS`] of silence.
pub fn combine(clips: &[(CaptureMeta, Vec<u8>)], max_secs: f32) -> Result<CombinedReference> {
    if clips.is_empty() {
        return Err(CoreError::BadRequest("no captures to combine".into()));
    }
    let mut decoded = Vec::with_capacity(clips.len());
    let mut rate = 0u32;
    for (_, wav) in clips {
        let (samples, sr) = decode_wav(wav)?;
        let sr = sr.max(1) as u32;
        if rate == 0 {
            rate = sr;
        }
        decoded.push(resample_mono(&samples, sr, rate));
    }
    let rate_f = rate as f32;
    let gap = vec![0.0f32; (COMBINE_GAP_SECS * rate_f).round() as usize];

    let mut samples: Vec<f32> = Vec::new();
    let mut texts: Vec<&str> = Vec::new();
    let mut used = 0usize;
    for ((meta, _), clip) in clips.iter().zip(decoded.iter()) {
        let secs = clip.len() as f32 / rate_f;
        if used > 0 && (samples.len() as f32 + gap.len() as f32) / rate_f + secs > max_secs {
            break;
        }
        if used > 0 {
            samples.extend_from_slice(&gap);
        }
        if used == 0 && secs > max_secs {
            // A single long take is cut at the limit, at a quiet point.
            let cut = trim_reference(clip, rate, max_secs);
            samples.extend_from_slice(&cut);
        } else {
            samples.extend_from_slice(clip);
        }
        if !meta.prompt_text.trim().is_empty() {
            texts.push(meta.prompt_text.trim());
        }
        used += 1;
    }

    Ok(CombinedReference {
        wav: encode_wav(&samples, rate as i32)?,
        text: texts.join(" "),
        used,
        dropped: clips.len() - used,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use laverock_core::encode_wav;

    fn tone(secs: f32) -> Vec<u8> {
        tone_at(secs, 220.0)
    }

    fn tone_at(secs: f32, hz: f32) -> Vec<u8> {
        let sr = 24_000u32;
        let samples: Vec<f32> = (0..(sr as f32 * secs) as usize)
            .map(|i| 0.3 * (i as f32 * hz * std::f32::consts::TAU / sr as f32).sin())
            .collect();
        encode_wav(&samples, sr as i32).expect("encode")
    }

    fn store() -> (CaptureStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "laverock-captures-{}-{}",
            std::process::id(),
            reference::random_suffix()
        ));
        let store = CaptureStore::open(&dir).expect("open");
        (store, dir)
    }

    fn meta(name: &str, text: &str) -> CaptureMeta {
        CaptureMeta {
            id: name.into(),
            name: name.into(),
            prompt_text: text.into(),
            created_at: 0,
            reference: None,
            sha256: String::new(),
        }
    }

    #[test]
    fn combine_stops_at_the_limit_and_keeps_transcripts_aligned() {
        let clips = vec![
            (meta("a", "第一段"), tone(2.0)),
            (meta("b", "第二段"), tone(2.0)),
            (meta("c", "第三段"), tone(2.0)),
        ];
        let joined = combine(&clips, 5.0).expect("combine");
        assert_eq!(joined.used, 2, "{joined:?}");
        assert_eq!(joined.dropped, 1);
        assert_eq!(joined.text, "第一段 第二段");
        let (samples, rate) = decode_wav(&joined.wav).expect("decode");
        assert!((samples.len() as f32 / rate as f32 - 4.12).abs() < 0.05);
    }

    #[test]
    fn combine_cuts_a_single_long_take() {
        let clips = vec![(meta("a", "长"), tone(9.0))];
        let joined = combine(&clips, 4.0).expect("combine");
        assert_eq!(joined.used, 1);
        let (samples, rate) = decode_wav(&joined.wav).expect("decode");
        assert!(samples.len() as f32 / rate as f32 <= 4.01);
    }

    #[test]
    fn list_flags_duplicates_and_missing_transcripts() {
        let (store, dir) = store();
        let wav = tone(2.0);
        let first = store.create("a", "文字", &wav, false).expect("create");
        let dup = store.create("b", "文字", &wav, false).expect("create");
        // A different pitch, so this take is not byte-identical too.
        let silent_text = store
            .create("c", "", &tone_at(2.0, 300.0), false)
            .expect("create");

        let views = store.list().expect("list");
        assert_eq!(views.len(), 3);
        let by_id = |id: &str| views.iter().find(|v| v.meta.id == id).expect(id);
        // Exactly one of the identical takes points at the other. Which
        // one is kept is deterministic (oldest, id as tie-break) but not
        // worth pinning down here.
        let (kept, dropped) = match (&by_id(&first.id).duplicate_of, &by_id(&dup.id).duplicate_of) {
            (None, Some(other)) if *other == first.id => (first.id.clone(), dup.id.clone()),
            (Some(other), None) if *other == dup.id => (dup.id.clone(), first.id.clone()),
            (a, b) => panic!("duplicates not paired: {a:?} / {b:?}"),
        };
        let codes: Vec<_> = by_id(&silent_text.id)
            .issues
            .iter()
            .map(|i| i.code)
            .collect();
        assert!(codes.contains(&"no_transcript"), "{codes:?}");

        let removed = store.dedupe().expect("dedupe");
        assert_eq!(removed, vec![(dropped.clone(), kept.clone())]);
        assert!(store.get(&dropped).is_err());
        assert!(store.get(&kept).is_ok());
        fs::remove_dir_all(&dir).ok();
    }
}
