//! YuE2: style + lyrics music generation, with optional chain-of-thought.

use std::process::Command;

use serde_json::{json, Value};

use crate::{Result, SynthRequest};

use super::super::{AudioCppEngine, FamilyResident};

impl AudioCppEngine {
    pub(crate) fn yue2_options(&self, req: &SynthRequest, cmd: &mut Command) -> Result<()> {
        // The style caption is the page's prompt; YuE2 reads `--text` as
        // lyrics, so the shared builder omits it for this family.
        cmd.arg("--request-option")
            .arg(format!("style={}", req.text.trim()));
        let lyrics = self.param_str(req, "lyrics", "");
        if !lyrics.is_empty() {
            cmd.arg("--lyrics").arg(lyrics);
        }
        let cot = self.param_str(req, "cot", "off");
        if cot != "off" {
            cmd.arg("--request-option").arg(format!("cot={cot}"));
        }
        if let Some(value) = req.params.get("guidance_scale") {
            if !value.is_null() {
                cmd.arg("--request-option")
                    .arg(format!("guidance_scale={}", scalar_text(value)));
            }
        }
        Ok(())
    }

    pub(crate) fn yue2_resident(&self, req: &SynthRequest) -> Result<FamilyResident> {
        let mut plan = FamilyResident::new();
        plan.options.insert("style".into(), json!(req.text.trim()));
        let lyrics = self.param_str(req, "lyrics", "");
        if !lyrics.is_empty() {
            plan.options.insert("lyrics".into(), json!(lyrics));
        }
        let cot = self.param_str(req, "cot", "off");
        if cot != "off" {
            plan.options.insert("cot".into(), json!(cot));
        }
        if let Some(value) = req.params.get("guidance_scale") {
            if !value.is_null() {
                plan.options.insert("guidance_scale".into(), value.clone());
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
