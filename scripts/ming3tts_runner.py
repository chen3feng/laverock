#!/usr/bin/env python3
"""Laverock runner for Ming-3.0-tts (0.5B dense).

Invoked as a subprocess by laverock-core with a venv that has the repo's
requirements installed (torch 2.6, transformers 4.46.3). The single
checkpoint handles every task; this runner maps laverock's modes onto them
and writes a 16-bit PCM mono WAV.

Modes:
  clone         task TTS: needs --ref-audio; transcript optional but
                recommended (the acoustic prompt requires it, otherwise the
                voice is cloned from the speaker embedding alone)
  multilingual  task MULTILINGUAL: needs --ref-audio + --language
  voice_design  task VOICE_DESIGN: needs --instruct (its "style" control)
  ip            task IP: needs --speaker (a built-in voice name)
"""

import argparse
import importlib.util
import os
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


def resolve_device(torch, requested: str) -> str:
    if requested != "auto":
        return requested
    if torch.cuda.is_available():
        return "cuda:0"
    if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def write_wav(path: str, waveform, sample_rate: int) -> None:
    """Write a (1, samples) tensor as 16-bit mono PCM WAV."""
    import numpy as np

    data = waveform.detach().cpu().float().reshape(-1).numpy()
    pcm = (np.clip(data, -1.0, 1.0) * 32767.0).astype("<i2")
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with wave.open(path, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(int(sample_rate))
        f.writeframes(pcm.tobytes())


def use_soundfile_audio_io(torch, torchaudio) -> None:
    """Route torchaudio load/save through soundfile in this process.

    torchaudio 2.9+ reads and writes through torchcodec, which needs a
    system FFmpeg and is missing from the ming venv. ming_tts only ever
    loads WAV prompt clips, and soundfile (libsndfile wheels included) is
    already a dependency, so shim the module for this process instead of
    adding an FFmpeg requirement.
    """
    try:
        import soundfile
    except ImportError:
        return
    if getattr(torchaudio, "_laverock_soundfile", False):
        return

    def load(path, *args, **kwargs):
        import numpy as np

        data, sample_rate = soundfile.read(path, dtype="float32", always_2d=True)
        return torch.from_numpy(np.ascontiguousarray(data)).transpose(0, 1), sample_rate

    def save(path, waveform, sample_rate, *args, **kwargs):
        data = waveform.detach().cpu().float().transpose(0, 1).numpy()
        soundfile.write(path, data, int(sample_rate), subtype="PCM_16")

    torchaudio.load = load
    torchaudio.save = save
    torchaudio._laverock_soundfile = True


def use_portable_dtype(model, torch, device: str) -> None:
    """Make the CUDA-oriented checkpoint run on MPS/CPU.

    ming_tts loads the checkpoint in bfloat16 and only wraps generation in
    bfloat16 autocast for CUDA. Off CUDA that context is a no-op, so the
    float32 speaker embedding from the ONNX extractor hits bfloat16 weights
    ("mat1 and mat2 must have the same dtype"), and the VAE's ISTFT cannot
    build a complex32 signal on CPU or MPS. Cast the weights to float32 and
    let the prompt encoder follow the encoder's dtype instead of the
    checkpoint's hardcoded bfloat16 cast.
    """
    if device.startswith("cuda"):
        return
    model.model.to(torch.float32)
    audio = model.model.audio
    encode = audio.encode_latent
    dtype = next(audio.encoder.parameters()).dtype

    def encode_latent(waveform, waveform_length, *args, **kwargs):
        return encode(waveform.to(dtype=dtype), waveform_length, *args, **kwargs)

    audio.encode_latent = encode_latent


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser()
    p.add_argument("--model", required=True)
    p.add_argument("--runtime", required=True)
    p.add_argument("--mode", required=True,
                   choices=["clone", "voice_design", "multilingual", "ip"])
    p.add_argument("--text", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--language", default="")
    p.add_argument("--instruct", default="")
    p.add_argument("--speaker", default="")
    p.add_argument("--ref-audio")
    p.add_argument("--ref-text", default="")
    p.add_argument("--device", default="auto")
    return p.parse_args()


def main() -> int:
    args = parse_args()
    # The repo's Python sources (ming_tts.py + the modules it imports) live
    # in --runtime; make them importable before anything pulls them in.
    sys.path.insert(0, os.path.abspath(args.runtime))

    emit("stage", "Loading model into memory")
    import torch
    import torchaudio

    use_soundfile_audio_io(torch, torchaudio)
    from ming_tts import MingTTS

    device = resolve_device(torch, args.device)

    # FST text normalization needs `pynini`, which has no universal wheel
    # (conda-forge only). Skip it silently when absent so installs stay
    # portable; inline numbers are still normalized by the pure Python
    # frontend regardless.
    pynini = importlib.util.find_spec("pynini") is not None
    try:
        model = MingTTS(args.model, device=device, text_normalization=pynini)
    except Exception:
        # A device or dtype mismatch should not be fatal to the request.
        device = "cpu"
        model = MingTTS(args.model, device=device, text_normalization=pynini)
    use_portable_dtype(model, torch, device)

    emit("stage", "Generating speech")
    if args.mode in ("clone", "multilingual"):
        if not args.ref_audio:
            print(f"{args.mode} mode requires --ref-audio", file=sys.stderr)
            return 2
        kwargs = {
            "task": "TTS" if args.mode == "clone" else "MULTILINGUAL",
            "text": args.text,
            "prompt_wav_path": args.ref_audio,
            "use_spk_emb": True,
        }
        if args.mode == "multilingual":
            kwargs["language_id"] = args.language or "Chinese"
        # The acoustic prompt is only used together with its transcript; a
        # missing transcript falls back to speaker-embedding-only cloning.
        if args.ref_text:
            kwargs["prompt_text"] = args.ref_text
    elif args.mode == "voice_design":
        if not args.instruct:
            print("voice_design mode requires --instruct", file=sys.stderr)
            return 2
        kwargs = {
            "task": "VOICE_DESIGN",
            "text": args.text,
            "controls": {"style": args.instruct},
            "use_zero_spk_emb": True,
        }
    else:  # ip
        if not args.speaker:
            print("ip mode requires --speaker", file=sys.stderr)
            return 2
        kwargs = {
            "task": "IP",
            "text": args.text,
            "controls": {"ip": args.speaker},
            "use_zero_spk_emb": True,
        }

    try:
        waveform = model.synthesize(**kwargs)
    except Exception as exc:
        print(f"synthesis failed: {exc}", file=sys.stderr)
        return 4

    if waveform is None or waveform.numel() == 0:
        print("model returned no audio", file=sys.stderr)
        return 3
    write_wav(args.out, waveform, model.sample_rate)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        traceback.print_exc()
        sys.exit(1)