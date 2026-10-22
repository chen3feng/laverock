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

pub struct ProfileStore {
    root: PathBuf,
}

impl ProfileStore {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let root = data_dir.as_ref().join("profiles");
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn list(&self) -> Result<Vec<ProfileMeta>> {
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
                Ok(meta) => out.push(meta),
                Err(e) => tracing::warn!(
                    "skipping unreadable profile {}: {e}",
                    entry.path().display()
                ),
            }
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.created_at));
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

    pub fn delete(&self, id: &str) -> Result<()> {
        let dir = self.dir(id)?;
        fs::remove_dir_all(dir)?;
        Ok(())
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
