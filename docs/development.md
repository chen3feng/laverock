# Development

English | [中文](development.zh-CN.md) · [← README](../README.md)

Everything here is for building and extending Laverock. If you only want to
use the app, the [README](../README.md) is enough.

## Requirements

- Rust stable (edition 2021) and Cargo.
- Linux only: the Tauri webview dependencies — `libwebkit2gtk-4.1-dev`,
  `libappindicator3-dev`, `librsvg2-dev`, `patchelf`, `libgtk-3-dev`.
- A C++ toolchain and CMake are not needed for the app. Only a source build
  of the MOSS-TTS runtime on a platform without a prebuilt package
  (`MOSS_RUNTIME_BUILD=1`) needs them.

## Running from source

```sh
cargo run -p laverock-server     # then open http://127.0.0.1:8787
```

The server serves the embedded web UI from `crates/laverock-server/assets/`.
With the default environment it uses `./data` and `./models`, so a checkout
keeps its own voices, history and models.

For the desktop shell:

```sh
./scripts/run-desktop.sh
```

The script builds `laverock-desktop` and `laverock-server`. On macOS it wraps
the binary in a minimal `.app` and ad-hoc signs it, because microphone access
(`getUserMedia`) needs a bundle with `NSMicrophoneUsageDescription` and its
own TCC identity — a bare binary launched from a shell is attributed to the
terminal and is usually denied without a prompt.

The shell supervises the server: if nothing answers on `127.0.0.1:8787` it
starts `laverock-server` itself (passing its data, models and tools
directories), then opens a window on the same web UI. Closing the window
hides it; the tray menu has show/hide, autostart at login, open data
directory and quit. `LAVEROCK_SERVER_BIN` points it at a specific server
binary, `LAVEROCK_URL` at a server elsewhere.

## ONNX on the GPU

ZipVoice, Kokoro and SenseVoice normally run on the CPU: the default build
links the static, CPU-only sherpa-onnx runtime. A CUDA build links the shared
runtime instead and asks for the `cuda` execution provider:

```sh
./scripts/setup-onnx-cuda.sh   # downloads the runtime, prints SHERPA_ONNX_LIB_DIR
SHERPA_ONNX_LIB_DIR=<printed above> \
  cargo build -p laverock-server --no-default-features --features onnx-cuda
```

The CUDA 13 and cuDNN 9 DLLs must be on `PATH` at run time; a PyTorch install
usually already provides them under `Lib/site-packages/torch/lib`. If the
driver has no usable device or cuDNN is missing, the engines log a warning
and stay on the CPU — a failed provider start would abort the process, so the
check happens before the session is created. `LAVEROCK_ONNX_PROVIDER=cpu`
forces the CPU in a CUDA build.

Only non-quantized models gain. The CUDA provider has no kernels for the INT8
operators the shipped models are exported with, so they run at CPU speed
either way. On an RTX 3060 Ti, an fp32 Kokoro export took 1.5 s for 9.6 s of
speech on the GPU against 10.2 s on four CPU threads, while the INT8 Kokoro
took 6.3 s in both builds.

audio.cpp also packages Kokoro (`kokoro_tts`), but it is not used: on the RTX
3060 Ti the two implementations measured the same RTF on the GPU (0.152
against 0.156) while the ONNX export was faster on the CPU (1.06 against
1.89), with a ~1 s load against ~95 s and a 325 MB download against 889 MB.
The ONNX path stays.

Installers therefore choose their files for the machine they run on, not for
the machine that built the app: the server exports `LAVEROCK_ONNX_VARIANT`
(`fp32` or `int8`, from the runtime provider and a local AVX2 check), and
`download-kokoro.sh` fetches the fp32 export in addition to the INT8 one when
it reads `fp32`. A hand-run script makes the same check itself;
`LAVEROCK_ONNX_VARIANT` or `KOKORO_FP32=1/0` overrides either. At load time
the engine uses `model_fp32` from the manifest when the file exists and the
machine prefers fp32, and warns if only the INT8 file is installed.

## Architecture

```text
laverock-core     engine trait + manifest registry + ONNX/GGUF runtimes
laverock-server   thin HTTP control plane + embedded SPA + model installer
laverock-desktop  Tauri shell: tray, autostart, server supervision
```

`laverock-core` turns an engine directory into a `SpeechEngine`
implementation selected by the manifest's `kind`. ONNX engines (ZipVoice,
Kokoro, SenseVoice) run in-process on the official
[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) Rust crate; GGUF engines
go through the audio.cpp or llama.cpp binaries; Python engines are driven as
subprocesses through a runner in `scripts/`, each in its own venv.

The server owns profiles, history, settings, pairing and the model catalog.
The SPA is plain HTML/CSS/JS compiled into the server with `include_str!`.
Its sources live in `crates/laverock-server/web/` — a shell `index.html`, a
stylesheet, the markup, and one JavaScript file per section — and
`python3 scripts/build-web.py` stitches them into the single
`assets/index.html` that ships (no runtime dependencies, no bundler). Edit the
files under `web/`, run the script, then rebuild the server; CI runs
`build-web.py --check` so a stale bundle cannot merge.

## Adding a model

A model of an already supported kind needs no code: create
`<models>/<id>/engine.json` and point its paths at the files in that
directory. Supported kinds are `zipvoice`, `voxcpm2`, `qwen3tts`, `longcat`,
`dots`, `cosyvoice`, `kokoro`, `ming3tts`, `chatterbox`, `indextts2`, `mosstts`
and `stt`.

```json
{
  "id": "kokoro-int8-multi-lang-v1_0",
  "kind": "kokoro",
  "name": "Kokoro 82M multi-language (INT8)",
  "languages": ["zh", "en", "ja"],
  "capabilities": ["tts", "preset-voices"],
  "kokoro": {
    "model": "model.int8.onnx",
    "voices": "voices.bin",
    "tokens": "tokens.txt",
    "data_dir": "espeak-ng-data",
    "length_scale": 1.0,
    "num_threads": 4
  },
  "defaults": { "speed": 1.0, "num_steps": 4 },
  "parameters": [
    {
      "name": "voice",
      "label": { "en": "Voice", "zh": "音色" },
      "type": "choice",
      "options": [{ "label": "Heart (af_heart)", "value": "3" }],
      "default": "3"
    }
  ]
}
```

- `capabilities` drives the UI (`clone`, `preset-voices`, `voice-design`,
  `instruct`, `multilingual`, `stt`).
- `parameters` is the whole control surface: `name`, `label`, `type`
  (`float` / `int` / `bool` / `choice`), `min`, `max`, `step`, `options`
  (label/value pairs), `default` and `hint`. The synth page builds the
  controls from it and the server validates submitted values against the same
  list. Without it, each kind gets its built-in set.
- `label`, `hint`, `name` and catalog descriptions are `{"en": …, "zh": …}`
  objects so the bilingual UI can translate them; a bare string works too for
  hand-written manifests.
- `Manifest::problem()` flags structural problems (for example an install
  from a retired runner) before the user hits a synthesis error.

To make a model installable from the UI, add an entry to
`crates/laverock-server/assets/catalog.json` with its `id`, `dir`, localized
`name`/`description`/`note`, `size`, `languages`, `capabilities` and the
`script` that installs it.

## Installer scripts

Model installers live in `scripts/download-*.sh` and are shared by the UI and
the command line. They are idempotent, honour `LAVEROCK_MODELS_DIR`,
`LAVEROCK_TOOLS_DIR`, `HF_ENDPOINT`, `PIP_INDEX_URL` and the proxy variables,
and print progress lines the server parses for the download bar. Use
`scripts/laverock-lib.sh` for the common helpers (download, venv, Python
discovery). See [Models](models.md#installing) for the per-engine variants.

## Tests

```sh
cargo fmt --all -- --check
# onnx-cpu (default) and onnx-cuda are mutually exclusive, so no --all-features.
cargo clippy --all-targets -- -D warnings
cargo test --workspace
./scripts/check-shell.sh          # bash 3.2 compatibility + shellcheck

# against a running server with ZipVoice and SenseVoice installed
./scripts/smoke-test.sh           # feature sweep
./scripts/e2e-test.sh             # record -> transcribe -> synthesize -> export

# with Playwright installed (`npm install -g playwright`) and the server running
node scripts/check-web-audio.mjs  # one clip plays at a time in the UI
```

CI runs those on macOS, Linux and Windows, then starts the server on Ubuntu
with the small ZipVoice model plus SenseVoice and exercises it with
`scripts/smoke-test.sh` and `scripts/e2e-test.sh`. Pushing a `v*` tag (or
dispatching the Release workflow) builds the macOS and Windows installers and
attaches them to the release.
