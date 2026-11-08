//! Standalone audio tools: denoise and vocal separation for any WAV.
//!
//! The capture library applies the same enhancement stages while storing a
//! take; this module is the page-level counterpart. Nothing is persisted:
//! the upload becomes a background job (so a long separation can be
//! cancelled and survives a page refresh), and the stems are downloadable
//! artifacts of that job.

use axum::{
    extract::{DefaultBodyLimit, Multipart, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use laverock_core::{
    decode_wav, encode_wav, i18n, Audio, CoreError, Enhancer, StemMode, StemRun, Stems,
};

use crate::routes::{request_lang, ApiError, AppState};

/// Largest accepted upload. The duration cap is what most oversized clips
/// hit; this bound keeps a pathological file from exhausting memory.
const MAX_UPLOAD_BYTES: usize = 200 * 1024 * 1024;
/// Multipart framing sits on top of the file itself.
const BODY_LIMIT: usize = MAX_UPLOAD_BYTES + 1024 * 1024;
/// Longest clip the tools page accepts.
const MAX_DURATION_SECS: f32 = 30.0 * 60.0;

pub(crate) fn router() -> Router<AppState> {
    Router::new().route("/api/tools", get(status)).route(
        "/api/tools/enhance",
        post(enhance).layer(DefaultBodyLimit::max(BODY_LIMIT)),
    )
}

fn discover(state: &AppState) -> Enhancer {
    Enhancer::discover(state.models.models_dir(), state.models.tools_dir())
}

/// `GET /api/tools` — which tools this installation can run.
async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let enhancer = discover(&state);
    Json(serde_json::json!({
        "denoise": enhancer.has_denoise(),
        "separation": enhancer.has_separation(),
        "mp3": crate::export::mp3_available(),
    }))
}

/// `POST /api/tools/enhance` — multipart `audio` (WAV) and `mode`
/// (`denoise`, `vocals`, `instrumental` or `both`). Returns `202` with a
/// `job_id`; follow `/api/jobs/{id}/events` for progress and artifacts.
async fn enhance(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let mut audio: Option<Vec<u8>> = None;
    let mut mode_raw = String::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("mode") => {
                mode_raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad mode: {e}")))?;
            }
            Some("audio") => {
                let mut bytes = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad audio upload: {e}")))?
                {
                    bytes.extend_from_slice(&chunk);
                }
                audio = Some(bytes);
            }
            _ => {}
        }
    }

    let mode = StemMode::parse(&mode_raw)
        .ok_or_else(|| ApiError::bad_request(format!("unknown tool mode: {mode_raw}")))?;
    let audio = audio
        .filter(|b| !b.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing required field: audio"))?;
    if audio.len() > MAX_UPLOAD_BYTES {
        return Err(user_error(i18n::Key::ToolTooLarge));
    }
    if !looks_like_wav(&audio) {
        return Err(user_error(i18n::Key::ToolWavOnly));
    }

    // Missing models are a setup problem, not a job failure: report them
    // before creating anything the UI would have to follow.
    let enhancer = discover(&state);
    if !enhancer.supports_stems(mode) {
        return Err(user_error(i18n::Key::EnhanceModelsMissing));
    }

    let job = state.jobs.create("tools")?;
    let worker = job.clone();
    let supervisor = job.clone();
    let lang = request_lang();
    let models = state.models.models_dir().to_path_buf();
    let tools = state.models.tools_dir().map(std::path::Path::to_path_buf);

    let handle = tokio::task::spawn_blocking(move || {
        worker.start();
        worker.set_progress(0.1, None);
        let (samples, rate) = match decode_wav(&audio) {
            Ok(decoded) => decoded,
            Err(e) => {
                worker.fail(&e.to_string());
                return;
            }
        };
        let rate = rate.max(1) as u32;
        if samples.len() as f32 / rate as f32 > MAX_DURATION_SECS {
            worker.fail(&i18n::Message::new(i18n::Key::ToolTooLong).render(lang));
            return;
        }
        if worker.is_cancelled() {
            worker.finish_cancelled(0);
            return;
        }
        worker.set_progress(0.2, None);

        let stage = |id: &str| {
            let key = match id {
                "separating" => i18n::Key::ToolStageSeparating,
                _ => i18n::Key::ToolStageDenoising,
            };
            let message = i18n::Message::new(key).render(lang);
            worker.emit("stage", serde_json::json!({ "message": message }));
        };
        let cancelled = || worker.is_cancelled();
        let run = StemRun {
            stage: &stage,
            cancelled: &cancelled,
        };
        // Discovery again inside the worker: the models may have been
        // removed between the request and the task starting.
        let enhancer = Enhancer::discover(&models, tools.as_deref());
        let stems = match enhancer.apply_stems_with(mode, &samples, rate, &run) {
            Ok(stems) => stems,
            Err(e) if worker.is_cancelled() => {
                worker.log(&render_error(&e, lang));
                worker.finish_cancelled(0);
                return;
            }
            Err(e) => {
                worker.fail(&render_error(&e, lang));
                return;
            }
        };
        worker.set_progress(0.85, None);

        match encode_outputs(&stems, mode) {
            Ok(outputs) => {
                for (name, bytes) in &outputs {
                    if let Err(e) = worker.artifact(name, bytes) {
                        worker.fail(&format!("cannot write {name}: {e}"));
                        return;
                    }
                }
                worker.finish(outputs.len() as u32);
            }
            Err(e) => worker.fail(&render_error(&e, lang)),
        }
    });

    // A panic inside the separator must not leave the job "running" forever.
    tokio::spawn(async move {
        if let Err(e) = handle.await {
            supervisor.fail(&format!("audio tool task failed: {e}"));
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "job_id": job.id() })),
    )
        .into_response())
}

fn user_error(key: i18n::Key) -> ApiError {
    ApiError::from(CoreError::UserError(i18n::Message::new(key)))
}

fn render_error(err: &CoreError, lang: i18n::Lang) -> String {
    match err {
        CoreError::UserError(message) => message.render(lang),
        other => other.to_string(),
    }
}

/// A WAV file starts with `RIFF` and names its form `WAVE` at offset 8.
/// The full parse still happens in the job; this only rejects the obvious
/// wrong formats before allocating a job.
fn looks_like_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

/// The downloadable files for `mode`. A single stem is a WAV; `both`
/// bundles the two stems in a ZIP.
fn encode_outputs(stems: &Stems, mode: StemMode) -> laverock_core::Result<Vec<(String, Vec<u8>)>> {
    let wav = |audio: &Audio| encode_wav(&audio.samples, audio.sample_rate);
    let instrumental = || {
        stems
            .instrumental
            .as_ref()
            .ok_or_else(|| CoreError::Synth("the separator returned no instrumental stem".into()))
    };
    match mode {
        StemMode::Denoise => Ok(vec![("denoised.wav".to_string(), wav(&stems.vocals)?)]),
        StemMode::Vocals => Ok(vec![("vocals.wav".to_string(), wav(&stems.vocals)?)]),
        StemMode::Instrumental => Ok(vec![(
            "instrumental.wav".to_string(),
            wav(instrumental()?)?,
        )]),
        StemMode::Both => {
            // The ZIP is the bundle the issue asks for; the two WAVs are
            // written alongside it so the page can play each stem and offer
            // them separately without unzipping anything.
            let vocals_wav = wav(&stems.vocals)?;
            let instrumental_wav = wav(instrumental()?)?;
            let zip = zip_stems(&vocals_wav, &instrumental_wav)?;
            Ok(vec![
                ("vocals.wav".to_string(), vocals_wav),
                ("instrumental.wav".to_string(), instrumental_wav),
                ("stems.zip".to_string(), zip),
            ])
        }
    }
}

/// Store the two WAVs uncompressed: PCM does not compress well, and the
/// archive opens instantly.
fn zip_stems(vocals: &[u8], instrumental: &[u8]) -> laverock_core::Result<Vec<u8>> {
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in [("vocals.wav", vocals), ("instrumental.wav", instrumental)] {
        zip.start_file(name, opts).map_err(zip_error)?;
        zip.write_all(bytes).map_err(CoreError::Io)?;
    }
    zip.finish()
        .map(|cursor| cursor.into_inner())
        .map_err(zip_error)
}

fn zip_error(err: zip::result::ZipError) -> CoreError {
    CoreError::Synth(format!("cannot build the stems archive: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono(seconds: f32) -> Audio {
        Audio::mono(vec![0.0; (seconds * 1000.0) as usize], 1_000)
    }

    #[test]
    fn recognizes_wav_headers() {
        assert!(looks_like_wav(b"RIFF\0\0\0\0WAVEfmt "));
        assert!(!looks_like_wav(b"ID3\0\0\0\0WAVEfmt "));
        assert!(!looks_like_wav(b"RIFF"));
        assert!(!looks_like_wav(b""));
    }

    #[test]
    fn picks_artifact_names_per_mode() {
        let stems = Stems {
            vocals: mono(0.01),
            instrumental: Some(mono(0.01)),
        };
        for (mode, names) in [
            (StemMode::Denoise, vec!["denoised.wav"]),
            (StemMode::Vocals, vec!["vocals.wav"]),
            (StemMode::Instrumental, vec!["instrumental.wav"]),
            (
                StemMode::Both,
                vec!["vocals.wav", "instrumental.wav", "stems.zip"],
            ),
        ] {
            let outputs = encode_outputs(&stems, mode).unwrap();
            let produced: Vec<&str> = outputs.iter().map(|(name, _)| name.as_str()).collect();
            assert_eq!(produced, names, "{mode:?}");
            for (_, bytes) in &outputs {
                assert!(!bytes.is_empty(), "{mode:?}");
            }
        }
    }

    #[test]
    fn both_mode_bundles_two_stems() {
        let vocals = encode_wav(&[0.1, 0.2], 16_000).unwrap();
        let instrumental = encode_wav(&[-0.1, -0.2], 16_000).unwrap();
        let bytes = zip_stems(&vocals, &instrumental).unwrap();

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 2);
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["instrumental.wav", "vocals.wav"]);
    }

    #[test]
    fn instrumental_mode_needs_the_second_stem() {
        let stems = Stems {
            vocals: mono(0.01),
            instrumental: None,
        };
        assert!(encode_outputs(&stems, StemMode::Instrumental).is_err());
        assert!(encode_outputs(&stems, StemMode::Both).is_err());
        assert!(encode_outputs(&stems, StemMode::Vocals).is_ok());
    }
}
