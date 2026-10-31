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
./scripts/download-kokoro.sh       # Kokoro-82M, ~135 MB INT8 (~450 MB with fp32), 8 languages, preset voices
./scripts/download-stt.sh          # SenseVoice STT, ~234 MB, auto-transcribes clips
./scripts/download-voxcpm2.sh      # VoxCPM2, ~3.3 GB, 30 languages, GGUF
./scripts/download-qwen3tts.sh     # Qwen3-TTS cloning, isolated venv + ~1.5 GB model
./scripts/download-longcat.sh      # LongCat-AudioDiT 1B, isolated venv + 5.7 GB weights
./scripts/download-dots.sh         # dots.tts SOAR, 48 kHz, audio.cpp runtime + 3 GB GGUF
./scripts/download-cosyvoice.sh    # CosyVoice3 0.5B, 2.3 GB GGUF (shares the runtime)
./scripts/download-ming3tts.sh     # Ming-3.0-tts 0.5B, venv + ~3 GB weights
./scripts/download-chatterbox.sh   # Chatterbox multilingual, venv + ~5 GB
./scripts/download-indextts2.sh    # IndexTTS-2, 3.6 GB Q8_0 GGUF (shares the runtime)
./scripts/download-mosstts.sh      # MOSS-TTS-Delay, macOS/Linux only, ~10 GB weights

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
| `kokoro-int8-multi-lang-v1_0` | Kokoro-82M | 54 preset voices | zh, en, ja, fr, es, it, pt, hi | ~135 MB (INT8) / ~450 MB with fp32 |
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
| `indextts2` | IndexTTS-2 Q8_0 | clone | zh, en | ~3.6 GB |
| `mosstts` | MOSS-TTS-Delay | clone, multilingual | 20 languages | ~10 GB (peak ~16 GB) |
| `enhance` | Voice cleanup (GTCRN + BS-RoFormer) | recording cleanup | — | ~165 MB |

The four Ming entries share one checkpoint, one venv and one runtime, so
only the first one costs a full download; the rest add a few kilobytes of
manifest.

`enhance` is not an engine: it adds two optional stages to the recording
library. Denoising (GTCRN, 16 kHz) removes room noise, fans and
traffic; vocal isolation (BS-RoFormer, 44.1 kHz) drops background music
and keeps the voice. Denoising is on by default and takes well under a
second for a short take. Separation reuses the audio.cpp runtime and
costs roughly real time to twice real time per take, so it is chosen
explicitly. Takes are processed after upload; the untouched recording is
kept, so a take can be re-processed with another mode or reverted from
its card.

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

The model ships as an INT8 export and an fp32 export. The installer fetches
the fp32 file as well on machines that run it faster — a CUDA build, or an
x86 CPU without AVX2, where ONNX Runtime's INT8 kernels fall back to scalar
code — and the engine then uses it automatically; elsewhere only the 135 MB
INT8 file is downloaded. Apple Silicon stays on INT8: its NEON kernels run
the quantized model well and there is no CUDA provider to prefer. Both files
may coexist; deleting `model.onnx` switches back to INT8.

**VoxCPM2** — 30 languages with zero-shot cloning and voice design. Runs on
the audio.cpp native runtime (CUDA with an NVIDIA GPU on Windows), shared
with dots.tts and CosyVoice3, and stays loaded in the resident server. A
reference longer than 14 s is trimmed from the start at a silence point;
3–10 s of clean speech works best. On an M4 Pro: RTF 0.62, about 6.5 GB of
memory once loaded.

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
is accurate. It runs in the resident server, so the weights survive between
requests. Measured on an M4 Pro with the Q8_0 checkpoint: RTF 1.39 (see
**Speed**), about 6.2 GB of memory once loaded.

**CosyVoice3** — 0.5B streaming LLM TTS sharing the audio.cpp runtime with
dots.tts, and the resident server with all four audio.cpp engines. Three
templates: zero-shot cloning (reference plus an accurate transcript, highest
similarity), cross-lingual cloning (audio only, for when the transcript is
unreliable) and instruction control ("用温和的语气，语速稍慢"). Top-K and
flow-decoder steps are tunable per request. On an M4 Pro: RTF 0.30, about
3.3 GB of memory once loaded.

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
Chinese; pick the target language in the engine parameters. A run can keep
emitting a quiet babble tail after the text ends; the engine cuts a long
low-level tail against the clip's own loudness, keeping 0.3 s of margin.

**IndexTTS-2** — bilibili's Chinese/English zero-shot clone model: a
GPT-style token model conditions on the reference clip (cut to 15 s) and a
flow-matching vocoder produces 22.05 kHz audio. Long text is split into
segments at sentence boundaries internally and the segments are joined with
about 0.2 s of silence (audio.cpp's `interval_silence_ms`), so there is no
request-length cliff. The transcript is not used. It runs on the audio.cpp
native runtime shared with dots.tts, CosyVoice3 and VoxCPM2, so no Python
environment is installed. The text
language is auto-detected (Chinese for Han text, English otherwise) or set
per request; the Q8_0 export is 3.6 GB, and an f16 or original-precision
package can be installed instead with `INDEXTTS_PACKAGE`. Beam search is the
default decoding; switching to greedy decoding in the engine parameters
produces about 25% faster generation with the same text. Like the other
audio.cpp engines it runs in the resident server, so only the first request
loads the 3.6 GB of weights. On an M4 Pro the Q8_0 checkpoint runs at
RTF 1.47 with about 4.1 GB of memory once loaded (see **Speed**).

**MOSS-TTS-Delay** — a multilingual zero-shot TTS/clone model driven by a
native llama.cpp binary, with no Python at inference time. The installer
downloads that runtime prebuilt and converts the audio tokenizer to GGUF
once; it falls back to compiling the runtime locally only on other
platforms, or with `MOSS_RUNTIME_BUILD=1`. Weights come from ModelScope, so
no Hugging Face login is needed. Cloning is optional. The engine synthesizes
long text in sentence-sized pieces, because the decoder degrades on very long
inputs.

The model is offered on macOS (Metal) and Linux (CPU) only. The prebuilt
Windows runtime has no GPU backend and corrupts its host heap every few
runs, so the catalog no longer offers it on Windows. The runtime's `-ngl`
option can offload layers to a CUDA build, but at the pinned revision every
run that touched the GPU crashed (see **Speed**), so there is no GPU escape
hatch either.

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
| CosyVoice3 | 0.30 | 0.6 s | 3.6 s |
| MOSS-TTS | 0.33 | 2.4 s | 5.7 s |
| VoxCPM2 | 0.62 | 0.0 s | 6.2 s |
| Kokoro-82M | 0.64 | 0.7 s | 7.0 s |
| Ming 3.0 voice design | 0.98 | 4.0 s | 13.8 s |
| Ming 3.0 multilingual | 0.99 | 3.8 s | 13.6 s |
| Ming 3.0 built-in voices | 0.99 | 3.7 s | 13.6 s |
| Ming 3.0 cloning | 1.00 | 3.8 s | 13.8 s |
| Qwen3-TTS 0.6B cloning | 1.00 | 4.3 s | 14.3 s |
| Qwen3-TTS 0.6B custom voice | 1.00 | 3.9 s | 13.9 s |
| Qwen3-TTS 1.7B design | 1.20 | 4.1 s | 16.1 s |
| dots.tts SOAR | 1.39 | 0.0 s | 13.7 s |
| IndexTTS-2 Q8_0 | 1.47 | 1.1 s | 15.8 s |
| Chatterbox | 3.88 | 9.8 s | 48.6 s |

RTF is generation time divided by audio duration, so below 1 is faster than
real time. The fixed cost is what a request pays before the first sample —
starting the engine's process, reading the reference clip, and, for engines
that keep nothing between requests, loading the weights. A request therefore
takes `fixed cost + RTF × audio duration`; the third column applies that to
10 s of speech. Engines with a large fixed cost and a low RTF (LongCat,
CosyVoice3) are good for long passages and wasteful for one-liners.

The four audio.cpp engines (dots, CosyVoice3, VoxCPM2, IndexTTS-2) now share
a resident `audiocpp_server` process, started on first use: the model stays
loaded and the table's fixed cost is the warm steady state. The first request
after launching the app (or after 15 idle minutes, when the server releases
the weights) pays a one-off load of roughly 3–11 s depending on the model.

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

### On a Windows desktop

Measured on a desktop PC: Intel Core i7-3770K (4 cores, 8 threads), 32 GB of
RAM, NVIDIA GeForce RTX 3060 Ti (8 GB), Windows 10 Pro 22H2.

| Engine | RTF | Fixed cost | 10 s of speech |
| --- | --- | --- | --- |
| LongCat-AudioDiT 1B | 0.00 | 40.3 s | 40.3 s |
| CosyVoice3 | 0.18 | 0.6 s | 2.3 s |
| VoxCPM2 | 0.30 | 0.2 s | 3.2 s |
| IndexTTS-2 Q8_0 | 0.60 | 1.0 s | 7.0 s |
| dots.tts SOAR | 0.62 | 0.5 s | 6.7 s |
| Chatterbox | 2.18 | 67.1 s | 88.9 s |
| ZipVoice | 2.45 | 6.8 s | 31.2 s |
| Kokoro-82M | 3.08 | 0.6 s | 31.5 s |
| Ming 3.0 built-in voices | 4.16 | 17.7 s | 59.4 s |
| Ming 3.0 multilingual | 4.25 | 18.7 s | 61.2 s |
| Ming 3.0 voice design | 4.71 | 25.8 s | 73.0 s |
| Ming 3.0 cloning | 5.15 | 24.6 s | 76.1 s |
| Qwen3-TTS 0.6B cloning | 5.16 | 22.7 s | 74.3 s |
| Qwen3-TTS 0.6B custom voice | 5.28 | 16.8 s | 69.6 s |
| Qwen3-TTS 1.7B design | 5.64 | 16.7 s | 73.1 s |
| MOSS-TTS | — | — | — |

The audio.cpp engines run on CUDA here and match or beat the M4 Pro — dots
0.62 against 1.39, CosyVoice3 0.18 against 0.30, VoxCPM2 0.30 against 0.62,
IndexTTS-2 0.60 against 1.47 — and their resident fixed cost stays at or
under a second. The ONNX engines are 4–14× slower than on the M4 Pro because
the CPU is a 2012-era quad-core without AVX2; ZipVoice's 6.8 s of fixed cost
is mostly encoding the reference clip on that CPU. Qwen3-TTS and Ming pay
4–5× more as well: their autoregressive loop is bound by single-thread CPU
speed, not by the GPU.

LongCat reads 0.00 because every request takes about 40 s whatever the
length — 5 s and 17.6 s of speech both finish in roughly 40 s — so the fit
only sees the per-request cost, a fresh Python process loading the weights
each time. Generation itself is much faster than real time on the 3060 Ti,
but the duration term falls below the noise of that load.

MOSS-TTS could not be benchmarked here: the prebuilt Windows runtime has no
CUDA backend (every layer is assigned to the CPU) and it dies inside the
binary every few runs with 0xC0000374 (heap corruption) or 0xC0000409. It
does sometimes complete — one request through the engine returned 0.64 s of
audio after 171.8 s, model load included — but the crash rate makes a timed
fit meaningless. The engine now retries a crashed process up to three times;
before, the retry never ran on Windows because the crash arrives as an exit
code rather than a signal.

A CUDA build of the same runtime (CUDA 13.3, arch 86) does use the GPU —
layers and KV cache land on `CUDA0` — but every run that touched it crashed:
5 of 5 with `-ngl -1`, 3 of 3 with CUDA graphs disabled, 3 of 3 with a
partial `-ngl 20`, while `-ngl 0` succeeded in two of three runs. The catalog
therefore no longer offers MOSS-TTS on Windows.

The ONNX engines ran on the CPU even in a CUDA build: the shipped models are
INT8 exports and ONNX Runtime's CUDA provider has no kernels for their
quantized operators, so INT8 Kokoro took 6.3 s either way and GPU memory
stayed flat. An fp32 export of the same model, as a control, took 1.5 s for
9.6 s of speech on the GPU against 10.2 s on four CPU threads — the GPU path
works, it just has nothing to accelerate in the INT8 models. The Kokoro row
above is the INT8 figure. Kokoro now installs that fp32 export on such a
machine and prefers it, because on this CPU it wins even without the GPU:
without AVX2 the INT8 kernels fall back to scalar code, and the same text
took 6.2 s warm against 14.8 s. How to build that runtime is in
[Development](development.md#onnx-on-the-gpu).

The 8 GB of VRAM held one audio.cpp model at a time. Leaving dots, VoxCPM2
and CosyVoice3 resident together (about 6.2 + 6.5 + 3.3 GB) crashed the
CosyVoice3 server on its second request, so every row above was measured
against a freshly started server. The app now stops the previously used engine's resident server as
soon as another engine synthesizes (about 800 MB of VRAM came back switching
from dots to VoxCPM2), so only one model is ever resident; the measurements
still used one engine per server process so every engine paid the same
start-up cost. The audio.cpp rows are therefore warm numbers: switching back
to an engine pays its model load again — dots took 44.5 s from a cold start
against 11.1 s with its server already up. Method otherwise as above, except that LongCat got three text
lengths (5, 12.5 and 17.6 s of speech) after the two-length fit came out
with a negative slope.

## Requirements

| Feature | Requirement |
| --- | --- |
| ONNX engines (ZipVoice, Kokoro, SenseVoice) | none beyond the app |
| audio.cpp engines (dots, CosyVoice, VoxCPM2, IndexTTS-2) | prebuilt CLI and resident server are downloaded |
| Python engines (Qwen3-TTS, LongCat, Ming, Chatterbox) | Python 3.10–3.12; the scripts create an isolated venv |
| MOSS-TTS | macOS/Linux only (not offered on Windows); prebuilt runtime is downloaded; Python 3 for the one-time tokenizer conversion |
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

IndexTTS-2 is released by [bilibili](https://github.com/index-tts/index-tts)
under the bilibili Model Use License: free to use, with a separate license
required above 100 million monthly active users or RMB 1 billion annual
revenue, and a prohibition on using it to improve other AI models. The GGUF
export and native runtime come from audio.cpp.

MOSS-TTS-Delay is released by [OpenMOSS](https://github.com/OpenMOSS) under
Apache-2.0; the native runtime is built from the OpenMOSS llama.cpp fork.

SenseVoice is released by FunAudioLLM under MIT and packaged by sherpa-onnx.
