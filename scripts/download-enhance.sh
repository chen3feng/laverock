#!/usr/bin/env bash
# Voice cleanup for the recording library: remove room noise and
# background music from takes before they become voice references.
#
#   ENHANCE_DENOISE_URL=...   another sherpa-onnx enhancement model
#   AC_VERSION=...            pin the audio.cpp runtime release
#
# Disk: ~10 MB denoiser + ~164 MB vocal separator, plus the shared
# audio.cpp runtime, downloaded only when no audio.cpp engine installed
# it already.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init enhance

# 1. Denoiser (sherpa-onnx GTCRN, 16 kHz).
DENOISE_URL="${ENHANCE_DENOISE_URL:-https://github.com/k2-fsa/sherpa-onnx/releases/download/speech-enhancement-models/gtcrn_simple.onnx}"
lv_download "$DENOISE_URL" "$DEST/gtcrn_simple.onnx"

# 2. Vocal separator (BS-RoFormer Q8_0, 44.1 kHz).
lv_hf_download_file audio-cpp/audio.cpp-gguf \
  "BS-RoFormer-ep368-GGUF/bs-roformer-ep368-q8_0.gguf" \
  "$DEST/bs-roformer.gguf" 8 "Downloading the vocal separator (~164 MB)"

# 3. The separator runs through audio.cpp's CLI, shared with the TTS
#    engines. Install the runtime only if it is not there yet.
AC_VERSION="${AC_VERSION:-v0.9.2-drama3-base-small}"
lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR" > /dev/null

echo
echo "Voice cleanup installed at $DEST"
echo "Reload the Captures page to use it."
