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

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=laverock-lib.sh
. "$SCRIPT_DIR/laverock-lib.sh"

# Canonical model id/dir matches the HuggingFace repo naming; the GitHub
# release asset and its extracted directory spell it differently.
MODEL="sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17-int8"
ASSET="sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
HF_REPO="csukuangfj/$MODEL"
GH_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/${ASSET}.tar.bz2"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODELS_DIR="${LAVEROCK_MODELS_DIR:-$ROOT/models}"
DEST="$MODELS_DIR/$MODEL"
mkdir -p "$DEST"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

download_github() {
  lv_stage "Downloading SenseVoice INT8 tarball (~234 MB)"
  lv_download "$GH_URL" "$TMP/model.tar.bz2" 1
  [ -s "$TMP/model.tar.bz2" ] || return 1
  lv_stage "Extracting"
  mkdir -p "$TMP/extract"
  tar -xjf "$TMP/model.tar.bz2" -C "$TMP/extract"
  # The archive's top-level directory is named $ASSET, not $MODEL.
  local src
  src="$(find "$TMP/extract" -maxdepth 2 -type f -name model.int8.onnx -print -quit | xargs -r dirname)"
  [ -n "$src" ] || return 1
  rm -rf "$DEST"
  mv "$src" "$DEST"
}

# The API JSON is flat; pull every rfilename without a JSON dependency.
download_hf() {
  local outfile="$1"
  local base="${HF_ENDPOINT:-https://huggingface.co}"
  base="${base%/}"
  lv_stage "Fetching file list from HuggingFace (${base})"
  local listing="$TMP/files.json"
  curl -sSL --fail --retry 3 --retry-delay 2 \
    "$base/api/models/$HF_REPO" -o "$listing"

  local files
  files="$(grep -oE '"rfilename":[[:space:]]*"[^"]*"' "$listing" \
    | sed 's/.*"rfilename":[[:space:]]*"//; s/"$//')"
  if [ -z "$files" ]; then
    echo "cannot list files of $HF_REPO" >&2
    return 1
  fi

  : > "$outfile"
  while IFS= read -r f; do
    f="${f%$'\r'}"
    [ -z "$f" ] && continue
    case "$f" in
      model.int8.onnx|tokens.txt) printf '%s\n' "$f" >> "$outfile" ;;
      *) continue ;;
    esac
  done <<EOF_LIST
$files
EOF_LIST
}

if [ "${STT_SOURCE:-}" = "hf" ]; then
  download_hf "$TMP/todo.txt"
elif ! download_github; then
  echo "GitHub tarball unavailable, falling back to per-file HuggingFace download" >&2
  download_hf "$TMP/todo.txt"
fi

# HF fallback: fetch needed files, up to 8 in parallel.
if [ -f "$TMP/todo.txt" ]; then
  base="${HF_ENDPOINT:-https://huggingface.co}"; base="${base%/}"
  total_files=$(grep -vc '^$' "$TMP/todo.txt" || true)
  n=0
  while IFS= read -r f; do
    f="${f%$'\r'}"
    [ -z "$f" ] && continue
    [ -s "$DEST/$f" ] && continue
    out="$DEST/$f"
    mkdir -p "$(dirname "$out")"
    url_f="${f// /%20}"
    (
      curl -sSL --fail --retry 5 --retry-delay 2 --retry-all-errors \
        "$base/$HF_REPO/resolve/main/$url_f" -o "$out.tmp"
      mv "$out.tmp" "$out"
    ) &
    n=$((n + 1))
    if [ $((n % 8)) -eq 0 ]; then
      lv_stage "Downloading STT model (${n}/${total_files})"
      wait || true
    fi
  done < "$TMP/todo.txt"
  wait || true
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
