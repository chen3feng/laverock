#!/usr/bin/env python3
"""Laverock runner for Chatterbox Multilingual V3 (0.5B).

Invoked as a subprocess by laverock-core with a venv that has the official
`chatterbox-tts` package installed. Loads the multilingual model from a
pre-downloaded local weights directory (no HF probe) and clones a reference
clip, optionally into another language. Writes a 16-bit PCM mono WAV.
"""

import argparse
import os
import sys
import traceback
import wave

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
os.environ.setdefault("HF_HUB_ETAG_TIMEOUT", "3")
os.environ.setdefault("HF_HUB_DOWNLOAD_TIMEOUT", "10")


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
    import numpy as np

    data = waveform.detach().cpu().float().reshape(-1).numpy()
    pcm = (np.clip(data, -1.0, 1.0) * 32767.0).astype("<i2")
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with wave.open(path, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(int(sample_rate))
        f.writeframes(pcm.tobytes())


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
        waveform = model.generate(
            args.text,
            language_id=args.language,
            audio_prompt_path=args.ref_audio,
        )
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