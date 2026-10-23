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
import sys
import traceback
import wave


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
        )
    else:
        wavs, sr = model.generate_voice_design(
            text=args.text,
            instruct=args.instruct or "",
            language=args.language,
            non_streaming_mode=True,
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
