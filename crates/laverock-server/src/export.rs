//! Audio export formats.
//!
//! MP3 encoding uses an external encoder: `ffmpeg` when available,
//! otherwise the standalone `lame` CLI. Both are small, widely packaged
//! and avoid linking an LGPL encoder into the server binary. Detection
//! looks at `LAVEROCK_FFMPEG` / `LAVEROCK_LAME`, `PATH`, and the usual
//! Homebrew/system locations (a GUI-launched app has a minimal PATH).

use laverock_core::{CoreError, Result};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::OnceLock,
};

const BITRATE_KBPS: u32 = 192;
const MAX_WAV_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoder {
    Ffmpeg,
    Lame,
}

/// Whether MP3 export is available in this installation.
pub fn mp3_available() -> bool {
    encoder().is_some()
}

pub fn wav_to_mp3(wav: &[u8]) -> Result<Vec<u8>> {
    if wav.len() > MAX_WAV_BYTES {
        return Err(CoreError::BadRequest("音频过大，无法导出 MP3".into()));
    }
    let encoder = encoder().ok_or_else(|| {
        CoreError::BadRequest("未找到 ffmpeg 或 lame，无法导出 MP3；安装其中之一后重试".into())
    })?;

    let mut cmd = match encoder {
        Encoder::Ffmpeg => {
            let mut c = Command::new(find_encoder("ffmpeg").expect("detected"));
            c.args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "wav",
                "-i",
                "pipe:0",
                "-codec:a",
                "libmp3lame",
                "-b:a",
                &format!("{BITRATE_KBPS}k"),
                "-f",
                "mp3",
                "pipe:1",
            ]);
            c
        }
        Encoder::Lame => {
            let mut c = Command::new(find_encoder("lame").expect("detected"));
            // `-` reads WAV from stdin and writes MP3 to stdout.
            c.args(["--silent", "-b", &BITRATE_KBPS.to_string(), "-", "-"]);
            c
        }
    };
    run_encoder(&mut cmd, wav)
}

fn run_encoder(cmd: &mut Command, wav: &[u8]) -> Result<Vec<u8>> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| CoreError::Synth(format!("无法启动 MP3 编码器：{e}")))?;

    let mut stdin = child.stdin.take().expect("piped stdin");
    let payload = wav.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&payload);
    });

    let output = child
        .wait_with_output()
        .map_err(|e| CoreError::Synth(format!("等待 MP3 编码器失败：{e}")))?;
    let _ = writer.join();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .lines()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(CoreError::Synth(format!(
            "MP3 编码失败（{:?}）：{}",
            output.status.code(),
            tail.trim()
        )));
    }
    if output.stdout.is_empty() {
        return Err(CoreError::Synth("MP3 编码器没有输出".into()));
    }
    Ok(output.stdout)
}

fn encoder() -> Option<Encoder> {
    static ENCODER: OnceLock<Option<Encoder>> = OnceLock::new();
    *ENCODER.get_or_init(|| {
        if find_encoder("ffmpeg").is_some() {
            Some(Encoder::Ffmpeg)
        } else if find_encoder("lame").is_some() {
            Some(Encoder::Lame)
        } else {
            None
        }
    })
}

fn find_encoder(name: &str) -> Option<PathBuf> {
    let override_key = format!("LAVEROCK_{}", name.to_ascii_uppercase());
    if let Some(path) = std::env::var_os(&override_key) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    let exe = format!("{name}{}", if cfg!(windows) { ".exe" } else { "" });
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(&exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // Finder-launched apps do not inherit the shell PATH.
    let mut dirs: Vec<PathBuf> = vec![
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
        "/usr/bin".into(),
        "/bin".into(),
    ];
    if let Some(dir) = std::env::var_os("ProgramFiles") {
        dirs.push(PathBuf::from(dir).join("ffmpeg").join("bin"));
    }
    dirs.into_iter()
        .map(|dir| dir.join(&exe))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_input() {
        let huge = vec![0u8; MAX_WAV_BYTES + 1];
        assert!(wav_to_mp3(&huge).is_err());
    }

    #[test]
    fn converts_a_silent_wav_when_an_encoder_exists() {
        if !mp3_available() {
            eprintln!("skipping: no ffmpeg/lame");
            return;
        }
        let wav = laverock_core::encode_wav(&vec![0.0; 24_000], 24_000).unwrap();
        let mp3 = wav_to_mp3(&wav).expect("encode");
        assert!(mp3.len() > 1000);
        // ID3 tag or an MPEG frame sync at the start.
        assert!(mp3.starts_with(b"ID3") || (mp3[0] == 0xFF && mp3[1] & 0xE0 == 0xE0));
    }
}
