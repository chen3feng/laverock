//! IndexTTS-2: GPT-style token model + flow-matching vocoder, cloned from a
//! reference clip (zh/en).

use std::process::Command;

use serde_json::{json, Value};

use crate::{Result, SynthRequest};

use super::super::{AudioCppEngine, FamilyResident};

impl AudioCppEngine {
    pub(crate) fn indextts2_task(&self, _req: &SynthRequest) -> &'static str {
        // IndexTTS-2 has no preset voices: cloning is the only path.
        "clon"
    }

    pub(crate) fn indextts2_options(&self, req: &SynthRequest, cmd: &mut Command) -> Result<()> {
        // The language label helps the text front-end read mixed
        // zh/en text. "auto" (the default) lets audio.cpp pick zh
        // when the text contains Han characters and en otherwise,
        // so it is passed as no option at all.
        if let Some(code) = self.language_tag(req).filter(|c| c != "auto") {
            cmd.arg("--language").arg(code);
        }
        // Beam search (the model default of 3) reads noticeably
        // better but costs ~20% more GPT decode time; greedy
        // decoding (1) is the fast path.
        let beams = match req.params.get("num_beams") {
            Some(Value::String(raw)) => raw.trim().parse::<u32>().ok(),
            Some(value) => value.as_u64().map(|n| n as u32),
            None => None,
        }
        .unwrap_or(3)
        .clamp(1, 8);
        cmd.arg("--request-option")
            .arg(format!("num_beams={beams}"));
        // Long text is cut into segments by audio.cpp; the model's
        // own full-stop pause inside a segment is shorter than a
        // comma's, so the only audible lever is where segments are
        // cut and how much silence goes between them.
        let chunk = self.param_i64(req, "text_chunk_size", 600).clamp(40, 600);
        cmd.arg("--text-chunk-size").arg(chunk.to_string());
        let pause = self
            .param_i64(req, "interval_silence_ms", 400)
            .clamp(0, 2000);
        cmd.arg("--request-option")
            .arg(format!("interval_silence_ms={pause}"));
        Ok(())
    }

    pub(crate) fn indextts2_resident(&self, req: &SynthRequest) -> Result<FamilyResident> {
        let mut plan = FamilyResident::new();
        if let Some(code) = self.language_tag(req).filter(|c| c != "auto") {
            plan.options.insert("language".into(), json!(code));
        }
        let beams = match req.params.get("num_beams") {
            Some(Value::String(raw)) => raw.trim().parse::<u32>().ok(),
            Some(value) => value.as_u64().map(|n| n as u32),
            None => None,
        }
        .unwrap_or(3)
        .clamp(1, 8);
        plan.options.insert("num_beams".into(), json!(beams));
        let chunk = self.param_i64(req, "text_chunk_size", 600).clamp(40, 600);
        plan.options.insert("text_chunk_size".into(), json!(chunk));
        let pause = self
            .param_i64(req, "interval_silence_ms", 400)
            .clamp(0, 2000);
        plan.options
            .insert("interval_silence_ms".into(), json!(pause));
        Ok(plan)
    }

    pub(crate) fn indextts2_server_tasks(&self) -> Vec<&'static str> {
        vec!["clon"]
    }
}
