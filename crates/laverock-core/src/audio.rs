//! WAV (de)coding for reference clips and synthesis output.
//!
//! Reference audio can come from browser uploads and mic recordings;
//! everything is normalized to mono f32 for the inference API.

use crate::{CoreError, Result};
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use std::io::Cursor;

/// Read a WAV into interleaved normalized f32 samples plus its stream spec.
fn read_interleaved(bytes: &[u8]) -> Result<(Vec<f32>, WavSpec)> {
    let reader = WavReader::new(Cursor::new(bytes))
        .map_err(|e| CoreError::Wav(format!("not a readable WAV: {e}")))?;
    let spec: WavSpec = reader.spec();

    // hound returns integer samples at their native width (a 16-bit file
    // yields ±32767, not ±2^31), so the scale depends on the bit depth.
    // Dividing everything by i32::MAX turns 16-bit audio into silence.
    let int_scale = match spec.bits_per_sample {
        8 => 1.0 / 128.0,
        16 => 1.0 / 32_768.0,
        24 => 1.0 / 8_388_608.0,
        32 => 1.0 / 2_147_483_648.0,
        _ => 1.0 / 32_768.0,
    };

    let samples: Vec<f32> = match spec.sample_format {
        SampleFormat::Int => reader
            .into_samples::<i32>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Wav(format!("sample decode failed: {e}")))?
            .iter()
            .map(|&s| s as f32 * int_scale)
            .collect(),
        SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Wav(format!("sample decode failed: {e}")))?,
    };
    Ok((samples, spec))
}

/// Decode any WAV hound can read to mono f32 plus its sample rate.
/// Stereo is averaged to mono.
pub fn decode_wav(bytes: &[u8]) -> Result<(Vec<f32>, i32)> {
    let (samples, spec) = read_interleaved(bytes)?;
    let channels = spec.channels.max(1) as usize;

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

/// Decode a WAV to interleaved f32 keeping its spatial channels, plus the
/// channel count and native sample rate. Mono and stereo pass through; MP3
/// has no surround, so three or more channels are downmixed to stereo
/// (even-indexed channels to left, odd to right).
pub fn decode_wav_channels(bytes: &[u8]) -> Result<(Vec<f32>, u16, u32)> {
    let (samples, spec) = read_interleaved(bytes)?;
    let channels = spec.channels.max(1) as usize;
    if channels <= 2 {
        return Ok((samples, channels as u16, spec.sample_rate));
    }

    let mut stereo = Vec::with_capacity(samples.len() / channels * 2);
    for frame in samples.chunks(channels) {
        let (mut left, mut right) = (0.0f32, 0.0f32);
        let (mut nl, mut nr) = (0usize, 0usize);
        for (i, &sample) in frame.iter().enumerate() {
            if i % 2 == 0 {
                left += sample;
                nl += 1;
            } else {
                right += sample;
                nr += 1;
            }
        }
        stereo.push(left / nl.max(1) as f32);
        stereo.push(right / nr.max(1) as f32);
    }
    Ok((stereo, 2, spec.sample_rate))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_amplitude() {
        let samples = vec![0.0, 0.5, -0.5, 0.99, -0.99];
        let bytes = encode_wav(&samples, 24_000).expect("encode");
        let (decoded, rate) = decode_wav(&bytes).expect("decode");
        assert_eq!(rate, 24_000);
        for (a, b) in samples.iter().zip(decoded.iter()) {
            assert!(
                (a - b).abs() < 0.001,
                "amplitude changed: {a} -> {b} (16-bit input must not be scaled by 2^31)"
            );
        }
    }

    #[test]
    fn downmixes_stereo() {
        // Two channels: +0.5 and -0.5 average to silence.
        let spec = WavSpec {
            channels: 2,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut w = WavWriter::new(&mut cursor, spec).expect("writer");
            for v in [16_384i16, -16_384, 8_192, 8_192] {
                w.write_sample(v).expect("write");
            }
            w.finalize().expect("finalize");
        }
        let (decoded, rate) = decode_wav(cursor.get_ref()).expect("decode");
        assert_eq!(rate, 16_000);
        assert_eq!(decoded.len(), 2);
        assert!(decoded[0].abs() < 0.001, "stereo downmix should cancel");
        assert!((decoded[1] - 0.25).abs() < 0.001, "should average to 0.25");
    }

    #[test]
    fn keeps_mono_and_stereo_channels() {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut w = WavWriter::new(
                &mut cursor,
                WavSpec {
                    channels: 2,
                    sample_rate: 44_100,
                    bits_per_sample: 16,
                    sample_format: SampleFormat::Int,
                },
            )
            .expect("writer");
            // One stereo frame: left 0.5, right -0.5.
            w.write_sample(16_384i16).expect("write");
            w.write_sample(-16_384i16).expect("write");
            w.finalize().expect("finalize");
        }
        let (pcm, channels, rate) = decode_wav_channels(cursor.get_ref()).expect("decode");
        assert_eq!(channels, 2);
        assert_eq!(rate, 44_100);
        assert_eq!(pcm.len(), 2);
        assert!((pcm[0] - 0.5).abs() < 0.001);
        assert!((pcm[1] + 0.5).abs() < 0.001);
    }

    #[test]
    fn downmixes_surround_to_stereo() {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut w = WavWriter::new(
                &mut cursor,
                WavSpec {
                    channels: 4,
                    sample_rate: 48_000,
                    bits_per_sample: 16,
                    sample_format: SampleFormat::Int,
                },
            )
            .expect("writer");
            // One frame: L 0.4, R -0.4, BL 0.2, BR -0.2.
            for v in [13_107i16, -13_107, 6_554, -6_554] {
                w.write_sample(v).expect("write");
            }
            w.finalize().expect("finalize");
        }
        let (pcm, channels, _) = decode_wav_channels(cursor.get_ref()).expect("decode");
        assert_eq!(channels, 2);
        assert_eq!(pcm.len(), 2);
        // Even-indexed channels (0, 2) -> left, odd (1, 3) -> right.
        assert!((pcm[0] - 0.3).abs() < 0.001, "left = (0.4 + 0.2) / 2");
        assert!((pcm[1] + 0.3).abs() < 0.001, "right = (-0.4 + -0.2) / 2");
    }
}
