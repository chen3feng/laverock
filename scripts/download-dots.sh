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

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init dots

AC_VERSION="${DOTS_VERSION:-v0.9.2-drama3-base-small}"
MODEL_KIND="${DOTS_MODEL:-dots.tts-soar}"

case "$MODEL_KIND" in
  *mf*) PACKAGE="DotTTS-MF-GGUF/dots-tts-mf-q8_0.gguf" ;;
  *)    PACKAGE="DotTTS-SOAR-GGUF/dots-tts-soar-q8_0.gguf" ;;
esac
GGUF="$DEST/$(basename "$PACKAGE")"

# 1. Shared audio.cpp runtime (also used by the CosyVoice engine).
BIN="$(lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR")"
BACKEND="$(lv_audiocpp_backend)"

# 2. Weights (Q8_0 GGUF) from Hugging Face, in parallel ranged requests.
lv_hf_download_file audio-cpp/audio.cpp-gguf "$PACKAGE" "$GGUF" 8 \
  "Downloading model weights (Q8_0, ~3 GB)"

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
echo "Reload the Models page (or restart the server) to pick it up."
