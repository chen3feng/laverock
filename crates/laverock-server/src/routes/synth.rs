//! Speech/music synthesis, blocking WAV or Server-Sent Events.

use axum::{
    body::{Body, Bytes},
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use laverock_core::{
    decode_wav, encode_wav_channels, trim_silence, Audio, CoreError, SynthRequest, TaskKind,
};
use std::sync::Arc;

use super::{ApiError, AppState};

/// Upper bound on how many variant takes one streaming request may ask
/// for. Kept small because each take is a full inference run.
const MAX_SAMPLES: u32 = 4;

pub(crate) fn router() -> Router<AppState> {
    Router::new().route("/api/engines/{id}/synthesize", post(synthesize))
}

#[allow(clippy::too_many_arguments)]
async fn synthesize(
    Path(engine_id): Path<String>,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let mut text: Option<String> = None;
    let mut prompt_bytes: Option<Vec<u8>> = None;
    let mut prompt_text: Option<String> = None;
    let mut profile_id: Option<String> = None;
    let mut samples: u32 = 1;
    // Raw values for manifest-declared synthesis parameters.
    let mut raw_params: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("malformed multipart: {e}")))?
    {
        let name = field.name().map(str::to_string);
        match name.as_deref() {
            Some("text") => {
                text = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad text field: {e}")))?,
                );
            }
            Some("prompt") => {
                prompt_bytes =
                    Some(
                        field.bytes().await.map(|v| v.to_vec()).map_err(|e| {
                            ApiError::bad_request(format!("bad prompt upload: {e}"))
                        })?,
                    );
            }
            Some("prompt_text") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad prompt_text: {e}")))?;
                if !raw.trim().is_empty() {
                    prompt_text = Some(raw);
                }
            }
            Some("profile_id") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad profile_id: {e}")))?;
                if !raw.trim().is_empty() {
                    profile_id = Some(raw.trim().to_string());
                }
            }
            Some("samples") => {
                // Not an engine parameter: how many variant takes a single
                // streaming request should produce. Clamped to a sane cap.
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad samples field: {e}")))?;
                samples = match raw.trim().parse::<u32>() {
                    Ok(n) => n.clamp(1, MAX_SAMPLES),
                    Err(_) => 1,
                };
            }
            Some(other) => {
                // Anything else is a candidate engine parameter; validated
                // against the engine's declared parameter list below.
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("bad {other} field: {e}")))?;
                raw_params.insert(other.to_string(), raw);
            }
            None => {}
        }
    }

    let text = text
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing required field: text"))?;
    // A saved voice takes precedence over an ad-hoc upload. Reference audio
    // is optional at the API level: preset/voice-design engines do not need
    // one, and each engine reports what it requires.
    //
    // A voice built from several takes is joined here, cut at the engine's
    // own reference limit — that is the clip the engine conditions on.
    let (prompt_bytes, prompt_text) = if let Some(id) = profile_id {
        let limit = state
            .manager
            .reference_limit_secs(&engine_id)
            .unwrap_or(15.0);
        let combined = state.profiles.reference(&id, limit)?;
        if combined.dropped > 0 {
            tracing::info!(
                "voice {id}: {} of {} takes used ({limit} s limit for {engine_id})",
                combined.used,
                combined.used + combined.dropped
            );
        }
        (Some(combined.wav), Some(combined.text))
    } else {
        let text = prompt_text
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        (prompt_bytes.filter(|b| !b.is_empty()), text)
    };

    // Validate all engine parameters against the manifest's list.
    let param_specs = state.manager.parameters(&engine_id)?;
    let params = resolve_params(&param_specs, &raw_params)?;

    // The two historical typed fields live in the parameter list too.
    let defaults = state.manager.manifest_defaults(&engine_id)?;
    let speed = params
        .get("speed")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(defaults.speed);
    let num_steps = params
        .get("num_steps")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .unwrap_or(defaults.num_steps);

    let (prompt_samples, prompt_sr) = match prompt_bytes {
        Some(bytes) => {
            let (samples, rate) = decode_wav(&bytes)?;
            // Temporary recordings usually start before the speaker does
            // (and linger after they stop). Trim that dead air exactly like
            // profile ingest does; internal pauses stay, since only some
            // engines need them fixed up (MOSS-TTS does it itself).
            let trimmed = trim_silence(&samples, rate.max(1) as u32, 0.10);
            if trimmed.is_empty() {
                return Err(CoreError::UserError(laverock_core::i18n::Message::new(
                    laverock_core::i18n::Key::ReferenceSilent,
                ))
                .into());
            }
            (trimmed.into_owned(), rate)
        }
        None => (Vec::new(), 0),
    };

    let manager = state.manager.clone();
    let history = state.history.clone();
    let info = state.manager.list().into_iter().find(|e| e.id == engine_id);
    let engine_name = info
        .as_ref()
        .map(|e| e.name.clone())
        .unwrap_or_else(|| engine_id.clone());
    // Music entries are flagged in history so the UI can label them and
    // offer the right actions; everything else is speech.
    let engine_kind = info
        .as_ref()
        .map(|e| e.kind.clone())
        .unwrap_or_else(|| "speech".to_string());
    let task = TaskKind::from_output_kind(&engine_kind);

    // The desktop UI asks for a Server-Sent-Events stream so long Qwen
    // generations can report progress; other clients keep getting the WAV.
    let wants_stream = headers
        .get("x-laverock-stream")
        .and_then(|v| v.to_str().ok())
        == Some("1");
    // The `samples` field only has meaning over the streaming/SSE path
    // (multiple WAV frames in one response). A blocking request keeps its
    // single `audio/wav` contract for OpenAI/curl-style callers, so the
    // extra takes are silently dropped here.
    let _ = samples;
    if !wants_stream {
        // Blocking inference off the async runtime.
        let request = SynthRequest {
            text: text.clone(),
            prompt_text: prompt_text.clone(),
            prompt_samples: prompt_samples.clone(),
            prompt_sr,
            speed,
            num_steps,
            params: params.clone(),
            input_samples: Vec::new(),
            input_sr: 0,
            progress: None,
            audio: None,
        };
        let history_engine = engine_id.clone();
        let Audio {
            samples,
            sample_rate,
            channels,
        } = tokio::task::spawn_blocking(move || manager.synthesize(&history_engine, &request))
            .await
            .map_err(|e| ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("engine task panicked: {e}"),
            })??;

        let wav = encode_wav_channels(&samples, sample_rate, channels)?;
        if let Err(e) = history.add_typed(
            &engine_id,
            &engine_name,
            task,
            &engine_kind,
            &text,
            &wav,
            sample_rate,
            channels,
            None,
        ) {
            tracing::warn!("history add failed: {e}");
        }
        return Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "audio/wav".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"laverock.wav\"".to_string(),
                ),
            ],
            wav,
        )
            .into_response());
    }

    use axum::body::Bytes;
    use base64::Engine;
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::ReceiverStream;

    let sse = |event: &str, data: &str| -> Bytes {
        Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
    };
    let (tx, rx) = mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(16);

    let history_text = text.clone();
    tokio::task::spawn_blocking(move || {
        let tx2 = tx.clone();
        let sink: laverock_core::ProgressSink = Arc::new(move |event| {
            let frame = match event {
                laverock_core::ProgressEvent::Stage(label) => sse("stage", &label),
                laverock_core::ProgressEvent::Steps(n) => sse("steps", &n.to_string()),
            };
            // Progress frames are lossy: never block the engine on a
            // disconnected or slow UI. The final WAV frame is allowed to
            // wait for buffer space (it errors immediately if closed).
            let _ = tx2.try_send(Ok(frame));
        });
        let mut params = params;
        // One shared group id ties all takes of this request together in
        // history, so the UI can show them as variants of a single try.
        let generation = history.new_generation_id();
        let mut produced = 0u32;
        let mut last_channels: Option<u16> = None;
        let mut failed = false;
        for index in 0..samples {
            // Deterministic engines expose a `seed` parameter; bump it so
            // each additional take actually differs. Engines that sample
            // internally already vary between runs without this.
            if index > 0 {
                if let Some(seed) = params.get("seed").and_then(|v| v.as_i64()) {
                    params.insert("seed".to_string(), serde_json::json!(seed + index as i64));
                }
            }
            let request = SynthRequest {
                text: text.clone(),
                prompt_text: prompt_text.clone(),
                prompt_samples: prompt_samples.clone(),
                prompt_sr,
                speed,
                num_steps,
                params: params.clone(),
                input_samples: Vec::new(),
                input_sr: 0,
                progress: Some(sink.clone()),
                audio: None,
            };
            match manager.synthesize(&engine_id, &request) {
                Ok(Audio {
                    samples: audio,
                    sample_rate,
                    channels,
                }) => match encode_wav_channels(&audio, sample_rate, channels) {
                    Ok(wav) => {
                        // Best-effort: a prune failure must not drop the
                        // audio the client just generated.
                        if let Err(e) = history.add_typed(
                            &engine_id,
                            &engine_name,
                            task,
                            &engine_kind,
                            &history_text,
                            &wav,
                            sample_rate,
                            channels,
                            Some((generation.clone(), index)),
                        ) {
                            tracing::warn!("history add failed: {e}");
                        }
                        produced += 1;
                        last_channels = Some(channels);
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&wav);
                        // Envelope carries index/total so the UI lays out a
                        // variant gallery without an extra batch-end marker.
                        let payload = serde_json::json!({ "i": index, "n": samples, "wav": b64 });
                        let _ = tx.blocking_send(Ok(sse("wav", &payload.to_string())));
                    }
                    Err(e) => {
                        failed = true;
                        let _ = tx.blocking_send(Ok(error_frame(&sse, &e.to_string())));
                        break;
                    }
                },
                Err(e) => {
                    failed = true;
                    let _ = tx.blocking_send(Ok(error_frame(&sse, &e.to_string())));
                    break;
                }
            }
        }
        if !failed {
            let payload = serde_json::json!({
                "count": produced,
                "channels": last_channels.unwrap_or(1),
            });
            let _ = tx.blocking_send(Ok(sse("done", &payload.to_string())));
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(ReceiverStream::new(rx)))
        .map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: e.to_string(),
        })
}

fn error_frame(sse: &impl Fn(&str, &str) -> Bytes, message: &str) -> Bytes {
    let json = serde_json::json!({ "message": message }).to_string();
    sse("error", &json)
}

/// Parse and range-check raw form fields against the engine's declared
/// parameter list. Unknown fields are rejected; absent fields take the
/// manifest defaults.
fn resolve_params(
    specs: &[laverock_core::ParamSpec],
    raw: &std::collections::BTreeMap<String, String>,
) -> Result<std::collections::BTreeMap<String, serde_json::Value>, ApiError> {
    use laverock_core::{ParamKind, ParamSpec};
    let mut out = std::collections::BTreeMap::new();
    for spec in specs {
        let ParamSpec {
            name,
            kind,
            min,
            max,
            options,
            default,
            hint,
            ..
        } = spec;
        let value = match raw.get(name) {
            Some(v) if !v.trim().is_empty() => parse_param(spec, v.trim())?,
            _ => default.clone(),
        };
        let _ = (min, max, hint, kind);
        if *kind == ParamKind::Choice {
            if let serde_json::Value::String(choice) = &value {
                if !options.iter().any(|o| &o.value == choice) {
                    return Err(ApiError::bad_request(format!("{name}: invalid option")));
                }
            }
        }
        out.insert(name.clone(), value);
    }
    let known: std::collections::HashSet<&str> = specs.iter().map(|p| p.name.as_str()).collect();
    if let Some(name) = raw.keys().map(String::as_str).find(|n| !known.contains(n)) {
        return Err(ApiError::bad_request(format!("unknown parameter: {name}")));
    }
    Ok(out)
}

fn parse_param(spec: &laverock_core::ParamSpec, raw: &str) -> Result<serde_json::Value, ApiError> {
    use laverock_core::ParamKind;
    let name = &spec.name;
    let range_ok = |n: f64| -> bool {
        spec.min
            .as_ref()
            .and_then(|v| v.as_f64())
            .is_none_or(|min| n >= min)
            && spec
                .max
                .as_ref()
                .and_then(|v| v.as_f64())
                .is_none_or(|max| n <= max)
    };
    let value = match spec.kind {
        ParamKind::Float => {
            let n: f64 = raw
                .parse()
                .map_err(|_| ApiError::bad_request(format!("{name}: expected a number")))?;
            if !range_ok(n) {
                return Err(ApiError::bad_request(format!("{name}: out of range")));
            }
            serde_json::json!(n)
        }
        ParamKind::Int => {
            let n: i64 = raw
                .parse()
                .map_err(|_| ApiError::bad_request(format!("{name}: expected an integer")))?;
            if !range_ok(n as f64) {
                return Err(ApiError::bad_request(format!("{name}: out of range")));
            }
            serde_json::json!(n)
        }
        ParamKind::Bool => serde_json::json!(matches!(
            raw.to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        )),
        ParamKind::Choice => serde_json::Value::String(raw.to_string()),
        ParamKind::Text | ParamKind::TextArea => {
            // Free-form text: bounded so a stray paste cannot bloat the
            // request or the engine command line. Lyrics for a full song
            // comfortably fit in 4000 characters.
            const MAX_TEXT_PARAM: usize = 4000;
            let text = raw.trim();
            if text.chars().count() > MAX_TEXT_PARAM {
                return Err(ApiError::bad_request(format!(
                    "{name}: 文本过长（最多 {MAX_TEXT_PARAM} 字）"
                )));
            }
            serde_json::Value::String(text.to_string())
        }
    };
    Ok(value)
}
