# laverock

English | [中文](README.zh-CN.md)

A local-first voice cloning studio. Speech synthesis and voice cloning run
entirely on your own computer — no cloud account, API key or upload. One
desktop app brings together a model manager, a saved-voice library, a
generation history and a phone remote that joins over your local network.

## What it does

- **Voice cloning and preset voices** — save 3–10 s of speech as a named
  voice, then have any cloning engine read new text in it. Engines without a
  reference clip are covered by preset-voice and voice-design modes.
- **15 models behind one interface** — from a 135 MB ONNX model that runs in
  real time on CPU to 2B diffusion and LLM systems. Install, update and
  remove them from the app, with live download progress.
- **Recording library and voices** — every recording becomes a take,
  transcribed automatically with on-device speech recognition. A voice is
  one take or several joined together, and voices export or import as
  `.lvvoice.zip`.
- **History** — every generation is kept, playable and downloadable as WAV
  or MP3, newest first.
- **Phone remote** — turn on LAN access, scan a QR code, and use the same UI
  from a phone on the same Wi-Fi, over HTTPS.
- **OpenAI-compatible API** — clients written for OpenAI's TTS API work by
  changing only the base URL.
- **Bilingual UI** — Chinese and English, following the system language by
  default.

Everything runs offline after the models are downloaded. macOS, Windows and
Linux are supported; CPU inference works everywhere, and the larger engines
use Metal on Apple Silicon or CUDA on NVIDIA GPUs when available.

## Get started

1. Download the installer for your platform from the
   [releases page](https://github.com/chen3feng/laverock/releases) — macOS
   is Apple Silicon only.
2. Open Laverock and pick a model on the **Models** page. ZipVoice (~160 MB)
   is the smallest and fastest on CPU; Kokoro (~135 MB) is the lightest way
   to try preset voices. The packages ship without weights, so this is the
   one download you need.
3. On **Library**, record a take or upload a clip; it is transcribed for
   you. On **Voices**, tick the takes you want and save them as one voice
   (or choose a preset voice, depending on the engine). On **Synthesize**,
   pick the engine and voice, type the text, and generate.

The app keeps models, voices and history in the system application-data
directory and supervises its own local server, so there is nothing else to
configure.

## Supported models

| Model | Modes | Languages | Download |
| --- | --- | --- | --- |
| ZipVoice Distill INT8 | clone | zh, en | ~160 MB |
| Kokoro-82M INT8 | 54 preset voices | 8 | ~135 MB |
| VoxCPM2 | clone, voice design | 30 | ~3.3 GB |
| LongCat-AudioDiT 1B | clone | zh, en | ~7 GB |
| dots.tts SOAR | clone | 24 | ~3.1 GB |
| CosyVoice3 0.5B | clone, instructions | 9 + Cantonese | ~2.3 GB |
| Qwen3-TTS 0.6B / 1.7B | clone, preset, voice design | 10 | 2–5 GB |
| Ming-3.0-tts 0.5B | clone, preset, voice design, multilingual | 7 | ~3 GB |
| Chatterbox Multilingual | clone | 23 | ~5 GB |
| MOSS-TTS-Delay | clone | 20 | ~10 GB |
| SenseVoice Small (optional) | speech recognition | 5 | ~234 MB |

Requirements, per-engine notes and manual install scripts are in
**[Models](docs/models.md)**.

## Documentation

| Document | Contents |
| --- | --- |
| [Models](docs/models.md) | catalog, capabilities, speed, requirements, manual installation, credits |
| [Voice quality](docs/quality.md) | how natural each engine sounds, how closely it clones, how well it holds up |
| [HTTP API](docs/api.md) | REST endpoints, examples, OpenAI-compatible API |
| [Phone remote](docs/remote.md) | pairing, LAN security, HTTPS, installing as an app |
| [Configuration](docs/configuration.md) | environment variables, data directories, proxy and mirrors, interface language |
| [Development](docs/development.md) | building from source, running the server, architecture, adding engines |

## License

MIT. Bundled and downloaded model weights keep their own licenses; see
[Models](docs/models.md#credits).
