#!/usr/bin/env python3
"""Measure the quality of every installed TTS engine.

Four things matter for a cloned voice, and each has a cheap on-device
measure:

  accuracy     the words that came back, read by the local STT model and
               compared with the input (character-level CER, for both zh and en)
  similarity   CAM++ speaker embedding of the output vs the reference clip
  stability    take-to-take agreement, and how far the voice drifts across
               a long paragraph (embedding cosine by position)
  naturalness  DNSMOS P.835 (OVR/SIG/BAK) plus pitch and pacing statistics

Everything runs locally: the laverock server synthesizes and transcribes,
onnxruntime computes the embeddings and the MOS predictor.

Score with the STT engine's `use_itn` turned off: with ITN on, spoken numbers
come back as digits ("三千八百点五" -> "3800.5") and a correct reading is
scored as an error against a test sentence written in characters.

Usage (any venv with numpy/soundfile/librosa/onnxruntime works, e.g. the
ming3tts one):

  python3 scripts/eval-quality.py --base http://127.0.0.1:8897 \\
      --models /path/to/onnx --out /tmp/quality --ref reference.wav \\
      --ref-text "第一句，今天天气很好。" --report quality.md

  --phase synth   only synthesize (slow: models load per request)
  --phase score   only score whatever is already in --out
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import unicodedata
import urllib.request
import uuid
from pathlib import Path

import numpy as np
import soundfile as sf

SR = 16000

# Test material. `base` cases feed the headline metrics; `hard` cases are the
# places where TTS still slips — heteronyms, tongue twisters, numbers and
# units, acronyms, and code-switching. `spoken` is the form a careful reader
# would say out loud, which is what CER must compare against (an ASR hears
# "三千八百点五", not "3800.5").
CASES = [
    # --- base ---------------------------------------------------------
    {"id": "s1a", "lang": "zh", "group": "base", "text": "你好，今天天气不错。"},
    {"id": "s1b", "lang": "zh", "group": "base", "text": "你好，今天天气不错。"},
    {"id": "s1c", "lang": "zh", "group": "base", "text": "你好，今天天气不错。"},
    {"id": "s2", "lang": "zh", "group": "base",
     "text": "人工智能技术在过去十年里取得了令人瞩目的进展，语音合成也变得越来越自然。"},
    {"id": "s3", "lang": "zh", "group": "base",
     "text": "清晨的城市还没有完全醒来，街道上只有零星的车辆和早起锻炼的人。"
             "咖啡店的门刚刚打开，烘焙的香气顺着风飘到人行道上。"
             "有人说，一座城市真正的样子，藏在它醒来的那一个小时里。"
             "我沿着河边慢慢走着，看着太阳一点点升起来，把水面染成淡金色。"},
    {"id": "e1", "lang": "en", "group": "base",
     "text": "Hello, this is a quick test of the voice engine."},

    # --- hard ---------------------------------------------------------
    {"id": "h_poly", "lang": "zh", "group": "hard",
     "text": "银行行长说，他重新开始了这段很长的旅程。",
     "note": "多音字：行 háng/xíng，长 zhǎng/cháng，重 chóng/zhòng"},
    {"id": "h_twist", "lang": "zh", "group": "hard",
     "text": "四是四，十是十，十四是十四，四十是四十。",
     "note": "平翘舌绕口令"},
    {"id": "h_num", "lang": "zh", "group": "hard",
     "text": "订单金额 3800.5 元，日期是 2026 年 10 月 30 日，完成率 85%。",
     "spoken": "订单金额三千八百点五元，日期是二零二六年十月三十日，完成率百分之八十五。",
     "note": "数字、小数点、年份、百分号"},
    {"id": "h_mix", "lang": "zh", "group": "hard",
     "text": "这个 model 的 inference 速度很快，但 latency 还需要优化。",
     "note": "中英混读"},
    {"id": "h_en_twist", "lang": "en", "group": "hard",
     "text": "She sells sea shells by the sea shore."},
    {"id": "h_en_acr", "lang": "en", "group": "hard",
     "text": "The API returns an error when the JSON payload is too large.",
     "note": "英文缩写"},
]

DESIGN_PROMPT = "年轻女性，声音温柔，语速适中。"

# Engines spell the choice however their model expects it ("en" for most,
# "English" for MinG). Match against the declared options instead of guessing.
LANG_ALIASES = {"zh": ("chinese", "mandarin", "中文"), "en": ("english", "英文")}


def language_option(params: list[dict], lang: str) -> str | None:
    for param in params:
        if param.get("name") != "language":
            continue
        for option in param.get("options") or []:
            value = str(option.get("value", ""))
            low = value.lower()
            if low == lang or low.startswith(lang):
                return value
            if any(low.startswith(alias) for alias in LANG_ALIASES.get(lang, ())):
                return value
    return None

# Both models are small and freely redistributable; the script fetches them
# into --models on first use.
EVAL_MODELS = {
    "campplus.onnx": "https://github.com/k2-fsa/sherpa-onnx/releases/download/"
                     "speaker-recongition-models/"
                     "3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx",
    "sig_bak_ovr.onnx": "https://raw.githubusercontent.com/microsoft/DNS-Challenge/"
                        "master/DNSMOS/DNSMOS/sig_bak_ovr.onnx",
}


# ---------------------------------------------------------------- audio utils

def load_wav(path: str | Path, sr: int = SR) -> np.ndarray:
    import librosa

    audio, rate = sf.read(str(path), always_2d=False)
    if audio.ndim > 1:
        audio = audio.mean(axis=1)
    audio = audio.astype(np.float32)
    if rate != sr:
        audio = librosa.resample(audio, orig_sr=rate, target_sr=sr)
    peak = float(np.max(np.abs(audio))) if audio.size else 0.0
    if peak > 1.0:
        audio = audio / peak
    return audio


def kaldi_fbank(wav: np.ndarray, sr: int = SR, n_mels: int = 80,
                frame_len: int = 400, hop: int = 160, nfft: int = 512,
                low_freq: float = 20.0) -> np.ndarray:
    """Kaldi-style log-mel filterbank, the input CAM++ was trained on."""
    if wav.size < frame_len:
        wav = np.pad(wav, (0, frame_len - wav.size))
    # DC removal + pre-emphasis, as kaldi-native-fbank does by default.
    wav = wav - wav.mean()
    wav = np.append(wav[0], wav[1:] - 0.97 * wav[:-1])
    n_frames = 1 + (len(wav) - frame_len) // hop
    idx = np.arange(frame_len)[None, :] + hop * np.arange(n_frames)[:, None]
    frames = wav[idx]
    # Povey window.
    n = np.arange(frame_len)
    window = (0.5 - 0.5 * np.cos(2 * np.pi * n / (frame_len - 1))) ** 0.85
    spec = np.fft.rfft(frames * window, nfft)
    power = spec.real ** 2 + spec.imag ** 2

    def mel(f):
        return 1127.0 * np.log1p(f / 700.0)

    high_freq = sr / 2.0
    mels = np.linspace(mel(low_freq), mel(high_freq), n_mels + 2)
    hz = 700.0 * (np.exp(mels / 1127.0) - 1.0)
    bins = np.fft.rfftfreq(nfft, 1.0 / sr)
    fbank = np.zeros((n_frames, n_mels), dtype=np.float32)
    for m in range(n_mels):
        left, center, right = hz[m], hz[m + 1], hz[m + 2]
        up = (bins - left) / max(center - left, 1e-9)
        down = (right - bins) / max(right - center, 1e-9)
        weights = np.maximum(0.0, np.minimum(up, down))
        fbank[:, m] = power @ weights
    return np.log(fbank + 1e-10)


def speaker_embedding(session, wav: np.ndarray) -> np.ndarray:
    feats = kaldi_fbank(wav)[None, ...].astype(np.float32)
    emb = session.run(None, {"x": feats})[0][0]
    emb = emb / (np.linalg.norm(emb) + 1e-9)
    return emb.astype(np.float32)


def cosine(a: np.ndarray, b: np.ndarray) -> float:
    return float(np.dot(a, b) / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-9))


def dnsmos(session, wav: np.ndarray) -> dict:
    """Microsoft DNSMOS P.835 mapped to SIG/BAK/OVR on the MOS scale."""
    seg_len = int(9.01 * SR)
    audio = wav.copy()
    while audio.size < seg_len:
        audio = np.append(audio, audio)
    hops = max(1, int(np.floor(audio.size / SR) - 9.01) + 1)
    sigs, baks, ovrs = [], [], []
    for i in range(hops):
        seg = audio[i * SR: i * SR + seg_len]
        if seg.size < seg_len:
            continue
        raw = session.run(None, {"input_1": seg[None, :].astype(np.float32)})[0][0]
        sigs.append(raw[0]); baks.append(raw[1]); ovrs.append(raw[2])
    if not sigs:
        return {"sig": 0.0, "bak": 0.0, "ovr": 0.0}
    sig, bak, ovr = float(np.mean(sigs)), float(np.mean(baks)), float(np.mean(ovrs))
    p_sig = np.poly1d([-0.08397278, 1.22083953, 0.0052439])
    p_bak = np.poly1d([-0.13166888, 1.60915514, -0.39604546])
    p_ovr = np.poly1d([-0.06766283, 1.11546468, 0.04602535])
    return {"sig": float(p_sig(sig)), "bak": float(p_bak(bak)), "ovr": float(p_ovr(ovr))}


def pitch_stats(wav: np.ndarray) -> dict:
    """Median pitch, pitch range in semitones, and how much of it is voiced."""
    import librosa

    f0, voiced, _ = librosa.pyin(wav, fmin=60, fmax=500, sr=SR,
                                 frame_length=1024, hop_length=160)
    vals = f0[~np.isnan(f0)]
    if vals.size < 5:
        return {"f0_median": 0.0, "f0_range_st": 0.0, "voiced_ratio": 0.0}
    st = 12 * np.log2(vals / 27.5)
    return {
        "f0_median": float(np.median(vals)),
        "f0_range_st": float(np.percentile(st, 90) - np.percentile(st, 10)),
        "voiced_ratio": float(np.count_nonzero(~np.isnan(f0)) / max(1, f0.size)),
    }


def pacing_stats(wav: np.ndarray, n_chars: int) -> dict:
    """Speech rate over the audible part, and the share of near-silence."""
    import librosa

    rms = librosa.feature.rms(y=wav, frame_length=512, hop_length=160)[0]
    floor = max(float(np.max(rms)) * 0.05, 1e-5)
    audible = float(np.count_nonzero(rms > floor) / max(1, rms.size))
    dur = wav.size / SR
    return {
        "duration": dur,
        "chars_per_sec": n_chars / max(0.05, dur * audible),
        "pause_ratio": 1.0 - audible,
    }


# ------------------------------------------------------------- text distance

def normalize(text: str, lang: str) -> str:
    text = unicodedata.normalize("NFKC", text).lower()
    if lang == "en":
        words = re.findall(r"[a-z0-9']+", text)
        # ASR spells acronyms out ("a p i"); join runs of single letters so
        # they compare equal to "api".
        merged: list[str] = []
        run = ""
        for word in words:
            if len(word) == 1 and word.isalpha():
                run += word
                continue
            if run:
                merged.append(run)
                run = ""
            merged.append(word)
        if run:
            merged.append(run)
        return " ".join(merged)
    return "".join(ch for ch in text
                   if ch.isalnum() and not unicodedata.combining(ch))


def edit_distance(a: str, b: str, unit_words: bool = False) -> int:
    if unit_words:
        a, b = a.split(), b.split()
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i] + [0] * len(b)
        for j, cb in enumerate(b, 1):
            cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb))
        prev = cur
    return prev[-1]


def repetition_ratio(text: str, lang: str) -> float:
    """Share of repeated n-grams: a stuck decoder loops on a phrase."""
    normalized = normalize(text, lang)
    units = normalized.split() if lang == "en" else list(normalized)
    n = 3 if lang == "en" else 4
    grams = [tuple(units[i:i + n]) for i in range(max(0, len(units) - n + 1))]
    if not grams:
        return 0.0
    return 1.0 - len(set(grams)) / len(grams)


def error_rate(ref: str, hyp: str, lang: str) -> float:
    """Character-level distance (for en too: an ASR splits "seashells" into
    "sea shells" and a word-level metric would call that a mistake)."""
    ref_n, hyp_n = normalize(ref, lang), normalize(hyp, lang)
    if lang == "en":
        ref_n, hyp_n = ref_n.replace(" ", ""), hyp_n.replace(" ", "")
    if not ref_n:
        return 0.0
    return edit_distance(ref_n, hyp_n) / len(ref_n)


# ------------------------------------------------------------------- network

def fetch_eval_models(models_dir: Path) -> None:
    models_dir.mkdir(parents=True, exist_ok=True)
    for name, url in EVAL_MODELS.items():
        dest = models_dir / name
        if dest.exists() and dest.stat().st_size > 100_000:
            continue
        print(f"downloading {name}", flush=True)
        tmp = dest.with_suffix(".part")
        with urllib.request.urlopen(url, timeout=600) as resp, open(tmp, "wb") as out:
            while chunk := resp.read(1 << 20):
                out.write(chunk)
        tmp.rename(dest)


def http_json(url: str) -> dict:
    with urllib.request.urlopen(url, timeout=60) as resp:
        return json.load(resp)


def synth(base: str, engine: str, text: str, out: Path,
          prompt: Path | None, prompt_text: str | None,
          extra: dict[str, str], timeout: float = 900) -> dict:
    boundary = uuid.uuid4().hex
    parts: list[bytes] = []

    def field(name: str, value: str) -> None:
        parts.append(
            f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n'
            f"{value}\r\n".encode())

    field("text", text)
    if prompt_text:
        field("prompt_text", prompt_text)
    for key, value in extra.items():
        field(key, value)
    if prompt is not None:
        parts.append(
            f'--{boundary}\r\nContent-Disposition: form-data; name="prompt"; '
            f'filename="{prompt.name}"\r\nContent-Type: audio/wav\r\n\r\n'.encode())
        parts.append(prompt.read_bytes())
        parts.append(b"\r\n")
    parts.append(f"--{boundary}--\r\n".encode())
    body = b"".join(parts)

    req = urllib.request.Request(
        f"{base}/api/engines/{engine}/synthesize", data=body,
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"})
    started = time.time()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            audio = resp.read()
        elapsed = time.time() - started
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(audio)
        return {"ok": True, "seconds": elapsed, "bytes": len(audio)}
    except Exception as exc:  # noqa: BLE001 - reported in the table
        detail = ""
        if hasattr(exc, "read"):
            try:
                detail = exc.read().decode("utf-8", "replace")[:300]
            except Exception:  # noqa: BLE001
                pass
        return {"ok": False, "seconds": time.time() - started,
                "error": f"{exc} {detail}".strip()}


def transcribe(base: str, wav: Path) -> str:
    boundary = uuid.uuid4().hex
    body = (
        f'--{boundary}\r\nContent-Disposition: form-data; name="audio"; '
        f'filename="{wav.name}"\r\nContent-Type: audio/wav\r\n\r\n'.encode()
        + wav.read_bytes()
        + f"\r\n--{boundary}--\r\n".encode())
    req = urllib.request.Request(
        f"{base}/api/transcribe", data=body,
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"})
    try:
        with urllib.request.urlopen(req, timeout=300) as resp:
            return json.load(resp).get("text", "")
    except Exception as exc:  # noqa: BLE001
        return f"<transcribe failed: {exc}>"


# --------------------------------------------------------------- synth phase

def engine_specs(base: str) -> list[dict]:
    engines = http_json(f"{base}/api/engines")
    specs = []
    for engine in engines:
        caps = engine.get("capabilities") or []
        if "tts" not in caps:
            continue
        param_list = engine.get("parameters") or []
        params = {p.get("name") for p in param_list}
        extra: dict[str, str] = {}
        if "voice-design" in caps and "instruct" in params:
            extra["instruct"] = DESIGN_PROMPT
        specs.append({
            "id": engine["id"],
            "clone": "clone" in caps,
            "extra": extra,
            # Engines validate their parameters and reject unknown ones, so
            # only pass `language` where the manifest declares it.
            "language_value": language_option(param_list, "en"),
        })
    return specs


def run_synth(args) -> dict:
    ref = Path(args.ref).resolve() if args.ref else None
    specs = engine_specs(args.base)
    if args.engines:
        wanted = set(args.engines.split(","))
        specs = [s for s in specs if s["id"] in wanted]
    results: dict = {}
    total = len(specs) * len(CASES)
    done = 0
    for spec in specs:
        engine_id = spec["id"]
        results[engine_id] = {}
        prompt = ref if spec["clone"] and ref else None
        for case in CASES:
            lang, text = case["lang"], case["text"]
            done += 1
            label = f"[{done}/{total}] {engine_id}/{case['id']}"
            out = Path(args.out) / engine_id / f"{case['id']}.wav"
            if out.exists() and out.stat().st_size > 1000 and not args.force:
                print(f"{label}: cached", flush=True)
                continue
            print(f"{label}: synthesizing", flush=True)
            extra = dict(spec["extra"])
            if lang == "en" and spec["language_value"]:
                extra.setdefault("language", spec["language_value"])
            res = synth(args.base, engine_id, text, out, prompt,
                        args.ref_text if prompt else None, extra,
                        timeout=args.timeout)
            res["lang"] = lang
            res["text"] = text
            res["group"] = case["group"]
            results[engine_id][case["id"]] = res
            print(f"{label}: {'ok' if res['ok'] else 'FAIL'} "
                  f"{res['seconds']:.1f}s {res.get('error', '')[:120]}", flush=True)
        Path(args.out, "synth.json").write_text(
            json.dumps(results, ensure_ascii=False, indent=1))
    return results


# --------------------------------------------------------------- score phase

def drift_stats(session, wav: np.ndarray, ref_emb: np.ndarray | None) -> dict:
    """How steady the speaker identity is across a long paragraph."""
    win, hop = 3 * SR, int(1.5 * SR)
    if wav.size < win:
        return {}
    embs = []
    for start in range(0, wav.size - win + 1, hop):
        seg = wav[start:start + win]
        if float(np.sqrt(np.mean(seg ** 2))) < 1e-3:
            continue
        embs.append(speaker_embedding(session, seg))
    if len(embs) < 3:
        return {}
    embs = np.stack(embs)
    early = embs[: max(1, len(embs) // 3)].mean(axis=0)
    late = embs[-max(1, len(embs) // 3):].mean(axis=0)
    to_early = [cosine(e, early) for e in embs]
    slope = float(np.polyfit(np.arange(len(to_early)), to_early, 1)[0])
    stats = {
        "windows": len(embs),
        "drift_first_vs_last": cosine(early, late),
        "drift_min": float(min(to_early)),
        "drift_slope_per_window": slope,
    }
    if ref_emb is not None:
        stats["ref_similarity_early"] = cosine(early, ref_emb)
        stats["ref_similarity_late"] = cosine(late, ref_emb)
    return stats


def run_score(args) -> dict:
    import onnxruntime as ort

    tools = {
        "spk": ort.InferenceSession(str(Path(args.models) / "campplus.onnx"),
                                    providers=["CPUExecutionProvider"]),
        "mos": ort.InferenceSession(str(Path(args.models) / "sig_bak_ovr.onnx"),
                                    providers=["CPUExecutionProvider"]),
    }
    ref_emb = None
    if args.ref:
        ref_emb = speaker_embedding(tools["spk"], load_wav(args.ref))

    out_root = Path(args.out)
    clone_caps = {spec["id"]: spec["clone"] for spec in engine_specs(args.base)}
    results: dict = {}
    for engine_dir in sorted(p for p in out_root.iterdir() if p.is_dir()):
        engine_id = engine_dir.name
        # Without the capability the reference-cosine is just "how much this
        # preset voice happens to resemble the reference", not cloning.
        rows: dict = {"cases": {}, "clone": clone_caps.get(engine_id, False)}
        embeds: dict[str, np.ndarray] = {}
        for case in CASES:
            lang, text = case["lang"], case["text"]
            wav_path = engine_dir / f"{case['id']}.wav"
            if not wav_path.exists() or wav_path.stat().st_size < 1000:
                continue
            try:
                wav = load_wav(wav_path)
            except Exception as exc:  # noqa: BLE001
                rows["cases"][case["id"]] = {"error": f"unreadable: {exc}"}
                continue
            if wav.size < SR // 10:
                rows["cases"][case["id"]] = {"error": "too short"}
                continue
            hyp = transcribe(args.base, wav_path)
            expected = case.get("spoken") or text
            emb = speaker_embedding(tools["spk"], wav)
            embeds[case["id"]] = emb
            row = {
                "lang": lang,
                "group": case["group"],
                "error_rate": error_rate(expected, hyp, lang),
                "transcript": hyp,
                "repetition": repetition_ratio(hyp, lang),
                "duration": wav.size / SR,
                "similarity": (cosine(emb, ref_emb)
                               if ref_emb is not None and rows["clone"] else None),
                "mos": dnsmos(tools["mos"], wav),
                "pitch": pitch_stats(wav),
                "pacing": pacing_stats(wav, len(normalize(expected, lang))),
                "rms_db": float(20 * np.log10(max(1e-6, float(np.sqrt(np.mean(wav ** 2)))))),
            }
            rows["cases"][case["id"]] = row
            print(f"{engine_id}/{case['id']}: cer={row['error_rate']:.3f} "
                  f"sim={row['similarity'] if row['similarity'] is None else round(row['similarity'], 3)} "
                  f"ovr={row['mos']['ovr']:.2f}", flush=True)

        takes = [embeds[c] for c in ("s1a", "s1b", "s1c") if c in embeds]
        if len(takes) >= 2:
            pairs = [cosine(takes[i], takes[j])
                     for i in range(len(takes)) for j in range(i + 1, len(takes))]
            rows["take_consistency"] = float(np.mean(pairs))
            rows["take_spread"] = float(1 - np.min(pairs))

        s3 = engine_dir / "s3.wav"
        if s3.exists():
            try:
                rows["drift"] = drift_stats(tools["spk"], load_wav(s3), ref_emb)
            except Exception as exc:  # noqa: BLE001
                rows["drift"] = {"error": str(exc)}
        results[engine_id] = rows
    Path(args.out, "score.json").write_text(
        json.dumps(results, ensure_ascii=False, indent=1))
    return results


# -------------------------------------------------------------------- report

def fmt(value, digits=2, suffix="", scale=1.0) -> str:
    if value is None:
        return "–"
    try:
        return f"{value * scale:.{digits}f}{suffix}"
    except (TypeError, ValueError):
        return "–"


def run_report(args) -> None:
    score = json.loads(Path(args.out, "score.json").read_text())
    synth_log = {}
    if Path(args.out, "synth.json").exists():
        synth_log = json.loads(Path(args.out, "synth.json").read_text())

    def mean(values):
        values = [v for v in values if v is not None]
        return float(np.mean(values)) if values else None

    def worst(values):
        values = [v for v in values if v is not None]
        return float(max(values)) if values else None

    hard_ids = [c["id"] for c in CASES if c["group"] == "hard"]
    hard_titles = {
        "h_poly": "heteronyms", "h_twist": "tongue twister",
        "h_num": "numbers/dates", "h_mix": "zh-en mix",
        "h_en_twist": "en twister", "h_en_acr": "en acronyms",
    }

    core_header = ("| Engine | Clone | CER zh | CER en | Repeat | Similarity | "
                   "Take-to-take | Drift (min) | DNSMOS OVR | SIG | Pause | "
                   "Pitch range (st) | Chars/s |")
    core = [core_header,
            "|" + " --- |" * (core_header.count("|") - 1)]
    hard = ["| Engine | " + " | ".join(hard_titles.get(h, h) for h in hard_ids) + " |",
            "| --- |" + " --- |" * len(hard_ids)]
    transcripts: list[str] = []

    for engine_id in sorted(score):
        rows = score[engine_id]
        cases = rows.get("cases", {})
        base_zh = [c for k, c in cases.items()
                   if c.get("group") == "base" and c.get("lang") == "zh"
                   and "error_rate" in c]
        base_en = [c for k, c in cases.items()
                   if c.get("group") == "base" and c.get("lang") == "en"
                   and "error_rate" in c]
        clone = rows.get("clone", False)
        drift = rows.get("drift") or {}
        core.append("| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |".format(
            engine_id,
            "yes" if clone else "–",
            fmt(mean([c["error_rate"] for c in base_zh]), 3),
            fmt(mean([c["error_rate"] for c in base_en]), 3),
            fmt(mean([c.get("repetition") for c in cases.values()]), 3),
            fmt(mean([c.get("similarity") for c in cases.values()
                      if c.get("group") == "base"]), 2),
            fmt(rows.get("take_consistency"), 3),
            fmt(drift.get("drift_min"), 2),
            fmt(mean([c["mos"]["ovr"] for c in cases.values()
                      if "mos" in c and c.get("group") == "base"]), 2),
            fmt(mean([c["mos"]["sig"] for c in cases.values()
                      if "mos" in c and c.get("group") == "base"]), 2),
            fmt(mean([c["pacing"]["pause_ratio"] for c in cases.values()
                      if "pacing" in c and c.get("group") == "base"]), 2),
            fmt(mean([c["pitch"]["f0_range_st"] for c in cases.values()
                      if "pitch" in c and c.get("group") == "base"]), 1),
            fmt(mean([c["pacing"]["chars_per_sec"] for c in cases.values()
                      if "pacing" in c and c.get("group") == "base"]), 1),
        ))
        cells = []
        for case_id in hard_ids:
            entry = cases.get(case_id) or {}
            cells.append(fmt(entry.get("error_rate"), 3))
        hard.append(f"| {engine_id} | " + " | ".join(cells) + " |")

        lines = [f"**{engine_id}**"]
        for case_id in hard_ids:
            entry = cases.get(case_id) or {}
            heard = entry.get("transcript")
            if heard:
                lines.append(f"- `{case_id}` ({fmt(entry.get('error_rate'), 3)}): {heard}")
        if len(lines) > 1:
            transcripts.append("\n".join(lines))

    failures = []
    for engine_id, cases in synth_log.items():
        for case, res in cases.items():
            if not res.get("ok"):
                failures.append(f"{engine_id}/{case}: {res.get('error', '')[:160]}")

    text = "## Overall\n\n" + "\n".join(core)
    text += "\n\n## Hard cases (CER)\n\n" + "\n".join(hard)
    case_lines = " · ".join(f"`{c['id']}` {c['text']}" for c in CASES if c["group"] == "hard")
    text += "\n\nCases: " + case_lines
    if transcripts:
        text += "\n\n## What the engines actually said (hard cases)\n\n"
        text += "\n\n".join(transcripts)
    if failures:
        text += "\n\n## Failed runs\n\n" + "\n".join(f"- {f}" for f in failures)
    Path(args.report).write_text(text + "\n")
    print(text)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base", default="http://127.0.0.1:8899")
    parser.add_argument("--out", default="/tmp/quality")
    parser.add_argument("--models", default="/tmp/quality-models",
                        help="directory with campplus.onnx and sig_bak_ovr.onnx")
    parser.add_argument("--ref", default=None, help="reference clip for cloning")
    parser.add_argument("--ref-text", default=None, help="transcript of the reference")
    parser.add_argument("--engines", default=None, help="comma-separated subset")
    parser.add_argument("--phase", default="all", choices=["synth", "score", "all"])
    parser.add_argument("--report", default="quality.md")
    parser.add_argument("--force", action="store_true", help="re-synthesize cached clips")
    parser.add_argument("--timeout", type=float, default=900)
    args = parser.parse_args()

    Path(args.out).mkdir(parents=True, exist_ok=True)
    fetch_eval_models(Path(args.models))
    if args.phase in ("synth", "all"):
        run_synth(args)
    if args.phase in ("score", "all"):
        run_score(args)
        run_report(args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
