# laverock

Local-first voice cloning studio. Models run on your own computer; a phone
on the same network will later be able to connect as a remote control.

Status: **MVP**. Two engines — ZipVoice (Chinese/English, ONNX, ~160 MB) and
VoxCPM2 (30 languages, GGUF/llama.cpp, ~3.3 GB) — behind one local HTTP
server with an embedded web UI, a saved-voice library, and phone remote
control over LAN.

## Architecture (target)

```
laverock-core     engine trait + manifest registry + ONNX/GGUF runtimes
laverock-server   thin HTTP control plane (loopback for now)
```

- Adding a model of a supported kind is just a directory with an
  `engine.json` manifest under `models/` — no code changes.
- Inference uses the official [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
  Rust crate (static, prebuilt ONNX Runtime; no Python, no system libraries).

## Requirements

- Rust (stable, tested with 1.98)
- macOS / Linux / Windows; CPU inference works everywhere. The MVP ships
  ZipVoice distill INT8 (~160 MB on disk including vocoder).

## Quick start

```sh
# 1. Download a model (idempotent)
./scripts/download-model.sh        # ZipVoice, ~160 MB, zh/en, ONNX
./scripts/download-voxcpm2.sh      # VoxCPM2, ~3.3 GB, 30 langs, GGUF

# 2. Run
cargo run -p laverock-server
# open http://127.0.0.1:8787
```

Engines are discovered from `models/*/engine.json`. ZipVoice needs the
reference transcript; VoxCPM2 clones from the audio alone. Adding another
model of a supported kind is just another directory with a manifest.

Pick or record 3–10 s of reference speech, type its exact transcript
(required by ZipVoice for timbre/prosody alignment), type the text to
synthesize, and press generate.

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
| GET | `/api/profiles` | saved voices (metadata only) |
| POST | `/api/profiles` | multipart `name`, `prompt_text`, `prompt` (WAV) → saved voice |
| GET | `/api/profiles/:id/audio` | reference clip of a saved voice |
| DELETE | `/api/profiles/:id` | delete a saved voice |
| GET | `/api/session` | loopback/paired state (public) |
| POST | `/api/pair/start` | desktop-only: create a 5-minute pairing code + QR |
| POST | `/api/pair/claim` | exchange a pairing code for a device cookie (public) |
| GET | `/api/devices` · DELETE `/api/devices/:id` | desktop-only: list/revoke phones |
| GET/POST | `/api/settings/lan` | desktop-only: toggle the LAN listener |

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
| `LAVEROCK_HOST` | `127.0.0.1` | bind address (loopback only until pairing/TLS lands) |
| `LAVEROCK_PORT` | `8787` | port |
| `LAVEROCK_MODELS_DIR` | `models` | directory scanned for `*/engine.json` |
| `LAVEROCK_DATA_DIR` | `data` | saved voices are stored under `<data>/profiles/<id>/` |

## Model credits

ZipVoice model weights are Apache-2.0, released by
[k2-fsa/ZipVoice](https://github.com/k2-fsa/ZipVoice) and packaged by
[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).

## License

MIT
