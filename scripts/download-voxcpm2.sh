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

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODELS_DIR="${LAVEROCK_MODELS_DIR:-$ROOT/models}"
TOOLS="$ROOT/.tools"
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
  echo "Cloning llama.cpp-omni ..."
  git clone --depth 1 https://github.com/tc-mb/llama.cpp-omni.git "$SRC"
fi
cd "$SRC"
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git -C "$SRC" pull --ff-only >/dev/null 2>&1 || true
fi

# 2. Build the CLI target once.
if [ ! -x "$SRC/build/bin/voxcpm2-cli" ]; then
  echo "Building voxcpm2-cli (first build takes several minutes) ..."
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

# 4. Weights.
[ -f "$DEST/VoxCPM2-BaseLM-Q8_0.gguf" ] || {
  echo "Downloading BaseLM Q8_0 (1.6 GB) ..."
  curl -L --fail --retry 3 -o "$DEST/VoxCPM2-BaseLM-Q8_0.gguf" "$HF/VoxCPM2-BaseLM-Q8_0.gguf"
}
[ -f "$DEST/VoxCPM2-Acoustic-F16.gguf" ] || {
  echo "Downloading Acoustic F16 (1.7 GB) ..."
  curl -L --fail --retry 3 -o "$DEST/VoxCPM2-Acoustic-F16.gguf" "$HF/VoxCPM2-Acoustic-F16.gguf"
}

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