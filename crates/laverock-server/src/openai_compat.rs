//! OpenAI-compatible TTS endpoints.
//!
//! Any client built against OpenAI's `audio/speech` API (the official
//! SDKs, Open WebUI, Home Assistant, shell scripts…) can point at this
//! server with only the base URL changed. There are no API keys: the
//! `Authorization` header is accepted and ignored.
//!
//! - `model` selects an installed engine by its manifest id (aliases
//!   `tts-1`, `tts-1-hd`, `gpt-4o-mini-tts` pick the only/default engine).
//! - `voice` is either a preset-voice id (`af_heart` for Kokoro) or a
//!   saved profile name/id for cloning engines.
//! - `response_format` is `wav` (default), `mp3` (needs ffmpeg/lame) or
//!   `pcm` (headerless signed 16-bit little-endian mono).

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use laverock_core::{EngineManager, SynthRequest};

use crate::{export, profiles::ProfileStore, AppState};

const MODELS_CREATED: u64 = 1_737_763_200;
const OPENAI_ALIASES: &[&str] = &["tts-1", "tts-1-hd", "gpt-4o-mini-tts"];

#[derive(Debug, Deserialize)]
pub struct SpeechBody {
    model: Option<String>,
    input: Option<String>,
    voice: Option<String>,
    response_format: Option<String>,
    speed: Option<f32>,
    #[serde(default)]
    instructions: Option<String>,
}

#[derive(Debug, Serialize)]
struct ModelEntry {
    id: String,
    object: &'static str,
    created: u64,
    owned_by: &'static str,
}

fn oai_error(status: StatusCode, message: &str, param: &str, code: &str) -> Response {
    (
        status,
        Json(json!({
            "error": { "message": message, "type": "invalid_request_error",
                       "param": param, "code": code }
        })),
    )
        .into_response()
}

pub async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let mut data: Vec<ModelEntry> = OPENAI_ALIASES
        .iter()
        .map(|id| ModelEntry {
            id: (*id).to_string(),
            object: "model",
            created: MODELS_CREATED,
            owned_by: "laverock",
        })
        .collect();
    for e in state.manager.list() {
        data.push(ModelEntry {
            id: e.id,
            object: "model",
            created: MODELS_CREATED,
            owned_by: "laverock",
        });
    }
    Json(json!({ "object": "list", "data": data }))
}

pub async fn get_model(
    State(state): State<AppState>,
    axum::extract::Path(model_id): axum::extract::Path<String>,
) -> Response {
    if OPENAI_ALIASES.contains(&model_id.as_str())
        || state.manager.list().iter().any(|e| e.id == model_id)
    {
        return Json(json!({
            "id": model_id, "object": "model", "created": MODELS_CREATED, "owned_by": "laverock",
        }))
        .into_response();
    }
    oai_error(
        StatusCode::NOT_FOUND,
        &format!("The model '{model_id}' does not exist."),
        "model",
        "model_not_found",
    )
}

/// Resolve a `voice` argument to either a preset param value (Kokoro
/// voice id) or a saved profile (clone engines).
enum VoiceTarget {
    /// `(param name, param value, optional speed override)`.
    Preset { name: String, value: String },
    /// Saved profile with its reference bytes and transcript.
    Profile {
        wav: Vec<u8>,
        prompt_text: String,
        engine_hint: Option<String>,
    },
    /// Voice-design engines take their description from `instruct`.
    DesignOnly,
}

/// If `voice` matches an engine's declared preset choice (Kokoro's
/// `voice`, Qwen custom voice's `speaker`), return its parameter name
/// and the value to send (a voice name is mapped to its option value,
/// which may be an integer sid).
fn engine_preset(
    manager: &EngineManager,
    engine_id: &str,
    voice: &str,
) -> Option<(String, String)> {
    manager
        .list()
        .into_iter()
        .find(|e| e.id == engine_id)
        .and_then(|e| {
            e.parameters.into_iter().find_map(|p| {
                if p.kind != laverock_core::ParamKind::Choice {
                    return None;
                }
                p.options.iter().find_map(|o| {
                    // Match the sid/value, the whole bilingual label, or a
                    // bare voice id embedded in the label ("… (af_heart)").
                    let label_id = o
                        .label
                        .rfind('(')
                        .zip(o.label.rfind(')'))
                        .filter(|(a, b)| b > a)
                        .map(|(a, b)| &o.label[a + 1..b]);
                    (o.value == voice || o.label == voice || label_id.is_some_and(|id| id == voice))
                        .then(|| (p.name.clone(), o.value.clone()))
                })
            })
        })
}

fn resolve_voice(
    profiles: &ProfileStore,
    voice: &str,
) -> Result<VoiceTarget, (StatusCode, String, String)> {
    match profiles.get(voice) {
        Ok((meta, wav)) => Ok(VoiceTarget::Profile {
            wav,
            prompt_text: meta.prompt_text,
            engine_hint: meta.engine_id,
        }),
        Err(_) => {
            // Try a case-insensitive name match.
            if let Some(view) = profiles.list().ok().and_then(|ps| {
                ps.into_iter()
                    .find(|p| p.meta.name.eq_ignore_ascii_case(voice))
            }) {
                if let Ok((meta, wav)) = profiles.get(&view.meta.id) {
                    return Ok(VoiceTarget::Profile {
                        wav,
                        prompt_text: meta.prompt_text,
                        engine_hint: meta.engine_id,
                    });
                }
            }
            Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Voice '{voice}' not found. Pass a preset id (e.g. af_heart, Vivian) or a saved voice name/id."
                ),
                "voice_not_found".to_string(),
            ))
        }
    }
}

fn pcm16(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[allow(clippy::too_many_lines)]
pub async fn create_speech(
    State(state): State<AppState>,
    Json(body): Json<SpeechBody>,
) -> Response {
    let text = match body.input.as_deref().map(str::trim) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => {
            return oai_error(
                StatusCode::BAD_REQUEST,
                "Missing required parameter 'input'.",
                "input",
                "missing_input",
            )
        }
    };

    let format = body.response_format.as_deref().unwrap_or("wav").to_string();
    let speed = body.speed.unwrap_or(1.0);
    if !(0.25..=4.0).contains(&speed) {
        return oai_error(
            StatusCode::BAD_REQUEST,
            "speed must be between 0.25 and 4.0.",
            "speed",
            "invalid_speed",
        );
    }

    // Engine selection: explicit id, an OpenAI alias, or the only engine.
    let mut engine_id = match body.model.as_deref() {
        Some(m) if !OPENAI_ALIASES.contains(&m) => {
            if state.manager.list().iter().any(|e| e.id == m) {
                m.to_string()
            } else {
                return oai_error(
                    StatusCode::BAD_REQUEST,
                    &format!(
                        "Model '{m}' is not downloaded. GET /v1/models lists available models."
                    ),
                    "model",
                    "model_not_found",
                );
            }
        }
        _ => match default_engine(&state.manager) {
            Some(id) => id,
            None => {
                return oai_error(
                    StatusCode::BAD_REQUEST,
                    "No TTS engine installed; install one from the Models page.",
                    "model",
                    "model_not_found",
                )
            }
        },
    };

    let was_alias = body
        .model
        .as_deref()
        .map(|m| OPENAI_ALIASES.contains(&m))
        .unwrap_or(true);

    let voice_arg = body.voice.as_deref().unwrap_or("").trim().to_string();
    // For explicit models, only that engine's preset choices qualify.
    // With an alias, a preset id may belong to another engine (e.g. the
    // Qwen custom speaker "Vivian"), so search across engines.
    let mut preset = engine_preset(&state.manager, &engine_id, &voice_arg);
    if preset.is_none() && was_alias && !voice_arg.is_empty() {
        for e in state.manager.list() {
            if let Some(found) = engine_preset(&state.manager, &e.id, &voice_arg) {
                engine_id = e.id.clone();
                preset = Some(found);
                break;
            }
        }
    }

    let is_voice_design = state
        .manager
        .list()
        .iter()
        .find(|e| e.id == engine_id)
        .is_some_and(|e| e.capabilities.iter().any(|c| c == "voice-design"));

    let target = if let Some((name, value)) = preset {
        VoiceTarget::Preset { name, value }
    } else if voice_arg.is_empty() {
        // Default to the engine's first preset choice; voice-design engines
        // take their description from `instruct` instead of a voice.
        match default_preset_voice(&state.manager, &engine_id) {
            Some((param, value)) => VoiceTarget::Preset { name: param, value },
            None if is_voice_design => VoiceTarget::DesignOnly,
            None => {
                return oai_error(
                    StatusCode::BAD_REQUEST,
                    "Missing required parameter 'voice' (a preset id or saved voice name).",
                    "voice",
                    "voice_required",
                )
            }
        }
    } else {
        match resolve_voice(&state.profiles, &voice_arg) {
            Ok(t) => t,
            Err((status, msg, code)) => return oai_error(status, &msg, "voice", &code),
        }
    };

    // A saved profile may pin a specific engine; honor it when the model
    // argument was just an alias.
    if let VoiceTarget::Profile {
        engine_hint: Some(hint),
        ..
    } = &target
    {
        if was_alias && state.manager.list().iter().any(|e| &e.id == hint) {
            engine_id = hint.clone();
        }
    }

    // The OpenAI API has no sampling-steps concept; use the engine's
    // manifest default (some runners such as dots.tts reject a zero).
    let num_steps = match state.manager.manifest_defaults(&engine_id) {
        Ok(d) => d.num_steps,
        Err(e) => return core_error(e),
    };
    let mut request = SynthRequest {
        text,
        prompt_text: None,
        prompt_samples: Vec::new(),
        prompt_sr: 0,
        speed,
        num_steps,
        params: std::collections::BTreeMap::new(),
        progress: None,
    };
    match &target {
        VoiceTarget::Preset { name, value } => {
            request
                .params
                .insert(name.clone(), serde_json::Value::String(value.clone()));
        }
        VoiceTarget::Profile {
            wav, prompt_text, ..
        } => {
            let (samples, sr) = match laverock_core::decode_wav(wav) {
                Ok(v) => v,
                Err(e) => {
                    return oai_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("saved voice audio is unreadable: {e}"),
                        "voice",
                        "bad_voice",
                    )
                }
            };
            request.prompt_samples = samples;
            request.prompt_sr = sr;
            request.prompt_text = Some(prompt_text.clone());
        }
        VoiceTarget::DesignOnly => {}
    }
    if let Some(instruct) = body.instructions.clone().filter(|s| !s.trim().is_empty()) {
        request
            .params
            .insert("instruct".to_string(), serde_json::Value::String(instruct));
    }

    let manager = state.manager.clone();
    let eid = engine_id.clone();
    let spoken_text = request.text.clone();
    let audio = match tokio::task::spawn_blocking(move || manager.synthesize(&eid, &request)).await
    {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => return core_error(e),
        Err(e) => {
            return oai_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("synthesis task panicked: {e}"),
                "",
                "server_error",
            )
        }
    };

    // Encode the WAV once (also what history stores), then convert on
    // demand for other formats. Recording history is best effort.
    let wav_bytes = match laverock_core::encode_wav(&audio.samples, audio.sample_rate) {
        Ok(b) => b,
        Err(e) => return core_error(e),
    };
    if let Err(e) = state.history.add(
        &engine_id,
        &engine_display(&state.manager, &engine_id),
        &spoken_text,
        &wav_bytes,
        audio.sample_rate,
    ) {
        tracing::warn!("history add failed: {e}");
    }

    match format.as_str() {
        "wav" => audio_response(wav_bytes, "audio/wav", "speech.wav"),
        "pcm" => audio_response(pcm16(&audio.samples), "audio/pcm", "speech.pcm"),
        "mp3" => match export::wav_to_mp3(&wav_bytes) {
            Ok(bytes) => audio_response(bytes, "audio/mpeg", "speech.mp3"),
            Err(e) => oai_error(
                StatusCode::BAD_REQUEST,
                &format!("MP3 encoding unavailable: {e}. Install ffmpeg or lame, or use response_format=wav."),
                "response_format",
                "unsupported_format",
            ),
        },
        other => oai_error(
            StatusCode::BAD_REQUEST,
            &format!("Unsupported response_format '{other}'. Supported: wav, mp3, pcm."),
            "response_format",
            "unsupported_format",
        ),
    }
}

/// Default value of an engine's first preset-choice parameter
/// (`voice` for Kokoro, `speaker` for Qwen custom voice), if any.
fn default_preset_voice(manager: &EngineManager, engine_id: &str) -> Option<(String, String)> {
    manager
        .list()
        .into_iter()
        .find(|e| e.id == engine_id)
        .and_then(|e| {
            e.parameters
                .into_iter()
                .find(|p| p.kind == laverock_core::ParamKind::Choice)
                .and_then(|p| p.default.as_str().map(|v| (p.name, v.to_string())))
        })
}

fn engine_display(manager: &EngineManager, id: &str) -> String {
    manager
        .list()
        .into_iter()
        .find(|e| e.id == id)
        .map(|e| e.name)
        .unwrap_or_else(|| id.to_string())
}

fn default_engine(manager: &EngineManager) -> Option<String> {
    let engines = manager.list();
    engines
        .iter()
        .find(|e| e.id.starts_with("kokoro"))
        .or_else(|| engines.first())
        .map(|e| e.id.clone())
}

fn core_error(err: laverock_core::CoreError) -> Response {
    let status = match err {
        laverock_core::CoreError::NotFound(_) => StatusCode::NOT_FOUND,
        laverock_core::CoreError::BadRequest(_) | laverock_core::CoreError::Wav(_) => {
            StatusCode::BAD_REQUEST
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    oai_error(status, &err.to_string(), "", "engine_error")
}

fn audio_response(bytes: Vec<u8>, content_type: &str, filename: &str) -> Response {
    (
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, content_type.to_string()),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response()
}
