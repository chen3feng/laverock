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
        let route = self.param_str(req, "task_route", "text2music");
        cmd.arg("--task-route").arg(&route);
        let negative = self.param_str(req, "negative_prompt", "");
        if !negative.is_empty() {
            cmd.arg("--request-option")
                .arg(format!("negative_prompt={negative}"));
        }
        let track = self.param_str(req, "track_name", "");
        if !track.is_empty() {
            cmd.arg("--track-name").arg(track);
        }
        if route == "repaint" {
            cmd.arg("--repaint-start")
                .arg(format!("{}", self.param_f64(req, "repainting_start", 0.0)));
            cmd.arg("--repaint-end")
                .arg(format!("{}", self.param_f64(req, "repainting_end", 0.0)));
            cmd.arg("--repaint-mode")
                .arg(self.param_str(req, "repaint_mode", "balanced"));
            cmd.arg("--repaint-strength")
                .arg(format!("{}", self.param_f64(req, "repaint_strength", 0.5)));
        }
        if route.starts_with("cover") {
            // Cover conditioning has no dedicated CLI flag, but every
            // request option is accepted through --request-option.
            cmd.arg("--request-option").arg(format!(
                "audio_cover_strength={}",
                self.param_f64(req, "audio_cover_strength", 1.0)
            ));
            cmd.arg("--request-option").arg(format!(
                "cover_noise_strength={}",
                self.param_f64(req, "cover_noise_strength", 0.0)
            ));
        }
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
        let route = self.param_str(req, "task_route", "text2music");
        plan.options.insert("route".into(), json!(route));
        let negative = self.param_str(req, "negative_prompt", "");
        if !negative.is_empty() {
            plan.options
                .insert("negative_prompt".into(), json!(negative));
        }
        let track = self.param_str(req, "track_name", "");
        if !track.is_empty() {
            plan.options.insert("track_name".into(), json!(track));
        }
        if route == "repaint" {
            plan.options.insert(
                "repainting_start".into(),
                json!(self.param_f64(req, "repainting_start", 0.0)),
            );
            plan.options.insert(
                "repainting_end".into(),
                json!(self.param_f64(req, "repainting_end", 0.0)),
            );
            plan.options.insert(
                "repaint_mode".into(),
                json!(self.param_str(req, "repaint_mode", "balanced")),
            );
            plan.options.insert(
                "repaint_strength".into(),
                json!(self.param_f64(req, "repaint_strength", 0.5)),
            );
        }
        if route.starts_with("cover") {
            plan.options.insert(
                "audio_cover_strength".into(),
                json!(self.param_f64(req, "audio_cover_strength", 1.0)),
            );
            plan.options.insert(
                "cover_noise_strength".into(),
                json!(self.param_f64(req, "cover_noise_strength", 0.0)),
            );
        }
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
