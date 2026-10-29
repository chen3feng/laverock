#!/usr/bin/env bash
# Download the default MVP engine: ZipVoice distill INT8 (zh/en, ~104 MB).
# The archive contains ONNX models, lexicon/token data, and test reference
# clips. We additionally write engine.json — the manifest the server scans.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init

MODEL="sherpa-onnx-zipvoice-distill-int8-zh-en-emilia"
URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/${MODEL}.tar.bz2"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if [ -f "$MODELS_DIR/$MODEL/encoder.int8.onnx" ]; then
  echo "Model archive already extracted"
else
  # The tarball expands to the $MODEL directory under MODELS_DIR.
  lv_extract_tarball "$URL" "$TMP/model.tar.bz2" "$MODELS_DIR" \
    "Downloading main model (1/3) · ~104 MB"
fi

# The int8 archive omits the shared vocoder and the English lexicon;
# both are separate upstream artifacts.
cd "$MODELS_DIR/$MODEL"
if [ ! -f vocos_24khz.onnx ]; then
  lv_stage "Downloading vocoder (2/3)"
  lv_download \
    https://github.com/k2-fsa/sherpa-onnx/releases/download/vocoder-models/vocos_24khz.onnx \
    vocos_24khz.onnx
fi
if [ ! -f lexicon-en-us.txt ]; then
  lv_stage "Downloading English lexicon (3/3)"
  lv_download \
    https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/lexicon-en-us.txt \
    lexicon-en-us.txt
fi

# sherpa-onnx's lexicon parser understands neither `#` comments nor the
# `word || phones` separator in the upstream file, and the file's IPA
# phones (ɑː, ɪˌ, …) do not all exist in this model's tokens.txt. Keep
# only phones the model knows; that silences ~160k warnings and keeps
# every entry consistent with the token inventory.
if [ ! -f lexicon-en-us.clean.txt ]; then
  awk 'NR==FNR { tok[$1]=1; next }
       /^[[:space:]]*#/ { next }
       { gsub(/\|\|/, " ");
         w=tolower($1); out=""; n=0;
         for (i=2; i<=NF; i++) if ($i in tok) { out=out" "$i; n++ }
         if (n>0 && !seen[w]++) print w out }' \
    tokens.txt lexicon-en-us.txt > lexicon-en-us.clean.txt
fi
cd - >/dev/null

lv_stage "Writing engine manifest"
cat > "$MODELS_DIR/$MODEL/engine.json" <<EOF
{
  "id": "$MODEL",
  "kind": "zipvoice",
  "name": "ZipVoice Distill INT8 (Chinese/English)",
  "languages": ["zh", "en"],
  "capabilities": ["tts", "clone"],
  "zipvoice": {
    "tokens": "tokens.txt",
    "encoder": "encoder.int8.onnx",
    "decoder": "decoder.int8.onnx",
    "vocoder": "vocos_24khz.onnx",
    "data_dir": "espeak-ng-data",
    "lexicon": "lexicon.txt,lexicon-en-us.clean.txt",
    "feat_scale": 0.1,
    "t_shift": 0.5,
    "target_rms": 0.1,
    "guidance_scale": 1.0,
    "num_threads": 4
  },
  "defaults": { "speed": 1.0, "num_steps": 4 }
}
EOF

echo "Installed to $MODELS_DIR/$MODEL"
echo "Try it: cargo run -p laverock-server"
