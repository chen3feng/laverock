#!/usr/bin/env bash
# Install MiniMax Music 3 as a Laverock music engine.
#
# MiniMax Music 3 generates a song from a style caption and required
# lyrics through audio.cpp's `gen` task. It is a component package (language
# model, depth decoder, condition encoder, flow transformer, vocoder), so
# `--model` is the directory. The weights use the MiniMax-Music3 Community
# License.
#
#   MINIMAX_MUSIC_QUANT=q4_0|q8_0   component precision (default q4_0)
#   MINIMAX_MUSIC_VERSION=...       pin a different audio.cpp release
#
# Disk: ~8.4 GB (Q4_0), ~13.4 GB (Q8_0), plus the shared runtime.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init minimax-music3

AC_VERSION="${MINIMAX_MUSIC_VERSION:-v0.9.4}"
case "${MINIMAX_MUSIC_QUANT:-q4_0}" in
  q8|q8_0)
    QUANT="q8_0"
    QLABEL="Q8_0"
    SIZE="~13.4 GB"
    ;;
  q4|q4_0|*)
    QUANT="q4_0"
    QLABEL="Q4_0"
    SIZE="~8.4 GB"
    ;;
esac
NAME="MiniMax Music 3 ($QLABEL)"

# 1. Shared audio.cpp runtime.
BIN="$(lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR")"
BACKEND="$(lv_audiocpp_backend)"

# 2. Component package from Hugging Face. The depth decoder stays Q8_0 in
# both mixes; only the language model and flow transformer follow the
# requested precision.
REPO="audio-cpp/MiniMax-Music3-GGUF"
for rel in \
  "config.json" \
  "config/condition_encoder.json" \
  "config/language_model.json" \
  "config/rvq_depth_decoder.json" \
  "config/transformer.json" \
  "config/vocoder.json" \
  "tokenizer/tokenizer.json" \
  "tokenizer/tokenizer_config.json" \
  "condition_encoder.gguf" \
  "vocoder.gguf" \
  "language_model_${QUANT}.gguf" \
  "rvq_depth_decoder_q8_0.gguf" \
  "transformer_${QUANT}.gguf"
do
  lv_hf_download_file "$REPO" "$rel" "$DEST/$rel" 8 \
    "Downloading MiniMax Music 3 ($SIZE)"
done

# 3. Manifest. The component directory is the model.
cat > "$DEST/engine.json" <<EOF
{
  "id": "minimax-music3",
  "kind": "music",
  "name": "$NAME",
  "languages": ["zh", "en"],
  "capabilities": ["music", "lyrics"],
  "license": { "name": "MiniMax-Music3 Community License", "url": "https://huggingface.co/$REPO", "noncommercial": false },
  "music": {
    "binary": "$BIN",
    "model": ".",
    "backend": "$BACKEND",
    "family": "minimax_music3",
    "task": "gen",
    "seed": 42
  },
  "defaults": { "speed": 1.0, "num_steps": 30 }
}
EOF

echo
echo "$NAME installed at $DEST"
echo "Reload the Models page (or restart the server) to pick it up."
