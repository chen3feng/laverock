#!/usr/bin/env python3
"""Laverock runner for Chatterbox Multilingual V3 (0.5B).

Invoked as a subprocess by laverock-core with a venv that has the official
`chatterbox-tts` package installed. Loads the multilingual model from a
pre-downloaded local weights directory (no HF probe) and clones a reference
clip, optionally into another language. Writes a 16-bit PCM mono WAV.
"""

import argparse
import os
import re
import sys
import traceback
import wave

import numpy as np

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
os.environ.setdefault("HF_HUB_ETAG_TIMEOUT", "3")
os.environ.setdefault("HF_HUB_DOWNLOAD_TIMEOUT", "10")

# Chatterbox generates the whole string in one autoregressive pass. On long
# Chinese text its alignment checker stalls, EOS stays masked, and the
# surplus speech tokens decode to noise: a 143-character take spoke for 23 s
# and then buzzed for 15 s. Split the text into sentences, bound each pass,
# and trim the quiet tail that still follows the last word.
CHUNK_PAUSE_SECS = 0.15
MAX_CHUNK_CHARS = 120
# Only the Chinese (Cangjie) path expands text into roughly one token per
# 40 ms of speech. Other languages tokenize far more coarsely, already stop
# on their own, and would be clipped by the same cap.
CHINESE_TOKEN_RATIO = 2.0
CHINESE_BUDGET_FACTOR = 1.2
# Trailing babble sits well below the speech level; keep a short natural tail.
TAIL_TRIM_DB = 22.0
TAIL_KEEP_SECS = 0.12


def emit(event: str, value: str = "") -> None:
    print(f"@@LV {event} {value}".rstrip(), flush=True)


def resolve_device(torch, requested: str) -> str:
    if requested != "auto":
        return requested
    if torch.cuda.is_available():
        return "cuda:0"
    if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def write_wav(path: str, waveform, sample_rate: int) -> None:
    data = waveform.detach().cpu().float().reshape(-1).numpy()
    pcm = (np.clip(data, -1.0, 1.0) * 32767.0).astype("<i2")
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with wave.open(path, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(int(sample_rate))
        f.writeframes(pcm.tobytes())


def split_text(text: str) -> list[str]:
    """Split text into sentence-sized chunks.

    Sentence ends are Chinese punctuation or ASCII ``. ! ?`` followed by a
    space (so decimals survive). Over-long sentences are cut at commas,
    then hard-wrapped, so one pass never has to cover a whole paragraph.
    """
    ends = [m.end() for m in re.finditer(r"[。！？!?；;…]+|[.!?](?=\s|$)", text)]
    sentences = []
    start = 0
    for end in ends:
        sentences.append(text[start:end])
        start = end
    if text[start:].strip():
        sentences.append(text[start:])
    chunks: list[str] = []
    for sentence in sentences:
        sentence = sentence.strip()
        while len(sentence) > MAX_CHUNK_CHARS:
            cut = max(sentence.rfind(mark, 0, MAX_CHUNK_CHARS) for mark in "，,、")
            cut = cut + 1 if cut > 0 else MAX_CHUNK_CHARS
            chunks.append(sentence[:cut].strip())
            sentence = sentence[cut:].strip()
        if sentence:
            chunks.append(sentence)
    return chunks or [text]


def install_eos_unmask() -> None:
    """Restore the EOS logit the multilingual alignment checker masks.

    The checker suppresses EOS until it believes the text has been covered.
    On Chinese text its alignment stalls after a few characters, so EOS is
    never allowed and generation runs to the token cap. Its forced-EOS
    heuristics still apply; only the blanket mask is lifted.
    """
    try:
        from chatterbox.models.t3.inference.alignment_stream_analyzer import (
            AlignmentStreamAnalyzer,
        )
    except ImportError:
        # A future package layout may move the checker; the chunking and
        # trimming below still keep the output usable.
        return

    original = AlignmentStreamAnalyzer.step

    def step(self, logits, next_token=None):
        saved = logits[..., self.eos_idx].clone()
        out = original(self, logits, next_token=next_token)
        if out is logits and not bool((out[..., self.eos_idx] > 1e4).any()):
            out[..., self.eos_idx] = saved
        return out

    AlignmentStreamAnalyzer.step = step


def reset_alignment_hooks(t3) -> None:
    """Drop attention hooks left by earlier ``generate`` calls.

    Every call builds a fresh alignment checker and registers hooks on the
    transformer, but the previous hooks stay attached; on a multi-chunk
    request they pile up and copy each attention map to the CPU again.
    """
    for layer in t3.tfmr.layers:
        hooks = getattr(layer.self_attn, "_forward_hooks", None)
        if hooks is not None:
            hooks.clear()


def chinese_token_budget(model, text: str, language: str) -> int | None:
    """Token cap for Chinese, whose text tokens track speech length.

    One text token corresponds to roughly one 40 ms speech token, so a
    small margin covers normal pacing while stopping the babble that
    follows the text. Other languages (or Latin text under a Chinese
    language id) return ``None`` and keep the model's own stopping.
    """
    if not language.lower().startswith("zh"):
        return None
    from chatterbox.mtl_tts import punc_norm

    tokens = model.tokenizer.encode(punc_norm(text), language_id=language.lower())
    if len(tokens) < CHINESE_TOKEN_RATIO * len(text):
        return None
    return int(len(tokens) * CHINESE_BUDGET_FACTOR) + 10


def trim_trailing_noise(audio: np.ndarray, sample_rate: int) -> np.ndarray:
    """Cut the low-level babble Chatterbox emits after the last word.

    The tail sits more than 20 dB below the speech, so a percentile-relative
    gate finds the last real syllable without depending on absolute levels.
    A short fade avoids a click at the cut.
    """
    frame = max(1, int(0.02 * sample_rate))
    frames = audio.shape[0] // frame
    if frames == 0:
        return audio
    rms = np.sqrt(np.mean(audio[: frames * frame].reshape(frames, frame) ** 2, axis=1))
    loud = float(np.percentile(rms, 95))
    if loud <= 0:
        return audio
    above = np.where(rms > loud * 10 ** (-TAIL_TRIM_DB / 20))[0]
    if above.size == 0:
        return audio
    end = min(
        audio.shape[0],
        (int(above[-1]) + 1) * frame + int(TAIL_KEEP_SECS * sample_rate),
    )
    trimmed = audio[:end].copy()
    fade = min(int(0.03 * sample_rate), trimmed.shape[0])
    if fade > 0:
        trimmed[-fade:] *= np.linspace(1.0, 0.0, fade, dtype=trimmed.dtype)
    return trimmed


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser()
    p.add_argument("--model", required=True)
    p.add_argument("--text", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--t3", default="v3")
    p.add_argument("--language", default="zh")
    p.add_argument("--ref-audio")
    p.add_argument("--device", default="auto")
    return p.parse_args()


def main() -> int:
    args = parse_args()

    emit("stage", "Loading model into memory")
    import torch
    from chatterbox.mtl_tts import ChatterboxMultilingualTTS

    device = resolve_device(torch, args.device)
    # chatterbox-tts 0.1.7's from_local() takes (ckpt_dir, device) and
    # loads the multilingual T3 checkpoint (t3_mtl23ls_v2.safetensors)
    # by name from ckpt_dir; there is no t3_model argument on PyPI yet.
    model = ChatterboxMultilingualTTS.from_local(args.model, device=device)

    if not args.ref_audio:
        print("clone mode requires --ref-audio", file=sys.stderr)
        return 2

    emit("stage", "Generating speech")
    try:
        install_eos_unmask()
        # Prepare the voice once; every chunk reuses the same conditionals,
        # so the clone stays consistent across sentence boundaries.
        model.prepare_conditionals(args.ref_audio)
        chunks = split_text(args.text)
        budget = {"value": None}
        original_inference = model.t3.inference

        def inference(**kwargs):
            if budget["value"] is not None:
                kwargs["max_new_tokens"] = budget["value"]
            return original_inference(**kwargs)

        model.t3.inference = inference
        pieces = []
        for index, chunk in enumerate(chunks):
            budget["value"] = chinese_token_budget(model, chunk, args.language)
            reset_alignment_hooks(model.t3)
            if len(chunks) > 1:
                emit("stage", f"Generating speech ({index + 1}/{len(chunks)})")
            piece = model.generate(chunk, language_id=args.language)
            audio = trim_trailing_noise(piece.squeeze(0).numpy(), model.sr)
            pieces.append(audio)
            if index != len(chunks) - 1:
                pieces.append(
                    np.zeros(int(CHUNK_PAUSE_SECS * model.sr), dtype=audio.dtype)
                )
        waveform = torch.from_numpy(np.concatenate(pieces)).unsqueeze(0)
    except Exception as exc:
        print(f"synthesis failed: {exc}", file=sys.stderr)
        return 3

    if waveform is None or waveform.numel() == 0:
        print("model returned no audio", file=sys.stderr)
        return 4
    write_wav(args.out, waveform, model.sr)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        traceback.print_exc()
        sys.exit(1)
