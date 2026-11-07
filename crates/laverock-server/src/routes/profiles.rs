//! Saved voices: create, update, delete, reference audio and export/import.

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};

use super::{ApiError, AppState};
use crate::profiles::{ImportReport, ImportStrategy, ProfileMeta, ProfileView};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/profiles", get(list_profiles).post(create_profile))
        .route(
            "/api/profiles/import",
            post(import_profile).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route(
            "/api/profiles/{id}",
            delete(delete_profile).post(update_profile),
        )
        .route("/api/profiles/{id}/audio", get(profile_audio))
        .route("/api/profiles/{id}/captures", post(set_profile_captures))
        .route("/api/profiles/{id}/export", get(export_profile))
        .route("/api/profiles/export", post(export_profiles_batch))
}

async fn list_profiles(State(state): State<AppState>) -> Result<Json<Vec<ProfileView>>, ApiError> {
    Ok(Json(state.profiles.list()?))
}

async fn create_profile(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<ProfileMeta>, ApiError> {
    let mut name = None;
    let mut prompt_text = None;
    let mut wav: Option<Vec<u8>> = None;
    // Takes from the recording library, in order. When present they are
    // the voice's audio and `prompt` is not needed.
    let mut capture_ids: Vec<String> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("name") => {
                name = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad name: {e}")))?,
                )
            }
            Some("prompt_text") => {
                prompt_text = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad prompt_text: {e}")))?,
                )
            }
            Some("capture") | Some("captures") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad capture ids: {e}")))?;
                capture_ids.extend(
                    raw.split(',')
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string),
                );
            }
            Some("prompt") | Some("audio") => {
                wav = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad prompt upload: {e}")))?
                        .to_vec(),
                );
            }
            _ => {}
        }
    }

    let name = name.ok_or_else(|| ApiError::bad_request("missing required field: name"))?;
    if !capture_ids.is_empty() {
        return Ok(Json(
            state.profiles.create_from_captures(&name, &capture_ids)?,
        ));
    }
    let prompt_text =
        prompt_text.ok_or_else(|| ApiError::bad_request("missing required field: prompt_text"))?;
    let wav = wav
        .filter(|b| !b.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing required field: prompt (WAV reference)"))?;

    Ok(Json(state.profiles.create(&name, &prompt_text, &wav)?))
}

async fn update_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<ProfileMeta>, ApiError> {
    let mut name = None;
    let mut prompt_text = None;
    let mut wav: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("name") => {
                name = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad name: {e}")))?,
                )
            }
            Some("prompt_text") => {
                prompt_text = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad prompt_text: {e}")))?,
                )
            }
            Some("prompt") => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad prompt upload: {e}")))?;
                if !bytes.is_empty() {
                    wav = Some(bytes.to_vec());
                }
            }
            _ => {}
        }
    }

    // Current values fill in whatever the form did not send.
    let (current, _) = state.profiles.get(&id)?;
    let name = name.unwrap_or(current.name);
    let prompt_text = prompt_text.unwrap_or(current.prompt_text);
    Ok(Json(state.profiles.update(
        &id,
        &name,
        &prompt_text,
        wav.as_deref(),
    )?))
}

async fn delete_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.profiles.delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The reference clip a voice would hand an engine: the takes joined,
/// cut at the limit of the engine named in `?engine=`, if any.
async fn profile_audio(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ProfileAudioQuery>,
) -> Result<Response, ApiError> {
    let limit = q
        .engine
        .as_deref()
        .and_then(|engine| state.manager.reference_limit_secs(engine).ok())
        .unwrap_or(15.0);
    let combined = state.profiles.reference(&id, limit)?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "audio/wav".to_string())],
        combined.wav,
    )
        .into_response())
}

#[derive(serde::Deserialize)]
struct ProfileAudioQuery {
    engine: Option<String>,
}

#[derive(serde::Deserialize)]
struct CapturesBody {
    captures: Vec<String>,
}

/// Replace the takes a saved voice is built from, in order.
async fn set_profile_captures(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CapturesBody>,
) -> Result<Json<ProfileMeta>, ApiError> {
    Ok(Json(state.profiles.set_captures(&id, &body.captures)?))
}

/// Download a portable `.lvvoice.zip` archive of a saved voice.
async fn export_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let (meta, _) = state.profiles.get(&id)?;
    let zip = state.profiles.export_zip(&id)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                rfc5987_attachment(&meta.name, "lvvoice.zip"),
            ),
        ],
        zip,
    )
        .into_response())
}

/// Build a Content-Disposition with both an ASCII fallback filename and
/// a UTF-8 encoded one (RFC 5987), so CJK voice names survive downloads.
fn rfc5987_attachment(name: &str, ext: &str) -> String {
    let ascii_stem: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let ascii_stem = ascii_stem.trim_matches('-');
    let fallback = if ascii_stem.is_empty() {
        "voice"
    } else {
        ascii_stem
    };
    let encoded: String = percent_encode(name.trim().as_bytes());
    format!("attachment; filename=\"{fallback}.{ext}\"; filename*=UTF-8''{encoded}.{ext}")
}

/// Minimal RFC 5987 percent-encoding (unreserved chars stay literal).
fn percent_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        let allowed = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if allowed {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[derive(serde::Deserialize)]
struct ExportBatch {
    ids: Vec<String>,
}

/// Pack several selected voices into one `.lvvoices.zip` archive.
async fn export_profiles_batch(
    State(state): State<AppState>,
    Json(body): Json<ExportBatch>,
) -> Result<Response, ApiError> {
    if body.ids.is_empty() {
        return Err(ApiError::bad_request("no voices selected"));
    }
    // Validate ids before the blocking pack.
    let known: std::collections::HashSet<String> = state
        .profiles
        .list()?
        .into_iter()
        .map(|p| p.meta.id)
        .collect();
    for id in &body.ids {
        if !known.contains(id) {
            return Err(ApiError {
                status: StatusCode::NOT_FOUND,
                message: format!("voice {id} not found"),
            });
        }
    }
    let profiles = state.profiles.clone();
    let zip = tokio::task::spawn_blocking(move || profiles.export_zip_batch(&body.ids))
        .await
        .map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("export task panicked: {e}"),
        })??;
    let stamp = chrono_stamp();
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"voices-{stamp}.lvvoices.zip\""),
            ),
        ],
        zip,
    )
        .into_response())
}

/// `YYYYMMDD-HHMMSS` for archive filenames.
fn chrono_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Convert unix seconds to UTC calendar fields without a dependency.
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-from-civil inverse (proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    // Civil months start in March: mp 0..=9 -> 3..=12, 10..=11 -> 1..=2.
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Import a single- or multi-voice archive. `strategy` is
/// `skip` (default), `copy`, or `overwrite`.
async fn import_profile(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<ImportReport>, ApiError> {
    let mut bytes: Option<Vec<u8>> = None;
    let mut strategy = ImportStrategy::Skip;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("file") | Some("voice") => {
                let mut buf = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad upload: {e}")))?
                {
                    buf.extend_from_slice(&chunk);
                }
                bytes = Some(buf);
            }
            Some("strategy") => {
                let mut buf = String::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad upload: {e}")))?
                {
                    buf.push_str(&String::from_utf8_lossy(&chunk));
                }
                strategy = match buf.trim() {
                    "copy" => ImportStrategy::Copy,
                    "overwrite" => ImportStrategy::Overwrite,
                    _ => ImportStrategy::Skip,
                };
            }
            _ => {}
        }
    }
    let bytes =
        bytes.ok_or_else(|| ApiError::bad_request("missing voice archive (field: file)"))?;
    let profiles = state.profiles.clone();
    let report = tokio::task::spawn_blocking(move || profiles.import_archive(&bytes, strategy))
        .await
        .map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("import task panicked: {e}"),
        })??;
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn civil_dates() {
        // 1970-01-01 and a handful of known unix day boundaries.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_454), (2026, 1, 1));
        assert_eq!(civil_from_days(20_753), (2026, 10, 27));
        assert_eq!(civil_from_days(20_818), (2026, 12, 31));
        assert_eq!(civil_from_days(20_819), (2027, 1, 1));
    }
}
