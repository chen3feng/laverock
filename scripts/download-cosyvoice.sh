#!/usr/bin/env bash
# Install CosyVoice3 (FunAudioLLM) as a Laverock engine.
#
# CosyVoice3 is a 0.5B LLM-based TTS with zero-shot cloning, cross-lingual
# synthesis and instruction control. Like dots.tts it runs from a GGUF
# through audio.cpp's native runtime (Metal on macOS, CPU elsewhere), so
# no Python environment is involved and the runtime is shared with the
# other audio.cpp engines.
#
#   COSYVOICE_PRECISION=q8_0|f32   q8_0 (~2.3 GB, default) or f32 (~7 GB)
#
# Disk: Q8_0 2.3 GB (the runtime is downloaded once, ~35 MB).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=laverock-lib.sh
. "$SCRIPT_DIR/laverock-lib.sh"

MODELS_DIR="${LAVEROCK_MODELS_DIR:-$ROOT/models}"
TOOLS="${LAVEROCK_TOOLS_DIR:-$ROOT/.tools}"
DEST="$MODELS_DIR/cosyvoice"
AC_VERSION="${DOTS_VERSION:-v0.9.2-drama3-base-small}"
PRECISION="${COSYVOICE_PRECISION:-q8_0}"

case "$PRECISION" in
  f32) GGUF_NAME="cosyvoice3-f32.gguf"; SIZE_NOTE="约 7 GB" ;;
  *)   GGUF_NAME="cosyvoice3-q8_0.gguf"; SIZE_NOTE="约 2.3 GB" ;;
esac
GGUF="$DEST/$GGUF_NAME"

mkdir -p "$DEST"

# 1. Shared audio.cpp runtime.
BIN="$(lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR")"

# 2. Weights.
if [ ! -f "$GGUF" ]; then
  HF_BASE="${HF_ENDPOINT:-https://huggingface.co}"
  HF_BASE="${HF_BASE%/}"
  URL="$HF_BASE/audio-cpp/audio.cpp-gguf/resolve/main/CosyVoice3-GGUF/$GGUF_NAME"
  lv_stage "下载 CosyVoice3 权重（${SIZE_NOTE}）"
  TOTAL=$(curl -sIL "$URL" | tr -d '\r' | awk 'tolower($1)=="content-length:" { v = $2 } END { print v + 0 }')
  if [ "${TOTAL:-0}" -le 0 ]; then
    echo "cannot determine the download size for $URL" >&2
    exit 1
  fi
  lv_fetch_parallel "$URL" "$GGUF" "$TOTAL" 8
fi

# 3. Manifest.
cat > "$DEST/engine.json" <<EOF
{
  "id": "cosyvoice",
  "kind": "cosyvoice",
  "name": "CosyVoice3 0.5B ($PRECISION)",
  "languages": ["zh", "en", "ja", "ko", "de", "es", "fr", "it", "ru", "yue"],
  "capabilities": ["tts", "clone", "instruct"],
  "cosyvoice": {
    "binary": "$BIN",
    "model": "$(basename "$GGUF")",
    "backend": "best",
    "seed": 1986
  },
  "defaults": { "speed": 1.0, "num_steps": 10 }
}
EOF

echo
echo "CosyVoice3 installed at $DEST"
echo "Reload the 模型 page (or restart the server) to pick it up."
