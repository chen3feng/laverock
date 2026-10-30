#!/usr/bin/env bash
# Install IndexTTS-2 (bilibili) as a Laverock engine.
#
# IndexTTS-2 is a Chinese/English zero-shot clone model: a GPT-style token
# model conditions on a reference clip and a flow-matching vocoder emits
# 22.05 kHz audio. Instead of the official Python package (PyTorch plus
# ~9 GB of weights and auxiliary models) we run audio.cpp's GGUF export on
# its native runtime, which keeps the model resident and needs no Python.
#
#   INDEXTTS_PACKAGE=IndexTTS2-GGUF/index-tts2-f16.gguf   another precision
#   INDEXTTS_VERSION=...                                  pin another audio.cpp release
#
# The runtime is installed once under <tools>/audiocpp and shared with the
# dots.tts, CosyVoice and VoxCPM2 engines.
#
# Disk: GGUF 3.6 GB (Q8_0), plus a one-time ~80 MB runtime.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init indextts2

AC_VERSION="${INDEXTTS_VERSION:-v0.9.2-drama3-base-small}"
PACKAGE="${INDEXTTS_PACKAGE:-IndexTTS2-GGUF/index-tts2-q8_0.gguf}"
GGUF="$DEST/$(basename "$PACKAGE")"

# 1. Shared audio.cpp runtime (also used by dots, CosyVoice and VoxCPM2).
BIN="$(lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR")"
BACKEND="$(lv_audiocpp_backend)"

# 2. Weights (Q8_0 GGUF) from Hugging Face, in parallel ranged requests.
lv_hf_download_file audio-cpp/audio.cpp-gguf "$PACKAGE" "$GGUF" 8 \
  "Downloading model weights (Q8_0, ~3.6 GB)"

# 3. Manifest. IndexTTS-2 has no preset voices, and the transcript of the
#    reference clip is not used: the audio alone drives the clone.
lv_stage_key stage_write_manifest
cat > "$DEST/engine.json" <<EOF
{
  "id": "indextts2",
  "kind": "indextts2",
  "name": "IndexTTS-2 (Q8_0)",
  "languages": ["zh", "en"],
  "capabilities": ["tts", "clone"],
  "indextts2": {
    "binary": "$BIN",
    "model": "$(basename "$GGUF")",
    "backend": "$BACKEND",
    "seed": 42
  },
  "parameters": [
    {
      "name": "language", "label": {"en": "Text language", "zh": "文本语言"}, "type": "choice",
      "options": [
        { "value": "auto", "label": {"en": "Detect from text", "zh": "按文本自动判断"} },
        { "value": "zh", "label": {"en": "Chinese 中文", "zh": "中文 Chinese"} },
        { "value": "en", "label": {"en": "English 英语", "zh": "英语 English"} }
      ],
      "default": "auto",
      "hint": {"en": "Guides the text front-end; automatic picks Chinese for Han text and English otherwise.", "zh": "引导文本前端；自动时含汉字按中文读，否则按英文读。"}
    },
    {
      "name": "num_beams", "label": {"en": "Decoding", "zh": "解码方式"}, "type": "choice",
      "options": [
        { "value": "3", "label": {"en": "Beam search", "zh": "束搜索（质量优先）"} },
        { "value": "1", "label": {"en": "Greedy, about 25% faster", "zh": "贪心（快约 25%）"} }
      ],
      "default": "3",
      "hint": {"en": "Beam search reads slightly better; greedy skips the extra beams.", "zh": "束搜索韵律更稳；贪心省掉多余搜索，明显更快。"}
    }
  ],
  "defaults": { "speed": 1.0, "num_steps": 1 }
}
EOF

echo
echo "IndexTTS-2 installed at $DEST"
echo "Reload the Models page (or restart the server) to pick it up."
