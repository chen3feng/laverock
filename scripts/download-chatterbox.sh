#!/usr/bin/env bash
# Install the Chatterbox engine (Resemble AI) for laverock: an isolated
# Python venv with the official `chatterbox-tts` package, the multilingual
# T3 weights, and the laverock runner script.
#
#   CHATTERBOX_MODEL=ResembleAI/chatterbox   override the weight repo id
#
# chatterbox-tts 0.1.7 (the latest PyPI release) loads the multilingual T3
# checkpoint by a fixed filename (t3_mtl23ls_v2.safetensors) from the model
# directory; the newer GitHub `master` source exposes a `t3_model` argument
# (v3), but that is not yet packaged on PyPI. This script downloads the v2
# checkpoint the released package actually reads.
#
# Chatterbox is a 0.5B multilingual voice-clone TTS. It clones a reference
# clip and speaks any supported language (default Chinese). Weights are
# loaded from a local directory via `from_local`, so synthesis is offline.
#
# Disk: torch and friends ~2-3 GB, weights ~2 GB.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init

VENV="$TOOLS/chatterbox-venv"
ENGINE_ID="chatterbox"
DEST="$MODELS_DIR/chatterbox"
T3="v2"
MODEL="${CHATTERBOX_MODEL:-ResembleAI/chatterbox}"

PYTHON="$(lv_find_python "${CHATTERBOX_PYTHON:-}")" || {
  echo "a Python 3.10-3.12 interpreter is required" >&2; exit 1; }
echo "Using $("$PYTHON" --version 2>&1) ($PYTHON)"
mkdir -p "$DEST" "$TOOLS"

# 1. Isolated environment. lv_prepare_python_venv creates the venv, upgrades
#    pip, and pre-installs CUDA torch on Windows+NVIDIA. chatterbox-tts pins
#    torch==2.6.0 (CPU-only on PyPI); on Windows+NVIDIA we re-assert the CUDA
#    build afterwards so it stays put instead of being replaced by a generic
#    wheel.
lv_prepare_python_venv "$VENV" "$PYTHON"
lv_stage_key stage_install_pip_deps
"$VPY" -m pip install --upgrade "chatterbox-tts"
# chatterbox's Perth audio-watermarker (resemble-perth) does
# `from pkg_resources import resource_filename`, and setuptools >=81 removed
# pkg_resources entirely. Pin a version that still bundles it so the
# watermarker imports and the model can be constructed.
"$VPY" -m pip install --upgrade "setuptools<81"
if lv_is_windows && lv_has_nvidia; then
  "$VPY" -m pip install --ignore-installed --no-deps torch torchaudio \
    --index-url https://download.pytorch.org/whl/cu130
fi

# 2. Chatterbox weights. `from_local` reads a flat directory named by the
#    manifest "model"; pass the exact file stems this multilingual runner
#    needs so large unrelated artifacts (other T3/LLM checkpoints) are not
#    pulled. The v2 filename is fixed by the chatterbox-tts 0.1.7 package.
#    resolve-URL GET/range requests (not `hf download`) keep this working
#    through hf-mirror.com.
WEIGHTS="$DEST/weights"
lv_stage_key stage_download_model_tokenizer
lv_hf_download_repo "$MODEL" "$WEIGHTS" 8 \
  ve.pt \
  s3gen.pt \
  conds.pt \
  grapheme_mtl_merged_expanded_v1.json \
  "t3_mtl23ls_${T3}.safetensors" \
  Cangjie5_TC.json

# 3. Runner script.
cp "$SCRIPT_DIR/chatterbox_runner.py" "$DEST/chatterbox_runner.py"

# 4. Manifest (absolute paths are allowed and used as-is).
PY="$(lv_native_path "$VPY")"
RUNNER="$DEST/chatterbox_runner.py"
cat > "$DEST/engine.json" <<EOF
{
  "id": "$ENGINE_ID",
  "kind": "chatterbox",
  "name": "Chatterbox Multilingual V3 (voice clone)",
  "languages": ["zh", "en", "ar", "da", "de", "el", "es", "fi", "fr", "he", "hi", "it", "ja", "ko", "ms", "nl", "no", "pl", "pt", "ru", "sv", "sw", "tr"],
  "capabilities": ["tts", "clone", "multilingual"],
  "chatterbox": {
    "python": "$PY",
    "script": "$RUNNER",
    "model": "$WEIGHTS",
    "t3": "$T3",
    "device": "auto",
    "env": { "PYTHONIOENCODING": "utf-8" }
  },
  "parameters": [
    {
      "name": "language", "label": {"en": "Target language", "zh": "目标语言"}, "type": "choice",
      "options": [
        { "value": "zh", "label": {"en": "Chinese 中文", "zh": "中文 Chinese"} },
        { "value": "en", "label": {"en": "English 英语", "zh": "英语 English"} },
        { "value": "ar", "label": {"en": "Arabic 阿拉伯语", "zh": "阿拉伯语 Arabic"} },
        { "value": "da", "label": {"en": "Danish 丹麦语", "zh": "丹麦语 Danish"} },
        { "value": "de", "label": {"en": "German 德语", "zh": "德语 German"} },
        { "value": "el", "label": {"en": "Greek 希腊语", "zh": "希腊语 Greek"} },
        { "value": "es", "label": {"en": "Spanish 西班牙语", "zh": "西班牙语 Spanish"} },
        { "value": "fi", "label": {"en": "Finnish 芬兰语", "zh": "芬兰语 Finnish"} },
        { "value": "fr", "label": {"en": "French 法语", "zh": "法语 French"} },
        { "value": "he", "label": {"en": "Hebrew 希伯来语", "zh": "希伯来语 Hebrew"} },
        { "value": "hi", "label": {"en": "Hindi 印地语", "zh": "印地语 Hindi"} },
        { "value": "it", "label": {"en": "Italian 意大利语", "zh": "意大利语 Italian"} },
        { "value": "ja", "label": {"en": "Japanese 日语", "zh": "日语 Japanese"} },
        { "value": "ko", "label": {"en": "Korean 韩语", "zh": "韩语 Korean"} },
        { "value": "ms", "label": {"en": "Malay 马来语", "zh": "马来语 Malay"} },
        { "value": "nl", "label": {"en": "Dutch 荷兰语", "zh": "荷兰语 Dutch"} },
        { "value": "no", "label": {"en": "Norwegian 挪威语", "zh": "挪威语 Norwegian"} },
        { "value": "pl", "label": {"en": "Polish 波兰语", "zh": "波兰语 Polish"} },
        { "value": "pt", "label": {"en": "Portuguese 葡萄牙语", "zh": "葡萄牙语 Portuguese"} },
        { "value": "ru", "label": {"en": "Russian 俄语", "zh": "俄语 Russian"} },
        { "value": "sv", "label": {"en": "Swedish 瑞典语", "zh": "瑞典语 Swedish"} },
        { "value": "sw", "label": {"en": "Swahili 斯瓦希里语", "zh": "斯瓦希里语 Swahili"} },
        { "value": "tr", "label": {"en": "Turkish 土耳其语", "zh": "土耳其语 Turkish"} }
      ],
      "default": "zh",
      "hint": {"en": "Clone your reference voice but speak the target language.", "zh": "克隆参考声音但说目标语言。"}
    }
  ],
  "defaults": { "speed": 1.0, "num_steps": 1 }
}
EOF

echo
echo "Chatterbox Multilingual (t3=$T3) installed at $DEST"
echo "Restart the server to pick it up."