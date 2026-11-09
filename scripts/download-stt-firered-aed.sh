#!/usr/bin/env bash
# Install the FireRedASR2 AED INT8 speech-to-text model for sherpa-onnx.
# It is the accuracy-first option for Mandarin/English and 20+ Chinese
# dialects. It returns neither token timestamps nor punctuation, so
# subtitle cues fall back to splitting the clip on pauses. On-device, no
# Python required.
#
# Primary source is the sherpa-onnx GitHub release (one tarball). When it
# is unreachable (or STT_SOURCE=hf is set), the needed files are fetched
# from the HuggingFace repo instead; HF_ENDPOINT is honored (e.g.
# https://hf-mirror.com).
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"

MODEL="sherpa-onnx-fire-red-asr2-zh_en-int8-2026-02-26"
HF_REPO="csukuangfj/${MODEL}"
GH_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/${MODEL}.tar.bz2"

lv_init "$MODEL"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

download_github() {
  lv_extract_tarball "$GH_URL" "$TMP/model.tar.bz2" "$TMP/extract" \
    "Downloading FireRedASR2 AED INT8 tarball (~799 MB)" || return 1
  local src
  src="$(find "$TMP/extract" -maxdepth 2 -type f -name encoder.int8.onnx -print -quit | xargs -r dirname)"
  [ -n "$src" ] || return 1
  rm -rf "$DEST"
  mv "$src" "$DEST"
}

# HF fallback: only the inference files; docs and test clips are skipped.
download_hf() {
  lv_stage_key stage_fetch_listing "base=$(lv_hf_base)"
  lv_hf_download_repo "$HF_REPO" "$DEST" 8 \
    encoder.int8.onnx decoder.int8.onnx tokens.txt
}

if [ "${STT_SOURCE:-}" = "hf" ]; then
  download_hf
elif ! download_github; then
  echo "GitHub tarball unavailable, falling back to per-file HuggingFace download" >&2
  download_hf
fi

# Fail loudly before writing a manifest pointing at missing files.
for f in encoder.int8.onnx decoder.int8.onnx tokens.txt; do
  if [ ! -s "$DEST/$f" ]; then
    echo "missing required file: $f" >&2
    exit 1
  fi
done

lv_stage_key stage_write_manifest
cat > "$DEST/engine.json" <<EOF
{
  "id": "stt-fireredasr2-aed",
  "kind": "stt",
  "name": "FireRedASR2 AED (INT8)",
  "languages": ["zh", "en"],
  "capabilities": ["stt"],
  "license": {
    "name": "Apache-2.0",
    "url": "https://github.com/FireRedTeam/FireRedASR2S"
  },
  "stt": {
    "engine": "fire_red_asr",
    "encoder": "encoder.int8.onnx",
    "decoder": "decoder.int8.onnx",
    "tokens": "tokens.txt",
    "num_threads": 4
  },
  "defaults": { "speed": 1.0, "num_steps": 4 }
}
EOF

echo "Installed to $DEST"
echo "Pick it on the Transcribe page; it is the most accurate option"
