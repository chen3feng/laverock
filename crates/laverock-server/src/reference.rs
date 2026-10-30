//! Reference-clip ingest shared by the capture and voice stores.
//!
//! Both stores keep a validated, measured WAV per entry. Ingest trims the
//! dead air at the ends, re-measures the clip, and remembers what it found
//! so the library can warn about clips that will condition synthesis
//! badly. Content hashes and the on-disk id/naming helpers live here too:
//! they must agree between the two stores, because a capture can be moved
//! into a voice without touching a byte.

use laverock_core::i18n::{self, Key};
use laverock_core::{decode_wav, encode_wav, silence_profile, trim_silence, CoreError, Result};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_REFERENCE_BYTES: usize = 20 * 1024 * 1024;

/// Silence kept at each end when a recording is trimmed at ingest. Dead
/// air carries no timbre and conditioning models (MOSS-TTS in particular)
/// pick up the rhythm of their prompt, so the margins are kept small.
pub const REFERENCE_EDGE_KEEP_SECS: f32 = 0.10;

/// What ingest measured on a clip. Stored in the manifest so the library
/// can warn about clips that will condition synthesis badly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReferenceFacts {
    /// Duration of the stored (possibly trimmed) clip.
    pub duration_secs: f32,
    /// Length of the clip as originally recorded/uploaded.
    pub original_duration_secs: f32,
    /// Dead air removed from the start and end at ingest.
    pub trimmed_secs: f32,
    /// Longest pause between words, measured on the original clip.
    pub longest_pause_secs: f32,
    /// Peak amplitude of the original clip (1.0 = full scale).
    pub peak: f32,
    /// Fraction of original samples at or near full scale.
    pub clipped_fraction: f32,
}

/// Duration and size of a stored clip; missing or unreadable clips
/// report zero duration rather than failing the whole listing.
pub fn clip_facts(path: &Path) -> (f32, u64) {
    let Ok(bytes) = fs::read(path) else {
        return (0.0, 0);
    };
    let size = bytes.len() as u64;
    match decode_wav(&bytes) {
        Ok((samples, rate)) if rate > 0 => (samples.len() as f32 / rate as f32, size),
        _ => (0.0, size),
    }
}

pub fn check_size(wav: &[u8]) -> Result<()> {
    if wav.len() > MAX_REFERENCE_BYTES {
        return Err(CoreError::UserError(i18n::Message::new(
            Key::ReferenceTooLarge,
        )));
    }
    Ok(())
}

/// A clip after validation and ingest processing.
pub struct PreparedReference {
    /// Bytes to store as the clip.
    pub wav: Vec<u8>,
    /// The untouched upload, kept alongside when the clip was trimmed.
    pub original: Option<Vec<u8>>,
    pub facts: ReferenceFacts,
}

/// Measure a clip. `stored_len` is the length of the clip actually kept
/// (equal to `samples.len()` when nothing was trimmed).
pub fn facts_for(samples: &[f32], sample_rate: u32, stored_len: usize) -> ReferenceFacts {
    let rate = sample_rate.max(1) as f32;
    let silence = silence_profile(samples, sample_rate.max(1));
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let clipped = samples.iter().filter(|s| s.abs() >= 0.999).count() as f32;
    ReferenceFacts {
        duration_secs: stored_len as f32 / rate,
        original_duration_secs: samples.len() as f32 / rate,
        trimmed_secs: samples.len().saturating_sub(stored_len) as f32 / rate,
        longest_pause_secs: silence.longest_internal_pause_secs,
        peak,
        clipped_fraction: clipped / samples.len().max(1) as f32,
    }
}

/// Validate a clip, measure it, and — when `clean` is set — trim the dead
/// air at its ends. Internal pauses are left alone: they are part of the
/// recording's rhythm and engines that cannot cope with them clean their
/// own prompt (see the MOSS-TTS engine).
pub fn prepare(wav: &[u8], clean: bool) -> Result<PreparedReference> {
    let (samples, sample_rate) = decode_wav(wav)?;
    let rate = sample_rate.max(1) as u32;

    let trimmed = if clean {
        trim_silence(&samples, rate, REFERENCE_EDGE_KEEP_SECS)
    } else {
        Cow::Borrowed(samples.as_slice())
    };
    if trimmed.is_empty() {
        return Err(CoreError::UserError(i18n::Message::new(
            Key::ReferenceSilent,
        )));
    }

    let removed = samples.len().saturating_sub(trimmed.len());
    // Re-encode only when a meaningful amount of dead air was removed:
    // untouched clips stay byte-identical, which keeps the content
    // fingerprints used for import de-duplication stable.
    let (stored, original) = if clean && removed > (rate / 50) as usize {
        (encode_wav(&trimmed, sample_rate)?, Some(wav.to_vec()))
    } else {
        (wav.to_vec(), None)
    };

    Ok(PreparedReference {
        wav: stored,
        original,
        facts: facts_for(&samples, rate, trimmed.len()),
    })
}

/// Lowercase hex SHA-256 of a clip.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Ids name a directory; they are generated by [`new_id`] and only
/// escaped input ever reaches this check.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `<prefix><seconds>-<4 hex>`, unique enough within one data directory.
pub fn new_id(prefix: char) -> String {
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{prefix}{created_at}-{:04x}", random_suffix())
}

pub fn random_suffix() -> u16 {
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

/// Manifests are `*.json`, so the temp file lands next to them as
/// `profile.json.tmp` / `capture.json.tmp`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_keeps_untouched_bytes() {
        let sr = 24_000u32;
        let samples: Vec<f32> = (0..24_000)
            .map(|i| 0.3 * (i as f32 * 220.0 * std::f32::consts::TAU / sr as f32).sin())
            .collect();
        let wav = encode_wav(&samples, sr as i32).expect("encode");
        let kept = prepare(&wav, true).expect("prepare");
        assert_eq!(kept.wav, wav);
        assert!(kept.original.is_none());
        assert_eq!(kept.facts.trimmed_secs, 0.0);
    }
}
