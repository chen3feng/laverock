# Configuration

English | [中文](configuration.zh-CN.md) · [← README](../README.md)

Most users never need this page: the desktop app picks its directories,
writes its settings to the data directory, and supervises the server. It is
here for headless servers, custom installs and moving multi-gigabyte models
to another drive.

## Environment variables

These apply to the `laverock-server` binary.

| Variable | Default | Purpose |
| --- | --- | --- |
| `LAVEROCK_HOST` | `127.0.0.1` | bind address of the trusted loopback listener |
| `LAVEROCK_PORT` | `8787` | port |
| `LAVEROCK_DATA_DIR` | `data` | voices, history, settings, pairing state, TLS certificates |
| `LAVEROCK_MODELS_DIR` | `models` | directories scanned for `*/engine.json`; separate several with `:` (`;` on Windows), highest priority first |
| `LAVEROCK_TOOLS_DIR` | — | Python venvs and build tools for engines |
| `LAVEROCK_FFMPEG` | — | `ffmpeg` executable or a directory holding `ffmpeg` and `ffprobe`; used for video and non-WAV audio |
| `LAVEROCK_SCRIPTS_DIR` | auto-detected | `scripts/` used by the model installer |

The desktop shell sets `LAVEROCK_DATA_DIR`, `LAVEROCK_MODELS_DIR`,
`LAVEROCK_TOOLS_DIR` and `LAVEROCK_SCRIPTS_DIR` for the server it spawns.
`LAVEROCK_URL` points the shell at a server on another machine, and
`LAVEROCK_SERVER_BIN` at a specific server binary.

## Directories

| Build | Data | Models | Tools (venvs) |
| --- | --- | --- | --- |
| Source checkout | `./data` | `./models` | `./.tools` |
| macOS app | `~/Library/Application Support/sh.laverock.desktop` | `<data>/models` | `<data>/tools` |
| Windows app | `%APPDATA%\sh.laverock.desktop` | `<data>\models` | `<data>\tools` |
| Linux app | `~/.local/share/sh.laverock.desktop` | `<data>/models` | `<data>/tools` |

The data directory holds:

```text
profiles/     saved voices (a WAV plus metadata per voice)
history/      generated audio, newest first (capped at 100 entries / 1 GiB)
tls/          self-signed certificate for the LAN listener
settings.json download proxy, mirrors, UI language, workspace root
remote.json   paired phones (SHA-256 hashes, mode 0600)
```

To move models and venvs to another drive, set **workspace directory** in
the app's settings: models then live in `<workspace>/models` and venvs in
`<workspace>/tools`. Voices and history stay in the data directory, so the
workspace never contains the settings that define it. An explicit workspace
also overrides `LAVEROCK_MODELS_DIR` / `LAVEROCK_TOOLS_DIR`.

When `LAVEROCK_MODELS_DIR` lists several directories, they are layered: the
first directory that provides an engine id wins, later copies are reported
as `shadowed` by `GET /api/engines` and never loaded, and new installs go to
the first directory. This lets a read-only shared weights library and a
local overlay coexist.

## Download network

**Settings → Download network** is stored in `<data>/settings.json` and
passed to every model download script. It never affects inference.

| Setting | Environment | Default |
| --- | --- | --- |
| Proxy | `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` (and lowercase) | system browser proxy |
| Hugging Face mirror | `HF_ENDPOINT` | `https://huggingface.co` |
| PyPI mirror | `PIP_INDEX_URL` | PyPI |

Downloads follow the proxy the browser uses: an exported `HTTPS_PROXY` (or
`ALL_PROXY` / `HTTP_PROXY`) wins, otherwise the macOS system proxy
(`scutil --proxy`) or the Windows Internet Settings store is used. Choose
**Manual** to override it, or **Direct** to force a direct connection.
Manual URLs accept `http`, `https`, `socks5` and `socks5h`. PAC scripts are
not evaluated — enter the proxy by hand in that case. Scripts run with
`no_proxy=127.0.0.1,localhost` merged into any inherited exceptions, so a
local mirror keeps working.

## Interface language

The language selector at the bottom of the sidebar switches the whole UI
between English and Chinese: **Follow system** (browser language), 中文 or
English. The choice is saved in the browser; an explicit choice on the
desktop is also stored in `settings.json` as the machine default for newly
paired phones. Engine parameter labels, model catalog descriptions and
server-side validation messages follow the language too.

## Audio

- **History** keeps at most 100 entries or 1 GiB, whichever comes first;
  older generations are removed automatically.
- **MP3 export** is encoded in-process by
  [`rusty_mp3`](https://crates.io/crates/rusty_mp3) (Apache-2.0), so no system
  ffmpeg or lame is needed. Engine output at 16/24 kHz is encoded as
  MPEG-2; other rates are resampled to the nearest of the nine MPEG Layer III
  rates first.
- Generated WAVs are mono 16-bit PCM at the engine's native rate.

## Video and compressed audio

Subtitle projects accept WAV directly. Video (MP4, MOV, MKV, WebM …) and
compressed audio (MP3, M4A, FLAC, OGG …) need system `ffmpeg` and
`ffprobe`; without them only WAV can be imported and the page says so.
The binaries are found in this order:

1. **Settings → ffmpeg path** — the executable itself or a directory
   containing both binaries. `~` is expanded; the path must be absolute.
2. `LAVEROCK_FFMPEG` — same meaning.
3. `PATH`.

The upload is streamed to disk, probed for duration and picture size, and
its first audio track is converted to mono 48 kHz WAV. `GET /api/health`
reports `"ffmpeg": true|false`; `GET /api/settings` also returns the
resolved `ffmpeg_bin` so the page can show what was found.

Exporting the finished video copies the picture (`-c:v copy`) and re-encodes
only the audio: AAC in MP4/MOV/MKV, Opus in WebM. Soft subtitles use
`mov_text`, SRT or WebVTT for those containers respectively. The export is
cut to the project duration, so picture and sound end together.

## Packaged builds

Packaged desktop builds ship `scripts/` inside the app bundle (macOS
`Contents/Resources/scripts`, Windows/Linux beside the executable) and keep
downloads, build tools and venvs in the user data directory, so an installed
app never reads from a source checkout. On Windows, in-app model installs
need `bash` from Git for Windows; the download scripts are otherwise the
same on every platform.
