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

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=laverock-lib.sh
. "$SCRIPT_DIR/laverock-lib.sh"

MODELS_DIR="${LAVEROCK_MODELS_DIR:-$ROOT/models}"
TOOLS="${LAVEROCK_TOOLS_DIR:-$ROOT/.tools}"
MODEL="${LONGCAT_MODEL:-meituan-longcat/LongCat-AudioDiT-1B}"
DEST="$MODELS_DIR/longcat"
SRC="$TOOLS/LongCat-AudioDiT"
VENV="$TOOLS/longcat-venv"
WEIGHTS="$DEST/weights"
TOKENIZER="$DEST/tokenizer"
# Native (E:/..) forms for the Windows python child and the manifest; the
# POSIX vars above stay for bash-side mkdir/find/lv_track.
WEIGHTS_N="$(lv_native_path "$WEIGHTS")"
TOKENIZER_N="$(lv_native_path "$TOKENIZER")"

case "$MODEL" in
  *3.5B*|*3_5B*) SIZE_NOTE="~15.3 GB" ;;
  *) SIZE_NOTE="~5.7 GB" ;;
esac

# transformers 5.x needs Python 3.10+; prefer the newest supported one.
PYTHON="${LONGCAT_PYTHON:-}"
if [ -z "$PYTHON" ]; then
  candidates="python3.12 python3.11 python3.10 python3"
  if lv_is_windows; then
    candidates="python3.12 python3.11 python3.10 python python3 py"
  fi
  for candidate in $candidates; do
    if command -v "$candidate" >/dev/null 2>&1; then
      PYTHON="$candidate"
      break
    fi
  done
fi
command -v "$PYTHON" >/dev/null 2>&1 || { echo "a Python 3.10+ interpreter is required" >&2; exit 1; }
command -v git >/dev/null 2>&1 || { echo "git is required to fetch the audiodit package" >&2; exit 1; }
echo "Using $("$PYTHON" --version 2>&1) ($PYTHON)"

mkdir -p "$MODELS_DIR" "$DEST" "$TOOLS"

# 1. Official implementation (contains the `audiodit` package).
if [ ! -d "$SRC/.git" ]; then
  lv_stage "拉取 LongCat-AudioDiT 源码"
  git clone --depth 1 --progress https://github.com/meituan-longcat/LongCat-AudioDiT.git "$SRC"
fi

# 2. Isolated environment. Windows venvs use Scripts/ instead of bin/.
VPY="$(lv_venv_python "$VENV")"
if [ ! -f "$VPY" ]; then
  lv_stage "创建 Python 虚拟环境"
  "$PYTHON" -m venv "$VENV"
fi
export PIP_PROGRESS_BAR=on
lv_stage "安装依赖（PyTorch 与 transformers 5，约 1–2 GB）"
"$VPY" -m pip install --upgrade pip
# The default PyPI torch wheel is CPU-only on Windows; install a CUDA build
# first so an NVIDIA machine does not silently fall back to CPU inference.
# The requirements.txt install below then leaves torch/torchaudio alone.
if lv_is_windows && lv_has_nvidia; then
  "$VPY" -m pip install torch torchaudio --index-url https://download.pytorch.org/whl/cu130
fi
"$VPY" -m pip install -r "$SRC/requirements.txt"
# The UMT5 tokenizer ships a SentencePiece model; transformers can fall
# back to it when converting, so keep the library available.
"$VPY" -m pip install sentencepiece

# huggingface_hub 2.x ships `hf`; older versions ship `huggingface-cli`.
HFCLI="$(lv_venv_cli "$VENV" hf)"
[ -f "$HFCLI" ] || HFCLI="$(lv_venv_cli "$VENV" huggingface-cli)"

# 3. Weights and tokenizer, both local to the model directory so the app
#    runs fully offline afterwards.
export HF_HUB_DISABLE_PROGRESS_BARS=0
# Xet transfers do not reliably use the configured HTTP proxy and report
# no byte progress, so use plain HTTPS downloads here.
export HF_HUB_DISABLE_XET=1

# Expected size from the hub API, for a real percentage in the UI.
HF_BASE="${HF_ENDPOINT:-https://huggingface.co}"
HF_BASE="${HF_BASE%/}"

# Small repo files (config.json, README, images) come straight from the
# HF CLI; the multi-GB blob below uses parallel ranged curl instead.
lv_stage "下载模型配置"
"$HFCLI" download "$MODEL" --local-dir "$WEIGHTS_N" --exclude "*.safetensors"

# The safetensors blob dominates the download and is the most fragile part
# of an HF-CLI transfer: one flaky-proxy reset aborts the whole stream.
# Fetch it with the same parallel ranged curl the VoxCPM2 engine uses, so
# a reset costs only one unfinished slice and it resumes from partials.
WEIGHT_FILE="model.safetensors"
WEIGHT_TOTAL=$(curl -sL "$HF_BASE/api/models/$MODEL/tree/main" \
  | "$VPY" -c 'import json,sys
try:
    print(next(f.get("size", 0) for f in json.load(sys.stdin)
               if f.get("path") == "model.safetensors"))
except Exception:
    print(0)' 2>/dev/null || echo 0)
if [ "${WEIGHT_TOTAL:-0}" -le 0 ]; then
  echo "cannot determine the weight size for $MODEL" >&2
  exit 1
fi
WEIGHT_URL="$HF_BASE/$MODEL/resolve/main/$WEIGHT_FILE"
if [ ! -f "$WEIGHTS/$WEIGHT_FILE" ] || \
   [ "$(lv_filesize "$WEIGHTS/$WEIGHT_FILE")" -ne "$WEIGHT_TOTAL" ]; then
  lv_stage "下载模型权重（${SIZE_NOTE}）"
  lv_fetch_parallel "$WEIGHT_URL" "$WEIGHTS/$WEIGHT_FILE" "$WEIGHT_TOTAL" 8
fi
# An interrupted earlier attempt can leave multi-GB partials behind.
find "$WEIGHTS/.cache" -name "*.incomplete" -delete 2>/dev/null || true
lv_stage "下载 UMT5 分词器"
# Explicit filenames: `hf download` treats extra positionals as a file
# list (and then ignores --include).
"$HFCLI" download google/umt5-base \
  tokenizer.json tokenizer_config.json special_tokens_map.json spiece.model \
  --local-dir "$TOKENIZER_N"

# 4. Runner plus the audiodit package next to it.
lv_stage "安装运行脚本"
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
    "seed": 1024
  },
  "parameters": [
    { "name": "num_steps", "label": "采样步数 (NFE)", "type": "int",
      "min": 1, "max": 32, "step": 1, "default": 16,
      "hint": "扩散 ODE 步数；16 已足够，越多越慢" },
    { "name": "cfg", "label": "引导强度", "type": "float",
      "min": 0.0, "max": 10.0, "step": 0.1, "default": 4.0,
      "hint": "越高越贴近参考音色，过高可能失真" },
    { "name": "guidance_method", "label": "引导方式", "type": "choice",
      "options": [
        { "label": "APG（自适应投影，推荐）", "value": "apg" },
        { "label": "CFG（传统无分类器引导）", "value": "cfg" }
      ],
      "default": "apg", "hint": "APG 通常更自然、伪影更少" },
    { "name": "seed", "label": "随机种子", "type": "int",
      "min": 0, "max": 2147483647, "default": 1024,
      "hint": "相同种子 + 文本得到可复现的声音" }
  ],
  "defaults": { "speed": 1.0, "num_steps": 16 }
}
EOF

echo
echo "LongCat-AudioDiT ($MODEL) installed at $DEST"
echo "Restart the server (or press reload in the 模型 page) to pick it up."
