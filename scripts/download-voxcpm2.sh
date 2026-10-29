#!/usr/bin/env bash
# Install the VoxCPM2 engine (2B, 30-language zero-shot TTS) as a Laverock
# engine.
#
# VoxCPM2 runs from a single audio.cpp GGUF package through the same native
# runtime dots.tts and CosyVoice3 use (Metal on macOS, CUDA/CPU on Windows and
# Linux), so there is no separate voxcpm2-cli to build and no Python stack.
# The earlier llama.cpp-omni runner is gone: it had no working Windows CUDA
# build and its static binaries assumed AVX2 (SIGILL on pre-Haswell CPUs).
#
#   VOXCPM2_PRECISION=q8_0|bf16   weights precision (default q8_0)
#   DOTS_VERSION=...              pin a different audio.cpp release tag
#
# Disk: ~2.8 GB Q8_0 weights, plus the one-time shared audio.cpp runtime.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init voxcpm2

AC_VERSION="${DOTS_VERSION:-v0.9.2-drama3-base-small}"
PRECISION="${VOXCPM2_PRECISION:-q8_0}"
PACKAGE="VoxCPM2-GGUF/voxcpm2-${PRECISION}.gguf"
GGUF="$DEST/$(basename "$PACKAGE")"

# 1. Shared audio.cpp runtime (CUDA on Windows machines with an NVIDIA GPU).
BIN="$(lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR")"
BACKEND="$(lv_audiocpp_backend)"

# 2. Weights from the audio.cpp GGUF repo, in parallel ranged requests.
lv_hf_download_file audio-cpp/audio.cpp-gguf "$PACKAGE" "$GGUF" 8 \
  "Downloading VoxCPM2 weights (${PRECISION}, ~2.8 GB)"

# 3. Manifest. VoxCPM2 supports prompt-less TTS as well as zero-shot cloning;
#    the runner adds --voice-ref only when the request carries a prompt.
cat > "$DEST/engine.json" <<EOF
{
  "id": "voxcpm2",
  "kind": "voxcpm2",
  "name": "VoxCPM2 2B (${PRECISION})",
  "languages": ["zh", "en", "ja", "ko", "ar", "es", "fr", "+ 30 languages"],
  "capabilities": ["tts", "clone", "voice-design"],
  "voxcpm2": {
    "binary": "$BIN",
    "model": "$(basename "$GGUF")",
    "backend": "$BACKEND",
    "seed": 42
  },
  "defaults": { "speed": 1.0, "num_steps": 10 }
}
EOF

echo
echo "VoxCPM2 installed at $DEST"
echo "Run the server and it will appear in the engine list."
