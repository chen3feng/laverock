//! CosyVoice3: 0.5B streaming LLM TTS with zero-shot, cross-lingual and
//! instruction templates.

use std::process::Command;

use serde_json::json;

use crate::i18n::{self, Key};
use crate::{CoreError, Result, SynthRequest};

use super::super::{AudioCppEngine, FamilyResident};

/// Characters per CosyVoice3 long-form chunk. Its 0.5B LLM starts
/// dropping and repeating words once a single request carries much more
/// than ~150 characters (a 158-character request already repeated its
/// final clause, a 551-character one lost half the text), while
/// audio.cpp's long-form chunker defaults to 600 — far above that cliff.
/// 100 keeps every chunk inside the model's stable range.
const TEXT_CHUNK_CHARS: i64 = 100;

impl AudioCppEngine {
    pub(crate) fn cosyvoice_validate(&self, req: &SynthRequest) -> Result<()> {
        let template = self.param_str(req, "template", "zero_shot");
        if template == "zero_shot" && self.prompt_text(req).is_none() {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::ZeroShotTranscriptRequired,
            )));
        }
        if template == "instruct" && self.param_str(req, "instruct", "").is_empty() {
            return Err(CoreError::UserError(i18n::Message::new(
                Key::InstructRequired,
            )));
        }
        Ok(())
    }

    pub(crate) fn cosyvoice_task(&self, req: &SynthRequest) -> &'static str {
        let template = self.param_str(req, "template", "zero_shot");
        if template == "instruct" {
            "tts"
        } else {
            "clon"
        }
    }

    pub(crate) fn cosyvoice_options(&self, req: &SynthRequest, cmd: &mut Command) -> Result<()> {
        let template = self.param_str(req, "template", "zero_shot");
        cmd.arg("--request-option")
            .arg(format!("template_name={template}"));
        let top_k = self.param_i64(req, "top_k", 25);
        cmd.arg("--request-option").arg(format!("top_k={top_k}"));
        // Long text must be split before the LLM degenerates;
        // audio.cpp then stitches the chunks into one take.
        cmd.arg("--text-chunk-size")
            .arg(TEXT_CHUNK_CHARS.to_string());
        if template == "zero_shot" {
            if let Some(text) = self.prompt_text(req) {
                cmd.arg("--reference-text").arg(text);
            }
        }
        if template == "instruct" {
            let instruct = self.param_str(req, "instruct", "");
            cmd.arg("--request-option")
                .arg(format!("instruct={instruct}"));
        }
        Ok(())
    }

    pub(crate) fn cosyvoice_resident(&self, req: &SynthRequest) -> Result<FamilyResident> {
        let mut plan = FamilyResident::new();
        let template = self.param_str(req, "template", "zero_shot");
        plan.options.insert("template_name".into(), json!(template));
        plan.options
            .insert("top_k".into(), json!(self.param_i64(req, "top_k", 25)));
        plan.options
            .insert("text_chunk_size".into(), json!(TEXT_CHUNK_CHARS));
        if template == "zero_shot" {
            plan.reference_text = self.prompt_text(req).map(str::to_string);
        }
        if template == "instruct" {
            plan.options.insert(
                "instruct".into(),
                json!(self.param_str(req, "instruct", "")),
            );
        }
        Ok(plan)
    }

    pub(crate) fn cosyvoice_server_tasks(&self) -> Vec<&'static str> {
        vec!["clon", "tts"]
    }
}
