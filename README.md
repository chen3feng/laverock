# laverock

Local-first voice cloning studio. Models run on your own computer; a phone
on the same network will later be able to connect as a remote control.

Status: **MVP**. One engine (ZipVoice, Chinese/English zero-shot cloning),
one local HTTP server with an embedded web UI, and a saved-voice library so
a reference clip only has to be recorded once.

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
# 1. Download the model (idempotent; ~160 MB)
./scripts/download-model.sh

# 2. Run
cargo run -p laverock-server
# open http://127.0.0.1:8787
```

Pick or record 3–10 s of reference speech, type its exact transcript
(required by ZipVoice for timbre/prosody alignment), type the text to
synthesize, and press generate.

### Desktop shell

```sh
cargo run -p laverock-desktop
```

The shell supervises the server for you: if nothing answers on
`127.0.0.1:8787` it starts `laverock-server` itself, then opens a window on
the same web UI. Closing the window hides it; the tray menu has show/hide,
autostart at login, open data directory, and quit.

During development the shell shares the repository's `data/` directory with
the standalone server; release builds use the OS app-data directory. Set
`LAVEROCK_SERVER_BIN` to point at the server binary explicitly, or
`LAVEROCK_URL` to attach to a server elsewhere.

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
