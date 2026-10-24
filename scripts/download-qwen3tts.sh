#!/usr/bin/env bash
# Install the Qwen3-TTS engine: an isolated Python venv with the official
# `qwen-tts` package, the model weights, and the laverock runner script.
#
#   QWEN3TTS_VARIANT=base|custom|design   (default: base = voice cloning)
#   QWEN3TTS_MODEL=Qwen/...               override the model repo id
#
# Disk: torch and friends ~2-3 GB, model 1.5-4 GB depending on the variant.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=laverock-lib.sh
. "$SCRIPT_DIR/laverock-lib.sh"
MODELS_DIR="${LAVEROCK_MODELS_DIR:-$ROOT/models}"
TOOLS="${LAVEROCK_TOOLS_DIR:-$ROOT/.tools}"
DEST="$MODELS_DIR/qwen3tts"
VENV="$TOOLS/qwen-tts-venv"
VARIANT="${QWEN3TTS_VARIANT:-base}"

case "$VARIANT" in
  base)
    MODEL="${QWEN3TTS_MODEL:-Qwen/Qwen3-TTS-12Hz-0.6B-Base}"
    MODE="clone"; SPEAKER=""; EXTRA_CAPS='"clone"'; NAME="Qwen3-TTS 0.6B (voice clone)"
    ;;
  custom)
    MODEL="${QWEN3TTS_MODEL:-Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice}"
    MODE="custom_voice"; SPEAKER="Vivian"; EXTRA_CAPS='"presets","instruct"'
    NAME="Qwen3-TTS 0.6B (custom voice)"
    ;;
  design)
    MODEL="${QWEN3TTS_MODEL:-Qwen/Qwen3-TTS-12Hz-1.7B-VoiceDesign}"
    MODE="voice_design"; SPEAKER=""
    EXTRA_CAPS='"voice-design","instruct"'; NAME="Qwen3-TTS 1.7B (voice design)"
    ;;
  *)
    echo "QWEN3TTS_VARIANT must be base, custom or design" >&2
    exit 1
    ;;
esac

# torch has no wheels for the newest Python, so pick a supported one.
PYTHON="${QWEN3TTS_PYTHON:-}"
if [ -z "$PYTHON" ]; then
  for candidate in python3.12 python3.11 python3.10 python3; do
    if command -v "$candidate" >/dev/null 2>&1; then
      PYTHON="$candidate"
      break
    fi
  done
fi
command -v "$PYTHON" >/dev/null 2>&1 || { echo "a Python 3.10-3.12 interpreter is required" >&2; exit 1; }
echo "Using $("$PYTHON" --version 2>&1) ($PYTHON)"
mkdir -p "$MODELS_DIR" "$DEST" "$TOOLS"

# 1. Isolated environment — Qwen's torch/transformers pins stay here.
if [ ! -x "$VENV/bin/python" ]; then
  lv_stage "创建 Python 虚拟环境"
  "$PYTHON" -m venv "$VENV"
fi
# Progress bars on even though pip/hf see a pipe, not a terminal.
export PIP_PROGRESS_BAR=on HF_HUB_DISABLE_PROGRESS_BARS=0
lv_stage "安装 qwen-tts 依赖（约 2 GB，首次较慢）"
"$VENV/bin/python" -m pip install --upgrade pip
"$VENV/bin/python" -m pip install --upgrade "qwen-tts" "huggingface_hub[cli]"

# 2. Weights. A dedicated HF cache keeps them next to the engine and lets
#    the model resolve its tokenizer repo offline later.
export HF_HOME="$DEST/.hf-cache"
lv_stage "下载分词器"
"$VENV/bin/huggingface-cli" download Qwen/Qwen3-TTS-Tokenizer-12Hz
lv_stage "下载模型权重"
"$VENV/bin/huggingface-cli" download "$MODEL"

# 3. Runner script.
cp "$SCRIPT_DIR/qwen3tts_runner.py" "$DEST/qwen3tts_runner.py"

# 4. Manifest (absolute paths are allowed and used as-is).
PY="$VENV/bin/python"
cat > "$DEST/engine.json" <<EOF
{
  "id": "qwen3tts",
  "kind": "qwen3tts",
  "name": "$NAME",
  "languages": ["zh", "en", "ja", "ko", "de", "fr", "ru", "pt", "es", "it"],
  "capabilities": ["tts", $EXTRA_CAPS],
  "qwen3tts": {
    "python": "$PY",
    "script": "qwen3tts_runner.py",
    "model": "$MODEL",
    "mode": "$MODE",
    "speaker": "$SPEAKER",
    "language": "Auto",
    "device": "auto",
    "dtype": "auto",
    "attn": "sdpa",
    "env": { "HF_HOME": "$DEST/.hf-cache" }
  },
  "defaults": { "speed": 1.0, "num_steps": 1 }
}
EOF

echo
echo "Qwen3-TTS ($VARIANT) installed at $DEST"
echo "Restart the server to pick it up."
