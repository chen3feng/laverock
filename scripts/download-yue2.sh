#!/usr/bin/env bash
# Install YuE2 3B as a Laverock music engine.
#
# YuE2 generates a song from a style prompt and lyrics through audio.cpp's
# `gen` task. The style prompt is the Music page caption and the lyrics are
# its own field; chain-of-thought planning (`cot`) improves structure at the
# cost of speed. The weights are CC BY-NC 4.0, so this is not the default
# music engine.
#
#   YUE2_QUANT=q8_0|q4_0|bf16   main model precision (default q8_0)
#   YUE2_VAE=f16|f32            VAE precision (default f16)
#   YUE2_VERSION=...            pin a different audio.cpp release
#
# Disk: ~4.5 GB (Q8_0), ~2.9 GB (Q4_0), ~7.8 GB (BF16), plus the shared
# runtime.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init yue2

AC_VERSION="${YUE2_VERSION:-v0.9.4}"
case "${YUE2_QUANT:-q8_0}" in
  q4|q4_0)
    QUANT="q4_0"
    QLABEL="Q4_0"
    SIZE="~2.9 GB"
    ;;
  bf16)
    QUANT="bf16"
    QLABEL="BF16"
    SIZE="~7.8 GB"
    ;;
  q8|q8_0|*)
    QUANT="q8_0"
    QLABEL="Q8_0"
    SIZE="~4.5 GB"
    ;;
esac
case "${YUE2_VAE:-f16}" in
  f32) VAE="f32"; VLABEL="F32" ;;
  f16|*) VAE="f16"; VLABEL="F16" ;;
esac
NAME="YuE2 3B ($QLABEL, VAE $VLABEL)"

# 1. Shared audio.cpp runtime.
BIN="$(lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR")"
BACKEND="$(lv_audiocpp_backend)"

# 2. Weights, VAE and sidecars from Hugging Face.
REPO="audio-cpp/Yue2-3B-GGUF"
for rel in \
  "yue2-3b-${QUANT}.gguf" \
  "yue2-vae-${VAE}.gguf" \
  "sidecars/yue2-model-config.json" \
  "sidecars/yue2-generation-config.json" \
  "sidecars/yue2-qwen.tiktoken" \
  "sidecars/yue2-vae-config.json"
do
  lv_hf_download_file "$REPO" "$rel" "$DEST/$rel" 8 "Downloading YuE2 ($SIZE)"
done

# 3. Manifest. The package directory is the model, so its files sit
# directly in DEST.
cat > "$DEST/engine.json" <<EOF
{
  "id": "yue2",
  "kind": "music",
  "name": "$NAME",
  "languages": ["en"],
  "capabilities": ["music"],
  "license": { "name": "CC BY-NC 4.0", "url": "https://huggingface.co/$REPO", "noncommercial": true },
  "music": {
    "binary": "$BIN",
    "model": ".",
    "backend": "$BACKEND",
    "family": "yue2",
    "task": "gen",
    "seed": 42
  },
  "defaults": { "speed": 1.0, "num_steps": 8 }
}
EOF

echo
echo "$NAME installed at $DEST"
echo "Reload the Models page (or restart the server) to pick it up."
