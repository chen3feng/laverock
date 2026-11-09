#!/usr/bin/env bash
# Install the Qwen3-ASR 0.6B INT8 speech-to-text model for sherpa-onnx.
# It covers 30 languages and 20+ Chinese dialects, and unlike the FireRed
# builds it keeps punctuation. It returns no token timestamps, so subtitle
# cues fall back to splitting the clip on pauses. On-device, no Python.
#
# Primary source is the sherpa-onnx GitHub release (one tarball). When it
# is unreachable (or STT_SOURCE=hf is set), the needed files are fetched
# from the HuggingFace repo instead; HF_ENDPOINT is honored (e.g.
# https://hf-mirror.com).
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"

MODEL="sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25"
HF_REPO="csukuangfj/${MODEL}"
GH_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/${MODEL}.tar.bz2"

lv_init "$MODEL"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

download_github() {
  lv_extract_tarball "$GH_URL" "$TMP/model.tar.bz2" "$TMP/extract" \
    "Downloading Qwen3-ASR 0.6B INT8 tarball (~837 MB)" || return 1
  local src
  src="$(find "$TMP/extract" -maxdepth 2 -type f -name conv_frontend.onnx -print -quit | xargs -r dirname)"
  [ -n "$src" ] || return 1
  rm -rf "$DEST"
  mv "$src" "$DEST"
}

# HF fallback: only the inference files; docs and test clips are skipped.
download_hf() {
  lv_stage_key stage_fetch_listing "base=$(lv_hf_base)"
  lv_hf_download_repo "$HF_REPO" "$DEST" 8 \
    conv_frontend.onnx encoder.int8.onnx decoder.int8.onnx \
    tokenizer/merges.txt tokenizer/tokenizer_config.json tokenizer/vocab.json
}

if [ "${STT_SOURCE:-}" = "hf" ]; then
  download_hf
elif ! download_github; then
  echo "GitHub tarball unavailable, falling back to per-file HuggingFace download" >&2
  download_hf
fi

# Fail loudly before writing a manifest pointing at missing files.
for f in conv_frontend.onnx encoder.int8.onnx decoder.int8.onnx \
  tokenizer/merges.txt tokenizer/tokenizer_config.json tokenizer/vocab.json; do
  if [ ! -s "$DEST/$f" ]; then
    echo "missing required file: $f" >&2
    exit 1
  fi
done

lv_stage_key stage_write_manifest
cat > "$DEST/engine.json" <<EOF
{
  "id": "stt-qwen3-asr",
  "kind": "stt",
  "name": "Qwen3-ASR 0.6B (INT8)",
  "languages": ["zh", "en", "yue", "ja", "ko", "fr", "de", "es", "pt", "it",
                "ru", "ar", "hi", "th", "vi", "id", "tr", "ms", "nl", "sv",
                "da", "fi", "pl", "cs", "fil", "fa", "el", "hu", "mk", "ro"],
  "capabilities": ["stt"],
  "license": {
    "name": "Apache-2.0",
    "url": "https://github.com/QwenLM/Qwen3-ASR"
  },
  "stt": {
    "engine": "qwen3_asr",
    "conv_frontend": "conv_frontend.onnx",
    "encoder": "encoder.int8.onnx",
    "decoder": "decoder.int8.onnx",
    "tokenizer": "tokenizer",
    "num_threads": 4
  },
  "defaults": { "speed": 1.0, "num_steps": 4 }
}
EOF

echo "Installed to $DEST"
echo "Pick it on the Transcribe page; it keeps punctuation"
