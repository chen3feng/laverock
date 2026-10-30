# Models

English | [中文](models.zh-CN.md) · [← README](../README.md)

Laverock ships without model weights: the installers carry the app, and
models are installed on demand. Each model is a directory with an
`engine.json` manifest, so installing one never requires updating the app.

## Installing

**From the app** — open the **Models** page and press install on a card. The
download log streams into the card with a progress bar, and the engine list
refreshes when it finishes. Failed installs can be resumed from the same
card; scripts are idempotent.

**From the command line** — the app runs the same scripts, so headless
installs and CI use them directly:

```sh
./scripts/download-zipvoice.sh     # ZipVoice, ~160 MB, zh/en, ONNX
./scripts/download-kokoro.sh       # Kokoro-82M, ~135 MB, 8 languages, preset voices
./scripts/download-stt.sh          # SenseVoice STT, ~234 MB, auto-transcribes clips
./scripts/download-voxcpm2.sh      # VoxCPM2, ~3.3 GB, 30 languages, GGUF
./scripts/download-qwen3tts.sh     # Qwen3-TTS cloning, isolated venv + ~1.5 GB model
./scripts/download-longcat.sh      # LongCat-AudioDiT 1B, isolated venv + 5.7 GB weights
./scripts/download-dots.sh         # dots.tts SOAR, 48 kHz, audio.cpp runtime + 3 GB GGUF
./scripts/download-cosyvoice.sh    # CosyVoice3 0.5B, 2.3 GB GGUF (shares the runtime)
./scripts/download-ming3tts.sh     # Ming-3.0-tts 0.5B, venv + ~3 GB weights
./scripts/download-chatterbox.sh   # Chatterbox multilingual, venv + ~5 GB
./scripts/download-mosstts.sh      # MOSS-TTS-Delay, prebuilt llama.cpp runtime + ~10 GB weights

# Variants
QWEN3TTS_VARIANT=custom ./scripts/download-qwen3tts.sh    # 9 preset speakers
QWEN3TTS_VARIANT=design ./scripts/download-qwen3tts.sh    # 1.7B voice design
MING3TTS_VARIANT=voice_design ./scripts/download-ming3tts.sh
MING3TTS_VARIANT=multilingual ./scripts/download-ming3tts.sh
MING3TTS_VARIANT=ip ./scripts/download-ming3tts.sh        # built-in Ming voices
LONGCAT_MODEL=meituan-longcat/LongCat-AudioDiT-3.5B ./scripts/download-longcat.sh
DOTS_MODEL=dots.tts-mf ./scripts/download-dots.sh         # 4-step MeanFlow export
```

Scripts honour the proxy and mirror settings from **Settings → Download
network** (`HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`, `HF_ENDPOINT`,
`PIP_INDEX_URL`); see [Configuration](configuration.md). On Windows, in-app
installs need `bash` from Git for Windows.

Models are written to `<models dir>/<directory>/`. The models directory is
`models/` when running from a source checkout, and the application-data
directory for packaged builds; Python environments and build tools go to the
tools directory next to it.

## Catalog

`<engine id>` is what the API, the catalog and the OpenAI-compatible
endpoint call the model.

| Engine id | Model | Modes | Languages | Download |
| --- | --- | --- | --- | --- |
| `sherpa-onnx-zipvoice-distill-int8-zh-en-emilia` | ZipVoice Distill INT8 | clone | zh, en | ~160 MB |
| `kokoro-int8-multi-lang-v1_0` | Kokoro-82M INT8 | 54 preset voices | zh, en, ja, fr, es, it, pt, hi | ~135 MB |
| `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17-int8` | SenseVoice Small INT8 | speech recognition | zh, en, ja, ko, yue | ~234 MB |
| `voxcpm2` | VoxCPM2 | clone, voice design | 30 languages | ~3.3 GB |
| `longcat` | LongCat-AudioDiT 1B | clone | zh, en | ~7 GB incl. venv |
| `dots` | dots.tts SOAR Q8_0 | clone | 24 languages | ~3.1 GB |
| `cosyvoice` | CosyVoice3 0.5B Q8_0 | clone, instructions | 9 languages + Cantonese | ~2.3 GB |
| `qwen3tts` | Qwen3-TTS 0.6B | clone | 10 languages | ~2–3 GB incl. venv |
| `qwen3tts-custom` | Qwen3-TTS 0.6B custom voice | 9 preset speakers, instructions | 10 languages | ~2–3 GB |
| `qwen3tts-design` | Qwen3-TTS 1.7B | voice design, instructions | 10 languages | ~4–5 GB |
| `ming3tts` | Ming-3.0-tts 0.5B | clone | zh, en, de, es, fr, ko, ja | ~3 GB + venv |
| `ming3tts-design` | Ming-3.0-tts 0.5B | voice design, instructions | same | shared |
| `ming3tts-multilingual` | Ming-3.0-tts 0.5B | clone, multilingual | same | shared |
| `ming3tts-ip` | Ming-3.0-tts 0.5B | preset voices | zh, en | shared |
| `chatterbox` | Chatterbox Multilingual | clone | 23 languages | ~5 GB incl. venv |
| `mosstts` | MOSS-TTS-Delay | clone, multilingual | 20 languages | ~10 GB (peak ~16 GB) |

The four Ming entries share one checkpoint, one venv and one runtime, so
only the first one costs a full download; the rest add a few kilobytes of
manifest.

## Capabilities

| Capability | Meaning |
| --- | --- |
| `clone` | takes a reference clip (and, for some engines, its transcript) |
| `preset-voices` | built-in speakers; the synth page hides the voice library and shows a voice picker |
| `voice-design` | builds a voice from a natural-language description typed into the instruction field |
| `instruct` | accepts a delivery instruction, e.g. "speak slowly and warmly" |
| `multilingual` | can speak a language other than the reference clip's |
| `stt` | speech recognition, used to transcribe reference clips; not a TTS engine |

## Engine notes

**ZipVoice** — Chinese/English zero-shot cloning on a static ONNX runtime.
The smallest and fastest on CPU, and a good default. Cloning requires the
reference transcript; both must be accurate or the timbre drifts.

**Kokoro** — a lightweight 82M model that runs in real time on CPU, with 54
official preset voices across 8 languages. No cloning: pick a built-in voice
from the engine parameters. Pure ONNX — no Python or build toolchain.

**VoxCPM2** — 30 languages with zero-shot cloning and voice design. Runs on
the audio.cpp native runtime (CUDA with an NVIDIA GPU on Windows), shared
with dots.tts and CosyVoice3. A reference longer than 14 s is trimmed from
the start at a silence point; 3–10 s of clean speech works best.

**LongCat-AudioDiT** — a flow-matching diffusion model working directly in a
waveform latent space, with per-request NFE steps and guidance strength (APG
vs CFG). fp16 on CUDA, fp32 elsewhere; about 6 GB of RAM for the 1B variant.
One request produces at most ~30 s including the reference, so prefer 3–10 s
prompts. Cloning needs the transcript and trims clips that run much longer
than it; digit strings are read as Chinese numerals and long text is split
into chunks with short pauses.

**dots.tts** — 2B continuous-AR model with a 48 kHz AudioVAE, run through
audio.cpp (Metal on macOS, CPU elsewhere). Cloning defaults to the x-vector
mode, which needs only the reference audio, so an approximate transcript does
not hurt; the voice + text mode gives higher similarity when the transcript
is accurate. Measured on an M4 Pro with the Q8_0 checkpoint: RTF 1.4 (see
**Speed**), about 6 GB peak memory.

**CosyVoice3** — 0.5B streaming LLM TTS sharing the audio.cpp runtime with
dots.tts. Three templates: zero-shot cloning (reference plus an accurate
transcript, highest similarity), cross-lingual cloning (audio only, for when
the transcript is unreliable) and instruction control ("用温和的语气，
语速稍慢"). Top-K and flow-decoder steps are tunable per request.

**Qwen3-TTS** — the official `qwen-tts` package in an isolated venv. Device
selection is automatic: CUDA in bfloat16, Apple MPS in float32 (float16
produces NaN in the codec), otherwise CPU. Three installs share one venv:
the clone variant, `custom` (nine built-in speakers: Vivian/Ryan/Sohee…) and
`design` (1.7B, builds a voice from a description). Per-request timings are in
**Speed** above.

**Ming-3.0-tts** — a 0.5B dense checkpoint that ranks first among open
weights on the Artificial Analysis controlled-voice leaderboard. Cloning
works from the clip alone; a transcript improves fidelity but is optional.
Four installs share one checkpoint: cloning, voice design, multilingual
(clone a voice but speak German/Spanish/French/Korean/Japanese) and
built-in voices. FST text normalization needs `pynini` (conda-forge only)
and is skipped automatically when absent.

**Chatterbox** — Resemble AI's 0.5B multilingual clone model: clone a clip
and speak any of 23 languages. The transcript is not required. Defaults to
Chinese; pick the target language in the engine parameters.

**MOSS-TTS-Delay** — a multilingual zero-shot TTS/clone model driven by a
native llama.cpp binary, with no Python at inference time. The installer
builds that runtime from source and converts the audio tokenizer to GGUF;
weights come from ModelScope, so no Hugging Face login is needed. Cloning is
optional. The engine synthesizes long text in sentence-sized pieces, because
the decoder degrades on very long inputs.

**SenseVoice** — optional on-device speech recognition covering zh/en/ja/
ko/yue. Once installed, a recorded or uploaded reference clip is transcribed
automatically into the transcript field; the Transcribe button re-runs it.

## Speed

Measured on a MacBook Pro: Apple M4 Pro (14-core CPU — 10 performance and
4 efficiency — with a 20-core GPU), 48 GB unified memory, macOS 27.0.1.

| Engine | RTF | Fixed cost | 10 s of speech |
| --- | --- | --- | --- |
| LongCat-AudioDiT 1B | 0.17 | 8.6 s | 10.3 s |
| ZipVoice | 0.17 | 0.9 s | 2.6 s |
| CosyVoice3 | 0.30 | 4.1 s | 7.0 s |
| MOSS-TTS | 0.33 | 2.4 s | 5.7 s |
| VoxCPM2 | 0.60 | 1.9 s | 7.9 s |
| Kokoro-82M | 0.64 | 0.7 s | 7.0 s |
| Ming 3.0 voice design | 0.98 | 4.0 s | 13.8 s |
| Ming 3.0 multilingual | 0.99 | 3.8 s | 13.6 s |
| Ming 3.0 built-in voices | 0.99 | 3.7 s | 13.6 s |
| Ming 3.0 cloning | 1.00 | 3.8 s | 13.8 s |
| Qwen3-TTS 0.6B cloning | 1.00 | 4.3 s | 14.3 s |
| Qwen3-TTS 0.6B custom voice | 1.00 | 3.9 s | 13.9 s |
| Qwen3-TTS 1.7B design | 1.20 | 4.1 s | 16.1 s |
| dots.tts SOAR | 1.40 | 1.9 s | 16.0 s |
| Chatterbox | 3.88 | 9.8 s | 48.6 s |

RTF is generation time divided by audio duration, so below 1 is faster than
real time. The fixed cost is what a request pays before the first sample —
starting the engine's process, loading weights for the engines that keep
nothing between requests, and reading the reference clip. A request therefore
takes `fixed cost + RTF × audio duration`; the third column applies that to
10 s of speech. Engines with a large fixed cost and a low RTF (LongCat,
CosyVoice3) are good for long passages and wasteful for one-liners.

Method: requests go to the local server over loopback, with two Chinese texts
— a sentence that produces about 2 s of speech and a paragraph that produces
10–13 s. One warm-up request per engine is discarded, the two texts are then
requested in alternating pairs for at least two rounds, and
`T = fixed + RTF × duration` is fitted per engine from a freshly started
server. MOSS-TTS and Ming voice design got more points and a
median-of-slopes fit, because a single stalled request had skewed the plain
least-squares slope. Requests were serial, with no concurrency; the machine
was otherwise idle apart from Spotlight catching up after a reboot, so treat
differences under about 10% as noise. Chatterbox is the roughest row: the
same paragraph came out as 11 s of audio on one run and 19 s on another, so
its RTF is a median over runs that disagree.

## Requirements

| Feature | Requirement |
| --- | --- |
| ONNX engines (ZipVoice, Kokoro, SenseVoice) | none beyond the app |
| audio.cpp engines (dots, CosyVoice, VoxCPM2) | prebuilt runtime is downloaded |
| Python engines (Qwen3-TTS, LongCat, Ming, Chatterbox) | Python 3.10–3.12; the scripts create an isolated venv |
| MOSS-TTS | prebuilt runtime is downloaded; Python 3 for the one-time tokenizer conversion |
| In-app installs on Windows | `bash` from Git for Windows |

Engines that need Python run in their own venv and never load code into the
core process.

## Credits

ZipVoice model weights are Apache-2.0, released by
[k2-fsa/ZipVoice](https://github.com/k2-fsa/ZipVoice) and packaged by
[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).

Kokoro-82M weights are Apache-2.0, released by
[hexgrad/Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M); the ONNX
export and packaging come from sherpa-onnx.

VoxCPM2 is released by [OpenBMB](https://github.com/OpenBMB/VoxCPM) under
Apache-2.0; the GGUF export and native runtime come from
[audio.cpp](https://github.com/0xShug0/audio.cpp).

LongCat-AudioDiT code and weights are MIT, released by
[meituan-longcat/LongCat-AudioDiT](https://github.com/meituan-longcat/LongCat-AudioDiT);
its UMT5 tokenizer comes from `google/umt5-base`.

CosyVoice3 code and checkpoints are Apache-2.0, released by
[FunAudioLLM/CosyVoice](https://github.com/QwenAudio/CosyVoice); the GGUF
export also comes from audio.cpp.

dots.tts code and checkpoints are Apache-2.0, released by
[studio-dots-ai/dots.tts](https://github.com/studio-dots-ai/dots.tts). The
weights keep their Apache-2.0 license, and the runtime binary is
redistributed unmodified.

Qwen3-TTS is released by [QwenLM](https://github.com/QwenLM) under Apache-2.0.

Ming-3.0-tts is released by Ant Group; see its model card for the license
terms.

Chatterbox is released by [Resemble AI](https://github.com/resemble-ai/chatterbox)
under MIT.

MOSS-TTS-Delay is released by [OpenMOSS](https://github.com/OpenMOSS) under
Apache-2.0; the native runtime is built from the OpenMOSS llama.cpp fork.

SenseVoice is released by FunAudioLLM under MIT and packaged by sherpa-onnx.
