//! dots.tts: 2B continuous AR, 48 kHz, x-vector or continuation cloning.

use std::process::Command;

use serde_json::{json, Value};

use crate::i18n::{self, Key};
use crate::{CoreError, Result, SynthRequest};

use super::super::{AudioCppEngine, FamilyResident};

impl AudioCppEngine {
    pub(crate) fn dots_task(&self, _req: &SynthRequest) -> &'static str {
        "tts"
    }

    pub(crate) fn dots_options(&self, req: &SynthRequest, cmd: &mut Command) -> Result<()> {
        // x-vector (audio only) or continuation (audio + text).
        let clone_mode = self.param_str(req, "clone_mode", "xvector");
        if clone_mode == "continuation" {
            match self.prompt_text(req) {
                Some(text) => {
                    cmd.arg("--reference-text").arg(text);
                }
                None => {
                    return Err(CoreError::UserError(i18n::Message::new(
                        Key::TranscriptRequired,
                    )))
                }
            }
        }
        let guidance = req
            .params
            .get("guidance")
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .or(self.guidance);
        if let Some(guidance) = guidance {
            cmd.arg("--guidance-scale").arg(format!("{guidance}"));
        }
        if let Some(code) = self.language_tag(req) {
            cmd.arg("--language").arg(code);
        }
        Ok(())
    }

    pub(crate) fn dots_resident(&self, req: &SynthRequest) -> Result<FamilyResident> {
        let mut plan = FamilyResident::new();
        let clone_mode = self.param_str(req, "clone_mode", "xvector");
        if clone_mode == "continuation" {
            match self.prompt_text(req) {
                Some(text) => plan.reference_text = Some(text.to_string()),
                None => {
                    return Err(CoreError::UserError(i18n::Message::new(
                        Key::TranscriptRequired,
                    )))
                }
            }
        }
        let guidance = req
            .params
            .get("guidance")
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .or(self.guidance);
        if let Some(guidance) = guidance {
            plan.options
                .insert("guidance_scale".into(), json!(guidance));
        }
        if let Some(code) = self.language_tag(req) {
            plan.options.insert("language".into(), json!(code));
        }
        plan.options
            .insert("num_inference_steps".into(), json!(req.num_steps));
        Ok(plan)
    }

    pub(crate) fn dots_server_tasks(&self) -> Vec<&'static str> {
        vec!["tts"]
    }
}
