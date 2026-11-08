//! Subtitle projects: create from a clip, transcribe, edit cues, export.

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use laverock_core::{decode_wav, encode_wav, i18n, CoreError};

use super::{request_lang, subtitle_response, ApiError, AppState};
use crate::{
    dubbing,
    projects::{self, Mix, Project, Segment},
    subtitles,
};

/// A source clip is decoded in memory; 30 minutes of 48 kHz stereo is about
/// 330 MB, so this is generous for the WAV files the page accepts today.
const MAX_SOURCE_BYTES: usize = 200 * 1024 * 1024;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/projects",
            get(list_projects)
                .post(create_project)
                .layer(DefaultBodyLimit::max(MAX_SOURCE_BYTES + 1024 * 1024)),
        )
        .route(
            "/api/projects/{id}",
            get(get_project)
                .patch(update_project)
                .delete(delete_project),
        )
        .route("/api/projects/{id}/source", get(project_source))
        .route("/api/projects/{id}/transcribe", post(transcribe_project))
        .route("/api/projects/{id}/subtitles", get(download_subtitles))
        .route("/api/projects/{id}/synthesize", post(synthesize_project))
        .route("/api/projects/{id}/mix", post(mix_project))
        .route(
            "/api/projects/{id}/segments/{segment}/audio",
            get(segment_audio),
        )
        .route("/api/projects/{id}/files/{name}", get(project_file))
}

async fn list_projects(State(state): State<AppState>) -> Result<Json<Vec<Project>>, ApiError> {
    Ok(Json(state.projects.list()?))
}

/// `POST /api/projects` — multipart: `audio` (WAV, required), `name`, and
/// optional `subtitles` (SRT/VTT text). With subtitles the cues are
/// imported; otherwise the project starts empty and can be transcribed.
async fn create_project(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<Project>, ApiError> {
    let mut name = String::new();
    let mut audio: Option<Vec<u8>> = None;
    let mut subtitle_text = String::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("name") => {
                name = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad name: {e}")))?;
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
            Some("subtitles") => {
                subtitle_text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad subtitles: {e}")))?;
            }
            _ => {}
        }
    }

    let audio = audio
        .filter(|bytes| !bytes.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing required field: audio"))?;
    if !is_wav(&audio) {
        return Err(CoreError::UserError(i18n::Message::new(i18n::Key::ProjectWavOnly)).into());
    }
    let segments = if subtitle_text.trim().is_empty() {
        Vec::new()
    } else {
        let parsed = subtitles::parse_cues(&subtitle_text);
        if parsed.is_empty() {
            return Err(ApiError::bad_request(
                "no subtitle cues found in the imported file",
            ));
        }
        projects::from_transcript(&parsed)
    };
    let name = if name.trim().is_empty() {
        "Untitled".to_string()
    } else {
        name
    };

    // Decoding a long clip is blocking, and the bytes must stay for storage;
    // do both in one worker so the upload is not copied.
    let store = state.projects.clone();
    let project = tokio::task::spawn_blocking(move || {
        let (samples, rate) = decode_wav(&audio)?;
        let duration = samples.len() as f32 / rate.max(1) as f32;
        store.create(&name, &audio, duration, segments)
    })
    .await
    .map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("project task panicked: {e}"),
    })??;
    Ok(Json(project))
}

async fn get_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Project>, ApiError> {
    Ok(Json(state.projects.get(&id)?))
}

#[derive(Debug, serde::Deserialize)]
struct ProjectPatch {
    name: Option<String>,
    source_language: Option<String>,
    target_language: Option<String>,
    engine_id: Option<String>,
    voice_id: Option<String>,
    mix: Option<Mix>,
    segments: Option<Vec<Segment>>,
}

async fn update_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ProjectPatch>,
) -> Result<Json<Project>, ApiError> {
    // Validate the dubbing choices up front so the editor's save fails on a
    // stale voice instead of the synthesize job failing later.
    if let Some(engine) = non_empty(&body.engine_id) {
        check_engine(&state, engine)?;
    }
    if let Some(voice) = non_empty(&body.voice_id) {
        check_voice(&state, voice)?;
    }
    let project = state.projects.update(
        &id,
        body.name.as_deref(),
        body.source_language.as_deref(),
        body.target_language.as_deref(),
        body.engine_id.as_deref(),
        body.voice_id.as_deref(),
        body.mix,
        body.segments,
    )?;
    Ok(Json(project))
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// A dubbing engine must exist. An unknown id is a client error (the editor
/// may be holding a stale choice), not a missing resource.
fn check_engine(state: &AppState, engine: &str) -> Result<(), ApiError> {
    match state.manager.parameters(engine) {
        Ok(_) => Ok(()),
        Err(CoreError::NotFound(_)) => Err(ApiError::bad_request(format!(
            "unknown synthesis engine: {engine}"
        ))),
        Err(e) => Err(e.into()),
    }
}

fn check_voice(state: &AppState, voice: &str) -> Result<(), ApiError> {
    match state.profiles.get(voice) {
        Ok(_) => Ok(()),
        Err(CoreError::NotFound(_)) => {
            Err(ApiError::bad_request(format!("unknown voice: {voice}")))
        }
        Err(e) => Err(e.into()),
    }
}

async fn delete_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.projects.delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The clip the project was created from, for the editor's player.
async fn project_source(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let (_, bytes) = state.projects.source(&id)?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "audio/wav".to_string())],
        bytes,
    )
        .into_response())
}

/// Run speech recognition over the project's source and replace its cues.
async fn transcribe_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Project>, ApiError> {
    let (_, bytes) = state.projects.source(&id)?;
    if !is_wav(&bytes) {
        return Err(CoreError::UserError(i18n::Message::new(i18n::Key::ProjectWavOnly)).into());
    }
    let transcriber = state.transcriber.clone();
    let transcript = tokio::task::spawn_blocking(move || transcriber.transcribe_detailed(&bytes))
        .await
        .map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("transcribe task panicked: {e}"),
        })??;
    if transcript.segments.is_empty() {
        return Err(ApiError::bad_request("no speech recognized"));
    }
    let segments = projects::from_transcript(&transcript.segments);
    Ok(Json(state.projects.set_segments(&id, segments)?))
}

#[derive(Debug, serde::Deserialize)]
struct SubtitleQuery {
    /// `srt` (default) or `vtt`.
    format: Option<String>,
}

async fn download_subtitles(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<SubtitleQuery>,
) -> Result<Response, ApiError> {
    let raw = query.format.as_deref().unwrap_or("");
    let format = subtitles::Format::parse(raw)
        .ok_or_else(|| ApiError::bad_request(format!("unknown subtitle format: {raw}")))?;
    let project = state.projects.get(&id)?;
    let cues = projects::cues(&project);
    if cues.is_empty() {
        return Err(CoreError::UserError(i18n::Message::new(i18n::Key::ProjectNoCues)).into());
    }
    Ok(subtitle_response(&cues, format, "laverock"))
}

/// `POST /api/projects/{id}/synthesize` — dubbing job. The engine and voice
/// come from the saved project; repeated `segment` fields re-run only those
/// cues (a single cue's audio, without redoing the rest). Returns `202` with
/// a `job_id`; follow `/api/jobs/{id}/events` for progress.
async fn synthesize_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let mut only = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        if field.name() == Some("segment") {
            let raw = field
                .text()
                .await
                .map_err(|e| ApiError::bad_request(format!("bad segment field: {e}")))?;
            if !raw.trim().is_empty() {
                only.push(raw.trim().to_string());
            }
        }
    }

    let project = state.projects.get(&id)?;
    if project.engine_id.trim().is_empty() {
        return Err(user_error(i18n::Key::ProjectSynthNoEngine));
    }
    // Validate before creating a job: a stale engine or voice is a client
    // error, not a job that fails after the first cue.
    check_engine(&state, &project.engine_id)?;
    if !project.voice_id.trim().is_empty() {
        check_voice(&state, &project.voice_id)?;
    }

    let job = state.jobs.create("dubbing")?;
    let worker = job.clone();
    let supervisor = job.clone();
    let store = state.projects.clone();
    let manager = state.manager.clone();
    let profiles = state.profiles.clone();
    let lang = request_lang();

    let handle = tokio::task::spawn_blocking(move || {
        worker.start();
        let mut project = match store.get(&id) {
            Ok(project) => project,
            Err(e) => {
                worker.fail(&render_error(&e, lang));
                return;
            }
        };
        let report = |step: usize, total: usize| {
            let value = if total == 0 {
                0.0
            } else {
                step as f32 / total as f32
            };
            let message = i18n::Message::new(i18n::Key::ProjectSynthProgress)
                .arg("i", step + 1)
                .arg("n", total)
                .render(lang);
            worker.emit("stage", serde_json::json!({ "message": message }));
            worker.set_progress(value, Some(&message));
        };
        let cancelled = || worker.is_cancelled();
        match dubbing::synthesize(
            &store,
            &manager,
            &profiles,
            &mut project,
            &only,
            &report,
            &cancelled,
        ) {
            Ok(count) if worker.is_cancelled() => worker.finish_cancelled(count as u32),
            Ok(count) => worker.finish(count as u32),
            Err(e) if worker.is_cancelled() => {
                worker.log(&render_error(&e, lang));
                worker.finish_cancelled(0);
            }
            Err(e) => worker.fail(&render_error(&e, lang)),
        }
    });

    // A panic inside the engine must not leave the job "running" forever.
    tokio::spawn(async move {
        if let Err(e) = handle.await {
            supervisor.fail(&format!("dubbing task failed: {e}"));
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "job_id": job.id() })),
    )
        .into_response())
}

/// The audio generated for one cue, for the editor's play button.
async fn segment_audio(
    State(state): State<AppState>,
    Path((id, segment)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let bytes = state.projects.segment_audio(&id, &segment)?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "audio/wav".to_string())],
        bytes,
    )
        .into_response())
}

/// `POST /api/projects/{id}/mix` — lay the dubbed cues over the source.
///
/// Multipart fields `background_volume`, `voice_volume` and `ducking` are
/// optional overrides of the project's saved mix; they are persisted before
/// mixing so the editor and the export agree. Returns `202` with a
/// `job_id`; the finished file is both a job artifact and the project export
/// `mix.wav`.
async fn mix_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let mut mix = state.projects.get(&id)?.mix;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        let name = field.name().map(str::to_string);
        let raw = field
            .text()
            .await
            .map_err(|e| ApiError::bad_request(format!("bad mix field: {e}")))?;
        match name.as_deref() {
            Some("background_volume") => mix.background_volume = parse_level(&raw)?,
            Some("voice_volume") => mix.voice_volume = parse_level(&raw)?,
            Some("ducking") => {
                mix.ducking = matches!(
                    raw.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "on" | "yes"
                );
            }
            _ => {}
        }
    }
    let project = state
        .projects
        .update(&id, None, None, None, None, None, Some(mix), None)?;
    if !project.segments.iter().any(|s| !s.tts_path.is_empty()) {
        return Err(user_error(i18n::Key::ProjectMixNoAudio));
    }

    let job = state.jobs.create("mix")?;
    let worker = job.clone();
    let supervisor = job.clone();
    let store = state.projects.clone();
    let lang = request_lang();
    let handle = tokio::task::spawn_blocking(move || {
        worker.start();
        if worker.is_cancelled() {
            worker.finish_cancelled(0);
            return;
        }
        worker.set_progress(0.2, None);
        let audio = match dubbing::mix(&store, &project) {
            Ok(audio) => audio,
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
        let wav = match encode_wav(&audio.samples, audio.sample_rate) {
            Ok(wav) => wav,
            Err(e) => {
                worker.fail(&render_error(&e, lang));
                return;
            }
        };
        if let Err(e) = store.write_export(&id, "mix.wav", "audio", &wav) {
            worker.fail(&render_error(&e, lang));
            return;
        }
        worker.set_progress(0.9, None);
        if let Err(e) = worker.artifact("mix.wav", &wav) {
            worker.fail(&format!("cannot write mix.wav: {e}"));
            return;
        }
        worker.finish(1);
    });
    tokio::spawn(async move {
        if let Err(e) = handle.await {
            supervisor.fail(&format!("mix task failed: {e}"));
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "job_id": job.id() })),
    )
        .into_response())
}

fn parse_level(raw: &str) -> Result<f32, ApiError> {
    raw.trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && (0.0..=4.0).contains(value))
        .ok_or_else(|| ApiError::bad_request(format!("invalid mix level: {raw}")))
}

/// A recorded export (currently `mix.wav`), for the editor's player and
/// download button.
async fn project_file(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let bytes = state.projects.export_file(&id, &name)?;
    let content_type = if name.ends_with(".wav") {
        "audio/wav"
    } else {
        "application/octet-stream"
    };
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, content_type.to_string())],
        bytes,
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

fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}
