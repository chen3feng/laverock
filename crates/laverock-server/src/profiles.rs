//! Saved voices: a reference clip plus its transcript.
//!
//! Layout (one directory per profile):
//!   <data_dir>/profiles/<id>/profile.json
//!   <data_dir>/profiles/<id>/reference.wav
//!
//! A profile is deliberately engine-agnostic: the same reference audio
//! works with any cloning engine that accepts a prompt clip.

use laverock_core::{decode_wav, CoreError, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_REFERENCE_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMeta {
    pub id: String,
    pub name: String,
    pub prompt_text: String,
    #[serde(default)]
    pub engine_id: Option<String>,
    /// Unix seconds.
    pub created_at: u64,
}

/// A saved voice plus facts the library view needs without downloading
/// the clip first.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    #[serde(flatten)]
    pub meta: ProfileMeta,
    pub duration_secs: f32,
    pub bytes: u64,
}

pub struct ProfileStore {
    root: PathBuf,
}

impl ProfileStore {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let root = data_dir.as_ref().join("profiles");
        fs::create_dir_all(&root)?;
        Ok(Self { root })
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
                    let (duration_secs, bytes) = clip_facts(&entry.path().join("reference.wav"));
                    out.push(ProfileView {
                        meta,
                        duration_secs,
                        bytes,
                    });
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

    pub fn create(
        &self,
        name: &str,
        prompt_text: &str,
        engine_id: Option<String>,
        wav: &[u8],
    ) -> Result<ProfileMeta> {
        let name = name.trim();
        let prompt_text = prompt_text.trim();
        if name.is_empty() {
            return Err(CoreError::Wav("voice name must not be empty".into()));
        }
        if name.chars().count() > 80 {
            return Err(CoreError::Wav(
                "voice name is too long (max 80 chars)".into(),
            ));
        }
        if prompt_text.is_empty() {
            return Err(CoreError::Wav(
                "reference transcript must not be empty".into(),
            ));
        }
        if wav.len() > MAX_REFERENCE_BYTES {
            return Err(CoreError::Wav("reference audio exceeds 20 MB".into()));
        }
        // Reject clips that do not decode, so the library cannot fill up
        // with unusable references.
        decode_wav(wav)?;

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let id = format!("p{created_at}-{:04x}", random_suffix());

        let dir = self.root.join(&id);
        fs::create_dir_all(&dir)?;
        let meta = ProfileMeta {
            id: id.clone(),
            name: name.to_string(),
            prompt_text: prompt_text.to_string(),
            engine_id,
            created_at,
        };

        // Write the manifest last and atomically: a profile directory
        // without profile.json is ignored by list().
        fs::write(dir.join("reference.wav"), wav)?;
        write_atomic(
            &dir.join("profile.json"),
            &serde_json::to_vec_pretty(&meta)?,
        )?;
        Ok(meta)
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
        let dir = self.dir(id)?;
        let raw = fs::read_to_string(dir.join("profile.json"))?;
        let mut meta: ProfileMeta = serde_json::from_str(&raw)?;

        let name = name.trim();
        if name.is_empty() {
            return Err(CoreError::BadRequest("声音名称不能为空".into()));
        }
        if name.chars().count() > 80 {
            return Err(CoreError::BadRequest("声音名称过长（最多 80 字）".into()));
        }

        if let Some(wav) = wav {
            if wav.len() > MAX_REFERENCE_BYTES {
                return Err(CoreError::BadRequest("参考音频超过 20 MB".into()));
            }
            decode_wav(wav)?;
            fs::write(dir.join("reference.wav"), wav)?;
        }

        meta.name = name.to_string();
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
    /// `manifest.json` (format version + metadata) and `reference.wav`.
    /// The WAV is stored uncompressed — PCM already resists deflate well,
    /// and ZIP_STORED keeps packing/extraction instant.
    pub fn export_zip(&self, id: &str) -> Result<Vec<u8>> {
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        let (meta, wav) = self.get(id)?;
        let exported_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let manifest = serde_json::json!({
            "format": "laverock-voice",
            "version": 1,
            "name": meta.name,
            "prompt_text": meta.prompt_text,
            "engine_id": meta.engine_id,
            "exported_at": exported_at,
        });

        let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("manifest.json", opts)
            .map_err(|e| CoreError::Wav(format!("cannot write archive: {e}")))?;
        zip.write_all(serde_json::to_vec_pretty(&manifest)?.as_slice())
            .map_err(|e| CoreError::Wav(format!("cannot write archive: {e}")))?;
        zip.start_file("reference.wav", opts)
            .map_err(|e| CoreError::Wav(format!("cannot write archive: {e}")))?;
        zip.write_all(&wav)
            .map_err(|e| CoreError::Wav(format!("cannot write archive: {e}")))?;
        let cursor = zip
            .finish()
            .map_err(|e| CoreError::Wav(format!("cannot finish archive: {e}")))?;
        Ok(cursor.into_inner())
    }

    /// Import a `.lvvoice` archive, creating a fresh id. A name clash
    /// gets a numeric suffix. Returns the imported profile.
    pub fn import_zip(&self, bytes: &[u8]) -> Result<ProfileMeta> {
        use std::io::Read;
        use zip::ZipArchive;

        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| CoreError::BadRequest(format!("无效的音色文件（不是 ZIP）：{e}")))?;

        let mut manifest: Option<serde_json::Value> = None;
        let mut wav: Option<Vec<u8>> = None;
        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| CoreError::BadRequest(format!("无法读取音色文件：{e}")))?;
            if file.is_dir() {
                continue;
            }
            match file.name() {
                "manifest.json" => {
                    let mut buf = String::new();
                    file.read_to_string(&mut buf)
                        .map_err(|e| CoreError::BadRequest(format!("manifest 读取失败：{e}")))?;
                    manifest = Some(serde_json::from_str(&buf)?);
                }
                "reference.wav" => {
                    let mut buf = Vec::new();
                    file.read_to_end(&mut buf)
                        .map_err(|e| CoreError::BadRequest(format!("参考音频读取失败：{e}")))?;
                    wav = Some(buf);
                }
                _ => {}
            }
        }

        let manifest =
            manifest.ok_or_else(|| CoreError::BadRequest("音色文件缺少 manifest.json".into()))?;
        if manifest.get("format").and_then(|v| v.as_str()) != Some("laverock-voice") {
            return Err(CoreError::BadRequest(
                "不是 laverock 音色文件（format 不匹配）".into(),
            ));
        }
        let wav = wav.ok_or_else(|| CoreError::BadRequest("音色文件缺少 reference.wav".into()))?;
        let name = manifest
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("导入的声音")
            .trim()
            .to_string();
        let prompt_text = manifest
            .get("prompt_text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let engine_id = manifest
            .get("engine_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        // Validate the clip and enforce the size limit like a recording.
        decode_wav(&wav)?;
        if wav.len() > MAX_REFERENCE_BYTES {
            return Err(CoreError::BadRequest("参考音频超过 20 MB".into()));
        }
        let name = if name.is_empty() {
            "导入的声音".to_string()
        } else {
            self.unique_name(&name)
        };
        if prompt_text.is_empty() {
            // An imported clip may legitimately carry no transcript.
            self.create_without_transcript(&name, engine_id, &wav)
        } else {
            self.create(&name, &prompt_text, engine_id, &wav)
        }
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
        engine_id: Option<String>,
        wav: &[u8],
    ) -> Result<ProfileMeta> {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let id = format!("p{created_at}-{:04x}", random_suffix());
        let dir = self.root.join(&id);
        fs::create_dir_all(&dir)?;
        let meta = ProfileMeta {
            id: id.clone(),
            name: name.to_string(),
            prompt_text: String::new(),
            engine_id,
            created_at,
        };
        fs::write(dir.join("reference.wav"), wav)?;
        write_atomic(
            &dir.join("profile.json"),
            &serde_json::to_vec_pretty(&meta)?,
        )?;
        Ok(meta)
    }
}

/// Duration and size of a reference clip; missing or unreadable clips
/// report zero duration rather than failing the whole listing.
fn clip_facts(path: &Path) -> (f32, u64) {
    let Ok(bytes) = fs::read(path) else {
        return (0.0, 0);
    };
    let size = bytes.len() as u64;
    match decode_wav(&bytes) {
        Ok((samples, rate)) if rate > 0 => (samples.len() as f32 / rate as f32, size),
        _ => (0.0, size),
    }
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn random_suffix() -> u16 {
    // Cheap unique-enough suffix; ids also carry a second-resolution stamp.
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
