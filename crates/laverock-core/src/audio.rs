//! WAV (de)coding for reference clips and synthesis output.
//!
//! Reference audio can come from browser uploads and mic recordings;
//! everything is normalized to mono f32 for the inference API.

use crate::{CoreError, Result};
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use std::io::Cursor;

/// Decode any WAV hound can read to mono f32 plus its sample rate.
/// Stereo is averaged to mono.
pub fn decode_wav(bytes: &[u8]) -> Result<(Vec<f32>, i32)> {
    let reader = WavReader::new(Cursor::new(bytes))
        .map_err(|e| CoreError::Wav(format!("not a readable WAV: {e}")))?;
    let spec: WavSpec = reader.spec();
    let channels = spec.channels.max(1) as usize;

    let samples: Vec<f32> = match spec.sample_format {
        SampleFormat::Int => reader
            .into_samples::<i32>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Wav(format!("sample decode failed: {e}")))?
            .iter()
            .map(|&s| s as f32 / i32::MAX as f32)
            .collect(),
        SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Wav(format!("sample decode failed: {e}")))?,
    };

    // Downmix to mono.
    let mono = if channels == 1 {
        samples
    } else {
        samples
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    Ok((mono, spec.sample_rate as i32))
}

/// Encode mono f32 samples as 16-bit PCM WAV.
pub fn encode_wav(samples: &[f32], sample_rate: i32) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = WavWriter::new(
            &mut cursor,
            WavSpec {
                channels: 1,
                sample_rate: sample_rate as u32,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .map_err(|e| CoreError::Wav(format!("encoder init failed: {e}")))?;
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            writer
                .write_sample(v)
                .map_err(|e| CoreError::Wav(format!("write failed: {e}")))?;
        }
        writer
            .finalize()
            .map_err(|e| CoreError::Wav(format!("finalize failed: {e}")))?;
    }
    Ok(cursor.into_inner())
}
