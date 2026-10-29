//! WAV (de)coding for reference clips and synthesis output.
//!
//! Reference audio can come from browser uploads and mic recordings;
//! everything is normalized to mono f32 for the inference API.

use crate::{CoreError, Result};
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use std::borrow::Cow;
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

/// Resample mono f32 audio to `to` Hz with linear interpolation.
///
/// MOSS-TTS's native runtime only accepts 24 kHz references, while browser
/// uploads and mic recordings arrive at 44.1/48 kHz; linear interpolation
/// is plenty for a conditioning clip and keeps the engine torch-free.
/// Returns the input unchanged when the rate already matches.
pub fn resample_mono(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || to == 0 || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = to as f64 / from as f64;
    let out_len = ((samples.len() as f64) * ratio).round() as usize;
    let last = samples.len() - 1;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 / ratio;
            let idx = pos.floor() as usize;
            if idx >= last {
                samples[last]
            } else {
                let frac = (pos - idx as f64) as f32;
                samples[idx] * (1.0 - frac) + samples[idx + 1] * frac
            }
        })
        .collect()
}

/// Upper bound (seconds) for a VoxCPM2 voice reference. Its AudioVAE
/// encoder input is a fixed 240000-sample tensor at 16 kHz (= 15 s) and the
/// reference is padded up to a patch multiple before the runtime checks its
/// length, so 14 s leaves room for that padding and resampler tails.
pub const VOXCPM2_MAX_REFERENCE_SECS: f32 = 14.0;

/// Truncate mono samples to at most `max_seconds`, cutting at the
/// lowest-energy 30 ms window found in the final 2 s before the hard cap so
/// continuous speech is not sliced mid-word. The returned `Cow` borrows the
/// input unchanged when it already fits.
pub fn trim_reference(samples: &[f32], sample_rate: u32, max_seconds: f32) -> Cow<'_, [f32]> {
    let sr = sample_rate.max(1) as usize;
    let cap = (max_seconds.max(0.0) * sr as f32).round() as usize;
    if cap == 0 || samples.len() <= cap {
        return Cow::Borrowed(samples);
    }

    let window = (0.03 * sr as f32).round() as usize; // 30 ms
    let window = window.clamp(1, cap);
    let search_start = cap.saturating_sub(2 * sr); // look back up to 2 s
    let mut best = cap - window.min(cap);
    let mut best_energy = f32::INFINITY;
    let mut pos = search_start;
    while pos + window <= cap {
        let energy = samples[pos..pos + window]
            .iter()
            .map(|s| s.abs())
            .sum::<f32>()
            / window as f32;
        // `<=` keeps the latest window on a tie, maximizing retained length.
        if energy <= best_energy {
            best_energy = energy;
            best = pos;
        }
        pos += window;
    }
    Cow::Owned(samples[..best].to_vec())
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
    fn short_reference_is_untouched() {
        let samples = vec![0.1f32; 16_000 * 5]; // 5 s
        let out = trim_reference(&samples, 16_000, 14.0);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out.len(), samples.len());
    }

    #[test]
    fn long_reference_is_cut_within_cap() {
        let sr = 16_000u32;
        // 20 s: constant 0.4 tone (no quiet boundary anywhere).
        let samples = vec![0.4f32; sr as usize * 20];
        let out = trim_reference(&samples, sr, 14.0);
        assert!(out.len() <= (14.0 * sr as f32) as usize);
        // The cut stays in the final 2 s search window (>= 11.97 s kept).
        assert!(
            out.len() >= (11.9 * sr as f32) as usize,
            "kept {}",
            out.len()
        );
    }

    #[test]
    fn long_reference_cuts_at_silence() {
        let sr = 16_000u32;
        // Loud tone for the first 13 s, then silence through 20 s.
        let mut samples = vec![0.4f32; sr as usize * 13];
        samples.extend(vec![0.0f32; sr as usize * 7]);
        let out = trim_reference(&samples, sr, 14.0);
        // The cut must land in the silent tail (>= 13 s, <= 14 s).
        assert!(out.len() >= 13 * sr as usize, "kept {}", out.len());
        assert!(
            out.len() <= (14.0 * sr as f32) as usize,
            "kept {}",
            out.len()
        );
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

    #[test]
    fn resamples_mono_by_length_ratio() {
        let samples: Vec<f32> = (0..48_000).map(|i| i as f32 / 48_000.0).collect();
        assert_eq!(resample_mono(&samples, 48_000, 24_000).len(), 24_000);
        assert_eq!(resample_mono(&samples, 48_000, 16_000).len(), 16_000);
        // Same rate is a passthrough copy.
        assert_eq!(resample_mono(&samples, 48_000, 48_000), samples);
    }
}
