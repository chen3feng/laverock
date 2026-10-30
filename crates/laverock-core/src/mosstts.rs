//! MOSS-TTS-Delay (OpenMOSS) engine, backed by the native `llama-moss-tts`
//! binary.
//!
//! The installer builds that binary from the OpenMOSS llama.cpp fork and
//! converts the audio tokenizer to GGUF, so synthesis is fully offline and
//! torch-free: one executable drives the Qwen3 backbone plus the audio
//! encoder/decoder GGUFs. A reference clip is optional — without one the
//! model speaks in a default voice; with one it clones that voice. The
//! runtime only accepts 24 kHz mono references, so the engine resamples
//! before invoking the binary.
//!
//! One invocation handles one utterance: the backbone generates until it
//! predicts end-of-speech or runs out of token budget, and the decoder turns
//! the whole code stream into audio in a single pass. Both degrade on long
//! text (see [`MAX_CHUNK_CHARS`]), so the engine splits the text into
//! sentence-sized pieces, synthesises each one and joins the results.

use crate::{
    cap_silences, decode_wav, encode_wav, resample_mono, trim_silence, Audio, CoreError, Manifest,
    ProgressEvent, Result, SpeechEngine, SynthRequest,
};
use std::{
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::Duration,
};

/// The native runtime requires reference audio at this exact rate.
const REFERENCE_SAMPLE_RATE: u32 = 24_000;

/// The delay-pattern backbone has a "silence-token attractor": now and then
/// it emits codec frames that decode to near-silence, stacking up pauses of
/// 0.3–1.5 s inside an utterance. Longer runs of silence are cut back to
/// this length so the result stays listenable.
const MAX_OUTPUT_PAUSE_SECS: f32 = 0.45;

/// Pauses in the reference clip condition that same attractor, so the
/// prompt is cleaned harder than the output.
const MAX_REFERENCE_PAUSE_SECS: f32 = 0.25;

/// Longest text piece handed to one invocation, in characters.
///
/// The runtime decodes every generated frame in one pass, and that pass
/// falls apart on long input: 1,311 frames (105 s) decode cleanly, 2,469
/// (198 s) come out as a saturated signal instead of audio. A single
/// invocation also stops at `--max-new-tokens`, so a long document is cut
/// off mid-sentence. Sentence-sized pieces stay far below both limits.
const MAX_CHUNK_CHARS: usize = 200;

/// Narration runs at roughly 2.5–3 audio frames per character (12.5
/// frames/s), so four frames per character plus slack gives the model room
/// to finish its sentence without approaching the decoder's failure point.
const CHUNK_TOKENS_PER_CHAR: i64 = 4;
const CHUNK_MIN_TOKENS: i64 = 200;
const CHUNK_MAX_TOKENS: i64 = 900;

/// Gap inserted between two pieces so joins sound like sentence breaks
/// rather than mid-air restarts. Each piece already keeps a short silent
/// tail, so the audible gap ends up a little longer.
const CHUNK_PAUSE_SECS: f32 = 0.2;

/// Each piece is trimmed close to its last audible sample, so dropping it
/// into silence leaves a step at the edge that is audible as a click. A
/// short ramp at both ends removes the step without touching the speech.
const CHUNK_FADE_SECS: f32 = 0.01;

/// The vendored ggml build occasionally dies while loading a piece — heap
/// corruption or a Metal fault in roughly one launch out of six when
/// several run back to back, and none when they are spaced out. A fresh
/// process almost always completes, so launches are spaced and retried.
const CHUNK_LAUNCH_PAUSE: Duration = Duration::from_millis(1000);
const CHUNK_RETRY_PAUSE: Duration = Duration::from_millis(1500);
const CHUNK_ATTEMPTS: u32 = 3;

pub struct MossTtsEngine {
    id: String,
    name: String,
    binary: PathBuf,
    backbone: PathBuf,
    audio_encoder: PathBuf,
    audio_decoder: PathBuf,
    language: String,
    n_gpu_layers: i64,
    max_new_tokens: i64,
    env: Vec<(String, String)>,
    run_lock: Mutex<()>,
}

impl MossTtsEngine {
    pub fn build(dir: &Path, manifest: &Manifest) -> Result<Self> {
        let c = manifest.mosstts.as_ref().ok_or_else(|| {
            CoreError::Load(format!("{}: missing [mosstts] section", manifest.id))
        })?;

        let binary = resolve_path(dir, &c.binary);
        if !binary.is_file() {
            return Err(CoreError::Load(format!(
                "{}: llama-moss-tts not found at {}",
                manifest.id,
                binary.display()
            )));
        }
        let require = |label: &str, value: &str| -> Result<PathBuf> {
            let path = resolve_path(dir, value);
            if !path.is_file() {
                return Err(CoreError::Load(format!(
                    "{}: {label} GGUF not found at {}",
                    manifest.id,
                    path.display()
                )));
            }
            Ok(path)
        };
        let backbone = require("backbone", &c.backbone)?;
        let audio_encoder = require("audio encoder", &c.audio_encoder)?;
        let audio_decoder = require("audio decoder", &c.audio_decoder)?;

        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            binary,
            backbone,
            audio_encoder,
            audio_decoder,
            language: c.language.clone(),
            n_gpu_layers: c.n_gpu_layers,
            max_new_tokens: c.max_new_tokens,
            env: c.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            run_lock: Mutex::new(()),
        })
    }

    /// Token budget for one piece: enough for the text to finish, capped
    /// well below the decoder's breakdown, and never above the manifest.
    fn chunk_token_budget(&self, chunk: &str) -> i64 {
        let chars = chunk.chars().count() as i64;
        let budget =
            (chars * CHUNK_TOKENS_PER_CHAR + 120).clamp(CHUNK_MIN_TOKENS, CHUNK_MAX_TOKENS);
        budget.min(self.max_new_tokens.max(CHUNK_MIN_TOKENS))
    }

    /// Write the cleaned reference clip once; every piece reuses it.
    fn write_reference(&self, req: &SynthRequest, tmp: &Path) -> Result<Option<PathBuf>> {
        if req.prompt_samples.is_empty() {
            return Ok(None);
        }
        // Optional voice cloning: the runtime wants a 24 kHz mono WAV. Dead
        // air at the ends and long pauses inside the clip are stripped
        // first — the model copies the prompt's rhythm, and raw microphone
        // recordings (or TTS-generated references) are often full of gaps.
        let rate = req.prompt_sr.max(1) as u32;
        let trimmed = trim_silence(&req.prompt_samples, rate, 0.05);
        let cleaned = cap_silences(&trimmed, rate, MAX_REFERENCE_PAUSE_SECS);
        let pcm = resample_mono(&cleaned, rate, REFERENCE_SAMPLE_RATE);
        // An all-silent or near-empty clip carries no timbre; fall back to
        // the default voice instead of failing inside the runtime.
        if pcm.len() < (REFERENCE_SAMPLE_RATE as usize) / 5 {
            return Ok(None);
        }
        let path = tmp.join("reference.wav");
        let bytes = encode_wav(&pcm, REFERENCE_SAMPLE_RATE as i32)?;
        std::fs::write(&path, bytes).map_err(CoreError::from)?;
        Ok(Some(path))
    }

    /// Run one text piece, retrying when the runtime dies from a signal.
    /// `build` must produce a fresh command because `Command` is not
    /// cloneable and a retry needs a new process.
    fn run_chunk<F>(&self, out_wav: &Path, build: F) -> Result<(Vec<f32>, i32)>
    where
        F: Fn() -> Command,
    {
        for attempt in 1..=CHUNK_ATTEMPTS {
            match self.run_once(build(), out_wav) {
                Ok(samples) => return Ok(samples),
                // A signal death means the native runtime crashed inside
                // ggml; a fresh process usually gets through.
                Err((true, _)) if attempt < CHUNK_ATTEMPTS => {
                    let _ = std::fs::remove_file(out_wav);
                    std::thread::sleep(CHUNK_RETRY_PAUSE);
                }
                Err((_, message)) => return Err(CoreError::Synth(message)),
            }
        }
        unreachable!("the attempt loop always returns")
    }

    /// Run the native runtime once and decode the WAV it wrote. The error
    /// flag is true when the process was killed by a signal (retryable).
    fn run_once(
        &self,
        mut cmd: Command,
        out_wav: &Path,
    ) -> std::result::Result<(Vec<f32>, i32), (bool, String)> {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            (
                false,
                format!(
                    "failed to run {}: {e} (is the llama-moss-tts runtime built?)",
                    self.binary.display()
                ),
            )
        })?;

        // llama.cpp logs to stderr; keep a bounded tail for error reporting
        // instead of buffering a long generation's output in memory.
        let stdout = child.stdout.take().expect("piped stdout");
        let stdout_handle = std::thread::spawn(move || {
            let mut tail: Vec<String> = Vec::new();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                tail.push(line);
                if tail.len() > 12 {
                    tail.remove(0);
                }
            }
            tail
        });
        let stderr = child.stderr.take().expect("piped stderr");
        let stderr_handle = std::thread::spawn(move || {
            let mut buf = Vec::new();
            BufReader::new(stderr).read_to_end(&mut buf).ok();
            buf
        });

        let status = child.wait().map_err(|e| (false, e.to_string()))?;
        let stdout_tail = stdout_handle.join().unwrap_or_default();
        let stderr_bytes = stderr_handle.join().unwrap_or_default();

        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr_bytes);
            let mut tail: String = stderr
                .lines()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            if !stdout_tail.is_empty() {
                tail = format!("{tail}\n{}", stdout_tail.join("\n"));
            }
            return Err((
                status.code().is_none(),
                format!(
                    "MOSS-TTS 推理失败（退出码 {:?}）：\n{}",
                    status.code(),
                    tail.trim()
                ),
            ));
        }

        let bytes = std::fs::read(out_wav)
            .map_err(|e| (false, format!("runner produced no audio: {e}")))?;
        decode_wav(&bytes).map_err(|e| (false, e.to_string()))
    }
}

impl SpeechEngine for MossTtsEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn synthesize(&self, req: &SynthRequest) -> Result<Audio> {
        let _guard = self.run_lock.lock().expect("mosstts run lock");

        let tmp = std::env::temp_dir().join(format!(
            "laverock-mosstts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).map_err(CoreError::from)?;
        let reference = self.write_reference(req, &tmp)?;

        // Per-request language choice overrides the manifest default.
        let language = req
            .params
            .get("language")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.language);

        let mut chunks = split_text(&req.text, MAX_CHUNK_CHARS);
        if chunks.is_empty() {
            // Nothing to split (blank text); let the runtime report it.
            chunks.push(req.text.clone());
        }
        let total = chunks.len();

        let mut samples: Vec<f32> = Vec::new();
        let mut sample_rate: i32 = REFERENCE_SAMPLE_RATE as i32;

        for (index, chunk) in chunks.iter().enumerate() {
            if let Some(cb) = &req.progress {
                let label = if total > 1 {
                    format!("正在合成 MOSS-TTS 第 {}/{} 段", index + 1, total)
                } else {
                    "正在加载 MOSS-TTS 并合成（首次加载约数十秒）".to_string()
                };
                cb(ProgressEvent::Stage(label));
            }

            let out_wav = tmp.join(format!("out-{index}.wav"));
            // Space the launches out: back-to-back runs of the native
            // runtime are what trigger its occasional crash on load.
            if index > 0 {
                std::thread::sleep(CHUNK_LAUNCH_PAUSE);
            }
            let budget = self.chunk_token_budget(chunk).to_string();
            let build = || {
                let mut cmd = Command::new(&self.binary);
                cmd.arg("-m")
                    .arg(&self.backbone)
                    .arg("--audio-decoder-model")
                    .arg(&self.audio_decoder)
                    .arg("--text")
                    .arg(chunk)
                    .arg("--wav-out")
                    .arg(&out_wav)
                    .arg("-ngl")
                    .arg(self.n_gpu_layers.to_string())
                    .arg("--max-new-tokens")
                    .arg(&budget);
                if !language.is_empty() {
                    cmd.arg("--language").arg(language);
                }
                if let Some(path) = &reference {
                    cmd.arg("--audio-encoder-model")
                        .arg(&self.audio_encoder)
                        .arg("--reference-audio")
                        .arg(path);
                }
                // The vendored runtime's Metal backend reorders each graph
                // before allocation and its dependency accounting is wrong
                // for fused nodes: a node can be scheduled alongside work it
                // actually depends on, the scheduler then recycles a buffer
                // that is still in use, and the resulting write corrupts the
                // host heap (SIGSEGV in graph_optimize, or a malloc abort
                // somewhere unrelated later on).
                //
                // Measured over launches that supply a reference clip: 36 of
                // 131 crashed with the pass on, 12 of 172 with it off, so
                // about one in four becomes about one in fifteen. That is a
                // large cut, not a cure: a rarer corruption survives in the
                // audio encoder path (see issue #57) and the retry loop in
                // run_chunk absorbs the remainder. No measurable cost (3.08 s
                // vs 3.09 s for a short line, 6.62 s vs 6.61 s for a long
                // one). Manifest env entries are applied below, so an install
                // can still override it.
                cmd.env("GGML_METAL_GRAPH_OPTIMIZE_DISABLE", "1");
                for (key, value) in &self.env {
                    cmd.env(key, value);
                }
                cmd
            };

            let (chunk_samples, rate) = self.run_chunk(&out_wav, build)?;
            let rate = rate.max(1) as u32;
            sample_rate = rate as i32;

            // Collapse the delay-pattern model's leftover long pauses and
            // trim the dead air at both ends of the piece.
            let capped = cap_silences(&chunk_samples, rate, MAX_OUTPUT_PAUSE_SECS);
            let mut piece = trim_silence(&capped, rate, 0.1).into_owned();
            fade_edges(&mut piece, rate);

            if index > 0 && !samples.is_empty() {
                let pause = (rate as f32 * CHUNK_PAUSE_SECS).round() as usize;
                samples.extend(std::iter::repeat_n(0.0, pause));
            }
            samples.extend_from_slice(&piece);
        }

        let _ = std::fs::remove_dir_all(&tmp);

        Ok(Audio {
            samples,
            sample_rate,
        })
    }
}

/// Split text into pieces of at most `max_chars` characters, breaking after
/// sentence punctuation whenever possible and backing off to the last space
/// for scripts written without it. Whitespace around the cut is dropped.
///
/// Pieces are filled up to `max_chars` and cut at the last sentence end in
/// the piece, so each invocation handles as few launches as possible.
fn split_text(text: &str, max_chars: usize) -> Vec<String> {
    const ENDERS: &[char] = &['。', '！', '？', '!', '?', '；', ';', '…', '\n'];
    const TRAILERS: &[char] = &['”', '’', '」', '』', '）', ')', '》', '】', '"', '\''];

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut len = 0usize;
    // Byte offset just past the last complete sentence in `current`.
    let mut boundary: Option<usize> = None;
    let mut after_end = false;

    for c in text.chars() {
        // A complete sentence is closed once the next character is neither
        // part of its punctuation nor one of its closing quotes/brackets.
        if after_end && !TRAILERS.contains(&c) {
            boundary = Some(current.len());
            after_end = false;
        }
        current.push(c);
        len += 1;
        if ENDERS.contains(&c) {
            after_end = true;
            boundary = Some(current.len());
        } else if after_end {
            // Closing quote or bracket: it belongs to the sentence.
            boundary = Some(current.len());
        }
        if len < max_chars {
            continue;
        }

        // Prefer the last sentence end, then a word boundary, and only then
        // an exact character cut. Never cut in the first half of the piece:
        // holding a few extra characters is cheaper than a tiny launch.
        let half = current.len() / 2;
        let cut = boundary
            .filter(|&i| i >= half)
            .or_else(|| {
                current
                    .char_indices()
                    .rev()
                    .find(|(_, ch)| ch.is_whitespace())
                    .map(|(i, _)| i)
                    .filter(|&i| i >= half)
            })
            .or_else(|| current.char_indices().nth(max_chars).map(|(i, _)| i))
            .unwrap_or(current.len());

        let tail = current.split_off(cut);
        push_chunk(&mut chunks, &mut current);
        current = tail.trim_start().to_string();
        len = current.chars().count();
        boundary = None;
        after_end = false;
    }
    push_chunk(&mut chunks, &mut current);
    chunks
}

fn push_chunk(chunks: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        chunks.push(trimmed.to_string());
    }
    current.clear();
}

/// Ramp both edges of a piece to zero over [`CHUNK_FADE_SECS`], so joining
/// it with silence (or the next piece) cannot produce a step click.
fn fade_edges(samples: &mut [f32], rate: u32) {
    let n = samples.len();
    if n < 2 {
        return;
    }
    let fade = ((rate as f32 * CHUNK_FADE_SECS).round() as usize)
        .max(1)
        .min(n / 2);
    if fade == 0 {
        return;
    }
    for i in 0..fade {
        let gain = i as f32 / fade as f32;
        samples[i] *= gain;
        samples[n - 1 - i] *= gain;
    }
}

/// Join relative paths onto the model directory; absolute paths pass through.
fn resolve_path(dir: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::{fade_edges, split_text};

    #[test]
    fn fades_piece_edges_to_zero() {
        let mut samples = vec![0.5f32; 1200];
        fade_edges(&mut samples, 24_000);
        // 10 ms at 24 kHz is 240 samples.
        assert_eq!(samples[0], 0.0);
        assert_eq!(samples[1199], 0.0);
        assert!(samples[239] < 0.5 && samples[239] > 0.0);
        assert_eq!(samples[600], 0.5);
    }

    #[test]
    fn packs_whole_sentences_up_to_the_limit() {
        // Eight characters is "句子一。句子二。", so both sentences share a
        // piece instead of each sentence starting a launch of its own.
        let chunks = split_text("句子一。句子二。句子三。", 8);
        assert_eq!(chunks, vec!["句子一。句子二。", "句子三。"]);
    }

    #[test]
    fn keeps_closing_quotes_with_their_sentence() {
        let chunks = split_text("他说：“好。”下一句。", 7);
        assert_eq!(chunks, vec!["他说：“好。”", "下一句。"]);
    }

    #[test]
    fn hard_splits_text_without_punctuation() {
        let text = "字".repeat(450);
        let chunks = split_text(&text, 200);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 200));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn prefers_spaces_for_english() {
        let text = "word ".repeat(60);
        let chunks = split_text(&text, 200);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.chars().count() <= 200));
        // No chunk starts or ends with a partial word.
        for chunk in &chunks {
            assert!(chunk.starts_with("word"), "{chunk:?}");
            assert!(chunk.ends_with("word"), "{chunk:?}");
        }
        // The space at the cut is dropped; the words themselves all survive.
        assert_eq!(
            chunks.join(" ").split_whitespace().collect::<Vec<_>>(),
            text.split_whitespace().collect::<Vec<_>>()
        );
    }

    #[test]
    fn blank_input_produces_nothing() {
        assert!(split_text("  \n ", 200).is_empty());
    }

    #[test]
    fn short_text_stays_in_one_piece() {
        assert_eq!(split_text("一句话。", 200), vec!["一句话。"]);
    }
}
