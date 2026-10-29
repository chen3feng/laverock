#!/usr/bin/env bash
# Install LongCat-AudioDiT (Meituan): an isolated Python venv with the
# official `audiodit` package and its dependencies, the model weights,
# the UMT5 tokenizer, and the laverock runner.
#
#   LONGCAT_MODEL=meituan-longcat/LongCat-AudioDiT-3.5B   (default: 1B)
#   LONGCAT_PYTHON=python3.12
#
# Disk: torch and friends ~1-2 GB, weights 5.7 GB (1B) or 15.3 GB (3.5B).
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init longcat

MODEL="${LONGCAT_MODEL:-meituan-longcat/LongCat-AudioDiT-1B}"
SRC="$TOOLS/LongCat-AudioDiT"
VENV="$TOOLS/longcat-venv"
WEIGHTS="$DEST/weights"
TOKENIZER="$DEST/tokenizer"
# Native (E:/..) forms for the Windows python child and the manifest; the
# POSIX vars above stay for bash-side mkdir/find/cp.
WEIGHTS_N="$(lv_native_path "$WEIGHTS")"
TOKENIZER_N="$(lv_native_path "$TOKENIZER")"

case "$MODEL" in
  *3.5B*|*3_5B*) SIZE_NOTE="~15.3 GB" ;;
  *) SIZE_NOTE="~5.7 GB" ;;
esac

# transformers 5.x needs Python 3.10+; prefer the newest supported one.
PYTHON="$(lv_find_python "${LONGCAT_PYTHON:-}")" || {
  echo "a Python 3.10+ interpreter is required" >&2; exit 1; }
command -v git >/dev/null 2>&1 || { echo "git is required to fetch the audiodit package" >&2; exit 1; }
echo "Using $("$PYTHON" --version 2>&1) ($PYTHON)"

mkdir -p "$TOOLS"

# 1. Official implementation (contains the `audiodit` package).
if [ ! -d "$SRC/.git" ]; then
  lv_stage "Fetching LongCat-AudioDiT source"
  git clone --depth 1 --progress https://github.com/meituan-longcat/LongCat-AudioDiT.git "$SRC"
fi

# 2. Isolated environment. lv_prepare_python_venv creates the venv, upgrades
#    pip, and pre-installs CUDA torch on Windows+NVIDIA; VPY points at its
#    interpreter (also used by the HF helpers for JSON parsing via LV_PY).
lv_prepare_python_venv "$VENV" "$PYTHON"
lv_stage "Installing dependencies (PyTorch and transformers 5, ~1–2 GB)"
"$VPY" -m pip install -r "$SRC/requirements.txt"
# The UMT5 tokenizer ships a SentencePiece model; transformers can fall
# back to it when converting, so keep the library available.
"$VPY" -m pip install sentencepiece

# 3. Weights and tokenizer in plain local directories so the app runs fully
#    offline afterwards. Files come through resolve-URL GET/range requests
#    (lv_hf_download_repo), not `hf download`: huggingface_hub 0.36's
#    metadata HEAD negotiation rejects hf-mirror redirects, and a single
#    flaky-proxy reset used to abort the multi-GB stream. The helper resumes
#    per file and skips files already present with the right size.
lv_stage "Downloading model weights (${SIZE_NOTE})"
lv_hf_download_repo "$MODEL" "$WEIGHTS" 8
# An interrupted earlier attempt can leave multi-GB partials behind.
find "$WEIGHTS/.cache" -name "*.incomplete" -delete 2>/dev/null || true
lv_stage "Downloading the UMT5 tokenizer"
# Only the four tokenizer files: the umt5-base repo also ships multi-GB
# encoder weights that LongCat does not load.
lv_hf_download_repo google/umt5-base "$TOKENIZER" 8 \
  tokenizer.json tokenizer_config.json special_tokens_map.json spiece.model

# 4. Runner plus the audiodit package next to it.
lv_stage "Installing the runner script"
cp "$SCRIPT_DIR/longcat_runner.py" "$DEST/longcat_runner.py"
rm -rf "$DEST/audiodit"
cp -R "$SRC/audiodit" "$DEST/audiodit"

# 5. Manifest. `python` and the paths are absolute (allowed and used as-is),
# in native E:/.. form for the Windows python child.
PY="$(lv_native_path "$VPY")"
NAME="LongCat-AudioDiT 1B"
case "$MODEL" in *3.5B*|*3_5B*) NAME="LongCat-AudioDiT 3.5B" ;; esac
cat > "$DEST/engine.json" <<EOF
{
  "id": "longcat",
  "kind": "longcat",
  "name": "$NAME",
  "languages": ["zh", "en"],
  "capabilities": ["tts", "clone"],
  "longcat": {
    "python": "$PY",
    "script": "longcat_runner.py",
    "model": "$WEIGHTS_N",
    "tokenizer": "$TOKENIZER_N",
    "device": "auto",
    "dtype": "auto",
    "guidance": "apg",
    "cfg_strength": 4.0,
    "seed": 1024,
    "env": { "PYTHONIOENCODING": "utf-8" }
  },
  "parameters": [
    { "name": "num_steps", "label": {"en": "Sampling steps (NFE)", "zh": "采样步数"}, "type": "int",
      "min": 1, "max": 32, "step": 1, "default": 16,
      "hint": {"en": "Diffusion ODE steps; 16 is enough, more is slower.", "zh": "扩散 ODE 步数；16 已足够，越多越慢"} },
    { "name": "cfg", "label": {"en": "Guidance strength", "zh": "引导强度"}, "type": "float",
      "min": 0.0, "max": 10.0, "step": 0.1, "default": 4.0,
      "hint": {"en": "Higher follows the reference more closely; too high may distort.", "zh": "越高越贴近参考音色，过高可能失真"} },
    { "name": "guidance_method", "label": {"en": "Guidance method", "zh": "引导方式"}, "type": "choice",
      "options": [
        { "label": {"en": "APG (adaptive projection, recommended)", "zh": "自适应投影（推荐）"}, "value": "apg" },
        { "label": {"en": "CFG (classic classifier-free guidance)", "zh": "传统无分类器引导"}, "value": "cfg" }
      ],
      "default": "apg", "hint": {"en": "APG is usually more natural with fewer artifacts.", "zh": "APG 通常更自然、伪影更少"} },
    { "name": "seed", "label": {"en": "Random seed", "zh": "随机种子"}, "type": "int",
      "min": 0, "max": 2147483647, "default": 1024,
      "hint": {"en": "Same seed + text gives reproducible speech.", "zh": "相同种子 + 文本得到可复现的声音"} }
  ],
  "defaults": { "speed": 1.0, "num_steps": 16 }
}
EOF

echo
echo "LongCat-AudioDiT ($MODEL) installed at $DEST"
echo "Restart the server (or press reload on the Models page) to pick it up."
