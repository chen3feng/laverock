#!/usr/bin/env bash
# Download and build the VoxCPM2 engine.
#
# VoxCPM2 runs from GGUF weights via llama.cpp's `voxcpm2-cli`, which is
# not shipped as a standalone binary upstream, so we build it once from
# tc-mb/llama.cpp-omni. The result is cached under .tools/.
#
# Requires: git, cmake, a C++ toolchain (Xcode CLT on macOS / build-essential).
# Total download ~3.3 GB (BaseLM Q8_0 1.6 GB + Acoustic F16 1.7 GB).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=laverock-lib.sh
. "$SCRIPT_DIR/laverock-lib.sh"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODELS_DIR="${LAVEROCK_MODELS_DIR:-$ROOT/models}"
TOOLS="${LAVEROCK_TOOLS_DIR:-$ROOT/.tools}"
SRC="$TOOLS/llama.cpp-omni"
DEST="$MODELS_DIR/voxcpm2"
HF="${HF_ENDPOINT:-https://huggingface.co}/DennisHuang648/VoxCPM2-GGUF/resolve/main"

for tool in git cmake cc c++; do
  command -v "$tool" >/dev/null 2>&1 || { echo "missing: $tool" >&2; exit 1; }
done

# A newer macOS often ships an Xcode whose linker cannot parse the
# CommandLineTools SDK (or vice versa). When CommandLineTools is present,
# use its self-consistent toolchain instead of mixing the two.
if [ "$(uname -s)" = "Darwin" ] && [ -d /Library/Developer/CommandLineTools ]; then
  export DEVELOPER_DIR="/Library/Developer/CommandLineTools"
fi

mkdir -p "$MODELS_DIR" "$TOOLS"

# 1. Sources (shallow; cached).
if [ ! -d "$SRC/.git" ]; then
  lv_stage "拉取 llama.cpp-omni 源码"
  git clone --depth 1 --progress https://github.com/tc-mb/llama.cpp-omni.git "$SRC"
fi
cd "$SRC"
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git -C "$SRC" pull --ff-only >/dev/null 2>&1 || true
fi

# 2. Build the CLI target once.
if [ ! -x "$SRC/build/bin/voxcpm2-cli" ]; then
  lv_stage "编译 voxcpm2-cli（首次约数分钟）"
  case "$(uname -s)" in
    Darwin) BACKEND="-DGGML_METAL=ON";;
    *) BACKEND="-DGGML_BACKEND_DL=ON";;
  esac
  cmake -B build -S . \
    -DCMAKE_BUILD_TYPE=Release \
    -DGGML_NATIVE=OFF \
    -DBUILD_SHARED_LIBS=OFF \
    $BACKEND
  NPROC=$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)
  cmake --build build -j"$NPROC" --target voxcpm2-cli
fi

# 3. Place the runner next to the weights.
mkdir -p "$DEST"
cp "$SRC/build/bin/voxcpm2-cli" "$DEST/voxcpm2-cli"

# 4. Weights. Resume partial downloads and verify the exact byte count —
#    a truncated GGUF otherwise fails much later with an opaque error.
fetch_model() { # url dest expected_bytes
  local url="$1" dest="$2" want="$3"
  if [ -f "$dest" ] && [ "$(wc -c <"$dest" | tr -d ' ')" = "$want" ]; then
    echo "ok $(basename "$dest")"
    return 0
  fi
  lv_download "$url" "$dest"
  local got
  got=$(wc -c <"$dest" | tr -d ' ')
  if [ "$got" != "$want" ]; then
    echo "size mismatch for $(basename "$dest"): got $got, want $want" >&2
    return 1
  fi
}

lv_stage "下载权重（1/2）· BaseLM 约 1.6 GB"
fetch_model "$HF/VoxCPM2-BaseLM-Q8_0.gguf" \
  "$DEST/VoxCPM2-BaseLM-Q8_0.gguf" 1727309920
lv_stage "下载权重（2/2）· Acoustic 约 1.7 GB"
fetch_model "$HF/VoxCPM2-Acoustic-F16.gguf" \
  "$DEST/VoxCPM2-Acoustic-F16.gguf" 1825096352
lv_stage "写入引擎清单"

# 5. Manifest.
cat > "$DEST/engine.json" <<'EOF'
{
  "id": "voxcpm2",
  "kind": "voxcpm2",
  "name": "VoxCPM2 (30 语言 · 零样本克隆)",
  "languages": ["zh", "en", "ja", "ko", "ar", "es", "fr", "+ 30 languages"],
  "capabilities": ["tts", "clone", "voice-design"],
  "voxcpm2": {
    "base_lm": "VoxCPM2-BaseLM-Q8_0.gguf",
    "acoustic": "VoxCPM2-Acoustic-F16.gguf",
    "cfg": 2.0,
    "temperature": 1.0,
    "seed": 42,
    "cpu": false
  },
  "defaults": { "speed": 1.0, "num_steps": 10 }
}
EOF

echo
echo "VoxCPM2 installed at $DEST"
echo "Run the server and it will appear in the engine list."