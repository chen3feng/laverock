#!/usr/bin/env bash
# Install the Qwen3-TTS engine: an isolated Python venv with the official
# `qwen-tts` package, the model weights, and the laverock runner script.
#
#   QWEN3TTS_VARIANT=base|custom|design   (default: base = voice cloning)
#   QWEN3TTS_MODEL=Qwen/...               override the model repo id
#
# Disk: torch and friends ~2-3 GB, model 1.5-4 GB depending on the variant.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init

VENV="$TOOLS/qwen-tts-venv"
VARIANT="${QWEN3TTS_VARIANT:-base}"
# base keeps its historical directory/id; the other modes sit alongside.
case "$VARIANT" in
  base)   ENGINE_ID="qwen3tts"; DEST="$MODELS_DIR/qwen3tts" ;;
  custom) ENGINE_ID="qwen3tts-custom"; DEST="$MODELS_DIR/qwen3tts-custom" ;;
  design) ENGINE_ID="qwen3tts-design"; DEST="$MODELS_DIR/qwen3tts-design" ;;
esac

case "$VARIANT" in
  base)
    MODEL="${QWEN3TTS_MODEL:-Qwen/Qwen3-TTS-12Hz-0.6B-Base}"
    MODE="clone"; SPEAKER=""; EXTRA_CAPS='"clone"'; NAME="Qwen3-TTS 0.6B (voice clone)"
    ;;
  custom)
    MODEL="${QWEN3TTS_MODEL:-Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice}"
    MODE="custom_voice"; SPEAKER="Vivian"; EXTRA_CAPS='"preset-voices","instruct"'
    NAME="Qwen3-TTS 0.6B (custom voice)"
    ;;
  design)
    MODEL="${QWEN3TTS_MODEL:-Qwen/Qwen3-TTS-12Hz-1.7B-VoiceDesign}"
    MODE="voice_design"; SPEAKER=""
    EXTRA_CAPS='"voice-design","instruct"'; NAME="Qwen3-TTS 1.7B (voice design)"
    ;;
  *)
    echo "QWEN3TTS_VARIANT must be base, custom or design" >&2
    exit 1
    ;;
esac

# torch has no wheels for the newest Python, so pick a supported one.
PYTHON="$(lv_find_python "${QWEN3TTS_PYTHON:-}")" || {
  echo "a Python 3.10-3.12 interpreter is required" >&2; exit 1; }
echo "Using $("$PYTHON" --version 2>&1) ($PYTHON)"
mkdir -p "$DEST" "$TOOLS"

# 1. Isolated environment — Qwen's torch/transformers pins stay here.
#    lv_prepare_python_venv creates the venv, upgrades pip, and pre-installs
#    CUDA torch on Windows+NVIDIA; VPY then points at its interpreter.
lv_prepare_python_venv "$VENV" "$PYTHON"
lv_stage "安装 qwen-tts 依赖（约 2 GB，首次较慢）"
"$VPY" -m pip install --upgrade "qwen-tts"

# 2. Weights. The model repo already bundles its speech tokenizer under
#    speech_tokenizer/ plus the text tokenizer, so download it whole into one
#    plain directory and load by path — no HF cache and fully offline.
#    resolve-URL GET/range requests (not `hf download`) keep this working
#    through hf-mirror.com, whose redirects huggingface_hub 0.36 rejects.
WEIGHTS="$DEST/weights"
lv_stage "下载模型与分词器（首次约 2.5 GB）"
lv_hf_download_repo "$MODEL" "$WEIGHTS" 8

# 3. Runner script.
cp "$SCRIPT_DIR/qwen3tts_runner.py" "$DEST/qwen3tts_runner.py"

# 4. Manifest (absolute paths are allowed and used as-is).
#    custom_voice exposes the nine preset speakers; voice_design exposes
#    a free-text voice description field.
PY="$(lv_native_path "$VPY")"
PARAMETERS=""
if [ "$MODE" = "custom_voice" ]; then
  PARAMETERS='"parameters": [
    {
      "name": "speaker", "label": "预置音色 / Preset speaker", "type": "choice",
      "options": [
        { "value": "Vivian",   "label": "Vivian · 中文女声（明亮年轻） / bright young female" },
        { "value": "Serena",   "label": "Serena · 中文女声（温柔） / warm gentle female" },
        { "value": "Uncle_Fu", "label": "Uncle Fu · 中文男声（低沉醇厚） / low mellow male" },
        { "value": "Dylan",    "label": "Dylan · 北京男声（清透自然） / clear Beijing male" },
        { "value": "Eric",     "label": "Eric · 成都男声（微哑活力） / lively husky male" },
        { "value": "Ryan",     "label": "Ryan · English male (dynamic rhythm)" },
        { "value": "Aiden",    "label": "Aiden · English male (sunny American)" },
        { "value": "Ono_Anna", "label": "Ono Anna · 日本語女性（轻盈灵动） / playful Japanese female" },
        { "value": "Sohee",    "label": "Sohee · 한국어 여성（温暖感性） / warm Korean female" }
      ],
      "default": "Vivian", "hint": "九个内置预置音色，无需参考音频。"
    },
    {
      "name": "instruct", "label": "语气指令 / Instruct", "type": "text_area",
      "default": "", "hint": "可选，用自然语言调整语气，如「用兴奋的语气」。"
    }
  ],'
elif [ "$MODE" = "voice_design" ]; then
  PARAMETERS='"parameters": [
    {
      "name": "instruct", "label": "音色描述 / Voice description", "type": "text_area",
      "default": "", "hint": "必填：用自然语言描述想要的音色，如「一位温柔的年轻女性，语速稍慢」。"
    }
  ],'
fi

cat > "$DEST/engine.json" <<EOF
{
  "id": "$ENGINE_ID",
  "kind": "qwen3tts",
  "name": "$NAME",
  "languages": ["zh", "en", "ja", "ko", "de", "fr", "ru", "pt", "es", "it"],
  "capabilities": ["tts", $EXTRA_CAPS],
  "qwen3tts": {
    "python": "$PY",
    "script": "qwen3tts_runner.py",
    "model": "weights",
    "mode": "$MODE",
    "speaker": "$SPEAKER",
    "language": "Auto",
    "device": "auto",
    "dtype": "auto",
    "attn": "sdpa",
    "env": { "PYTHONIOENCODING": "utf-8" }
  },
  $PARAMETERS
  "defaults": { "speed": 1.0, "num_steps": 1 }
}
EOF

echo
echo "Qwen3-TTS ($VARIANT) installed at $DEST"
echo "Restart the server to pick it up."
