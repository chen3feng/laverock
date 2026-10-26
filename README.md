# laverock

English | [中文](README.zh-CN.md)

Local-first voice cloning studio. Models run on your own computer; a phone
on the same network will later be able to connect as a remote control.

Status: **MVP**. Seven engines — ZipVoice (Chinese/English, ONNX, ~160 MB),
Kokoro-82M (54 preset voices, 8 languages, ONNX, ~135 MB, no cloning),
VoxCPM2 (30 languages, GGUF/llama.cpp, ~3.3 GB), Qwen3-TTS (10 languages,
official Python package in an isolated venv), LongCat-AudioDiT
(waveform-latent diffusion, zh/en, ~7 GB with its venv), dots.tts
(2B continuous AR, 48 kHz, 24 languages, ~3 GB GGUF) and CosyVoice3
(0.5B streaming LLM TTS, 9 languages + Cantonese, ~2.3 GB GGUF) — behind
one local HTTP server with an embedded web UI, a saved-voice library,
and phone remote control over LAN.

Preset-voices engines (Kokoro) take no reference audio: when selected,
the synthesis page hides the saved-voice/recorder section and offers a
built-in voice picker instead. The UI switches on the engine's
`capabilities` (`clone` vs `preset-voices`).

## Architecture

```
laverock-core     engine trait + manifest registry + ONNX/GGUF runtimes
laverock-server   thin HTTP control plane + embedded SPA + model installer
laverock-desktop  Tauri shell: tray, autostart, server supervision
```

The UI has a sidebar with four sections:

- **合成** — pick an engine and a saved voice (or record/upload one), type
  the text, generate and play or download the result as WAV or MP3.
- **语音素材** — the saved reference voices: play, create, edit (name,
  transcript, replace the clip) and delete. Confirmations use an in-app
  dialog because WebKit in the desktop shell does not implement
  `confirm()`.
- **历史** — every generated WAV (text, engine, time, duration): play,
  download, delete one or clear all; capped at 100 entries / 1 GiB under
  the data directory.
- **模型** — installed engines and a catalog of installable models. A
  download streams its log into the card with a progress bar (percentage
  and current step; subprocess meters like git/pip/tqdm are parsed too),
  and the engine registry reloads when it finishes.
- **设置** — download proxy and mirrors, phone remote control, version.

Installable models are listed in
`crates/laverock-server/assets/catalog.json`; the server runs the same
`scripts/download-*.sh` used from the command line, with the settings
above passed as environment variables.

- Adding a model of a supported kind is just a directory with an
  `engine.json` manifest under `models/` — no code changes.
- A manifest may declare a `parameters` array; the synthesis page builds
  its controls from it (label, `float`/`int`/`bool`/`choice` type,
  min/max/step, options, default, hint) and the server validates every
  submitted value against the same list. Without one, each kind gets its
  built-in set: speed and diffusion steps for ZipVoice, plus
  CFG/temperature/seed for VoxCPM2, and nothing for the non-diffusion
  Qwen3-TTS runner.
- Inference uses the official [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
  Rust crate (static, prebuilt ONNX Runtime; no Python, no system libraries).

## Requirements

- Rust (stable, tested with 1.98)
- macOS / Linux / Windows; CPU inference works everywhere. The MVP ships
  ZipVoice distill INT8 (~160 MB on disk including vocoder).

## Download

Prebuilt alpha packages are on the [releases page](https://github.com/chen3feng/laverock/releases):

| Platform | File |
| --- | --- |
| macOS (Apple Silicon) | `Laverock_*_aarch64.dmg` |
| Windows 64-bit | `Laverock_*_x64-setup.exe` |

The packages do not contain model weights; run the download scripts below
once. macOS builds are ad-hoc signed, Windows builds are unsigned — expect
a one-time Gatekeeper/SmartScreen prompt.

## Quick start

```sh
# 1. Download a model (idempotent)
./scripts/download-model.sh        # ZipVoice, ~160 MB, zh/en, ONNX
./scripts/download-kokoro.sh       # Kokoro-82M, ~135 MB, 8 langs, preset voices
./scripts/download-voxcpm2.sh      # VoxCPM2, ~3.3 GB, 30 langs, GGUF
./scripts/download-qwen3tts.sh     # Qwen3-TTS, isolated venv + ~1.5 GB model
./scripts/download-longcat.sh      # LongCat-AudioDiT 1B, isolated venv + 5.7 GB weights
./scripts/download-dots.sh         # dots.tts SOAR, 48 kHz, audio.cpp runtime + 3 GB GGUF
./scripts/download-cosyvoice.sh    # CosyVoice3 0.5B, 2.3 GB GGUF (shares the runtime)

# 2. Run
cargo run -p laverock-server
# open http://127.0.0.1:8787
```

The same downloads are available from **模型** in the UI; the scripts are
convenient for headless installs and CI.

Engines are discovered from `models/*/engine.json`. ZipVoice and
LongCat-AudioDiT need the reference transcript; VoxCPM2, Qwen3-TTS and
dots.tts clone from the audio alone; CosyVoice3 needs it only for its
zero-shot template (its cross-lingual template does not). Kokoro takes no
reference audio at all — it has 54 built-in speakers chosen from a
manifest-declared `voice` parameter, and the UI hides the cloning section
for it. Qwen3-TTS also supports preset speakers and voice design through
`QWEN3TTS_VARIANT=custom|design`. Adding another
model of a supported kind is just another directory with a manifest;
engines that need Python run in their own venv under `.tools/`, never in
the core process.

Qwen3-TTS picks its device automatically: CUDA in bfloat16, Apple MPS in
float32 (float16 produces NaN in the codec), otherwise CPU. A 0.6B clone
takes roughly 25 s on Apple Silicon and 90 s on CPU per short sentence,
including model load.

LongCat-AudioDiT is a flow-matching diffusion model that works directly in
a waveform latent space; it exposes NFE steps, guidance strength and APG
vs. CFG as per-request parameters (see `parameters` in its manifest). It
runs fp16 on CUDA and fp32 elsewhere, keeps the weights and UMT5 tokenizer
inside the model directory, and its 1B variant needs about 6 GB of RAM. A
single request can produce at most ~30 s of audio including the reference
clip, so prefer 3–10 s prompts. Cloning aligns the transcript against the
reference duration: if the clip runs much longer than its transcript
implies, the runner trims it automatically at a quiet point (and says so
in the progress line), because otherwise the start of the target text is
sliced away and the first sentence disappears. Keep the transcript accurate. Digit
strings are read as Chinese numerals before synthesis (the model handles
Arabic numerals poorly, as upstream documents), and text that would
exceed the per-call budget is split into chunks joined with short pauses
instead of being squeezed, which is what silently dropped the opening
sentence on long passages. Install the larger 3.5B variant with
`LONGCAT_MODEL=meituan-longcat/LongCat-AudioDiT-3.5B ./scripts/download-longcat.sh`.

dots.tts (Studio Dots) is a 2B continuous-AR model whose flow-matching
head drives a 48 kHz AudioVAE. Laverock runs its GGUF export through
audio.cpp's native runtime — Metal on macOS, CPU (or a user-supplied
CUDA/Vulkan build) elsewhere — so there is no Python environment and the
whole install is ~3 GB. Cloning defaults to the x-vector mode (reference
audio only), which stays stable even when the saved transcript is
approximate; "声纹 + 文字" adds the transcript for higher similarity when
it is accurate. Long passages are chunked by the runtime. Measured on an
M4 Pro with the Q8_0 checkpoint: RTF 1.3–1.9, 48 kHz output, about 6 GB
peak memory. The runtime is pinned in `scripts/download-dots.sh`
(`DOTS_VERSION`), and `DOTS_MODEL=dots.tts-mf` selects the 4-step
MeanFlow export.

CosyVoice3 (FunAudioLLM) is a 0.5B streaming LLM TTS that shares the
audio.cpp runtime with dots.tts, so installing both costs one runtime
download. It exposes three templates: zero-shot cloning (reference audio
plus an accurate transcript, highest similarity), cross-lingual cloning
(reference audio only, for when the transcript is unreliable) and
instruction control (describe the delivery in words, e.g. "用温和的语气，
语速稍慢"). Top-K and flow-decoder steps are tunable per request, and
long text is chunked by the runtime.

Pick or record 3–10 s of reference speech, type its exact transcript
(required by ZipVoice for timbre/prosody alignment), type the text to
synthesize, and press generate.

### Interface language

The language selector at the bottom of the sidebar switches the whole UI
between English and Chinese: **Follow system** (browser language), 中文,
or English. The choice is saved in this browser; an explicit desktop
choice is also stored as the machine default for newly paired devices.
Manifest parameter labels/options and the model catalog descriptions
switch too, and common server-side validation messages are translated in
the English UI.

### Proxy and mirrors

**设置 → 下载网络** stores `<data>/settings.json` and passes the values to
every model download script. They never affect inference.

| Setting | Environment | Default |
|---|---|---|
| 代理 | `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` (and lowercase) | system browser proxy |
| Hugging Face 镜像 | `HF_ENDPOINT` | `https://huggingface.co` |
| PyPI 镜像 | `PIP_INDEX_URL` | PyPI |

Downloads follow the proxy the browser uses: an exported `HTTPS_PROXY`
(or `ALL_PROXY` / `HTTP_PROXY`) wins, otherwise the macOS system proxy
(`scutil --proxy`) or the Windows Internet Settings store is used. Pick
"手动指定" to override it, or "不使用代理" to force a direct connection;
manual URLs accept `http`, `https`, `socks5` and `socks5h`. PAC scripts
are not evaluated — enter the proxy by hand in that case. Scripts run
with `no_proxy=127.0.0.1,localhost` merged into any inherited exceptions,
so a local mirror keeps working.

### Desktop shell

```sh
./scripts/run-desktop.sh
```

On macOS the script wraps the binary in a minimal `.app` and ad-hoc signs
it, because microphone access (`getUserMedia`) needs a bundle with
`NSMicrophoneUsageDescription` and its own TCC identity. A bare binary
launched from a shell is attributed to the terminal and is usually denied
without any prompt. The first time you press record, macOS asks for
permission; afterwards it is remembered.

The shell supervises the server for you: if nothing answers on
`127.0.0.1:8787` it starts `laverock-server` itself, then opens a window on
the same web UI. Closing the window hides it; the tray menu has show/hide,
autostart at login, open data directory, and quit.

During development the shell shares the repository's `data/` directory with
the standalone server; release builds use the OS app-data directory. Set
`LAVEROCK_SERVER_BIN` to point at the server binary explicitly, or
`LAVEROCK_URL` to attach to a server elsewhere.

### Phone remote

Turn on **手机遥控 → 局域网访问** in the desktop UI. The server then opens a
second listener on the machine's LAN address (the loopback listener stays
separate), and **生成配对二维码** produces a QR code plus a six-digit code.

- Phones load the same SPA as the desktop; no second frontend.
- **Loopback is trusted; the LAN listener is not.** Everything except `/`,
  `/api/session` and `/api/pair/claim` requires a device token, accepted
  either as `Authorization: Bearer …` or as the `laverock_device` cookie
  (needed for `<audio>` and streamed responses).
- Pairing codes are single-use, expire after five minutes, and are
  invalidated after eight wrong attempts. Only loopback can create one.
- Tokens are 32 random bytes, stored only as SHA-256 hashes in
  `data/remote.json` (mode 0600). Revoking a phone takes effect on its
  next request.
- The pairing state survives restarts (`lan_enabled` is persisted).

The LAN listener serves **HTTPS** with a self-signed certificate
(`data/tls/`, regenerated if the LAN address changes) so a phone gets a
secure context — required for the microphone and for PWA installation. The
first visit warns about the untrusted certificate; choose "advanced →
proceed". The certificate's SHA-256 fingerprint is shown next to the QR
code so an extra-cautious user can verify it matches. The LAN address is
resolved at startup — if the machine switches networks, toggle LAN access
off and on again.

### API

| Method | Path | Description |
|---|---|---|
| GET | `/api/health` | service status |
| GET | `/api/engines` | registered engines and defaults |
| POST | `/api/engines/:id/synthesize` | multipart `text` + either `profile_id` or (`prompt` WAV + `prompt_text`), optional `speed`, `num_steps` → WAV |
| GET | `/api/profiles` | saved voices with duration/size |
| POST | `/api/profiles` | multipart `name`, `prompt_text`, `prompt` (WAV) → saved voice |
| GET | `/api/profiles/:id/audio` | reference clip of a saved voice |
| POST | `/api/audio/mp3` | body: WAV → MP3 (needs ffmpeg or lame) |
| POST | `/api/profiles/:id` | multipart `name`, `prompt_text`, optional `prompt` → edit a saved voice |
| DELETE | `/api/profiles/:id` | delete a saved voice |
| GET | `/api/history` | generated audio, newest first |
| GET | `/api/history/:id/audio` | generated audio (`?format=mp3` to export) |
| DELETE | `/api/history/:id` · `/api/history` | delete one/all generations |
| GET | `/api/session` | loopback/paired state (public) |
| POST | `/api/pair/start` | desktop-only: create a 5-minute pairing code + QR |
| POST | `/api/pair/claim` | exchange a pairing code for a device cookie (public) |
| GET | `/api/devices` · DELETE `/api/devices/:id` | desktop-only: list/revoke phones |
| GET/POST | `/api/settings/lan` | desktop-only: toggle the LAN listener |
| GET/POST | `/api/settings` | desktop-only: download proxy, mirrors, machine UI language |
| GET | `/api/models` | desktop-only: catalog, install state, job log tail (`?lang=en` for English) |
| POST | `/api/models/:id/install` | desktop-only: start a download in the background |
| POST | `/api/models/reload` | desktop-only: re-scan `models/*/engine.json` |

Example:

```sh
curl -X POST http://127.0.0.1:8787/api/engines/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia/synthesize \
  -F text='你好，这是一段测试。' \
  -F prompt=@reference.wav \
  -F prompt_text='参考音频里说的话' \
  -o out.wav
```

### Configuration

| Env var | Default | Purpose |
|---|---|---|
| `LAVEROCK_HOST` | `127.0.0.1` | bind address for the trusted loopback listener |
| `LAVEROCK_PORT` | `8787` | port |
| `LAVEROCK_MODELS_DIR` | `models` | directory scanned for `*/engine.json` |
| `LAVEROCK_DATA_DIR` | `data` | profiles, settings, pairing state, TLS certs |
| `LAVEROCK_SCRIPTS_DIR` | auto-detected | `scripts/` used by the model installer |
| `LAVEROCK_FFMPEG` / `LAVEROCK_LAME` | auto-detected | explicit MP3 encoder path |
| `LAVEROCK_TOOLS_DIR` | `<repo>/.tools` | build tools and Python venvs for engines |

Packaged desktop builds ship `scripts/` inside the app bundle (macOS
`Contents/Resources/scripts`, Windows/Linux beside the executable) and
keep downloads, build tools and venvs in the user data directory
(`~/Library/Application Support/sh.laverock.desktop/models` and
`…/tools` on macOS), so an installed app never reads from a source
checkout. On Windows, in-app model installs need `bash` from Git for
Windows; the download scripts are otherwise the same on every platform.

## Model credits

ZipVoice model weights are Apache-2.0, released by
[k2-fsa/ZipVoice](https://github.com/k2-fsa/ZipVoice) and packaged by
[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).

Kokoro-82M weights are Apache-2.0, released by
[hexgrad/Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M); the ONNX
export and packaging come from sherpa-onnx.

LongCat-AudioDiT code and weights are MIT, released by
[meituan-longcat/LongCat-AudioDiT](https://github.com/meituan-longcat/LongCat-AudioDiT);
its UMT5 tokenizer comes from `google/umt5-base`.

CosyVoice3 code and checkpoints are Apache-2.0, released by
[FunAudioLLM/CosyVoice](https://github.com/QwenAudio/CosyVoice); the GGUF
export also comes from audio.cpp.

dots.tts code and checkpoints are Apache-2.0, released by
[studio-dots-ai/dots.tts](https://github.com/studio-dots-ai/dots.tts).
The GGUF export and the native runtime come from
[0xShug0/audio.cpp](https://github.com/0xShug0/audio.cpp); the weights keep
their Apache-2.0 license, and the runtime binary is redistributed
unmodified.

## License

MIT
