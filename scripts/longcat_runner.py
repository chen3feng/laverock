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


_DIGIT_CN = {
    "0": "零", "1": "一", "2": "二", "3": "三", "4": "四",
    "5": "五", "6": "六", "7": "七", "8": "八", "9": "九",
}


def normalize_numbers(text: str) -> str:
    """Read digit strings as Chinese numerals.

    Upstream notes the model handles Arabic numerals poorly (issue #9)
    and recommends text normalization. Digit-by-digit reading matches how
    people typically read phone numbers, years and digit sequences, which
    is also what reference clips usually contain.
    """
    text = re.sub(r"(?<=\d),(?=\d)", "", text)
    text = re.sub(r"[０-９]", lambda m: chr(ord(m.group(0)) - 0xFF10 + ord("0")), text)

    def repl(match: re.Match) -> str:
        return "".join(
            "点" if c in ".．" else _DIGIT_CN[c] for c in match.group(0)
        )

    return re.sub(r"\d+(?:[.．]\d+)?", repl, text)


def split_into_chunks(text: str, budget_seconds: float) -> list:
    """Split text so each chunk's estimated speech fits the budget.

    LongCat can only produce `max_wav_duration` (30 s) per call, prompt
    included. Handing it more text than fits makes it drop the beginning,
    so split on sentence punctuation and, when a single sentence is still
    too long, on commas and finally characters.
    """
    budget = max(1.0, budget_seconds)

    def fits(part: str) -> bool:
        return approx_duration_from_text(part, max_duration=1e9) <= budget

    units = [u for u in re.split(r"(?<=[。！？!?；;\n])", text) if u.strip()]
    chunks: list = []
    current = ""
    for unit in units:
        candidate = current + unit
        if fits(candidate):
            current = candidate
            continue
        if current.strip():
            chunks.append(current.strip())
        current = ""
        if fits(unit):
            current = unit
            continue
        # A single sentence over budget: prefer comma-level pieces.
        parts = [p for p in re.split(r"(?<=[，,、：:])", unit) if p.strip()]
        for part in parts:
            candidate = current + part
            if fits(candidate):
                current = candidate
            else:
                if current.strip():
                    chunks.append(current.strip())
                current = ""
                if fits(part):
                    current = part
                else:
                    # Last resort: fixed-size slices by estimated chars.
                    per_char = 0.21
                    step = max(1, int(budget / per_char))
                    for i in range(0, len(part), step):
                        chunks.append(part[i:i + step])
    if current.strip():
        chunks.append(current.strip())
    return chunks or [text]


def normalize_volume(y, target_dbfs: float = -6.0):
    import torch

    max_sample = torch.max(torch.abs(y))
    current_dbfs = 20 * torch.log10(max_sample + 1e-8)
    gain = 10 ** ((target_dbfs - current_dbfs) / 20)
    return torch.clip(y * gain, -1.0, 1.0)


def trim_to_silence(audio, limit_sec: float, sr: int):
    """Cut a long prompt near `limit_sec`, preferring a quiet point.

    Cutting mid-word makes the model continue the reference's last
    syllable at the start of the generated audio, so look for the
    quietest 50 ms window before the limit and append a short pause.
    """
    import torch

    dtype = audio.dtype
    end = min(audio.shape[-1], int(limit_sec * sr))
    win = max(1, int(0.05 * sr))
    step = max(1, int(0.02 * sr))
    search_from = max(0, end - int(1.0 * sr))
    best = None
    for start in range(search_from, end - win + 1, step):
        seg = audio[..., start:start + win]
        if seg.shape[-1] < win:
            break
        rms = float(seg.pow(2).mean().sqrt())
        if best is None or rms < best[0]:
            best = (rms, start + win)
    cut = best[1] if best is not None and best[0] < 0.01 else end
    cut = max(cut, int(0.5 * sr))
    trimmed = audio[..., :cut]
    pause = torch.zeros(*audio.shape[:-1], int(0.1 * sr), dtype=dtype)
    return torch.cat([trimmed, pause], dim=-1)


def speech_extent_secs(audio, sr: int, floor_db: float = -40.0) -> float:
    """Seconds from the first to the last 50 ms frame that carries speech.

    A clip's file length includes leading and trailing silence, which adds
    no content. Measuring the voiced span keeps a recording with a quiet
    room at either end from looking like a clip that carries extra speech.
    `audio` is a ``(1, samples)`` tensor already normalized to -6 dBFS
    peak, so the threshold is absolute.
    """
    mono = audio.reshape(-1)
    win = max(1, int(0.05 * sr))
    if mono.numel() < win:
        return mono.numel() / sr
    frames = mono[: mono.numel() // win * win].reshape(-1, win)
    rms = frames.pow(2).mean(dim=1).sqrt()
    voiced = (rms > 10 ** (floor_db / 20.0)).nonzero()
    if voiced.numel() == 0:
        return 0.0
    start = int(voiced[0].item()) * win
    end = min(mono.numel(), (int(voiced[-1].item()) + 1) * win)
    return (end - start) / sr


def load_audio(path: str, sr: int):
    """Reference clip at `sr`, mono, volume-normalized like upstream."""
    import librosa
    import torch

    audio, _ = librosa.load(path, sr=sr, mono=True)
    audio = torch.from_numpy(audio).unsqueeze(0)
    return normalize_volume(audio, -6)


# `approx_duration_from_text` charges an English letter 0.082 s, the rate
# inside a word. A run of capitals ("ABCDEFG") is read letter by letter and
# takes roughly 0.35 s each, so the difference is added when deciding
# whether a reference clip carries more speech than its transcript.
SPELLED_LETTER_EXTRA_SECS = 0.35 - 0.082


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

    emit("stage", "Loading model into memory")
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

    text = normalize_text(normalize_numbers(args.text))
    no_prompt = not args.ref_audio

    prompt_wav = None
    prompt_dur = 0
    prompt_time = 0.0
    approx_pd = 0.0
    prompt_text = ""
    if not no_prompt:
        prompt_text = normalize_text(normalize_numbers(args.ref_text or ""))
        if not prompt_text:
            print("clone mode requires --ref-text", file=sys.stderr)
            return 2

        # LongCat aligns the text condition against the prompt *duration*.
        # A clip that carries more speech than its transcript accounts for
        # pushes the start of the target text into the prompt region that
        # is sliced away before decoding — the first sentence disappears.
        # Trim such a clip to a duration consistent with the transcript.
        #
        # The duration estimate is a normal-pace baseline, so it cannot be
        # the only test. Counting digits and spelling out a run of capitals
        # is much slower than reading words, and a slow but complete
        # reading carries no extra speech. Cutting it would leave the rest
        # of the transcript to be spoken as the start of the generated
        # audio, i.e. the reference tail leaks into the take.
        approx_pd = approx_duration_from_text(prompt_text, max_duration=max_duration)
        spelled = sum(len(run) for run in re.findall(r"[A-Z]{2,}", args.ref_text or ""))
        plausible_pd = min(
            max_duration, approx_pd + spelled * SPELLED_LETTER_EXTRA_SECS
        )
        audio = load_audio(args.ref_audio, sr)
        speech_secs = speech_extent_secs(audio, sr)
        if speech_secs > max(2.0, plausible_pd * 2.0):
            audio = trim_to_silence(audio, max(2.0, approx_pd * 1.4), sr)
            emit(
                "stage",
                "Reference clip carries more speech than the transcript; "
                f"trimmed to {audio.shape[-1] / sr:.1f}s",
            )
        elif speech_secs < approx_pd / 1.8:
            emit(
                "stage",
                "Note: the transcript is much longer than the reference "
                "speech; the start may be clipped",
            )
        prompt_wav = audio.unsqueeze(0).to(device)

        # Upstream pads one hop of margin, then drops it, so the VAE sees
        # a full receptive field at the tail.
        off = 3
        pw = audio
        if pw.shape[-1] % full_hop != 0:
            pw = F.pad(pw, (0, full_hop - pw.shape[-1] % full_hop))
        pw = F.pad(pw, (0, full_hop * off))
        with torch.no_grad():
            plt = model.vae.encode(pw.unsqueeze(0).to(device))
        if off:
            plt = plt[..., :-off]
        prompt_dur = plt.shape[-1]
        prompt_time = prompt_dur * full_hop / sr

    if prompt_time >= max_duration - 0.5:
        print(
            f"reference audio is {prompt_time:.1f}s, but LongCat-AudioDiT "
            f"can only generate {max_duration:.0f}s including the prompt; "
            "use a 3–10s reference clip",
            file=sys.stderr,
        )
        return 2

    ratio = 1.0
    if not no_prompt:
        ratio = float(np.clip(prompt_time / max(approx_pd, 1e-3), 1.0, 1.5))
    # Text seconds that fit in one call, after accounting for the prompt.
    budget = max(1.0, (max_duration - prompt_time - 0.5) / ratio)
    chunks = split_into_chunks(text, budget)
    if len(chunks) > 1:
        emit("stage", f"Long text: synthesizing in {len(chunks)} chunks")

    pieces = []
    for index, chunk in enumerate(chunks):
        if len(chunks) > 1:
            emit("stage", f"Synthesizing chunk {index + 1}/{len(chunks)}")
        full_text = chunk if no_prompt else f"{prompt_text} {chunk}"
        inputs = tokenizer([full_text], padding="longest", return_tensors="pt")
        dur_sec = approx_duration_from_text(chunk, max_duration=budget) * ratio
        duration = int(dur_sec * sr // full_hop)
        duration = min(duration + prompt_dur, int(max_duration * sr // full_hop))
        if len(chunks) == 1:
            emit("stage", f"Generating speech ({dur_sec:.1f}s)")
        torch.manual_seed(args.seed + index)
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
        pieces.append(
            output.waveform.squeeze().detach().to(torch.float32).cpu().numpy()
        )

    # A short pause between chunks keeps sentence boundaries audible.
    gap = np.zeros(int(0.12 * sr), dtype=np.float32)
    merged = pieces[0]
    for piece in pieces[1:]:
        merged = np.concatenate([merged, gap, piece])
    wav = np.clip(merged, -1.0, 1.0)
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
