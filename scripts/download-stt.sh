#!/usr/bin/env bash
# Install the SenseVoice Small INT8 speech-to-text model for sherpa-onnx.
# Reference clips are transcribed on device (zh/en/ja/ko/yue, with inverse
# text normalization), no Python required.
#
# Primary source is the sherpa-onnx GitHub release (one tarball). When it
# is unreachable (or STT_SOURCE=hf is set), the two needed files are
# fetched from the HuggingFace repo instead; HF_ENDPOINT is honored
# (e.g. https://hf-mirror.com).
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"

# Canonical model id/dir matches the GitHub asset naming; the HuggingFace
# repo spells it differently again: it omits the -int8 suffix and holds
# model.int8.onnx (239 MB) next to the full-precision weights.
MODEL="sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17-int8"
ASSET="sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
HF_REPO="csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17"
GH_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/${ASSET}.tar.bz2"

lv_init "$MODEL"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

download_github() {
  # Extract into TMP: the archive's top-level directory is named $ASSET,
  # not $MODEL, so locate the model dir and move it into place.
  lv_extract_tarball "$GH_URL" "$TMP/model.tar.bz2" "$TMP/extract" \
    "Downloading SenseVoice INT8 tarball (~234 MB)" || return 1
  local src
  src="$(find "$TMP/extract" -maxdepth 2 -type f -name model.int8.onnx -print -quit | xargs -r dirname)"
  [ -n "$src" ] || return 1
  rm -rf "$DEST"
  mv "$src" "$DEST"
}

# HF fallback: only the two inference files; the repo's docs and test
# clips are skipped. lv_hf_download_repo lists via the tree API, resumes
# per file, and never uses --retry-all-errors (which double-appends a
# fixed curl range after a mid-transfer reset).
download_hf() {
  lv_stage "Fetching files from HuggingFace ($(lv_hf_base))"
  lv_hf_download_repo "$HF_REPO" "$DEST" 8 model.int8.onnx tokens.txt
}

if [ "${STT_SOURCE:-}" = "hf" ]; then
  download_hf
elif ! download_github; then
  echo "GitHub tarball unavailable, falling back to per-file HuggingFace download" >&2
  download_hf
fi

# Fail loudly before writing a manifest pointing at missing files.
for f in model.int8.onnx tokens.txt; do
  if [ ! -s "$DEST/$f" ]; then
    echo "missing required file: $f" >&2
    exit 1
  fi
done

lv_stage "Writing engine manifest"
cat > "$DEST/engine.json" <<EOF
{
  "id": "$MODEL",
  "kind": "stt",
  "name": "SenseVoice Small (INT8)",
  "languages": ["zh", "en", "ja", "ko", "yue"],
  "capabilities": ["stt"],
  "stt": {
    "engine": "sense_voice",
    "model": "model.int8.onnx",
    "tokens": "tokens.txt",
    "language": "auto",
    "use_itn": true,
    "num_threads": 4
  },
  "defaults": { "speed": 1.0, "num_steps": 4 }
}
EOF

echo "Installed to $DEST"
echo "Try it: record or upload a reference clip in the app and press transcribe"
