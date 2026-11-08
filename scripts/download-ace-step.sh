#!/usr/bin/env bash
# Install ACE-Step 1.5 (text-to-music / lyrics-to-song) as a Laverock engine.
#
# ACE-Step generates a complete song from a style caption and optional
# lyrics through audio.cpp's `gen` task: 48 kHz stereo, 50+ languages, MIT
# licensed. It runs on the same native runtime as the speech engines, so
# there is no Python/PyTorch environment to install.
#
#   ACE_STEP_QUANT=q8_0    smaller ~6.2 GB export. The planner occasionally
#                          fails to sample from the Q8_0 build; BF16 is the
#                          reliable default.
#   ACE_STEP_LORA=<path>   optional LoRA / style adapter for the DiT
#   ACE_STEP_LORA_SCALE=1  adapter strength
#   ACE_STEP_VERSION=...   pin a different audio.cpp release
#
# Disk: BF16 GGUF ~10 GB (Q8_0 ~6.2 GB), plus the shared ~40 MB runtime.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init ace-step

AC_VERSION="${ACE_STEP_VERSION:-v0.9.4}"
case "${ACE_STEP_QUANT:-bf16}" in
  q8|q8_0)
    FLAVOR="q8_0"
    SIZE="~6.2 GB"
    NAME="ACE-Step 1.5 Turbo (Q8_0)"
    ;;
  bf16|*)
    FLAVOR="bf16"
    SIZE="~10 GB"
    NAME="ACE-Step 1.5 Turbo (BF16)"
    ;;
esac

# The GGUF package is a directory of one file per variant; audio.cpp takes
# the directory (not the file) as --model and picks the DiT from the name.
PACKAGE="ACE-Step1.5-GGUF/turbo/ace-step-1.5-turbo-${FLAVOR}.gguf"
GGUF="$DEST/turbo/$(basename "$PACKAGE")"
mkdir -p "$DEST/turbo"

# 1. Shared audio.cpp runtime (also used by the speech engines).
BIN="$(lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR")"
BACKEND="$(lv_audiocpp_backend)"

# 2. Weights from Hugging Face, in parallel ranged requests.
lv_hf_download_file audio-cpp/audio.cpp-gguf "$PACKAGE" "$GGUF" 8 \
  "Downloading ACE-Step weights ($SIZE)"

# 3. Optional LoRA / style adapter. audio.cpp merges the adapter while
# loading the DiT; the path is relative to the model directory (or
# absolute). It is a session option, so it applies when the engine starts.
SESSION='{}'
if [ -n "${ACE_STEP_LORA:-}" ]; then
  LORA_PATH=$(printf '%s' "$ACE_STEP_LORA" | sed 's/\\/\\\\/g; s/"/\\"/g')
  LORA_SCALE="${ACE_STEP_LORA_SCALE:-1.0}"
  SESSION=$(printf '{"ace_step.lora": "%s", "ace_step.lora_scale": "%s"}' \
    "$LORA_PATH" "$LORA_SCALE")
fi

# 4. Manifest.
cat > "$DEST/engine.json" <<EOF
{
  "id": "ace-step",
  "kind": "music",
  "name": "$NAME",
  "languages": ["zh", "en", "+ 50 languages"],
  "capabilities": ["music"],
  "license": { "name": "MIT", "url": "https://huggingface.co/ACE-Step/Ace-Step1.5", "noncommercial": false },
  "music": {
    "binary": "$BIN",
    "model": "turbo",
    "backend": "$BACKEND",
    "family": "ace_step",
    "task": "gen",
    "seed": 42,
    "load_options": { "ace_step.dit_model_path": "acestep-v15-turbo" },
    "session_options": $SESSION
  },
  "defaults": { "speed": 1.0, "num_steps": 8 }
}
EOF

echo
echo "ACE-Step installed at $DEST"
echo "Reload the Models page (or restart the server) to pick it up."
