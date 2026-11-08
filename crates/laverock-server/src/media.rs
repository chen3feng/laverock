//! System `ffmpeg` discovery, media probing and audio extraction.
//!
//! Video projects need an external `ffmpeg`/`ffprobe`: the server decodes
//! WAV itself, but anything else (MP4, MOV, MKV, MP3, M4A …) is converted to
//! the WAV the rest of the pipeline expects. The binaries come from, in
//! order, the path in Settings, `LAVEROCK_FFMPEG`, or `PATH`.

use laverock_core::{i18n, CoreError, Result};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// What `ffprobe` found in an uploaded file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaInfo {
    /// Container duration in seconds; 0 when the container does not say.
    pub duration: f32,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub has_video: bool,
    pub has_audio: bool,
}

impl Ffmpeg {
    /// Resolve the two binaries. `configured` is the Settings value: a
    /// directory, the `ffmpeg` executable itself, or a bare name looked up
    /// in `PATH`. When it is empty, `LAVEROCK_FFMPEG` is consulted, then
    /// `PATH`.
    pub fn discover(configured: &str) -> Option<Self> {
        let configured = configured.trim();
        if !configured.is_empty() {
            return Self::from_configured(configured);
        }
        let from_env = std::env::var("LAVEROCK_FFMPEG").unwrap_or_default();
        let from_env = from_env.trim();
        if !from_env.is_empty() {
            return Self::from_configured(from_env);
        }
        Some(Self {
            ffmpeg: find_in_path("ffmpeg")?,
            ffprobe: find_in_path("ffprobe")?,
        })
    }

    fn from_configured(raw: &str) -> Option<Self> {
        let raw_path = PathBuf::from(raw);
        if raw_path.is_dir() {
            return Some(Self {
                ffmpeg: find_in_dir(&raw_path, "ffmpeg")?,
                ffprobe: find_in_dir(&raw_path, "ffprobe").or_else(|| find_in_path("ffprobe"))?,
            });
        }
        // A bare name such as `ffmpeg` means "look in PATH".
        if raw_path.components().count() == 1 {
            return Some(Self {
                ffmpeg: find_in_path(raw)?,
                ffprobe: find_in_path("ffprobe")?,
            });
        }
        let path = with_exe_suffix(raw_path);
        if !is_executable(&path) {
            return None;
        }
        let dir = path.parent()?;
        Some(Self {
            ffprobe: find_in_dir(dir, "ffprobe").or_else(|| find_in_path("ffprobe"))?,
            ffmpeg: path,
        })
    }

    /// Read duration and stream layout with `ffprobe`.
    pub fn probe(&self, input: &Path) -> Result<MediaInfo> {
        let output = Command::new(&self.ffprobe)
            .args([
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
            ])
            .arg(input)
            .output()
            .map_err(|e| CoreError::UserError(media_failed(format!("ffprobe: {e}"))))?;
        if !output.status.success() {
            return Err(CoreError::UserError(media_failed(stderr_detail(
                &output.stderr,
            ))));
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| CoreError::UserError(media_failed(format!("ffprobe: {e}"))))?;

        let mut info = MediaInfo {
            duration: number(value.pointer("/format/duration")),
            ..MediaInfo::default()
        };
        if let Some(streams) = value.get("streams").and_then(|v| v.as_array()) {
            for stream in streams {
                match stream.get("codec_type").and_then(|v| v.as_str()) {
                    Some("audio") => info.has_audio = true,
                    Some("video") => {
                        // Cover art is a video stream too; it is not a picture.
                        let attached = stream
                            .pointer("/disposition/attached_pic")
                            .and_then(|v| v.as_i64())
                            == Some(1);
                        if attached {
                            continue;
                        }
                        info.has_video = true;
                        info.width =
                            stream.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        info.height =
                            stream.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        info.fps = parse_rate(stream)
                            .or_else(|| parse_rate_key(stream, "r_frame_rate"))
                            .unwrap_or(0.0);
                        if info.duration <= 0.0 {
                            info.duration = number(stream.get("duration"));
                        }
                    }
                    _ => {}
                }
            }
        }
        if !info.duration.is_finite() || info.duration < 0.0 {
            info.duration = 0.0;
        }
        Ok(info)
    }

    /// Extract the first audio stream as mono 48 kHz PCM WAV.
    pub fn extract_audio(&self, input: &Path, output: &Path) -> Result<()> {
        let result = Command::new(&self.ffmpeg)
            .args(["-y", "-v", "error", "-i"])
            .arg(input)
            .args(["-vn", "-map", "0:a:0?", "-ac", "1", "-ar", "48000"])
            .args(["-c:a", "pcm_s16le"])
            .arg(output)
            .output()
            .map_err(|e| CoreError::UserError(media_failed(format!("ffmpeg: {e}"))))?;
        if !result.status.success() {
            return Err(CoreError::UserError(media_failed(stderr_detail(
                &result.stderr,
            ))));
        }
        Ok(())
    }

    /// Mux the finished audio (and optional soft subtitles) back into the
    /// source video. The picture is copied, never re-encoded.
    pub fn mux_video(&self, opts: &MuxOptions) -> Result<()> {
        let mut cmd = Command::new(&self.ffmpeg);
        cmd.args(["-y", "-v", "error", "-i"]).arg(opts.video);
        let mut next_input = 1;
        let audio_index = opts.audio.map(|audio| {
            let index = next_input;
            cmd.arg("-i").arg(audio);
            next_input += 1;
            index
        });
        let subtitle_index = opts.subtitles.map(|subs| {
            let index = next_input;
            cmd.arg("-i").arg(subs);
            index
        });

        cmd.args(["-map", "0:v:0"]);
        match audio_index {
            Some(index) => {
                cmd.arg("-map").arg(format!("{index}:a:0"));
            }
            // `?` keeps a source without audio playable.
            None => {
                cmd.args(["-map", "0:a:0?"]);
            }
        }
        if let Some(index) = subtitle_index {
            cmd.arg("-map").arg(format!("{index}:0"));
        }
        cmd.args(["-c:v", "copy"]);
        match audio_index {
            Some(_) => {
                cmd.args(["-c:a", opts.audio_codec, "-b:a", "192k"]);
            }
            None => {
                cmd.args(["-c:a", "copy"]);
            }
        }
        if let Some(codec) = opts.subtitle_codec {
            cmd.args(["-c:s", codec]);
            if let Some(language) = opts.subtitle_language.filter(|l| !l.is_empty()) {
                cmd.arg("-metadata:s:s:0")
                    .arg(format!("language={language}"));
            }
        }
        // End the audio and the picture together, even when the mix runs
        // past the video.
        if opts.duration > 0.0 {
            cmd.arg("-t").arg(format!("{:.3}", opts.duration));
        }
        cmd.arg(opts.output);

        let result = cmd
            .output()
            .map_err(|e| CoreError::UserError(export_failed(format!("ffmpeg: {e}"))))?;
        if !result.status.success() {
            return Err(CoreError::UserError(export_failed(stderr_detail(
                &result.stderr,
            ))));
        }
        Ok(())
    }
}

/// Inputs for [`Ffmpeg::mux_video`].
pub struct MuxOptions<'a> {
    pub video: &'a Path,
    /// `None` keeps the source's own audio track.
    pub audio: Option<&'a Path>,
    pub subtitles: Option<&'a Path>,
    /// Codec the output container accepts, from [`subtitle_codec`].
    pub subtitle_codec: Option<&'a str>,
    pub subtitle_language: Option<&'a str>,
    /// Audio codec for a replaced track, from [`audio_codec`].
    pub audio_codec: &'a str,
    /// Output length in seconds; 0 leaves the streams alone.
    pub duration: f32,
    pub output: &'a Path,
}

/// The output container for a source extension: the same one when we can
/// write it, Matroska otherwise (it holds almost any codec).
pub fn export_container(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => "mp4",
        "mov" => "mov",
        "webm" => "webm",
        "mkv" => "mkv",
        _ => "mkv",
    }
}

/// Soft-subtitle codec for a container, or `None` when it cannot hold one.
pub fn subtitle_codec(container: &str) -> Option<&'static str> {
    match container {
        "mp4" | "mov" => Some("mov_text"),
        "mkv" => Some("srt"),
        "webm" => Some("webvtt"),
        _ => None,
    }
}

/// Audio codec accepted by the output container.
pub fn audio_codec(container: &str) -> &'static str {
    if container == "webm" {
        "libopus"
    } else {
        "aac"
    }
}

fn media_failed(detail: String) -> i18n::Message {
    i18n::Message::new(i18n::Key::ProjectMediaFailed).arg("detail", detail)
}

fn export_failed(detail: String) -> i18n::Message {
    i18n::Message::new(i18n::Key::ProjectExportFailed).arg("detail", detail)
}

fn stderr_detail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    if text.is_empty() {
        return "ffmpeg exited with an error".to_string();
    }
    // One line keeps the toast readable; the full log is rarely useful.
    let line = text.lines().last().unwrap_or(text).trim();
    line.chars().take(300).collect()
}

fn number(value: Option<&serde_json::Value>) -> f32 {
    value
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| v.as_f64())
        })
        .map(|v| v as f32)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.0)
}

/// `avg_frame_rate` is a rational such as `30000/1001`.
fn parse_rate(stream: &serde_json::Value) -> Option<f32> {
    parse_rate_key(stream, "avg_frame_rate")
}

fn parse_rate_key(stream: &serde_json::Value, key: &str) -> Option<f32> {
    let raw = stream.get(key)?.as_str()?;
    let (num, den) = raw.split_once('/').unwrap_or((raw, "1"));
    let num: f64 = num.parse().ok()?;
    let den: f64 = den.parse().ok()?;
    if num <= 0.0 || den <= 0.0 {
        return None;
    }
    Some((num / den) as f32)
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .map(with_exe_suffix)
        .find(|candidate| is_executable(candidate))
}

fn find_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let candidate = with_exe_suffix(dir.join(name));
    is_executable(&candidate).then_some(candidate)
}

/// Windows resolves executables through `PATHEXT`; the two binaries are
/// always `.exe` in practice, so one suffix is enough here.
fn with_exe_suffix(path: PathBuf) -> PathBuf {
    if cfg!(windows) && path.extension().is_none() {
        path.with_extension("exe")
    } else {
        path
    }
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rational_frame_rates() {
        let stream = serde_json::json!({ "avg_frame_rate": "30000/1001" });
        let fps = parse_rate(&stream).unwrap();
        assert!((fps - 29.97).abs() < 0.01);

        let stream = serde_json::json!({ "avg_frame_rate": "25/1" });
        assert_eq!(parse_rate(&stream), Some(25.0));

        for bad in ["0/0", "abc", "25/0"] {
            let stream = serde_json::json!({ "avg_frame_rate": bad });
            assert_eq!(parse_rate(&stream), None, "{bad}");
        }
    }

    #[test]
    fn reads_numeric_and_string_durations() {
        assert_eq!(number(Some(&serde_json::json!("12.5"))), 12.5);
        assert_eq!(number(Some(&serde_json::json!(3.0))), 3.0);
        assert_eq!(number(Some(&serde_json::json!("N/A"))), 0.0);
        assert_eq!(number(None), 0.0);
    }

    #[test]
    fn missing_binaries_are_not_discovered() {
        // A path that cannot exist must not resolve to something else.
        let fake = std::env::temp_dir().join("laverock-no-such-ffmpeg-dir");
        assert!(Ffmpeg::discover(&fake.to_string_lossy()).is_none());
    }

    #[test]
    fn picks_a_container_that_matches_the_source() {
        for (ext, container) in [
            ("mp4", "mp4"),
            ("M4V", "mp4"),
            ("mov", "mov"),
            ("mkv", "mkv"),
            ("webm", "webm"),
            ("avi", "mkv"),
            ("", "mkv"),
        ] {
            assert_eq!(export_container(ext), container, "{ext}");
        }
    }

    #[test]
    fn only_some_containers_hold_soft_subtitles() {
        assert_eq!(subtitle_codec("mp4"), Some("mov_text"));
        assert_eq!(subtitle_codec("mov"), Some("mov_text"));
        assert_eq!(subtitle_codec("mkv"), Some("srt"));
        assert_eq!(subtitle_codec("webm"), Some("webvtt"));
        assert_eq!(subtitle_codec("mkv"), Some("srt"));
        assert_eq!(subtitle_codec("avi"), None);
        assert_eq!(audio_codec("webm"), "libopus");
        assert_eq!(audio_codec("mp4"), "aac");
    }
}
