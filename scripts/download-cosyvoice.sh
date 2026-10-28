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

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init cosyvoice

AC_VERSION="${DOTS_VERSION:-v0.9.2-drama3-base-small}"
PRECISION="${COSYVOICE_PRECISION:-q8_0}"

case "$PRECISION" in
  f32) GGUF_NAME="cosyvoice3-f32.gguf"; SIZE_NOTE="约 7 GB" ;;
  *)   GGUF_NAME="cosyvoice3-q8_0.gguf"; SIZE_NOTE="约 2.3 GB" ;;
esac
GGUF="$DEST/$GGUF_NAME"

# 1. Shared audio.cpp runtime.
BIN="$(lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR")"
BACKEND="$(lv_audiocpp_backend)"

# 2. Weights.
lv_hf_download_file audio-cpp/audio.cpp-gguf \
  "CosyVoice3-GGUF/$GGUF_NAME" "$GGUF" 8 \
  "下载 CosyVoice3 权重（${SIZE_NOTE}）"

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
    "backend": "$BACKEND",
    "seed": 1986
  },
  "defaults": { "speed": 1.0, "num_steps": 10 }
}
EOF

echo
echo "CosyVoice3 installed at $DEST"
echo "Reload the 模型 page (or restart the server) to pick it up."
