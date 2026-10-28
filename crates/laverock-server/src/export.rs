//! Audio export formats.
//!
//! MP3 is encoded in-process by the pure-Rust, zero-dependency `rusty_mp3`
//! encoder (Apache-2.0; MPEG-1/2/2.5 Layer III), so MP3 export needs no
//! system ffmpeg/lame install. Engine output is mono at 16/24 kHz, encoded
//! as MPEG-2; a stereo WAV uploaded to `/api/audio/mp3` keeps both channels.
//! Sample rates outside the nine MPEG Layer III rates are resampled with
//! linear interpolation to the nearest supported rate.

use laverock_core::{decode_wav_channels, CoreError, Result};
use rusty_mp3::{Mp3Encoder, Mp3EncoderConfig};

const BITRATE_KBPS: u32 = 192;
const MAX_WAV_BYTES: usize = 64 * 1024 * 1024;

/// The nine sample rates legal in MPEG-1/2/2.5 Layer III.
const MP3_RATES: [u32; 9] = [
    8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000,
];

/// Whether MP3 export is available in this installation.
///
/// Always true: the encoder is compiled into the binary. Kept as a flag so
/// `/api/health` and the UI do not have to special-case the format.
pub fn mp3_available() -> bool {
    true
}

pub fn wav_to_mp3(wav: &[u8]) -> Result<Vec<u8>> {
    if wav.len() > MAX_WAV_BYTES {
        return Err(CoreError::BadRequest("音频过大，无法导出 MP3".into()));
    }
    let (pcm, channels, sample_rate) =
        decode_wav_channels(wav).map_err(|e| CoreError::Wav(format!("无法解析 WAV 音频：{e}")))?;
    if pcm.is_empty() {
        return Err(CoreError::BadRequest("WAV 中没有可编码的音频数据".into()));
    }

    let channels = channels.max(1);
    let rate = nearest_mp3_rate(sample_rate);
    let pcm = if rate == sample_rate {
        pcm
    } else {
        resample_linear(&pcm, channels as usize, sample_rate, rate)
    };

    let mut encoder = Mp3Encoder::new(Mp3EncoderConfig {
        bitrate_kbps: BITRATE_KBPS,
        vbr_quality: None,
    });
    // 192 kbps is an MPEG-1 rate; MPEG-2/2.5 (16/24 kHz engine output) snaps
    // to the nearest legal rate inside the encoder.
    encoder
        .push_pcm_f32(&pcm, channels, rate)
        .map_err(|e| CoreError::Synth(format!("MP3 编码失败：{e}")))?;
    encoder.finish();

    let mut mp3 = Vec::new();
    while let Ok(packet) = encoder.next_packet() {
        mp3.extend_from_slice(&packet);
    }
    if mp3.is_empty() {
        return Err(CoreError::Synth("MP3 编码器没有输出".into()));
    }
    Ok(mp3)
}

fn nearest_mp3_rate(rate: u32) -> u32 {
    MP3_RATES
        .iter()
        .copied()
        .min_by_key(|candidate| candidate.abs_diff(rate))
        .unwrap_or(44_100)
}

/// Linear-interpolation resample of interleaved PCM. Only the safety net for
/// uploaded WAVs at rates MP3 cannot represent; engine output at 16/24 kHz
/// passes through untouched.
fn resample_linear(pcm: &[f32], channels: usize, from: u32, to: u32) -> Vec<f32> {
    let frames = pcm.len() / channels;
    let out_frames = (frames as u64 * u64::from(to) / u64::from(from)) as usize;
    let mut out = Vec::with_capacity(out_frames * channels);
    for i in 0..out_frames {
        let position = i as f64 * f64::from(from) / f64::from(to);
        let i0 = position as usize;
        let fraction = (position - i0 as f64) as f32;
        let i1 = (i0 + 1).min(frames - 1);
        for c in 0..channels {
            let a = pcm[i0 * channels + c];
            let b = pcm[i1 * channels + c];
            out.push((a + (b - a) * fraction).clamp(-1.0, 1.0));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};
    use rusty_mp3::Mp3Decoder;
    use std::io::Cursor;

    fn tone_wav(rate: u32, channels: u16, seconds: f32) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = WavWriter::new(
                &mut cursor,
                WavSpec {
                    channels,
                    sample_rate: rate,
                    bits_per_sample: 16,
                    sample_format: SampleFormat::Int,
                },
            )
            .expect("writer");
            let frames = (rate as f32 * seconds) as i64;
            for frame in 0..frames {
                let sample =
                    ((frame as f32 / rate as f32) * 440.0 * std::f32::consts::TAU).sin() * 0.4;
                for _ in 0..channels {
                    writer
                        .write_sample((sample * 32_767.0) as i16)
                        .expect("write");
                }
            }
            writer.finalize().expect("finalize");
        }
        cursor.into_inner()
    }

    /// Drain an MP3 through rusty_mp3's own decoder and return the decoded
    /// frames.
    fn decode_mp3(mp3: &[u8]) -> Vec<rusty_mp3::DecodedAudio> {
        let mut decoder = Mp3Decoder::new();
        decoder.push(mp3);
        decoder.flush();
        let mut frames = Vec::new();
        while let Ok(frame) = decoder.next_frame() {
            frames.push(frame);
        }
        frames
    }

    fn assert_mp3_sync(mp3: &[u8]) {
        assert!(mp3.len() > 1000);
        // The stream starts with the LAME-style Info tag frame: an MPEG frame
        // sync, not an ID3v2 header.
        assert_eq!(mp3[0], 0xFF);
        assert_eq!(mp3[1] & 0xE0, 0xE0);
    }

    #[test]
    fn reports_available_without_external_tools() {
        assert!(mp3_available());
    }

    #[test]
    fn rejects_oversized_input() {
        let huge = vec![0u8; MAX_WAV_BYTES + 1];
        assert!(wav_to_mp3(&huge).is_err());
    }

    #[test]
    fn rejects_non_wav_input() {
        let err = wav_to_mp3(b"not a wav file at all").unwrap_err();
        assert!(err.to_string().contains("无法解析 WAV"));
    }

    #[test]
    fn encodes_24khz_mono_engine_output() {
        let wav = tone_wav(24_000, 1, 2.0);
        let mp3 = wav_to_mp3(&wav).expect("encode");
        assert_mp3_sync(&mp3);
        let frames = decode_mp3(&mp3);
        assert!(!frames.is_empty(), "decoder should find audio frames");
        assert!(frames.iter().all(|f| f.sample_rate == 24_000));
        assert!(frames.iter().all(|f| f.channels == 1));
    }

    #[test]
    fn encodes_16khz_mono() {
        let wav = tone_wav(16_000, 1, 1.0);
        let mp3 = wav_to_mp3(&wav).expect("encode");
        assert_mp3_sync(&mp3);
        let frames = decode_mp3(&mp3);
        assert!(frames.iter().all(|f| f.sample_rate == 16_000));
    }

    #[test]
    fn preserves_stereo() {
        let wav = tone_wav(44_100, 2, 1.0);
        let mp3 = wav_to_mp3(&wav).expect("encode");
        assert_mp3_sync(&mp3);
        let frames = decode_mp3(&mp3);
        assert!(frames.iter().all(|f| f.channels == 2));
    }

    #[test]
    fn resamples_rates_mp3_cannot_represent() {
        // 96 kHz -> nearest MPEG-1 rate (48 kHz); 7 kHz -> 8 kHz (MPEG-2.5).
        for (input, expected) in [(96_000u32, 48_000u32), (7_000, 8_000)] {
            let wav = tone_wav(input, 1, 0.5);
            let mp3 = wav_to_mp3(&wav).expect("encode");
            let frames = decode_mp3(&mp3);
            assert!(
                frames.iter().all(|f| f.sample_rate == expected),
                "{input} Hz should resample to {expected} Hz"
            );
        }
    }

    #[test]
    fn resample_preserves_length_ratio() {
        let pcm = vec![0.0f32; 24_000]; // one mono second
        let out = resample_linear(&pcm, 1, 24_000, 48_000);
        assert_eq!(out.len(), 48_000);
        let out = resample_linear(&pcm, 1, 24_000, 16_000);
        assert_eq!(out.len(), 16_000);
    }
}
