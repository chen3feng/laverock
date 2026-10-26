#!/usr/bin/env bash
# Install dots.tts (Studio Dots) as a Laverock engine.
#
# dots.tts is a 2B continuous-AR model with a 48 kHz AudioVAE. Instead of
# a Python environment we run its GGUF export through audio.cpp's native
# runtime (Metal on macOS, CPU/Vulkan/CUDA elsewhere), which keeps the
# model resident and avoids a 5 GB PyTorch stack.
#
#   DOTS_MODEL=dots.tts-mf   install the 4-step MeanFlow variant instead
#   DOTS_VERSION=...         pin a different audio.cpp release
#
# The runtime is installed once under <tools>/audiocpp and shared with
# the CosyVoice engine.
#
# Disk: GGUF 3.0 GB (Q8_0), plus a one-time ~35 MB runtime.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=laverock-lib.sh
. "$SCRIPT_DIR/laverock-lib.sh"

MODELS_DIR="${LAVEROCK_MODELS_DIR:-$ROOT/models}"
DEST="$MODELS_DIR/dots"
AC_VERSION="${DOTS_VERSION:-v0.9.2-drama3-base-small}"
MODEL_KIND="${DOTS_MODEL:-dots.tts-soar}"

case "$MODEL_KIND" in
  *mf*) PACKAGE="DotTTS-MF-GGUF/dots-tts-mf-q8_0.gguf" ;;
  *)    PACKAGE="DotTTS-SOAR-GGUF/dots-tts-soar-q8_0.gguf" ;;
esac
GGUF="$DEST/$(basename "$PACKAGE")"

mkdir -p "$DEST"

# 1. Shared audio.cpp runtime (also used by the CosyVoice engine).
TOOLS="${LAVEROCK_TOOLS_DIR:-$ROOT/.tools}"
BIN="$(lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR")"
case "$(uname -s)" in
  Darwin) BACKEND="metal" ;;
  *)      BACKEND="best" ;;
esac

# 2. Weights (Q8_0 GGUF) from Hugging Face, in parallel ranged requests.
if [ ! -f "$GGUF" ]; then
  HF_BASE="${HF_ENDPOINT:-https://huggingface.co}"
  HF_BASE="${HF_BASE%/}"
  URL="$HF_BASE/audio-cpp/audio.cpp-gguf/resolve/main/$PACKAGE"
  lv_stage "下载模型权重（Q8_0，约 3 GB）"
  TOTAL=$(curl -sIL "$URL" | tr -d '\r' | awk 'tolower($1)=="content-length:" { v = $2 } END { print v + 0 }')
  if [ "${TOTAL:-0}" -le 0 ]; then
    echo "cannot determine the download size for $URL" >&2
    exit 1
  fi
  lv_fetch_parallel "$URL" "$GGUF" "$TOTAL" 8
fi

# 3. Manifest.
case "$MODEL_KIND" in
  *mf*) NAME="dots.tts MeanFlow (Q8_0)" ;;
  *)    NAME="dots.tts SOAR (Q8_0)" ;;
esac
cat > "$DEST/engine.json" <<EOF
{
  "id": "dots",
  "kind": "dots",
  "name": "$NAME",
  "languages": ["zh", "en", "ja", "ko", "fr", "de", "es", "ru", "pt", "it", "ar", "+ 24 languages"],
  "capabilities": ["tts", "clone"],
  "dots": {
    "binary": "$BIN",
    "model": "$(basename "$GGUF")",
    "backend": "$BACKEND",
    "guidance": 1.2,
    "seed": 42
  },
  "defaults": { "speed": 1.0, "num_steps": 10 }
}
EOF

echo
echo "dots.tts installed at $DEST"
echo "Reload the 模型 page (or restart the server) to pick it up."
