//! MiniMax Music 3: text-to-music with a required lyrics field.

use std::process::Command;

use serde_json::{json, Value};

use crate::{Result, SynthRequest};

use super::super::{AudioCppEngine, FamilyResident};

/// Numeric options passed straight through as request options.
const NUMBER_OPTIONS: &[&str] = &["guidance_scale", "ar_guidance_scale", "top_k"];

impl AudioCppEngine {
    pub(crate) fn minimax_music3_options(
        &self,
        req: &SynthRequest,
        cmd: &mut Command,
    ) -> Result<()> {
        // `--text` is the style caption; the lyrics travel separately.
        let lyrics = self.param_str(req, "lyrics", "");
        if !lyrics.is_empty() {
            cmd.arg("--request-option").arg(format!("lyrics={lyrics}"));
        }
        let duration = self.param_f64(req, "duration_sec", 30.0).clamp(1.0, 300.0);
        cmd.arg("--request-option")
            .arg(format!("duration_sec={duration}"));
        for name in NUMBER_OPTIONS {
            if let Some(value) = req.params.get(*name) {
                if !value.is_null() {
                    cmd.arg("--request-option")
                        .arg(format!("{name}={}", scalar_text(value)));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn minimax_music3_resident(&self, req: &SynthRequest) -> Result<FamilyResident> {
        let mut plan = FamilyResident::new();
        let lyrics = self.param_str(req, "lyrics", "");
        if !lyrics.is_empty() {
            plan.options.insert("lyrics".into(), json!(lyrics));
        }
        let duration = self.param_f64(req, "duration_sec", 30.0).clamp(1.0, 300.0);
        plan.options.insert("duration_sec".into(), json!(duration));
        for name in NUMBER_OPTIONS {
            if let Some(value) = req.params.get(*name) {
                if !value.is_null() {
                    plan.options.insert((*name).into(), value.clone());
                }
            }
        }
        Ok(plan)
    }
}

/// Render a scalar the way audio.cpp's request options expect it.
fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
