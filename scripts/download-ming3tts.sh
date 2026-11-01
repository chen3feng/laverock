#!/usr/bin/env bash
# Install the Ming-3.0-tts 0.5B dense engine for laverock.
#
#   MING3TTS_VARIANT=clone|voice_design|multilingual|ip   (default: clone)
#   MING3TTS_MODEL=...          override the weight repo id
#   MING3TTS_RUNTIME_URL=...    override the runtime source tarball
#
# All variants share one checkpoint, one Python venv and one copy of the
# repo's Python runtime; only the emitted engine.json (and the mode it
# selects) differs. Installing a second variant is therefore cheap: the
# shared pieces in models/.ming3tts-0.5b are reused if already present.
#
# Disk: torch and friends ~2-3 GB, model weights ~3 GB.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init

VARIANT="${MING3TTS_VARIANT:-clone}"
MODEL="${MING3TTS_MODEL:-inclusionAI/Ming-3.0-tts-0.5b}"
RUNTIME_URL="${MING3TTS_RUNTIME_URL:-https://codeload.github.com/inclusionAI/Ming-3.0-tts/tar.gz/refs/heads/main}"
# Hidden shared dir: has no engine.json, so the engine scan ignores it.
SHARED="$MODELS_DIR/.ming3tts-0.5b"

case "$VARIANT" in
  clone)        ENGINE_ID="ming3tts"; DEST="$MODELS_DIR/ming3tts"; MODE="clone" ;;
  voice_design) ENGINE_ID="ming3tts-design"; DEST="$MODELS_DIR/ming3tts-design"; MODE="voice_design" ;;
  multilingual) ENGINE_ID="ming3tts-multilingual"; DEST="$MODELS_DIR/ming3tts-multilingual"; MODE="multilingual" ;;
  ip)           ENGINE_ID="ming3tts-ip"; DEST="$MODELS_DIR/ming3tts-ip"; MODE="ip" ;;
  *)
    echo "MING3TTS_VARIANT must be clone, voice_design, multilingual or ip" >&2
    exit 1
    ;;
esac

PYTHON="$(lv_find_python "${MING3TTS_PYTHON:-}")" || {
  echo "a Python 3.10-3.12 interpreter is required" >&2; exit 1; }
echo "Using $("$PYTHON" --version 2>&1) ($PYTHON)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$DEST" "$SHARED" "$TOOLS"

# 1. Weights. Same offline-per-file download the other engines use; honors
#    HF_ENDPOINT so a mirror works through a flaky upstream.
if [ ! -s "$SHARED/weights/config.json" ]; then
  WEIGHTS="$SHARED/weights"
  mkdir -p "$WEIGHTS"
  lv_stage_key stage_download_model_tokenizer
  lv_hf_download_repo "$MODEL" "$WEIGHTS" 8
fi

# 2. Repo Python runtime (ming_tts.py + the modules it imports). The tarball
#    is gzip, unlike the bzip2 model archives, so extract it directly.
#    Copy every top-level module plus the packages they import: an earlier
#    installer copied only a hand-picked list and broke on prompt_contract,
#    fm and audio_tokenizer.
if [ ! -f "$SHARED/runtime/ming_tts.py" ] \
   || [ ! -f "$SHARED/runtime/prompt_contract.py" ] \
   || [ ! -f "$SHARED/runtime/fm/dit.py" ] \
   || [ ! -f "$SHARED/runtime/audio_tokenizer/modeling_audio_vae.py" ]; then
  lv_stage_key stage_fetch_runtime
  if ! lv_download "$RUNTIME_URL" "$TMP/runtime.tar.gz"; then
    echo "cannot download the Ming-3.0-tts runtime in MING3TTS_RUNTIME_URL, make sure your proxy/mirror is reachable" >&2
    exit 1
  fi
  mkdir -p "$TMP/tar" "$SHARED/runtime"
  tar -xzf "$TMP/runtime.tar.gz" -C "$TMP/tar"
  TOP="$(find "$TMP/tar" -mindepth 1 -maxdepth 1 -type d | head -n1)"
  [ -n "$TOP" ] || { echo "runtime archive has no top-level directory" >&2; exit 1; }
  cp "$TOP"/*.py "$TOP"/requirements.txt "$SHARED/runtime/"
  cp -R "$TOP"/frontend "$TOP"/fm "$TOP"/audio_tokenizer "$SHARED/runtime/"
fi

# 3. Isolated venv.
VENV="$TOOLS/ming3tts-venv"
lv_prepare_python_venv "$VENV" "$PYTHON"
lv_stage_key stage_install_pip_deps
# Drop the torch/torchaudio pins (and comments) so the build that
# lv_prepare_python_venv selected (e.g. CUDA torch on Windows+NVIDIA) stays
# put instead of being replaced by a generic CPU wheel.
grep -vE '^(torch|torchaudio)==|^#|^\s*$' "$SHARED/runtime/requirements.txt" \
  > "$TMP/requirements.txt" || true
"$VPY" -m pip install -r "$TMP/requirements.txt"

# 3b. With the pins dropped, a dependency (x-transformers) pulls a torch
#     build but nothing pulls torchaudio, which ming_tts.py and
#     spkemb_extractor.py import. Install the pair if it is not importable,
#     keeping the CUDA build lv_prepare_python_venv selected on Windows.
if ! "$VPY" -c 'import torch, torchaudio' 2>/dev/null; then
  if lv_is_windows && lv_has_nvidia; then
    lv_stage_key stage_torch_cuda
    "$VPY" -m pip install torch torchaudio \
      --index-url https://download.pytorch.org/whl/cu130
  else
    lv_stage_key stage_torch
    "$VPY" -m pip install torch torchaudio
  fi
fi

# 4. Runner + manifest (absolute paths are allowed and used as-is).
cp "$SCRIPT_DIR/ming3tts_runner.py" "$SHARED/runtime/ming3tts_runner.py"
PY="$(lv_native_path "$VPY")"
RUNNER="$SHARED/runtime/ming3tts_runner.py"
RUNTIME="$SHARED/runtime"
WEIGHTS="$SHARED/weights"
# Rust resolves manifest paths itself, so MSYS /e/.. mounts must be emitted
# as E:/..; only the venv interpreter used to be converted.
RUNNER_NATIVE="$(lv_native_path "$RUNNER")"
RUNTIME_NATIVE="$(lv_native_path "$RUNTIME")"
WEIGHTS_NATIVE="$(lv_native_path "$WEIGHTS")"

case "$MODE" in
  clone)
    NAME="Ming-3.0-tts 0.5B (voice clone)"
    CAPS='"tts","clone"'; PARAMETERS=""
    ;;
  voice_design)
    NAME="Ming-3.0-tts 0.5B (voice design)"
    CAPS='"tts","voice-design","instruct"'
    PARAMETERS='"parameters": [
      {
        "name": "instruct", "label": {"en": "Voice description", "zh": "音色描述"}, "type": "text_area",
        "default": "", "hint": {"en": "Required: describe the voice in words, e.g. \"a warm, calm male narrator\".", "zh": "必填：用自然语言描述想要的音色，如「一位温暖沉稳的男声旁白」。"}
      }
    ],'
    ;;
  multilingual)
    NAME="Ming-3.0-tts 0.5B (multilingual clone)"
    CAPS='"tts","clone","multilingual"'
    PARAMETERS='"parameters": [
      {
        "name": "language", "label": {"en": "Target language", "zh": "目标语言"}, "type": "choice",
        "options": [
          { "value": "Chinese",   "label": {"en": "Chinese 中文", "zh": "中文 Chinese"} },
          { "value": "English",   "label": {"en": "English 英语", "zh": "英语 English"} },
          { "value": "German",    "label": {"en": "German 德语", "zh": "德语 German"} },
          { "value": "Spanish",   "label": {"en": "Spanish 西班牙语", "zh": "西班牙语 Spanish"} },
          { "value": "French",    "label": {"en": "French 法语", "zh": "法语 French"} },
          { "value": "Korean",    "label": {"en": "Korean 韩语", "zh": "韩语 Korean"} },
          { "value": "Japanese",  "label": {"en": "Japanese 日语", "zh": "日语 Japanese"} }
        ],
        "default": "Chinese",
        "hint": {"en": "Clone your reference voice but speak the target language.", "zh": "克隆参考声音但说目标语言。"}
      }
    ],'
    ;;
  ip)
    NAME="Ming-3.0-tts 0.5B (preset voices)"
    CAPS='"tts","preset-voices"'
    PARAMETERS='"parameters": [
      {
        "name": "speaker", "label": {"en": "Voice", "zh": "音色"}, "type": "choice",
        "options": [
          { "value": "UK_George", "label": {"en": "George · UK English male", "zh": "George · 英式男声"} },
          { "value": "US_Emma",   "label": {"en": "Emma · US English female", "zh": "Emma · 美式女声"} },
          { "value": "lingxiaorou", "label": {"en": "Xiaorou · Mandarin female", "zh": "灵小柔 · 中文女声"} },
          { "value": "lingkai",   "label": {"en": "Kai · Mandarin male", "zh": "灵楷 · 中文男声"} }
        ],
        "default": "lingxiaorou",
        "hint": {"en": "Built-in Ming voices; no reference clip needed.", "zh": "Ming 内置音色，无需参考音频。"}
      }
    ],'
    ;;
esac

lv_stage_key stage_write_manifest
cat > "$DEST/engine.json" <<EOF
{
  "id": "$ENGINE_ID",
  "kind": "ming3tts",
  "name": "$NAME",
  "languages": ["zh", "en", "de", "es", "fr", "ko", "ja"],
  "capabilities": [$CAPS],
  "ming3tts": {
    "python": "$PY",
    "script": "$RUNNER_NATIVE",
    "model": "$WEIGHTS_NATIVE",
    "runtime": "$RUNTIME_NATIVE",
    "mode": "$MODE",
    "device": "auto",
    "env": { "PYTHONIOENCODING": "utf-8" }
  },
  $PARAMETERS
  "defaults": { "speed": 1.0, "num_steps": 1 }
}
EOF

echo
echo "Ming-3.0-tts ($VARIANT) installed at $DEST"
echo "Restart the server to pick it up. Share the weights/venv with other variants."