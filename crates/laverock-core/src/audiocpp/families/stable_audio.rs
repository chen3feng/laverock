//! Stable Audio 3: text-to-music, sound effects and audio-conditioned
//! generation (init audio and inpainting).

use std::process::Command;

use serde_json::{json, Value};

use crate::{Result, SynthRequest};

use super::super::{AudioCppEngine, FamilyResident};

/// Options that are always passed as text when they are set.
const TEXT_OPTIONS: &[&str] = &["negative_prompt", "sampler_mode"];
/// Numeric options passed as request options.
const NUMBER_OPTIONS: &[&str] = &[
    "guidance_scale",
    "init_noise_level",
    "apg_scale",
    "batch_size",
];
/// Comma-separated inpaint masks.
const MASK_OPTIONS: &[&str] = &["inpaint_mask_start_sec", "inpaint_mask_end_sec"];

impl AudioCppEngine {
    pub(crate) fn stable_audio_options(&self, req: &SynthRequest, cmd: &mut Command) -> Result<()> {
        let duration = self.param_f64(req, "duration_sec", 120.0).max(1.0);
        cmd.arg("--duration-seconds").arg(format!("{duration}"));

        let kind = self.param_str(req, "audio_input_kind", "none");
        if kind != "none" {
            cmd.arg("--request-option")
                .arg(format!("audio_input_kind={kind}"));
        }
        for name in TEXT_OPTIONS {
            let value = self.param_str(req, name, "");
            if !value.is_empty() {
                cmd.arg("--request-option").arg(format!("{name}={value}"));
            }
        }
        for name in NUMBER_OPTIONS.iter().chain(MASK_OPTIONS) {
            if !option_applies(name, &kind) {
                continue;
            }
            if let Some(value) = req.params.get(*name) {
                if !value.is_null() && !value.as_str().is_some_and(str::is_empty) {
                    cmd.arg("--request-option")
                        .arg(format!("{name}={}", scalar_text(value)));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn stable_audio_resident(&self, req: &SynthRequest) -> Result<FamilyResident> {
        let mut plan = FamilyResident::new();
        let duration = self.param_f64(req, "duration_sec", 120.0).max(1.0);
        plan.options.insert("duration_sec".into(), json!(duration));

        let kind = self.param_str(req, "audio_input_kind", "none");
        if kind != "none" {
            plan.options.insert("audio_input_kind".into(), json!(kind));
        }
        for name in TEXT_OPTIONS {
            let value = self.param_str(req, name, "");
            if !value.is_empty() {
                plan.options.insert((*name).into(), json!(value));
            }
        }
        for name in NUMBER_OPTIONS.iter().chain(MASK_OPTIONS) {
            if !option_applies(name, &kind) {
                continue;
            }
            if let Some(value) = req.params.get(*name) {
                if !value.is_null() && !value.as_str().is_some_and(str::is_empty) {
                    plan.options.insert((*name).into(), value.clone());
                }
            }
        }
        Ok(plan)
    }
}

/// Audio-conditioned options only make sense for the mode that uses them.
fn option_applies(name: &str, kind: &str) -> bool {
    match name {
        "init_noise_level" => kind != "none",
        "inpaint_mask_start_sec" | "inpaint_mask_end_sec" => kind == "inpaint_audio",
        _ => true,
    }
}

/// Render a scalar the way audio.cpp's request options expect it.
fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
