//! audio.cpp `model_spec.json` support.
//!
//! audio.cpp describes every family in a typed spec: its tasks, the
//! request/session/load options it accepts, their ranges and defaults.
//! Laverock reads the same document so a new family only needs an
//! `engine.json` plus a spec — no Rust match arms, no hand-written CLI
//! mapping. The parser is deliberately tolerant: fields we do not use are
//! ignored, and an option type we do not know is treated as free text.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::manifest::{Localized, ParamKind, ParamOption, ParamSpec};
use crate::{CoreError, Result};

/// One family's request contract.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ModelSpec {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Tasks this family can run (`vc`, `svc`, `tts`, `gen`, …).
    #[serde(default)]
    pub tasks: Vec<String>,
    /// audio.cpp uses an object keyed by task (`{"vc": ["speaker_reference"]}`);
    /// a flat array is accepted too.
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub options: SpecOptions,
}

/// Capabilities, either grouped by task or listed flat.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Capabilities {
    #[default]
    None,
    List(Vec<String>),
    ByTask(BTreeMap<String, Vec<String>>),
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SpecOptions {
    #[serde(default)]
    pub request: Vec<OptionSpec>,
    #[serde(default)]
    pub session: Vec<OptionSpec>,
    #[serde(default)]
    pub load: Vec<OptionSpec>,
}

/// One option an audio.cpp family accepts.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OptionSpec {
    pub name: String,
    /// `string | int | float | bool | enum | path | audio_path`; unknown
    /// types fall back to free text.
    #[serde(rename = "type", default = "default_type")]
    pub kind: String,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub required: bool,
    /// Optional display text; audio.cpp specs only carry `description`.
    #[serde(default)]
    pub label: Option<Localized>,
    #[serde(default)]
    pub description: Option<Localized>,
}

fn default_type() -> String {
    "string".to_string()
}

impl ModelSpec {
    /// Parse a spec document.
    pub fn parse(raw: &str) -> Result<Self> {
        serde_json::from_str(raw).map_err(|e| CoreError::Load(format!("invalid model spec: {e}")))
    }

    /// Whether this family declares a capability for a task.
    pub fn supports(&self, task: &str, capability: &str) -> bool {
        match &self.capabilities {
            Capabilities::None => false,
            Capabilities::List(list) => list.iter().any(|c| c == capability),
            Capabilities::ByTask(map) => map
                .get(task)
                .is_some_and(|list| list.iter().any(|c| c == capability)),
        }
    }

    /// Request options as UI controls. Every option becomes a manifest
    /// parameter, so the existing parameter grid renders the family.
    pub fn parameters(&self) -> Vec<ParamSpec> {
        self.options
            .request
            .iter()
            .map(OptionSpec::parameter)
            .collect()
    }

    /// Validate raw request values and fill in the defaults.
    ///
    /// Values that resolve to nothing (an optional option the caller left
    /// alone) are omitted rather than sent as null, so audio.cpp applies
    /// its own default.
    pub fn resolve(&self, raw: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>> {
        if let Some(name) = raw
            .keys()
            .find(|name| !self.options.request.iter().any(|spec| &spec.name == *name))
        {
            return Err(CoreError::BadRequest(format!(
                "unknown option: {name} (not declared by the model spec)"
            )));
        }

        let mut out = BTreeMap::new();
        for spec in &self.options.request {
            let value = raw
                .get(&spec.name)
                .cloned()
                .or_else(|| spec.default.clone())
                .unwrap_or(Value::Null);
            if value.is_null() || value.as_str().is_some_and(str::is_empty) {
                if spec.required {
                    return Err(CoreError::BadRequest(format!(
                        "{}: a value is required",
                        spec.name
                    )));
                }
                continue;
            }
            out.insert(spec.name.clone(), spec.coerce(&value)?);
        }
        Ok(out)
    }
}

impl OptionSpec {
    fn parameter(&self) -> ParamSpec {
        let kind = match self.kind.as_str() {
            "enum" | "choice" => ParamKind::Choice,
            "bool" | "boolean" => ParamKind::Bool,
            "int" | "integer" => ParamKind::Int,
            "float" | "number" => ParamKind::Float,
            "text_area" => ParamKind::TextArea,
            _ => ParamKind::Text,
        };
        let default = self.default.clone().unwrap_or_else(|| match kind {
            ParamKind::Choice => self
                .values
                .first()
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null),
            ParamKind::Bool => Value::Bool(false),
            // "Unset": the engine omits it and audio.cpp decides.
            ParamKind::Int | ParamKind::Float => Value::Null,
            _ => Value::String(String::new()),
        });
        let options = self
            .values
            .iter()
            .map(|value| ParamOption {
                label: Localized::Text(value.clone()),
                value: value.clone(),
            })
            .collect();
        ParamSpec {
            name: self.name.clone(),
            label: self
                .label
                .clone()
                .or_else(|| self.description.clone())
                .unwrap_or_else(|| Localized::Text(self.name.clone())),
            kind,
            min: self.min.map(|v| serde_json::json!(v)),
            max: self.max.map(|v| serde_json::json!(v)),
            step: self.step.map(|v| serde_json::json!(v)),
            options,
            default,
            hint: self.description.clone().unwrap_or_default(),
        }
    }

    fn coerce(&self, value: &Value) -> Result<Value> {
        let bad =
            |expected: &str| CoreError::BadRequest(format!("{}: expected {expected}", self.name));
        let out = match self.kind.as_str() {
            "enum" | "choice" => {
                let text = value
                    .as_str()
                    .ok_or_else(|| bad("one of the listed values"))?;
                if !self.values.is_empty() && !self.values.iter().any(|v| v == text) {
                    return Err(CoreError::BadRequest(format!(
                        "{}: invalid value {text:?} (expected one of {})",
                        self.name,
                        self.values.join(", ")
                    )));
                }
                Value::String(text.to_string())
            }
            "bool" | "boolean" => match value {
                Value::Bool(b) => Value::Bool(*b),
                Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                    "1" | "true" | "on" | "yes" => Value::Bool(true),
                    "0" | "false" | "off" | "no" => Value::Bool(false),
                    _ => return Err(bad("true or false")),
                },
                _ => return Err(bad("true or false")),
            },
            "int" | "integer" => {
                let n = as_f64(value).ok_or_else(|| bad("an integer"))?;
                if n.fract() != 0.0 {
                    return Err(bad("an integer"));
                }
                self.check_range(n)?;
                serde_json::json!(n as i64)
            }
            "float" | "number" => {
                let n = as_f64(value).ok_or_else(|| bad("a number"))?;
                if !n.is_finite() {
                    return Err(bad("a finite number"));
                }
                self.check_range(n)?;
                serde_json::json!(n)
            }
            _ => {
                let text = value.as_str().ok_or_else(|| bad("text"))?;
                Value::String(text.to_string())
            }
        };
        Ok(out)
    }

    fn check_range(&self, n: f64) -> Result<()> {
        if let Some(min) = self.min {
            if n < min {
                return Err(CoreError::BadRequest(format!(
                    "{}: {n} is below the minimum {min}",
                    self.name
                )));
            }
        }
        if let Some(max) = self.max {
            if n > max {
                return Err(CoreError::BadRequest(format!(
                    "{}: {n} is above the maximum {max}",
                    self.name
                )));
            }
        }
        Ok(())
    }
}

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seed_vc() -> ModelSpec {
        ModelSpec::parse(
            r#"{
              "family": "seed_vc",
              "tasks": ["vc", "svc"],
              "capabilities": { "vc": ["speaker_reference"] },
              "options": {
                "request": [
                  { "name": "route", "type": "enum",
                    "values": ["v2_vc", "v1_svc"], "default": "v2_vc" },
                  { "name": "num_inference_steps", "type": "int",
                    "min": 1, "max": 100, "default": 30 },
                  { "name": "similarity_guidance_scale", "type": "float",
                    "default": 0.7, "min": 0.0, "max": 3.0 },
                  { "name": "voice_anonymization", "type": "bool" },
                  { "name": "noise_file", "type": "path" }
                ],
                "session": [],
                "load": []
              }
            }"#,
        )
        .expect("spec")
    }

    #[test]
    fn capabilities_are_grouped_by_task() {
        let spec = seed_vc();
        assert!(spec.supports("vc", "speaker_reference"));
        assert!(!spec.supports("svc", "speaker_reference"));
        assert!(!spec.supports("vc", "singing"));
    }

    #[test]
    fn request_options_become_parameters() {
        let params = seed_vc().parameters();
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "route",
                "num_inference_steps",
                "similarity_guidance_scale",
                "voice_anonymization",
                "noise_file"
            ]
        );
        assert_eq!(params[0].kind, ParamKind::Choice);
        assert_eq!(params[0].default, json!("v2_vc"));
        assert_eq!(params[1].kind, ParamKind::Int);
        assert_eq!(params[1].min, Some(json!(1.0)));
        assert_eq!(params[2].kind, ParamKind::Float);
        assert_eq!(params[3].kind, ParamKind::Bool);
        assert_eq!(params[3].default, json!(false));
        // A path option without a default shows an empty field; an empty
        // value means "unset" and is not sent to audio.cpp.
        assert_eq!(params[4].kind, ParamKind::Text);
        assert_eq!(params[4].default, json!(""));
    }

    #[test]
    fn resolve_fills_defaults_and_omits_unset() {
        let spec = seed_vc();
        let resolved = spec.resolve(&BTreeMap::new()).expect("defaults");
        assert_eq!(resolved["route"], json!("v2_vc"));
        assert_eq!(resolved["num_inference_steps"], json!(30));
        assert_eq!(resolved["similarity_guidance_scale"], json!(0.7));
        // Options without a default are left to audio.cpp.
        assert!(!resolved.contains_key("voice_anonymization"));
        assert!(!resolved.contains_key("noise_file"));
    }

    #[test]
    fn resolve_accepts_strings_from_a_form() {
        let spec = seed_vc();
        let mut raw = BTreeMap::new();
        raw.insert("num_inference_steps".into(), json!("42"));
        raw.insert("similarity_guidance_scale".into(), json!("0.9"));
        raw.insert("voice_anonymization".into(), json!("true"));
        raw.insert("route".into(), json!("v1_svc"));
        let resolved = spec.resolve(&raw).expect("resolved");
        assert_eq!(resolved["num_inference_steps"], json!(42));
        assert_eq!(resolved["similarity_guidance_scale"], json!(0.9));
        assert_eq!(resolved["voice_anonymization"], json!(true));
        assert_eq!(resolved["route"], json!("v1_svc"));
    }

    #[test]
    fn resolve_rejects_bad_values() {
        let spec = seed_vc();
        let check = |name: &str, value: Value, needle: &str| {
            let mut raw = BTreeMap::new();
            raw.insert(name.to_string(), value);
            let err = spec.resolve(&raw).expect_err(name);
            assert!(err.to_string().contains(needle), "{err}");
        };
        check("route", json!("nope"), "invalid value");
        check("num_inference_steps", json!(0), "minimum");
        check("num_inference_steps", json!(101), "maximum");
        check("num_inference_steps", json!(1.5), "integer");
        check("similarity_guidance_scale", json!("loud"), "number");
        check("voice_anonymization", json!("maybe"), "true or false");
        check("surprise", json!(1), "unknown option");
    }

    #[test]
    fn a_flat_capability_list_is_accepted() {
        let spec =
            ModelSpec::parse(r#"{"family":"x","capabilities":["speaker_reference"],"options":{}}"#)
                .expect("spec");
        assert!(spec.supports("anything", "speaker_reference"));
        assert!(!spec.supports("anything", "singing"));
    }

    #[test]
    fn a_real_audio_cpp_spec_parses() {
        // Abridged from audio.cpp's model_specs/seed_vc.json: unknown
        // fields are ignored, and the option shape matches.
        let spec = ModelSpec::parse(
            r#"{
              "family": "seed_vc",
              "schema_version": 1,
              "display_name": "Seed-VC",
              "description": "Zero-shot voice conversion.",
              "category": "voice_conversion",
              "status": "supported",
              "tasks": ["vc", "svc"],
              "modes": ["offline"],
              "languages": ["language_agnostic"],
              "capabilities": { "vc": ["speaker_reference"] },
              "runtime": { "tags": ["gguf"] },
              "options": {
                "request": [
                  { "name": "route", "type": "enum",
                    "description": "Select the route.",
                    "values": ["v2_vc", "v1_svc"], "required": false },
                  { "name": "duration_scale", "type": "float",
                    "required": false, "min": 0.0, "default": 1.0 }
                ],
                "session": [
                  { "name": "weight_type", "type": "enum",
                    "preset": "weight_type_full", "default": "native" }
                ],
                "load": []
              },
              "package_defaults": { "download": { "repo": "audio-cpp/audio.cpp-gguf" } },
              "packages": [{ "id": "seed_vc_mlx_q8_0" }],
              "sources": [{ "format": "gguf" }]
            }"#,
        )
        .expect("real spec");
        assert_eq!(spec.family, "seed_vc");
        assert_eq!(spec.tasks, ["vc", "svc"]);
        assert_eq!(spec.parameters().len(), 2);
        // An enum without a default takes its first value.
        assert_eq!(spec.parameters()[0].default, json!("v2_vc"));
    }
}
