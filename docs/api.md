# HTTP API

English | [中文](api.zh-CN.md) · [← README](../README.md)

The server exposes a small REST API for everything the UI does, plus an
OpenAI-compatible endpoint so existing TTS clients work unchanged. The base
URL is `http://127.0.0.1:8787` (see [Configuration](configuration.md) to
change host or port).

Requests from the loopback address are trusted and need no credentials. The
LAN listener used by the [phone remote](remote.md) requires a device token
(`Authorization: Bearer …` or the `laverock_device` cookie); everything below
describes the loopback listener.

## Endpoints

| Method | Path | Description |
| --- | --- | --- |
| GET | `/api/health` | service status |
| GET | `/api/engines` | registered engines and their defaults |
| POST | `/api/engines/:id/synthesize` | multipart `text` plus either `profile_id` or (`prompt` WAV + `prompt_text`); optional engine parameters → WAV |
| GET | `/api/profiles` | saved voices with duration and size |
| POST | `/api/profiles` | multipart `name`, `prompt_text`, `prompt` (WAV) → saved voice |
| POST | `/api/profiles/:id` | multipart `name`, `prompt_text`, optional `prompt` → edit a saved voice |
| GET | `/api/profiles/:id/audio` | reference clip of a saved voice |
| DELETE | `/api/profiles/:id` | delete a saved voice |
| GET | `/api/profiles/:id/export` | portable `.lvvoice.zip` (manifest + WAV) |
| POST | `/api/profiles/export` | JSON `{ids: [...]}` → one `.lvvoices.zip` batch archive |
| POST | `/api/profiles/import` | multipart `file`: single or batch `.lvvoice(s).zip`; `strategy=skip` (default), `copy` or `overwrite` |
| GET | `/api/history` | generated audio, newest first |
| GET | `/api/history/:id/audio` | generated audio (`?format=mp3` to export) |
| DELETE | `/api/history/:id` · `/api/history` | delete one or all generations |
| POST | `/api/audio/mp3` | body: WAV → MP3 (built-in pure-Rust encoder, no ffmpeg needed) |
| POST | `/api/transcribe` | multipart `audio` (WAV) → transcript (needs the SenseVoice model) |
| GET | `/api/session` | loopback/paired state (public) |
| POST | `/api/pair/start` | desktop only: create a five-minute pairing code + QR |
| POST | `/api/pair/claim` | exchange a pairing code for a device cookie (public) |
| GET | `/api/devices` · DELETE `/api/devices/:id` | desktop only: list or revoke phones |
| GET/POST | `/api/settings/lan` | desktop only: toggle the LAN listener |
| GET/POST | `/api/settings` | desktop only: download proxy, mirrors, machine UI language |
| GET | `/api/models` | desktop only: catalog, install state, job log tail (`?lang=en` for English) |
| POST | `/api/models/:id/install` | desktop only: start a download in the background |
| POST | `/api/models/reload` | desktop only: re-scan `models/*/engine.json` |

Engine parameters (`speed`, `num_steps`, `cfg`, `seed`, …) are declared per
engine by its manifest and validated server-side against the same list the
UI renders; `GET /api/engines` returns them. The engine id is the model
directory id from the [catalog](models.md#catalog).

## Example

```sh
curl -X POST http://127.0.0.1:8787/api/engines/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia/synthesize \
  -F text='你好，这是一段测试。' \
  -F prompt=@reference.wav \
  -F prompt_text='参考音频里说的话' \
  -o out.wav
```

Reuse a saved voice instead of uploading the clip every time:

```sh
curl http://127.0.0.1:8787/api/profiles                       # find the id
curl -X POST http://127.0.0.1:8787/api/engines/voxcpm2/synthesize \
  -F text='Hello from a saved voice.' \
  -F profile_id=p1793075127-c66c \
  -o out.wav
```

## OpenAI-compatible API

Clients written against OpenAI's TTS API — the official SDKs, Open WebUI,
Home Assistant, SillyTavern, scripts — work by changing only the base URL.
There are no API keys; `Authorization` is accepted and ignored.

| Method | Path | Description |
| --- | --- | --- |
| GET | `/v1/models` | installed engines plus the `tts-1` aliases |
| GET | `/v1/models/{id}` | one model entry |
| POST | `/v1/audio/speech` | JSON body → audio |

`model` is an installed engine id, or `tts-1` / `tts-1-hd` / `gpt-4o-mini-tts`,
which use the requested voice's engine. `voice` is either a preset id
(`af_heart`, `zm_xiaoni`, …) for preset-voice engines, or a saved voice's
name or id for cloning engines. `response_format` is `wav` (default), `mp3`
or `pcm` (headerless 16-bit little-endian mono). Errors use OpenAI's
`{"error": {"message", "type", "param", "code"}}` shape.

```sh
curl http://127.0.0.1:8787/v1/audio/speech \
  -H 'Content-Type: application/json' \
  -d '{"model":"tts-1","input":"Hello world.","voice":"af_heart","response_format":"mp3"}' \
  -o speech.mp3
```

In the official Python SDK:

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:8787/v1", api_key="unused")
resp = client.audio.speech.create(
    model="tts-1", voice="zm_xiaoni", input="你好，世界。"
)
resp.write_to_file("speech.mp3")
```
