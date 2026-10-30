//! Saved voices: a reference clip plus its transcript.
//!
//! Layout (one directory per profile):
//!   <data_dir>/profiles/<id>/profile.json
//!   <data_dir>/profiles/<id>/reference.wav
//!
//! A profile is deliberately engine-agnostic: the same reference audio
//! works with any cloning engine that accepts a prompt clip.
//!
//! A voice may also be built from one or more [`crate::captures`] (the
//! takes in the recording library). Its `captures` list names them in
//! order; `reference.wav` then stays as a single-clip fallback for older
//! readers, while synthesis and preview use the captures joined together.

use crate::captures::{self, CaptureMeta, CaptureStore, CombinedReference};
use crate::reference::{
    self, clip_facts, is_valid_id, new_id, prepare, sha256_hex, write_atomic, PreparedReference,
    ReferenceFacts, MAX_REFERENCE_BYTES,
};
use laverock_core::i18n::{self, Key};
use laverock_core::{decode_wav, CoreError, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Longest reference assembled when packing a voice for export. Export
/// must be faithful rather than engine-specific, so it is generous; the
/// engine's own limit is applied at synthesis time.
const EXPORT_MAX_REFERENCE_SECS: f32 = 60.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMeta {
    pub id: String,
    pub name: String,
    pub prompt_text: String,
    /// Unix seconds.
    pub created_at: u64,
    /// Missing on profiles created before ingest analysis existed.
    #[serde(default)]
    pub reference: Option<ReferenceFacts>,
    /// Captures this voice is built from, in order. Empty means the voice
    /// uses its own `reference.wav` (every voice created before the
    /// recording library existed, until it is migrated).
    #[serde(default)]
    pub captures: Vec<String>,
}

/// A saved voice plus facts the library view needs without downloading
/// the clip first.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    #[serde(flatten)]
    pub meta: ProfileMeta,
    pub duration_secs: f32,
    pub bytes: u64,
    /// Number of takes behind this voice; 1 for a legacy single clip.
    pub capture_count: usize,
}

pub struct ProfileStore {
    root: PathBuf,
    captures: CaptureStore,
}

/// Validated capture ids plus their clips, in order.
type LoadedCaptures = (Vec<String>, Vec<(CaptureMeta, Vec<u8>)>);

impl ProfileStore {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let root = data_dir.as_ref().join("profiles");
        fs::create_dir_all(&root)?;
        let captures = CaptureStore::open(data_dir)?;
        Ok(Self { root, captures })
    }

    /// The recording library voices are built from.
    pub fn captures(&self) -> &CaptureStore {
        &self.captures
    }

    pub fn list(&self) -> Result<Vec<ProfileView>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let manifest = entry.path().join("profile.json");
            if !manifest.is_file() {
                continue;
            }
            match fs::read_to_string(&manifest)
                .map_err(CoreError::from)
                .and_then(|raw| serde_json::from_str::<ProfileMeta>(&raw).map_err(CoreError::from))
            {
                Ok(meta) => {
                    let clip = entry.path().join("reference.wav");
                    out.push(self.view(meta, &clip));
                }
                Err(e) => tracing::warn!(
                    "skipping unreadable profile {}: {e}",
                    entry.path().display()
                ),
            }
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.meta.created_at));
        Ok(out)
    }

    /// A voice as the library shows it: the stored facts, or — when the
    /// voice is built from captures — what those takes add up to, because
    /// that is the clip an engine actually receives.
    fn view(&self, mut meta: ProfileMeta, clip: &Path) -> ProfileView {
        let mut duration_secs;
        let mut bytes;
        let mut capture_count = 1;
        let takes: Vec<CaptureMeta> = meta
            .captures
            .iter()
            .filter_map(|id| self.captures.meta(id).ok())
            .collect();
        if takes.is_empty() {
            (duration_secs, bytes) = clip_facts(clip);
            // Profiles saved before ingest analysis existed get their
            // facts computed on the fly so the library can still flag
            // problematic clips.
            if meta.reference.is_none() {
                if let Ok(raw) = fs::read(clip) {
                    if let Ok((samples, rate)) = decode_wav(&raw) {
                        let rate = rate.max(1) as u32;
                        meta.reference = Some(reference::facts_for(&samples, rate, samples.len()));
                    }
                }
            }
        } else {
            capture_count = takes.len();
            duration_secs = 0.0;
            bytes = 0;
            for take in &takes {
                let (secs, size) = self.captures.facts(&take.id);
                duration_secs += secs;
                bytes += size;
            }
            meta.reference = Some(aggregate_facts(&takes));
        }
        ProfileView {
            meta,
            duration_secs,
            bytes,
            capture_count,
        }
    }

    pub fn dir(&self, id: &str) -> Result<PathBuf> {
        if !is_valid_id(id) {
            return Err(CoreError::NotFound(format!("profile {id}")));
        }
        let dir = self.root.join(id);
        if !dir.is_dir() {
            return Err(CoreError::NotFound(format!("profile {id}")));
        }
        Ok(dir)
    }

    pub fn get(&self, id: &str) -> Result<(ProfileMeta, Vec<u8>)> {
        let dir = self.dir(id)?;
        let raw = fs::read_to_string(dir.join("profile.json"))?;
        let meta: ProfileMeta = serde_json::from_str(&raw)?;
        let audio = fs::read(dir.join("reference.wav"))?;
        Ok((meta, audio))
    }

    pub fn create(&self, name: &str, prompt_text: &str, wav: &[u8]) -> Result<ProfileMeta> {
        self.create_impl(name, prompt_text, wav, true)
    }

    /// `clean` is false for archive imports: those clips were already
    /// processed (or curated) elsewhere, and leaving them byte-identical
    /// keeps the content fingerprints used for duplicate detection valid.
    ///
    /// Every saved voice's clip is also a capture, so recordings made on
    /// the synth page turn up in the recording library without the user
    /// saving them twice.
    fn create_impl(
        &self,
        name: &str,
        prompt_text: &str,
        wav: &[u8],
        clean: bool,
    ) -> Result<ProfileMeta> {
        self.create_impl_with(name, prompt_text, wav, clean, None)
    }

    /// `captures` names takes that already exist (an archive carrying its
    /// own per-take audio); otherwise the clip itself becomes one take.
    fn create_impl_with(
        &self,
        name: &str,
        prompt_text: &str,
        wav: &[u8],
        clean: bool,
        captures: Option<Vec<String>>,
    ) -> Result<ProfileMeta> {
        validate_name(name)?;
        let prompt_text = prompt_text.trim();
        if prompt_text.is_empty() {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::TranscriptMissing,
            )));
        }
        let (captures, stored) = self.store_or_use(name, prompt_text, wav, clean, captures)?;
        // The voice stores exactly the clip the capture kept, so the two
        // can never drift apart and no second trim is applied.
        let reference = prepare(&stored, false)?;
        self.write_profile(name, prompt_text, &reference, captures)
    }

    /// The capture ids to record on a new or replaced voice, and the clip
    /// bytes to store for it.
    fn store_or_use(
        &self,
        name: &str,
        prompt_text: &str,
        wav: &[u8],
        clean: bool,
        existing: Option<Vec<String>>,
    ) -> Result<(Vec<String>, Vec<u8>)> {
        match existing {
            Some(ids) => Ok((ids, wav.to_vec())),
            None => {
                let (capture, stored) = self.captures.ingest(name, prompt_text, wav, clean)?;
                Ok((vec![capture.id], stored))
            }
        }
    }

    /// Build a voice from existing captures, in the order given. The
    /// stored `reference.wav` is the takes joined up to the export cap:
    /// the voice is engine-agnostic, so its engine's limit is applied
    /// later, when it is synthesized with.
    pub fn create_from_captures(&self, name: &str, ids: &[String]) -> Result<ProfileMeta> {
        validate_name(name)?;
        let (ids, clips) = self.load_captures(ids)?;
        if clips.is_empty() {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::NothingSelected,
            )));
        }
        let combined = captures::combine(&clips, EXPORT_MAX_REFERENCE_SECS)?;
        if combined.text.is_empty() {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::TranscriptMissing,
            )));
        }
        let reference = prepare(&combined.wav, false)?;
        self.write_profile(name, &combined.text, &reference, ids)
    }

    fn write_profile(
        &self,
        name: &str,
        prompt_text: &str,
        reference: &PreparedReference,
        captures: Vec<String>,
    ) -> Result<ProfileMeta> {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let id = new_id('p');
        let dir = self.root.join(&id);
        fs::create_dir_all(&dir)?;
        // The takes measured the clip at ingest; that is the more useful
        // description of the voice than re-measuring the joined reference.
        let facts = self
            .capture_facts(&captures)
            .unwrap_or_else(|| reference.facts.clone());
        let meta = ProfileMeta {
            id: id.clone(),
            name: name.trim().to_string(),
            prompt_text: prompt_text.trim().to_string(),
            created_at,
            reference: Some(facts),
            captures,
        };

        // Write the manifest last and atomically: a profile directory
        // without profile.json is ignored by list().
        fs::write(dir.join("reference.wav"), &reference.wav)?;
        write_atomic(
            &dir.join("profile.json"),
            &serde_json::to_vec_pretty(&meta)?,
        )?;
        Ok(meta)
    }

    /// The clip an engine is given for this voice, joined up to `max_secs`.
    /// A voice without captures (one not migrated yet) keeps its own
    /// `reference.wav`; a single capture is returned byte-for-byte.
    pub fn reference(&self, id: &str, max_secs: f32) -> Result<CombinedReference> {
        let (meta, wav) = self.get(id)?;
        let clips: Vec<(CaptureMeta, Vec<u8>)> = meta
            .captures
            .iter()
            .filter_map(|cid| self.captures.get(cid).ok())
            .collect();
        if clips.len() > 1 {
            return captures::combine(&clips, max_secs);
        }
        let wav = clips.first().map(|(_, bytes)| bytes.clone()).unwrap_or(wav);
        Ok(CombinedReference {
            wav,
            text: meta.prompt_text,
            used: 1,
            dropped: 0,
        })
    }

    /// Replace the captures a voice is built from, in order. The voice's
    /// transcript is re-derived from them.
    pub fn set_captures(&self, id: &str, ids: &[String]) -> Result<ProfileMeta> {
        let (ids, _) = self.load_captures(ids)?;
        let dir = self.dir(id)?;
        let raw = fs::read_to_string(dir.join("profile.json"))?;
        let mut meta: ProfileMeta = serde_json::from_str(&raw)?;
        meta.prompt_text = self.joined_transcript(&ids).unwrap_or(meta.prompt_text);
        meta.captures = ids;
        write_atomic(
            &dir.join("profile.json"),
            &serde_json::to_vec_pretty(&meta)?,
        )?;
        Ok(meta)
    }

    /// Replace a take's transcript and refresh every voice built from it,
    /// so a re-transcription shows up in the voice library too.
    pub fn set_capture_transcript(&self, id: &str, text: &str) -> Result<CaptureMeta> {
        let meta = self.captures.update(id, None, Some(text))?;
        for view in self.list()? {
            let mut voice = view.meta;
            if !voice.captures.iter().any(|cid| cid == id) {
                continue;
            }
            let Some(joined) = self.joined_transcript(&voice.captures) else {
                continue;
            };
            if joined == voice.prompt_text {
                continue;
            }
            voice.prompt_text = joined;
            let dir = self.dir(&voice.id)?;
            write_atomic(
                &dir.join("profile.json"),
                &serde_json::to_vec_pretty(&voice)?,
            )?;
        }
        Ok(meta)
    }

    /// Delete a take from the library. Voices built from it lose it; a
    /// voice left with no takes falls back to its own `reference.wav`.
    pub fn delete_capture(&self, id: &str) -> Result<()> {
        self.captures.delete(id)?;
        for view in self.list()? {
            let mut meta = view.meta;
            if !meta.captures.iter().any(|cid| cid == id) {
                continue;
            }
            meta.captures.retain(|cid| cid != id);
            let dir = self.dir(&meta.id)?;
            write_atomic(
                &dir.join("profile.json"),
                &serde_json::to_vec_pretty(&meta)?,
            )?;
        }
        Ok(())
    }

    /// Delete takes whose audio duplicates an older take, then repoint
    /// every voice that referenced a deleted copy. Returns how many were
    /// removed.
    pub fn dedupe_captures(&self) -> Result<usize> {
        let removed = self.captures.dedupe()?;
        if removed.is_empty() {
            return Ok(0);
        }
        let remap: HashMap<String, String> = removed.into_iter().collect();
        for view in self.list()? {
            let mut meta = view.meta;
            let mut changed = false;
            let mut next: Vec<String> = Vec::with_capacity(meta.captures.len());
            for cid in &meta.captures {
                let id = remap.get(cid).cloned().unwrap_or_else(|| cid.clone());
                if id != *cid || next.contains(&id) {
                    changed = true;
                }
                if !next.contains(&id) {
                    next.push(id);
                }
            }
            if !changed {
                continue;
            }
            meta.captures = next;
            let dir = self.dir(&meta.id)?;
            write_atomic(
                &dir.join("profile.json"),
                &serde_json::to_vec_pretty(&meta)?,
            )?;
        }
        Ok(remap.len())
    }

    /// Give every voice that predates the recording library a single
    /// capture holding its `reference.wav`. Idempotent: a voice already
    /// naming captures is left alone, so this can run at every start.
    pub fn migrate_captures(&self) -> usize {
        let mut migrated = 0;
        let views = match self.list() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("cannot migrate voices to captures: {e}");
                return 0;
            }
        };
        for view in views {
            let mut meta = view.meta;
            let dir = match self.dir(&meta.id) {
                Ok(d) => d,
                Err(_) => continue,
            };
            if !meta.captures.is_empty() {
                // Voices are engine-agnostic now; drop the engine an
                // older version recorded on the manifest.
                self.drop_legacy_engine(&dir, &meta);
                continue;
            }
            let wav = match fs::read(dir.join("reference.wav")) {
                Ok(w) => w,
                Err(_) => continue,
            };
            // The clip was processed at ingest; keep the bytes untouched
            // so the voice and its capture stay identical.
            let capture = match self
                .captures
                .create(&meta.name, &meta.prompt_text, &wav, false)
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("cannot migrate voice {}: {e}", meta.id);
                    continue;
                }
            };
            meta.captures = vec![capture.id];
            let bytes = match serde_json::to_vec_pretty(&meta) {
                Ok(bytes) => bytes,
                Err(e) => {
                    tracing::warn!("cannot migrate voice {}: {e}", meta.id);
                    continue;
                }
            };
            if let Err(e) = write_atomic(&dir.join("profile.json"), &bytes) {
                tracing::warn!("cannot migrate voice {}: {e}", meta.id);
                continue;
            }
            migrated += 1;
        }
        migrated
    }

    /// Rewrite a manifest that still carries the retired `engine_id`.
    /// Unknown fields are ignored when parsing, so this only needs to
    /// notice the key and write the typed manifest back.
    fn drop_legacy_engine(&self, dir: &Path, meta: &ProfileMeta) {
        let path = dir.join("profile.json");
        let Ok(raw) = fs::read_to_string(&path) else {
            return;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return;
        };
        if value.get("engine_id").is_none() {
            return;
        }
        let Ok(bytes) = serde_json::to_vec_pretty(meta) else {
            return;
        };
        if let Err(e) = write_atomic(&path, &bytes) {
            tracing::warn!("cannot clean voice {}: {e}", meta.id);
        }
    }

    /// Keep only the ids that exist, in the given order and without
    /// repeats, and load their clips alongside.
    fn load_captures(&self, ids: &[String]) -> Result<LoadedCaptures> {
        let mut seen = HashSet::new();
        let mut kept = Vec::new();
        let mut clips = Vec::new();
        for cid in ids {
            if !seen.insert(cid.clone()) {
                continue;
            }
            let (meta, wav) = self.captures.get(cid)?;
            kept.push(cid.clone());
            clips.push((meta, wav));
        }
        Ok((kept, clips))
    }

    /// Facts of the named takes, aggregated; `None` when none exist.
    fn capture_facts(&self, ids: &[String]) -> Option<ReferenceFacts> {
        let takes: Vec<CaptureMeta> = ids
            .iter()
            .filter_map(|cid| self.captures.meta(cid).ok())
            .collect();
        (!takes.is_empty()).then(|| aggregate_facts(&takes))
    }

    /// Transcript of the given captures joined the same way
    /// [`captures::combine`] joins their audio; `None` when none of them
    /// has been transcribed.
    fn joined_transcript(&self, ids: &[String]) -> Option<String> {
        let texts: Vec<String> = ids
            .iter()
            .filter_map(|cid| self.captures.meta(cid).ok())
            .map(|m| m.prompt_text.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        (!texts.is_empty()).then(|| texts.join(" "))
    }

    /// Rename a voice, change its transcript, and/or replace the
    /// reference clip. The transcript may be cleared (ZipVoice will then
    /// ask for one at synthesis time).
    pub fn update(
        &self,
        id: &str,
        name: &str,
        prompt_text: &str,
        wav: Option<&[u8]>,
    ) -> Result<ProfileMeta> {
        self.update_impl_with(id, name, prompt_text, wav, true, None)
    }

    /// See [`ProfileStore::create_impl`] for `clean` and `captures`.
    fn update_impl_with(
        &self,
        id: &str,
        name: &str,
        prompt_text: &str,
        wav: Option<&[u8]>,
        clean: bool,
        captures: Option<Vec<String>>,
    ) -> Result<ProfileMeta> {
        let dir = self.dir(id)?;
        let raw = fs::read_to_string(dir.join("profile.json"))?;
        let mut meta: ProfileMeta = serde_json::from_str(&raw)?;

        validate_name(name)?;

        if let Some(wav) = wav {
            // A replaced clip is a new take: the old captures stay in the
            // library and the voice is rebuilt from the new one.
            let (captures, stored) = self.store_or_use(name, prompt_text, wav, clean, captures)?;
            let reference = prepare(&stored, false)?;
            fs::write(dir.join("reference.wav"), &reference.wav)?;
            meta.reference = self
                .capture_facts(&captures)
                .or(Some(reference.facts.clone()));
            meta.captures = captures;
        }

        meta.name = name.trim().to_string();
        meta.prompt_text = prompt_text.trim().to_string();
        write_atomic(
            &dir.join("profile.json"),
            &serde_json::to_vec_pretty(&meta)?,
        )?;
        Ok(meta)
    }
    pub fn delete(&self, id: &str) -> Result<()> {
        let dir = self.dir(id)?;
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    /// Pack a voice into a portable `.lvvoice` archive:
    /// `manifest.json` (format version + metadata), `reference.wav` and —
    /// for a voice built from several takes — `captures.json` plus one
    /// WAV per take, so the receiving side keeps the takes apart. The
    /// WAVs are stored uncompressed: PCM already resists deflate well,
    /// and ZIP_STORED keeps packing/extraction instant.
    pub fn export_zip(&self, id: &str) -> Result<Vec<u8>> {
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        let (meta, stored) = self.get(id)?;
        let combined = self.reference(id, EXPORT_MAX_REFERENCE_SECS)?;
        let takes = self.export_takes(&meta, &stored)?;
        let exported_at = now_secs();
        let manifest = serde_json::json!({
            "format": "laverock-voice",
            "version": 1,
            "name": meta.name,
            "prompt_text": combined.text,
            "audio_sha256": sha256_hex(&combined.wav),
            "captures": takes.len(),
            "exported_at": exported_at,
        });

        let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        start_file(
            &mut zip,
            "manifest.json",
            opts,
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        start_file(&mut zip, "reference.wav", opts, &combined.wav)?;
        write_takes(&mut zip, opts, "", &takes)?;
        let cursor = zip
            .finish()
            .map_err(|e| CoreError::Wav(format!("cannot finish archive: {e}")))?;
        Ok(cursor.into_inner())
    }

    /// The takes to write into an archive: one per capture, or a single
    /// entry holding `reference.wav` for a voice that has none yet.
    fn export_takes(&self, meta: &ProfileMeta, stored: &[u8]) -> Result<Vec<ExportTake>> {
        let takes: Vec<ExportTake> = meta
            .captures
            .iter()
            .filter_map(|cid| self.captures.get(cid).ok())
            .map(|(m, wav)| ExportTake {
                name: m.name,
                prompt_text: m.prompt_text,
                wav,
            })
            .collect();
        if !takes.is_empty() {
            return Ok(takes);
        }
        Ok(vec![ExportTake {
            name: meta.name.clone(),
            prompt_text: meta.prompt_text.clone(),
            wav: stored.to_vec(),
        }])
    }

    /// A name not already used by another voice, suffixed like "Name (2)".
    fn unique_name(&self, wanted: &str) -> String {
        let existing: std::collections::HashSet<String> = self
            .list()
            .map(|ps| ps.into_iter().map(|p| p.meta.name).collect())
            .unwrap_or_default();
        if !existing.contains(wanted) {
            return wanted.to_string();
        }
        for n in 2..1000 {
            let candidate = format!("{wanted} ({n})");
            if !existing.contains(&candidate) {
                return candidate;
            }
        }
        // Extremely unlikely; stamp with time to guarantee uniqueness.
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() % 1_000_000)
            .unwrap_or(0);
        format!("{wanted} ({stamp})")
    }

    fn create_without_transcript(
        &self,
        name: &str,
        wav: &[u8],
        captures: Option<Vec<String>>,
    ) -> Result<ProfileMeta> {
        // Imported archives keep their bytes untouched; see create_impl.
        let (captures, stored) = self.store_or_use(name, "", wav, false, captures)?;
        let reference = prepare(&stored, false)?;
        let text = self.joined_transcript(&captures).unwrap_or_default();
        self.write_profile(name, &text, &reference, captures)
    }

    /// Recreate the takes an archive carries. Returns `None` when it
    /// carried none (an older archive), so the caller falls back to one
    /// capture made from the joined clip.
    fn store_archive_captures(&self, voice: &str, takes: &[ArchiveCapture]) -> Option<Vec<String>> {
        if takes.is_empty() {
            return None;
        }
        let mut ids = Vec::with_capacity(takes.len());
        for take in takes {
            let name = if take.name.is_empty() {
                voice.to_string()
            } else {
                take.name.clone()
            };
            match self
                .captures
                .create(&name, &take.prompt_text, &take.wav, false)
            {
                Ok(meta) => ids.push(meta.id),
                Err(e) => tracing::warn!("cannot import take of {voice}: {e}"),
            }
        }
        (!ids.is_empty()).then_some(ids)
    }

    /// Export several voices into one `.lvvoices.zip` archive. Each
    /// voice is a `voices/<index>-<safe-name>/` directory holding the
    /// same manifest/WAV pair as a single export; a top-level
    /// `voices.json` indexes them. Compatible readers can still use
    /// the single-voice layout at the archive root.
    pub fn export_zip_batch(&self, ids: &[String]) -> Result<Vec<u8>> {
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        if ids.is_empty() {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::NothingSelected,
            )));
        }
        let exported_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let mut index = Vec::with_capacity(ids.len());
        for (i, id) in ids.iter().enumerate() {
            let (meta, stored) = self.get(id)?;
            let combined = self.reference(id, EXPORT_MAX_REFERENCE_SECS)?;
            let takes = self.export_takes(&meta, &stored)?;
            let safe = archive_safe_name(&meta.name);
            let dir = format!("voices/{:02}-{}", i + 1, safe);
            zip.add_directory(format!("{dir}/"), opts)
                .map_err(|e| CoreError::Wav(format!("cannot write archive: {e}")))?;
            let audio_hash = sha256_hex(&combined.wav);
            let entry = serde_json::json!({
                "dir": dir,
                "id": meta.id,
                "name": meta.name,
                "prompt_text": combined.text,
                    "audio_sha256": audio_hash,
                "captures": takes.len(),
            });
            let manifest = serde_json::json!({
                "format": "laverock-voice",
                "version": 1,
                "name": meta.name,
                "prompt_text": combined.text,
                    "audio_sha256": audio_hash,
                "captures": takes.len(),
                "exported_at": exported_at,
            });
            start_file(
                &mut zip,
                &format!("{dir}/manifest.json"),
                opts,
                &serde_json::to_vec_pretty(&manifest)?,
            )?;
            start_file(
                &mut zip,
                &format!("{dir}/reference.wav"),
                opts,
                &combined.wav,
            )?;
            write_takes(&mut zip, opts, &format!("{dir}/"), &takes)?;
            index.push(entry);
        }
        let bundle = serde_json::json!({
            "format": "laverock-voices",
            "version": 1,
            "count": index.len(),
            "exported_at": exported_at,
            "voices": index,
        });
        start_file(
            &mut zip,
            "voices.json",
            opts,
            &serde_json::to_vec_pretty(&bundle)?,
        )?;
        let cursor = zip
            .finish()
            .map_err(|e| CoreError::Wav(format!("cannot finish archive: {e}")))?;
        Ok(cursor.into_inner())
    }

    /// Fingerprints of every stored voice: content hash + name +
    /// transcript, used to recognize duplicates across machines.
    pub fn fingerprint_index(&self) -> Vec<VoiceFingerprint> {
        self.list()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| {
                let combined = self.reference(&p.meta.id, EXPORT_MAX_REFERENCE_SECS).ok()?;
                Some(VoiceFingerprint {
                    audio_sha256: sha256_hex(&combined.wav),
                    id: p.meta.id,
                })
            })
            .collect()
    }

    /// Import a single- or multi-voice archive.
    ///
    /// - `skip`: existing fingerprints are skipped (default)
    /// - `copy`: every voice is created, name clashes are suffixed
    /// - `overwrite`: a matching voice is replaced in place (id kept,
    ///   so history references survive); unmatched voices are added
    pub fn import_archive(&self, bytes: &[u8], strategy: ImportStrategy) -> Result<ImportReport> {
        let voices = read_archive_voices(bytes)?;
        if voices.is_empty() {
            return Err(CoreError::BadRequest("archive contains no voices".into()));
        }

        let mut report = ImportReport::default();
        for entry in voices {
            // Validate every clip up front; record failures but keep
            // going so one bad entry does not abort the whole batch.
            if let Err(e) = decode_wav(&entry.wav) {
                report.failed.push(format!("{}: {e}", entry.name));
                continue;
            }
            if entry.wav.len() > MAX_REFERENCE_BYTES {
                report
                    .failed
                    .push(format!("{}: reference audio exceeds 20 MB", entry.name));
                continue;
            }

            let existing = self
                .fingerprint_index()
                .into_iter()
                .find(|f| f.audio_sha256 == entry.audio_sha256);
            // Archives written by this version carry each take on its own;
            // older ones only have the joined clip.
            let takes = self.store_archive_captures(&entry.name, &entry.captures);

            match (strategy, existing) {
                (ImportStrategy::Skip, Some(_)) => report.skipped += 1,
                (ImportStrategy::Overwrite, Some(fp)) => {
                    self.update_impl_with(
                        &fp.id,
                        &entry.name,
                        &entry.prompt_text,
                        Some(&entry.wav),
                        false,
                        takes,
                    )?;
                    report.overwritten += 1;
                }
                _ => {
                    let name = self.unique_name(if entry.name.is_empty() {
                        "导入的声音"
                    } else {
                        &entry.name
                    });
                    if entry.prompt_text.is_empty() {
                        self.create_without_transcript(&name, &entry.wav, takes)?;
                    } else {
                        self.create_impl_with(&name, &entry.prompt_text, &entry.wav, false, takes)?;
                    }
                    report.imported += 1;
                }
            }
        }
        Ok(report)
    }
}

/// How a batch import treats voices already present on this machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImportStrategy {
    #[default]
    Skip,
    Copy,
    Overwrite,
}

/// Counts from [`ProfileStore::import_archive`].
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: usize,
    pub overwritten: usize,
    pub failed: Vec<String>,
}

fn validate_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CoreError::UserError(i18n::Message::new(
            Key::VoiceNameEmpty,
        )));
    }
    if name.chars().count() > 80 {
        return Err(CoreError::UserError(i18n::Message::new(
            Key::VoiceNameTooLong,
        )));
    }
    Ok(())
}

/// What a voice's captures add up to, for the library's warnings: the
/// sum of the takes, the worst pause/hiss, and the loudest peak.
fn aggregate_facts(takes: &[CaptureMeta]) -> ReferenceFacts {
    let mut out = ReferenceFacts::default();
    let mut total = 0.0f32;
    for take in takes {
        if let Some(f) = &take.reference {
            total += f.duration_secs;
            out.original_duration_secs += f.original_duration_secs;
            out.trimmed_secs += f.trimmed_secs;
            out.longest_pause_secs = out.longest_pause_secs.max(f.longest_pause_secs);
            out.peak = out.peak.max(f.peak);
            out.clipped_fraction = out.clipped_fraction.max(f.clipped_fraction);
        }
    }
    out.duration_secs = total;
    out
}

/// One take as written into an archive.
struct ExportTake {
    name: String,
    prompt_text: String,
    wav: Vec<u8>,
}

fn start_file(
    zip: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>,
    name: &str,
    opts: zip::write::SimpleFileOptions,
    bytes: &[u8],
) -> Result<()> {
    use std::io::Write;
    zip.start_file(name, opts)
        .map_err(|e| CoreError::Wav(format!("cannot write archive: {e}")))?;
    zip.write_all(bytes)
        .map_err(|e| CoreError::Wav(format!("cannot write archive: {e}")))
}

/// Write `captures.json` plus one WAV per take under `{prefix}captures/`,
/// so the receiving side keeps a multi-take voice in pieces instead of
/// only as the joined clip.
fn write_takes(
    zip: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>,
    opts: zip::write::SimpleFileOptions,
    prefix: &str,
    takes: &[ExportTake],
) -> Result<()> {
    let mut index = Vec::with_capacity(takes.len());
    for (i, take) in takes.iter().enumerate() {
        let file = format!(
            "{prefix}captures/{:02}-{}.wav",
            i + 1,
            archive_safe_name(&take.name)
        );
        start_file(zip, &file, opts, &take.wav)?;
        index.push(serde_json::json!({
            "file": file,
            "name": take.name,
            "prompt_text": take.prompt_text,
            "audio_sha256": sha256_hex(&take.wav),
        }));
    }
    start_file(
        zip,
        &format!("{prefix}captures.json"),
        opts,
        &serde_json::to_vec_pretty(&index)?,
    )
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

struct ArchiveVoice {
    name: String,
    prompt_text: String,
    wav: Vec<u8>,
    audio_sha256: String,
    /// Per-take audio, when the archive was written by a version that
    /// keeps takes apart. Empty for older archives.
    captures: Vec<ArchiveCapture>,
}

struct ArchiveCapture {
    name: String,
    prompt_text: String,
    wav: Vec<u8>,
}

/// Takes listed in `{dir}/captures.json`, resolved against the archive
/// index. A missing take is skipped rather than failing the import.
fn read_captures(
    files: &std::collections::HashMap<String, Vec<u8>>,
    dir: &str,
) -> Vec<ArchiveCapture> {
    let Some(index) = files.get(&format!("{dir}captures.json")) else {
        return Vec::new();
    };
    let Ok(entries) = serde_json::from_slice::<serde_json::Value>(index) else {
        return Vec::new();
    };
    let Some(entries) = entries.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries {
        let Some(file) = e.get("file").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(wav) = files.get(file.trim_start_matches('/')) else {
            continue;
        };
        out.push(ArchiveCapture {
            name: e
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
            prompt_text: e
                .get("prompt_text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
            wav: wav.clone(),
        });
    }
    out
}

/// Parse both the single-voice layout (root manifest) and the batch
/// layout (`voices.json` + `voices/*/`) into a flat voice list.
fn read_archive_voices(bytes: &[u8]) -> Result<Vec<ArchiveVoice>> {
    use std::io::Read;
    use zip::ZipArchive;

    let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| {
        CoreError::UserError(i18n::Message::new(Key::ArchiveNotZip).arg("error", e))
    })?;

    // Index: zip path -> file bytes (the archive is small, max a few
    // hundred MB of reference clips).
    let mut files: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| {
            CoreError::UserError(i18n::Message::new(Key::ArchiveUnreadable).arg("error", e))
        })?;
        if file.is_dir() {
            continue;
        }
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).map_err(|e| {
            CoreError::UserError(i18n::Message::new(Key::ArchiveUnreadable).arg("error", e))
        })?;
        files.insert(file.name().replace('\\', "/"), buf);
    }

    let mut out = Vec::new();

    if let Some(index_bytes) = files.get("voices.json") {
        let bundle: serde_json::Value = serde_json::from_slice(index_bytes)?;
        let entries = bundle
            .get("voices")
            .and_then(|v| v.as_array())
            .ok_or_else(|| CoreError::UserError(i18n::Message::new(Key::ArchiveVoicesMissing)))?;
        for e in entries {
            let dir = e
                .get("dir")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim_matches('/')
                .to_string();
            let Some(wav) = files.get(&format!("{dir}/reference.wav")) else {
                continue;
            };
            out.push(ArchiveVoice {
                name: e
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                prompt_text: e
                    .get("prompt_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                audio_sha256: e
                    .get("audio_sha256")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| sha256_hex(wav)),
                wav: wav.clone(),
                captures: read_captures(&files, &format!("{dir}/")),
            });
        }
        return Ok(out);
    }

    // Single-voice layout at the archive root.
    let manifest = files
        .get("manifest.json")
        .ok_or_else(|| CoreError::UserError(i18n::Message::new(Key::ArchiveMissingIndex)))?;
    let manifest: serde_json::Value = serde_json::from_slice(manifest)?;
    if manifest.get("format").and_then(|v| v.as_str()) != Some("laverock-voice") {
        return Err(CoreError::BadRequest(
            "不是 laverock 音色文件（format 不匹配）".into(),
        ));
    }
    let Some(wav) = files.get("reference.wav") else {
        return Err(CoreError::UserError(i18n::Message::new(
            Key::ArchiveMissingWav,
        )));
    };
    out.push(ArchiveVoice {
        name: manifest
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        prompt_text: manifest
            .get("prompt_text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        audio_sha256: sha256_hex(wav),
        wav: wav.clone(),
        captures: read_captures(&files, ""),
    });
    Ok(out)
}

/// One stored voice's identity for import de-duplication.
#[derive(Debug, Clone)]
pub struct VoiceFingerprint {
    pub audio_sha256: String,
    pub id: String,
}

/// Zip-safe directory component derived from a display name: keeps
/// Unicode letters (zip paths are UTF-8) and replaces only path/pattern
/// metacharacters and whitespace.
fn archive_safe_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_whitespace() => '-',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        "voice".to_string()
    } else {
        cleaned.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::random_suffix;
    use laverock_core::encode_wav;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_store() -> (ProfileStore, PathBuf) {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "laverock-profiles-{}-{n}-{}",
            std::process::id(),
            random_suffix()
        ));
        let store = ProfileStore::open(&dir).expect("open store");
        (store, dir)
    }

    /// 0.3 amplitude tone preceded by `leading` seconds of digital silence
    /// and followed by 0.5 s of it.
    fn wav_with_silence(leading: f32, speech: f32) -> Vec<u8> {
        let sr = 24_000u32;
        let mut samples = vec![0.0f32; (sr as f32 * leading) as usize];
        samples.extend(
            (0..(sr as f32 * speech) as usize)
                .map(|i| 0.3 * (i as f32 * 220.0 * std::f32::consts::TAU / sr as f32).sin()),
        );
        samples.extend(vec![0.0f32; (sr as f32 * 0.5) as usize]);
        encode_wav(&samples, sr as i32).expect("encode")
    }

    #[test]
    fn create_trims_dead_air_and_keeps_the_original() {
        let (store, dir) = temp_store();
        let wav = wav_with_silence(0.6, 1.0);
        let meta = store.create("Test", "text", &wav).expect("create");

        let facts = meta.reference.clone().expect("facts");
        assert!(facts.trimmed_secs > 0.5, "{facts:?}");
        assert!(
            facts.duration_secs < facts.original_duration_secs,
            "{facts:?}"
        );

        let (_, stored) = store.get(&meta.id).expect("get");
        assert!(stored.len() < wav.len(), "stored clip should be trimmed");
        // The untouched upload is kept by the take, not by the voice.
        let original = fs::read(
            dir.join("captures")
                .join(&meta.captures[0])
                .join(crate::captures::ORIGINAL_CAPTURE_FILE),
        )
        .expect("original kept");
        assert_eq!(original, wav);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn silent_reference_is_rejected() {
        let (store, dir) = temp_store();
        let wav = encode_wav(&vec![0.0f32; 24_000], 24_000).expect("encode");
        let err = store.create("Test", "text", &wav).expect_err("silent");
        assert!(matches!(err, CoreError::UserError(_)), "{err}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_path_leaves_bytes_untouched() {
        let (store, dir) = temp_store();
        let wav = wav_with_silence(0.6, 1.0);
        // clean = false is what archive import uses.
        let meta = store
            .create_impl("Imported", "text", &wav, false)
            .expect("create");
        let (_, stored) = store.get(&meta.id).expect("get");
        assert_eq!(stored, wav);
        assert_eq!(meta.reference.expect("facts").trimmed_secs, 0.0);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn replacing_a_clip_adds_a_take_and_keeps_the_old_one() {
        let (store, dir) = temp_store();
        let meta = store
            .create("Test", "text", &wav_with_silence(0.6, 1.0))
            .expect("create");
        assert_eq!(meta.captures.len(), 1);
        let first = meta.captures[0].clone();

        let tight = ws_tight_clip();
        let meta = store
            .update(&meta.id, "Test", "text", Some(&tight))
            .expect("update");
        // A clip without dead air needs no trimming.
        assert_eq!(meta.reference.expect("facts").trimmed_secs, 0.0);
        assert_eq!(meta.captures.len(), 1);
        assert_ne!(meta.captures[0], first);
        // The replaced take is still in the library, untouched.
        assert!(store.captures().get(&first).is_ok());
        fs::remove_dir_all(&dir).ok();
    }

    fn ws_tight_clip() -> Vec<u8> {
        let sr = 24_000u32;
        let samples: Vec<f32> = (0..24_000)
            .map(|i| 0.3 * (i as f32 * 220.0 * std::f32::consts::TAU / sr as f32).sin())
            .collect();
        encode_wav(&samples, sr as i32).expect("encode")
    }

    /// A tone at `hz` so takes in one test are not byte-identical.
    fn take(secs: f32, hz: f32) -> Vec<u8> {
        let sr = 24_000u32;
        let samples: Vec<f32> = (0..(sr as f32 * secs) as usize)
            .map(|i| 0.3 * (i as f32 * hz * std::f32::consts::TAU / sr as f32).sin())
            .collect();
        encode_wav(&samples, sr as i32).expect("encode")
    }

    #[test]
    fn migration_turns_a_saved_voice_into_a_single_take() {
        let (store, dir) = temp_store();
        let meta = store
            .create("Test", "text", &wav_with_silence(0.6, 1.0))
            .expect("create");
        // Roll the manifest back to the pre-library shape.
        let path = dir.join("profiles").join(&meta.id).join("profile.json");
        let mut raw: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("parse");
        raw.as_object_mut().expect("object").remove("captures");
        raw["engine_id"] = serde_json::json!("qwen3tts");
        fs::write(&path, serde_json::to_vec_pretty(&raw).expect("json")).expect("write");

        assert_eq!(store.migrate_captures(), 1);
        let (migrated, _) = store.get(&meta.id).expect("get");
        assert_eq!(migrated.captures.len(), 1);
        // The retired engine field is not written back.
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("parse");
        assert!(stored.get("engine_id").is_none());
        // Running again changes nothing.
        assert_eq!(store.migrate_captures(), 0);

        // What the engine gets is exactly the migrated take.
        let combined = store.reference(&meta.id, 15.0).expect("reference");
        let (_, wav) = store.captures().get(&migrated.captures[0]).expect("take");
        assert_eq!(combined.wav, wav);
        assert_eq!(combined.text, "text");
        // The take library shows it as one voice's worth of audio.
        let views = store.list().expect("list");
        assert_eq!(views[0].capture_count, 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn migration_drops_the_retired_engine_field() {
        let (store, dir) = temp_store();
        let meta = store
            .create("Test", "text", &take(2.0, 220.0))
            .expect("create");
        // A voice migrated by an older build still names its engine.
        let path = dir.join("profiles").join(&meta.id).join("profile.json");
        let mut raw: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("parse");
        raw["engine_id"] = serde_json::json!("qwen3tts");
        fs::write(&path, serde_json::to_vec_pretty(&raw).expect("json")).expect("write");

        // Nothing to migrate, but the stale field still goes.
        assert_eq!(store.migrate_captures(), 0);
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("parse");
        assert!(stored.get("engine_id").is_none());
        // The rest of the manifest survives the rewrite.
        assert_eq!(stored["name"], "Test");
        assert_eq!(stored["captures"].as_array().expect("captures").len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_voice_built_from_two_takes_joins_audio_and_transcript() {
        let (store, dir) = temp_store();
        let a = store
            .captures()
            .create("a", "第一段", &take(2.0, 220.0), false)
            .expect("take a");
        let b = store
            .captures()
            .create("b", "第二段", &take(2.0, 330.0), false)
            .expect("take b");
        let voice = store
            .create_from_captures("Duo", &[a.id.clone(), b.id.clone()])
            .expect("create");
        assert_eq!(voice.captures, vec![a.id.clone(), b.id.clone()]);
        assert_eq!(voice.prompt_text, "第一段 第二段");

        let combined = store.reference(&voice.id, 15.0).expect("reference");
        assert_eq!(combined.used, 2);
        assert_eq!(combined.dropped, 0);
        let (samples, rate) = decode_wav(&combined.wav).expect("decode");
        // Two 2 s takes plus the short join gap.
        let secs = samples.len() as f32 / rate as f32;
        assert!((secs - 4.12).abs() < 0.05, "{secs}");

        // A re-transcription lands on the voice too.
        store
            .set_capture_transcript(&b.id, "第二段改")
            .expect("retranscribe");
        let (voice, _) = store.get(&voice.id).expect("get");
        assert_eq!(voice.prompt_text, "第一段 第二段改");

        // Deleting a take drops it from the voice; the other stays.
        store.delete_capture(&b.id).expect("delete");
        let (voice, _) = store.get(&voice.id).expect("get");
        assert_eq!(voice.captures, vec![a.id]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn archive_keeps_takes_apart() {
        let (store, dir) = temp_store();
        let a = store
            .captures()
            .create("a", "第一段", &take(2.0, 220.0), false)
            .expect("take a");
        let b = store
            .captures()
            .create("b", "第二段", &take(2.0, 330.0), false)
            .expect("take b");
        let voice = store
            .create_from_captures("Duo", &[a.id.clone(), b.id.clone()])
            .expect("create");
        let zip = store.export_zip(&voice.id).expect("export");

        store.delete(&voice.id).expect("delete voice");
        let report = store
            .import_archive(&zip, ImportStrategy::Skip)
            .expect("import");
        assert_eq!(report.imported, 1);
        let imported = store
            .list()
            .expect("list")
            .into_iter()
            .find(|v| v.meta.name == "Duo")
            .expect("voice");
        assert_eq!(imported.capture_count, 2);
        assert_eq!(imported.meta.prompt_text, "第一段 第二段");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dedupe_repoints_voices_at_the_surviving_take() {
        let (store, dir) = temp_store();
        let wav = take(2.0, 220.0);
        let first = store
            .captures()
            .create("a", "文字", &wav, false)
            .expect("a");
        let second = store
            .captures()
            .create("b", "文字", &wav, false)
            .expect("b");
        let voice = store
            .create_from_captures("Duo", std::slice::from_ref(&second.id))
            .expect("create");

        assert_eq!(store.dedupe_captures().expect("dedupe"), 1);
        let (meta, _) = store.get(&voice.id).expect("get");
        // Both takes were made in the same second, so which one survives
        // is decided by id; what matters is that the voice follows it.
        let survivors: Vec<String> = store
            .captures()
            .list()
            .expect("list")
            .into_iter()
            .map(|c| c.meta.id)
            .collect();
        assert_eq!(survivors.len(), 1, "one duplicate take should remain");
        assert_eq!(meta.captures, survivors, "voice should follow the survivor");
        assert!(first.id == survivors[0] || second.id == survivors[0]);
        fs::remove_dir_all(&dir).ok();
    }
}
