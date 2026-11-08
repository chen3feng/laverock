//! Subtitle projects: create from a clip, transcribe, edit cues, export.

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use laverock_core::{decode_wav, i18n, CoreError};

use super::{subtitle_response, ApiError, AppState};
use crate::{
    projects::{self, Project, Segment},
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
    segments: Option<Vec<Segment>>,
}

async fn update_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ProjectPatch>,
) -> Result<Json<Project>, ApiError> {
    let project = state.projects.update(
        &id,
        body.name.as_deref(),
        body.source_language.as_deref(),
        body.target_language.as_deref(),
        body.segments,
    )?;
    Ok(Json(project))
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

fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}
