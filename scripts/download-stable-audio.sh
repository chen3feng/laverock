#!/usr/bin/env bash
# Install Stable Audio 3 as a Laverock music/sound-effect engine.
#
# Stable Audio 3 runs through audio.cpp's `gen` task on the shared native
# runtime. The Small Music build does text-to-music, init-audio and
# inpainting; Small SFX is tuned for short sound effects; Medium is the
# larger music model.
#
#   STABLE_AUDIO_VARIANT=music|sfx|medium   package to install (default music)
#   STABLE_AUDIO_QUANT=q8_0|f16             weight precision (default q8_0)
#   STABLE_AUDIO_VERSION=...                pin a different audio.cpp release
#
# Disk: ~1.7 GB (Small, Q8_0), ~3.6 GB (Medium, Q8_0), plus the shared
# runtime. The controls are built into Laverock for the `stable_audio`
# family, so the manifest only names the weights.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"

case "${STABLE_AUDIO_VARIANT:-music}" in
  sfx)
    DIR="stable-audio-sfx"
    PACKAGE="Stable-Audio-3-Small-SFX-GGUF/stable-audio-3-small-sfx"
    NAME="Stable Audio 3 Small SFX"
    ;;
  medium)
    DIR="stable-audio-medium"
    PACKAGE="Stable-Audio-3-Medium-GGUF/stable-audio-3-medium"
    NAME="Stable Audio 3 Medium"
    ;;
  music|*)
    DIR="stable-audio-music"
    PACKAGE="Stable-Audio-3-Small-Music-GGUF/stable-audio-3-small-music"
    NAME="Stable Audio 3 Small Music"
    ;;
esac
lv_init "$DIR"

AC_VERSION="${STABLE_AUDIO_VERSION:-v0.9.4}"
case "${STABLE_AUDIO_QUANT:-q8_0}" in
  f16) FLAVOR="f16" ;;
  q8|q8_0|*) FLAVOR="q8_0" ;;
esac

PACKAGE="$PACKAGE-${FLAVOR}.gguf"
GGUF="$DEST/$(basename "$PACKAGE")"

# 1. Shared audio.cpp runtime (also used by the speech and ACE-Step engines).
BIN="$(lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR")"
BACKEND="$(lv_audiocpp_backend)"

# 2. Weights from Hugging Face, in parallel ranged requests.
lv_hf_download_file audio-cpp/audio.cpp-gguf "$PACKAGE" "$GGUF" 8 \
  "Downloading $NAME weights"

# 3. Manifest. Sound effects are short, so their duration default is 8 s;
# every other control is shared with the music builds.
EXTRA=""
if [ "$DIR" = "stable-audio-sfx" ]; then
  EXTRA=',
  "parameters": [
    {
      "name": "duration_sec",
      "label": { "en": "Duration (seconds)", "zh": "时长（秒）" },
      "type": "float",
      "min": 1,
      "max": 300,
      "step": 1,
      "default": 8,
      "hint": {
        "en": "Requested clip length; longer clips take proportionally longer.",
        "zh": "期望的音频长度；越长耗时越久。"
      }
    }
  ]'
fi

cat > "$DEST/engine.json" <<EOF
{
  "id": "$DIR",
  "kind": "music",
  "name": "$NAME",
  "languages": ["en"],
  "capabilities": ["music"],
  "license": { "name": "Stability AI Community License", "url": "https://huggingface.co/stabilityai/stable-audio-3-small-music", "noncommercial": false },
  "music": {
    "binary": "$BIN",
    "model": "$(basename "$GGUF")",
    "backend": "$BACKEND",
    "family": "stable_audio",
    "task": "gen",
    "seed": 42
  },
  "defaults": { "speed": 1.0, "num_steps": 8 }$EXTRA
}
EOF

echo
echo "$NAME installed at $DEST"
echo "Reload the Models page (or restart the server) to pick it up."
