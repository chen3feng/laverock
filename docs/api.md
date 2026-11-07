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
| POST | `/api/engines/:id/synthesize` | multipart `text` plus either `profile_id` or (`prompt` WAV + `prompt_text`); optional engine parameters → WAV. Add `x-laverock-job: 1` to run it as a background job, or `x-laverock-stream: 1` + `stream_audio=1` to stream sentences |
| POST | `/api/engines/:id/unload` | release the model's memory (it stays installed) → `204` |
| POST | `/api/engines/unload` | release every loaded model → `204` |
| GET | `/api/system` | server RSS, resident child processes and runtimes of loaded engines |
| GET | `/api/jobs/:id` | job status, progress and artifacts |
| GET | `/api/jobs/:id/events` | job events as Server-Sent Events (snapshot, then replay and live) |
| POST | `/api/jobs/:id/cancel` | ask the worker to stop → `202` |
| GET | `/api/jobs/:id/files/:name` | download a finished artifact |
| GET | `/api/profiles` | saved voices with duration, size and take count |
| POST | `/api/profiles` | multipart `name`, `prompt_text`, `prompt` (WAV), or repeated `capture` ids → saved voice |
| POST | `/api/profiles/:id` | multipart `name`, `prompt_text`, optional `prompt` → edit a saved voice |
| POST | `/api/profiles/:id/captures` | JSON `{captures: [id, …]}` → replace the takes behind a voice |
| GET | `/api/profiles/:id/audio` | reference clip a voice would hand an engine (`?engine=` sets the take limit) |
| DELETE | `/api/profiles/:id` | delete a saved voice (its takes stay in the library) |
| GET | `/api/profiles/:id/export` | portable `.lvvoice.zip` (manifest + WAV) |
| POST | `/api/profiles/export` | JSON `{ids: [...]}` → one `.lvvoices.zip` batch archive |
| POST | `/api/profiles/import` | multipart `file`: single or batch `.lvvoice(s).zip`; `strategy=skip` (default), `copy` or `overwrite` |
| GET | `/api/captures` | recording library: takes with transcript, issues and duplicates |
| POST | `/api/captures` | multipart `audio` (WAV), optional `name`, `prompt_text` and `enhance` (`denoise`, `vocals`, `both`) → new take |
| POST | `/api/captures/:id` | multipart `name` and/or `prompt_text` → edit a take |
| GET | `/api/captures/:id/audio` | the take's audio (`?original=1` serves the untouched upload, when the take was enhanced) |
| POST | `/api/captures/:id/enhance` | JSON `{mode}` → re-run cleanup from the original upload, or revert with `off` |
| GET | `/api/enhance` | which cleanup stages are installed → `{denoise, vocals}` |
| POST | `/api/captures/:id/transcribe` | re-run speech recognition on the take |
| DELETE | `/api/captures/:id` | delete a take (it is removed from the voices using it) |
| POST | `/api/captures/dedupe` | delete byte-identical takes, repointing voices → `{removed}` |
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

A voice can be built from one or more **takes** in the recording library.
`POST /api/profiles` with a repeated `capture` field creates such a voice;
`GET /api/engines` reports `reference_limit_secs`, the length at which the
joined reference is cut, because each engine only conditions on that much.

Engine parameters (`speed`, `num_steps`, `cfg`, `seed`, …) are declared per
engine by its manifest and validated server-side against the same list the
UI renders; `GET /api/engines` returns them. Each engine also carries a
localized `description` (`{"en": …, "zh": …}`, or a plain string) for model
pickers. The engine id is the model
directory id from the [catalog](models.md#catalog).

`GET /api/engines` also reports live state per engine: `loaded`, `resident`
(whether it keeps a child process), `pid` for that process, and `backend`
(the runtime: `onnx`, `audiocpp`, `llama.cpp` or `python`). `GET /api/system`
summarises the same data with memory figures (RSS), which the Models cards and
the Settings resource panel display. `POST /api/engines/:id/unload` frees the
memory without removing anything from disk; the next synthesis loads the model
again.

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

## Streaming audio

`x-laverock-stream: 1` already turns the response into Server-Sent Events
(`stage`, `steps`, `wav`, `error`). Adding the multipart field
`stream_audio=1` makes the server synthesize long text sentence by
sentence and send each piece as it finishes:

```text
event: audio
data: {"i":0,"sr":24000,"ch":1,"wav":"<base64 WAV>"}
```

Every chunk is a complete WAV file, so a client can decode and schedule it
while the next sentence is still generating. The server splits text itself
at sentence boundaries (~100 characters) when the engine has no long-text
policy; engines that have one stream their own segments. When the run
finishes, the complete audio still arrives as the final `wav` frame and is
recorded once in history. Audio streaming applies only to a single take
(`samples=1`) and to speech; music and multi-take requests ignore the
field. Omitting it leaves the response byte-for-byte unchanged.

## Background jobs

A long generation can outlive the request that started it. Send
`x-laverock-job: 1` with `POST /api/engines/:id/synthesize` and the server
answers `202 {"job_id": "…"}` immediately; the work continues in the
background. Jobs live in the server process and their artifacts stay under
`data/jobs/<id>/` for a day.

`GET /api/jobs/:id/events` is an SSE stream: the first frame is a
`snapshot` (`id`, `kind`, `status`, `progress`, `message`, `artifacts`),
then buffered events are replayed and live ones follow until `done` or
`error`. Event kinds are `stage`, `progress`, `audio`, `artifact`, `done`
and `error`. Reconnecting replays what was missed, so a refreshed page
resumes instead of losing the run. `POST …/cancel` sets the cancellation
flag; the worker stops between takes.

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
