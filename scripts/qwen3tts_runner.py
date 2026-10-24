#!/usr/bin/env python3
"""Laverock runner for Qwen3-TTS.

Invoked as a subprocess by laverock-core with a Python environment that has
the official `qwen-tts` package installed. Writes a 16-bit PCM mono WAV.

Modes:
  clone          Base model; needs --ref-audio (and optionally --ref-text)
  custom_voice   CustomVoice model; needs --speaker, optional --instruct
  voice_design   VoiceDesign model; needs --instruct
"""

import argparse
import os
import re
import sys
import traceback
import wave

# Synthesis runs against the fully cached snapshot from the download
# script. transformers/hub probe the network on every launch (metadata and
# ETag checks); through a slow or dead proxy those requests stall for
# minutes with the UI stuck at "loading model". Default to offline cache
# use (set HF_HUB_OFFLINE=0 to override for development).
os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
os.environ.setdefault("HF_HUB_ETAG_TIMEOUT", "3")
os.environ.setdefault("HF_HUB_DOWNLOAD_TIMEOUT", "10")


def emit(event: str, value: str = "") -> None:
    """Print a progress marker the Rust side parses from stdout."""
    print(f"@@LV {event} {value}".rstrip(), flush=True)


def attach_step_counter(model) -> None:
    """Count talker decode steps by wrapping its forward.

    qwen-tts does not forward HF generate kwargs (stopping criteria never
    fire), so we instrument the talker module directly. Its autoregressive
    generate() calls forward once per codec frame; the 12 Hz codec means
    one call is ~1/12.5 s of produced audio.
    """
    talker = model.model.talker
    original_forward = talker.forward
    state = {"n": 0}

    import functools

    @functools.wraps(original_forward)
    def counting_forward(*args, **kwargs):
        state["n"] += 1
        if state["n"] % 4 == 0:
            emit("gen", str(state["n"]))
        return original_forward(*args, **kwargs)

    # functools.wraps keeps the inspected signature — transformers'
    # generate() validates kwargs against forward() and rejects everything
    # an opaque (*args, **kwargs) wrapper claims not to accept.
    talker.forward = counting_forward


def speed_up_cached_loading() -> None:
    """Skip transformers' mistral-tag hub probe.

    `PreTrainedTokenizerBase._patch_mistral_regex` calls `model_info`
    (a network round-trip) on every tokenizer load, purely to detect
    mistralai base models — Qwen repos are never mistral, so a no-op is
    equivalent and removes the last blocking metadata probe.
    """
    try:
        import transformers.tokenization_utils_base as tub

        tub.PreTrainedTokenizerBase._patch_mistral_regex = classmethod(
            lambda cls, tokenizer, *_a, **_k: tokenizer
        )
    except Exception:
        pass


def step_cap(text: str) -> int:
    """Bound codec steps at roughly 2x the expected audio length.

    A mismatched reference can make the talker miss EOS and run to the
    package default (2048 frames, several minutes). At 12.5 frames/s the
    cap clips that runaway instead of hanging the UI.
    """
    cjk = len(re.findall(r"[一-鿿぀-ヿ가-힯]", text))
    words = len(re.sub(r"[一-鿿぀-ヿ가-힯]", " ", text).split())
    est_seconds = cjk / 3.8 + words / 2.6
    return max(120, round(est_seconds * 12.5 * 2) + 80)


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser()
    p.add_argument("--model", required=True)
    p.add_argument("--mode", required=True,
                   choices=["clone", "custom_voice", "voice_design"])
    p.add_argument("--text", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--language", default="Auto")
    p.add_argument("--ref-audio")
    p.add_argument("--ref-text", default="")
    p.add_argument("--speaker", default="")
    p.add_argument("--instruct", default="")
    p.add_argument("--device", default="auto")
    p.add_argument("--dtype", default="auto")
    p.add_argument("--attn", default="sdpa")
    return p.parse_args()


def resolve_device(torch, requested: str) -> str:
    if requested != "auto":
        return requested
    if torch.cuda.is_available():
        return "cuda:0"
    if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def resolve_dtype(torch, requested: str, device: str):
    table = {
        "bfloat16": torch.bfloat16,
        "float16": torch.float16,
        "float32": torch.float32,
    }
    if requested in table:
        return table[requested]
    if device.startswith("cuda"):
        return torch.bfloat16
    # float16 on MPS makes the codec emit NaN probabilities; fp32 is the
    # safe accelerated choice on Apple Silicon.
    return torch.float32


def write_wav(path: str, samples, sample_rate: int) -> None:
    import numpy as np

    data = np.asarray(samples, dtype=np.float32).reshape(-1)
    pcm = (np.clip(data, -1.0, 1.0) * 32767.0).astype("<i2")
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with wave.open(path, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(int(sample_rate))
        f.writeframes(pcm.tobytes())


def main() -> int:
    args = parse_args()

    speed_up_cached_loading()
    emit("stage", "正在加载模型到内存")
    import torch
    from qwen_tts import Qwen3TTSModel

    device = resolve_device(torch, args.device)
    dtype = resolve_dtype(torch, args.dtype, device)

    load_kwargs = {"device_map": device, "dtype": dtype}
    if args.attn and args.attn != "none":
        load_kwargs["attn_implementation"] = args.attn

    try:
        model = Qwen3TTSModel.from_pretrained(args.model, **load_kwargs)
    except Exception:
        # Flash-attention or an unsupported dtype should not be fatal.
        load_kwargs.pop("attn_implementation", None)
        model = Qwen3TTSModel.from_pretrained(args.model, **load_kwargs)

    attach_step_counter(model)
    emit("stage", "正在合成语音")
    gen_limit = {"max_new_tokens": step_cap(args.text)}
    if args.mode == "clone":
        if not args.ref_audio:
            print("clone mode requires --ref-audio", file=sys.stderr)
            return 2
        wavs, sr = model.generate_voice_clone(
            text=args.text,
            language=args.language,
            ref_audio=args.ref_audio,
            ref_text=args.ref_text or None,
            x_vector_only_mode=not bool(args.ref_text),
            non_streaming_mode=True,
            **gen_limit,
        )
    elif args.mode == "custom_voice":
        if not args.speaker:
            print("custom_voice mode requires --speaker", file=sys.stderr)
            return 2
        wavs, sr = model.generate_custom_voice(
            text=args.text,
            speaker=args.speaker,
            language=args.language,
            instruct=args.instruct or None,
            non_streaming_mode=True,
            **gen_limit,
        )
    else:
        wavs, sr = model.generate_voice_design(
            text=args.text,
            instruct=args.instruct or "",
            language=args.language,
            non_streaming_mode=True,
            **gen_limit,
        )

    if not wavs:
        print("model returned no audio", file=sys.stderr)
        return 3
    write_wav(args.out, wavs[0], sr)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        traceback.print_exc()
        sys.exit(1)
