//! ACE-Step 1.5: text/lyrics-to-music at 48 kHz stereo.

use std::process::Command;

use serde_json::{json, Value};

use crate::{Result, SynthRequest};

use super::super::{AudioCppEngine, FamilyResident};

impl AudioCppEngine {
    pub(crate) fn ace_step_task(&self, _req: &SynthRequest) -> &str {
        self.task.as_str()
    }

    pub(crate) fn ace_step_options(&self, req: &SynthRequest, cmd: &mut Command) -> Result<()> {
        // `--text` carries the style caption; the lyrics are a
        // separate option and empty means instrumental.
        let lyrics = self.param_str(req, "lyrics", "");
        if !lyrics.is_empty() {
            cmd.arg("--lyrics").arg(lyrics);
        }
        let duration = self.param_f64(req, "duration_sec", 60.0).clamp(10.0, 600.0);
        cmd.arg("--duration-seconds").arg(format!("{duration}"));
        cmd.arg("--task-route")
            .arg(self.param_str(req, "task_route", "text2music"));
        let guidance = req
            .params
            .get("guidance_scale")
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .or(self.guidance);
        if let Some(guidance) = guidance {
            cmd.arg("--guidance-scale").arg(format!("{guidance}"));
        }
        Ok(())
    }

    pub(crate) fn ace_step_resident(&self, req: &SynthRequest) -> Result<FamilyResident> {
        let mut plan = FamilyResident::new();
        let lyrics = self.param_str(req, "lyrics", "");
        if !lyrics.is_empty() {
            plan.options.insert("lyrics".into(), json!(lyrics));
        }
        let duration = self.param_f64(req, "duration_sec", 60.0).clamp(10.0, 600.0);
        plan.options.insert("duration_sec".into(), json!(duration));
        plan.options.insert(
            "route".into(),
            json!(self.param_str(req, "task_route", "text2music")),
        );
        let guidance = req
            .params
            .get("guidance_scale")
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .or(self.guidance);
        if let Some(guidance) = guidance {
            plan.options
                .insert("guidance_scale".into(), json!(guidance));
        }
        plan.options
            .insert("num_inference_steps".into(), json!(req.num_steps));
        if let Some(code) = self.language_tag(req) {
            plan.options.insert("language".into(), json!(code));
        }
        Ok(plan)
    }

    pub(crate) fn ace_step_server_tasks(&self) -> Vec<&str> {
        vec![self.task.as_str()]
    }
}
