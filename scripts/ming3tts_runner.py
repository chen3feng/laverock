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
import random
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
    """Run the CUDA-oriented checkpoint on CUDA, MPS or CPU.

    ming_tts loads the checkpoint in bfloat16 and only wraps generation in
    bfloat16 autocast for CUDA. Off CUDA that context is a no-op, so the
    float32 speaker embedding from the ONNX extractor hits bfloat16 weights
    ("mat1 and mat2 must have the same dtype"), and the VAE's ISTFT cannot
    build a complex32 signal on CPU or MPS. Cast the weights to float32 and
    let the prompt encoder follow the encoder's dtype instead of the
    checkpoint's hardcoded bfloat16 cast. On CUDA the LLM keeps bfloat16;
    only the audio tokenizer is cast, because its ISTFT also lacks a
    complex32 CUDA kernel in recent torch.
    """
    audio = model.model.audio
    if not device.startswith("cuda"):
        model.model.to(torch.float32)
    # bfloat16 ISTFT builds a complex32 signal, and recent torch has no
    # mul_cuda kernel for BComplex32 either, so the tokenizer runs in
    # float32 on CUDA while the LLM stays bfloat16.
    audio.to(torch.float32)
    encode = audio.encode_latent
    dtype = next(audio.encoder.parameters()).dtype

    def encode_latent(waveform, waveform_length, *args, **kwargs):
        return encode(waveform.to(dtype=dtype), waveform_length, *args, **kwargs)

    audio.encode_latent = encode_latent


def use_float32_istft(torch) -> None:
    """Run the VAE's ISTFT outside bfloat16 autocast.

    ming_tts wraps CUDA generation in `torch.autocast(bfloat16)`, so the
    ISTFT's `mag * (x + 1j * y)` builds a complex32 tensor no matter what
    dtype the weights have, and recent torch has no CUDA kernel for it
    ("mul_cuda not implemented for 'BComplex32'"). Entering a disabled
    autocast region inside ISTFTHead.forward restores complex64.
    """
    try:
        from audio_tokenizer.istft import ISTFTHead
    except Exception:
        return
    original = ISTFTHead.forward

    def forward(self, *args, **kwargs):
        with torch.autocast(device_type="cuda", enabled=False):
            return original(self, *args, **kwargs)

    ISTFTHead.forward = forward


# Ming splits long text into fragments and generates each one separately.
# The default target is 24 semantic units, which cuts ordinary English
# sentences in half: the seam is audible as a level step, and the model
# sometimes swallows the first words of a fragment that starts
# mid-sentence. Most requests fit in one fragment at 64, so raise the
# target and only fall back to splitting for genuinely long text.
MING_MAX_SEMANTIC_LENGTH = 64
# Per-fragment level matching is bounded: a 4x cut is enough for the worst
# seam seen, and capping the boost avoids amplifying a quiet, noisy chunk.
FRAGMENT_GAIN_MIN = 0.25
FRAGMENT_GAIN_MAX = 2.0
# The released runtime seeds every inference with 1895 (infer.py) before it
# builds the model. The prompt latent is sampled from the VAE posterior, so
# without the same seed the model's start alignment drifts between runs and
# can swallow the first words of the target text.
MING_SEED = 1895


def seed_everything(torch, seed: int = MING_SEED) -> None:
    """Match the released runtime's seeding order (before the model loads)."""
    import numpy as np

    random.seed(seed)
    np.random.seed(seed)
    torch.manual_seed(seed)
    torch.cuda.manual_seed_all(seed)
    torch.backends.cudnn.deterministic = True
    torch.backends.cudnn.benchmark = False


def install_fragment_level_match(ming_tts, MingTTS) -> None:
    """Even out the loudness of the fragments ``MingTTS.synthesize`` joins.

    The runtime trims each fragment and then concatenates them without
    matching levels, so a seam can step up by ~10 dB. Wrap the per-fragment
    trim: the first fragment sets the reference RMS, and later fragments are
    scaled towards it. The wrapper also raises ``max_length`` so short texts
    are generated in one pass and never reach a seam.
    """
    original_trim = ming_tts.trim_trailing_silence
    target_rms = {"value": None}

    def trim(chunk, sample_rate, *args, **kwargs):
        chunk = original_trim(chunk, sample_rate, *args, **kwargs)
        if chunk.numel() == 0:
            return chunk
        rms = float(chunk.float().pow(2).mean().sqrt())
        if rms <= 0:
            return chunk
        if target_rms["value"] is None:
            target_rms["value"] = rms
            return chunk
        gain = target_rms["value"] / rms
        gain = max(FRAGMENT_GAIN_MIN, min(FRAGMENT_GAIN_MAX, gain))
        return chunk * gain

    ming_tts.trim_trailing_silence = trim

    original_synthesize = MingTTS.synthesize

    def synthesize(self, *args, **kwargs):
        target_rms["value"] = None
        kwargs.setdefault("max_length", MING_MAX_SEMANTIC_LENGTH)
        return original_synthesize(self, *args, **kwargs)

    MingTTS.synthesize = synthesize


def with_text_boundary(prompt_text: str) -> str:
    """End the reference transcript with a sentence boundary.

    The runtime concatenates the transcript and the target text in one
    ``Text input:`` field with no separator. When the transcript ends in a
    letter or digit (an auto-transcribed clip often does), the model reads
    the first target words as a continuation of the transcript and can
    swallow them. A terminal period restores the boundary; a space or a
    newline is not enough.
    """
    text = prompt_text.rstrip()
    if not text or text[-1] in ".!?。！？":
        return prompt_text
    has_cjk = any("一" <= char <= "鿿" for char in text)
    return text + ("。" if has_cjk else ".")


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
    seed_everything(torch)
    import ming_tts
    from ming_tts import MingTTS

    install_fragment_level_match(ming_tts, MingTTS)

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
    use_float32_istft(torch)

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
            kwargs["prompt_text"] = with_text_boundary(args.ref_text)
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