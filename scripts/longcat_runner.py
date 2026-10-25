#!/usr/bin/env python3
"""Laverock runner for LongCat-AudioDiT.

Invoked as a subprocess by laverock-core with a Python environment that
has the official `audiodit` package (copied next to this script) and its
dependencies installed. Writes a 16-bit PCM mono WAV.

Zero-shot cloning needs both the reference clip and its transcript; plain
TTS (no reference) also works.
"""

import argparse
import os
import re
import sys
import traceback

# Synthesis runs against the weights and tokenizer unpacked by the
# download script. Never let transformers/huggingface_hub probe the
# network on the hot path (a slow proxy used to stall Qwen for minutes).
os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
os.environ.setdefault("HF_HUB_ETAG_TIMEOUT", "3")
os.environ.setdefault("HF_HUB_DOWNLOAD_TIMEOUT", "10")

# `audiodit/` is unpacked next to this script inside the model directory.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))


def emit(event: str, value: str = "") -> None:
    print(f"@@LV {event} {value}".rstrip(), flush=True)


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser()
    p.add_argument("--model", required=True)
    p.add_argument("--tokenizer", default=None,
                   help="Local tokenizer directory (google/umt5-base files)")
    p.add_argument("--text", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--ref-audio", default=None)
    p.add_argument("--ref-text", default="")
    p.add_argument("--device", default="auto")
    p.add_argument("--dtype", default="auto")
    p.add_argument("--steps", type=int, default=16)
    p.add_argument("--cfg", type=float, default=4.0)
    p.add_argument("--guidance", default="apg", choices=["apg", "cfg"])
    p.add_argument("--seed", type=int, default=1024)
    return p.parse_args()


def resolve_device(torch, requested: str) -> str:
    if requested != "auto":
        return requested
    if torch.cuda.is_available():
        return "cuda"
    if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def resolve_vae_dtype(torch, requested: str, device: str):
    """Pick the WAV-VAE dtype.

    Upstream runs the VAE in fp16. That is fine on CUDA but risky on MPS
    (fp16 codec math has produced NaNs in other engines) and unsupported
    by some CPU kernels, so fp32 is used everywhere else.
    """
    if requested in ("float16", "fp16"):
        return torch.float16
    if requested in ("bfloat16", "bf16"):
        return torch.bfloat16
    if requested in ("float32", "fp32"):
        return torch.float32
    return torch.float16 if device.startswith("cuda") else torch.float32


def normalize_text(text: str) -> str:
    text = text.lower()
    text = re.sub(r'["“”‘’]', " ", text)
    text = re.sub(r"\s+", " ", text)
    return text


def normalize_volume(y, target_dbfs: float = -6.0):
    import torch

    max_sample = torch.max(torch.abs(y))
    current_dbfs = 20 * torch.log10(max_sample + 1e-8)
    gain = 10 ** ((target_dbfs - current_dbfs) / 20)
    return torch.clip(y * gain, -1.0, 1.0)


def load_audio(path: str, sr: int):
    """Reference clip at `sr`, mono, volume-normalized like upstream."""
    import librosa
    import torch

    audio, _ = librosa.load(path, sr=sr, mono=True)
    audio = torch.from_numpy(audio).unsqueeze(0)
    return normalize_volume(audio, -6)


def approx_duration_from_text(text: str, max_duration: float = 30.0) -> float:
    """Same heuristic as the upstream `utils.approx_duration_from_text`."""
    en_dur_per_char = 0.082
    zh_dur_per_char = 0.21
    text = re.sub(r"\s+", "", text)
    num_zh = num_en = num_other = 0
    for c in text:
        if "一" <= c <= "鿿":
            num_zh += 1
        elif c.isalpha():
            num_en += 1
        else:
            num_other += 1
    if num_zh > num_en:
        num_zh += num_other
    else:
        num_en += num_other
    return min(max_duration, num_zh * zh_dur_per_char + num_en * en_dur_per_char)


def main() -> int:
    args = parse_args()

    emit("stage", "正在加载模型到内存")
    import numpy as np
    import torch
    import torch.nn.functional as F

    import audiodit  # noqa: F401  (registers AudioDiTConfig/AudioDiTModel)
    from audiodit import AudioDiTModel
    from transformers import AutoTokenizer

    device = resolve_device(torch, args.device)
    torch.manual_seed(args.seed)
    if device.startswith("cuda"):
        torch.cuda.manual_seed(args.seed)

    model = AudioDiTModel.from_pretrained(args.model).to(device)
    vae_dtype = resolve_vae_dtype(torch, args.dtype, device)
    model.vae.to(vae_dtype)
    model.eval()

    tokenizer_name = args.tokenizer or model.config.text_encoder_model
    tokenizer = AutoTokenizer.from_pretrained(tokenizer_name)

    sr = model.config.sampling_rate
    full_hop = model.config.latent_hop
    max_duration = model.config.max_wav_duration

    text = normalize_text(args.text)
    no_prompt = not args.ref_audio

    if no_prompt:
        full_text = text
    else:
        prompt_text = normalize_text(args.ref_text or "")
        if not prompt_text:
            print("clone mode requires --ref-text", file=sys.stderr)
            return 2
        full_text = f"{prompt_text} {text}"

    inputs = tokenizer([full_text], padding="longest", return_tensors="pt")

    prompt_wav = None
    prompt_dur = 0
    if not no_prompt:
        prompt_wav = load_audio(args.ref_audio, sr).unsqueeze(0).to(device)
        # Upstream pads one hop of margin, then drops it, so the VAE sees
        # a full receptive field at the tail.
        off = 3
        pw = load_audio(args.ref_audio, sr)
        if pw.shape[-1] % full_hop != 0:
            pw = F.pad(pw, (0, full_hop - pw.shape[-1] % full_hop))
        pw = F.pad(pw, (0, full_hop * off))
        with torch.no_grad():
            plt = model.vae.encode(pw.unsqueeze(0).to(device))
        if off:
            plt = plt[..., :-off]
        prompt_dur = plt.shape[-1]

    prompt_time = prompt_dur * full_hop / sr
    if not no_prompt and prompt_time >= max_duration - 0.5:
        print(
            f"reference audio is {prompt_time:.1f}s, but LongCat-AudioDiT "
            f"can only generate {max_duration:.0f}s including the prompt; "
            "use a 3–10s reference clip",
            file=sys.stderr,
        )
        return 2
    dur_sec = approx_duration_from_text(text, max_duration=max_duration - prompt_time)
    if not no_prompt:
        approx_pd = approx_duration_from_text(prompt_text, max_duration=max_duration)
        ratio = np.clip(prompt_time / max(approx_pd, 1e-3), 1.0, 1.5)
        dur_sec = dur_sec * ratio
    duration = int(dur_sec * sr // full_hop)
    duration = min(duration + prompt_dur, int(max_duration * sr // full_hop))
    emit("stage", f"正在合成语音（{dur_sec:.1f} 秒）")

    with torch.no_grad():
        output = model(
            input_ids=inputs.input_ids,
            attention_mask=inputs.attention_mask,
            prompt_audio=prompt_wav,
            duration=duration,
            steps=args.steps,
            cfg_strength=args.cfg,
            guidance_method=args.guidance,
        )

    wav = output.waveform.squeeze().detach().to(torch.float32).cpu().numpy()
    wav = np.clip(wav, -1.0, 1.0)
    pcm = (wav * 32767.0).astype("<i2")
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    import wave

    with wave.open(args.out, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(int(sr))
        f.writeframes(pcm.tobytes())
    print(f"saved {args.out} ({len(wav) / sr:.2f}s)", file=sys.stderr, flush=True)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        traceback.print_exc()
        sys.exit(1)
