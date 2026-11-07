//! VoxCPM2: voice-design TTS that also clones from a reference clip.

use std::process::Command;

use serde_json::{json, Value};

use crate::{Result, SynthRequest};

use super::super::{AudioCppEngine, FamilyResident};

impl AudioCppEngine {
    pub(crate) fn voxcpm2_task(&self, req: &SynthRequest) -> &'static str {
        if req.prompt_samples.is_empty() {
            "tts"
        } else {
            "clon"
        }
    }

    pub(crate) fn voxcpm2_options(&self, req: &SynthRequest, cmd: &mut Command) -> Result<()> {
        // VoxCPM2 shares dots' guidance knob; voice-design text is
        // passed by the UI as the model's parenthesized description.
        let guidance = req
            .params
            .get("guidance")
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .or(self.guidance);
        if let Some(guidance) = guidance {
            cmd.arg("--guidance-scale").arg(format!("{guidance}"));
        }
        Ok(())
    }

    pub(crate) fn voxcpm2_resident(&self, req: &SynthRequest) -> Result<FamilyResident> {
        let mut plan = FamilyResident::new();
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
        plan.options
            .insert("num_inference_steps".into(), json!(req.num_steps));
        Ok(plan)
    }

    pub(crate) fn voxcpm2_server_tasks(&self) -> Vec<&'static str> {
        vec!["clon", "tts"]
    }
}
