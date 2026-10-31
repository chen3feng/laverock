#!/usr/bin/env bash
# Prepare the CUDA-enabled ONNX runtime for a source build.
#
# Downloads the shared sherpa-onnx build carrying the CUDA execution provider
# from the k2-fsa release for this OS and unpacks it under LAVEROCK_TOOLS_DIR.
# On Windows it also creates the `onnxruntime.lib` import library that archive
# leaves out but the Rust crate links against.
#
# Only the runtime is downloaded: the CUDA 13 and cuDNN 9 DLLs are not part of
# the archive. Install the NVIDIA driver, the CUDA 13 runtime and cuDNN 9 (a
# framework's PyTorch wheel already ships the last two) and put them on PATH.
#
# Afterwards build with
#
#   SHERPA_ONNX_LIB_DIR=<printed by this script> \
#     cargo build -p laverock-server --no-default-features --features onnx-cuda
#
# The server then asks for CUDA and falls back to the CPU when the driver, a
# device or cuDNN is missing.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"

VERSION="1.13.8"
CUDA="cuda-13.x-cudnn-9.x-onnxruntime1.28.2"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) PLATFORM="win-x64-cuda" ;;
  Linux) PLATFORM="linux-x64-gpu" ;;
  *)
    echo "No prebuilt CUDA build for $(uname -s); sherpa-onnx publishes Windows and Linux only." >&2
    exit 1
    ;;
esac

ARCHIVE="sherpa-onnx-v${VERSION}-${CUDA}-${PLATFORM}.tar.bz2"
STEM="sherpa-onnx-v${VERSION}-${CUDA}-${PLATFORM}"
URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/v${VERSION}/${ARCHIVE}"

lv_init
DEST="$TOOLS/onnx-cuda"
LIB="$DEST/$STEM/lib"

if [ ! -f "$LIB/sherpa-onnx-c-api.dll" ] && [ ! -f "$LIB/libsherpa-onnx-c-api.so" ]; then
  lv_extract_tarball "$URL" "$TOOLS/$ARCHIVE" "$DEST" "Download the CUDA ONNX runtime"
fi
if [ ! -d "$LIB" ]; then
  echo "archive did not unpack to $LIB" >&2
  exit 1
fi

# find_msvc <tool>: locate dumpbin/lib from PATH or from a Visual Studio
# Build Tools install (rustc finds MSVC through the registry, so a plain
# shell usually has neither on PATH).
find_msvc() {
  local tool="$1"
  if command -v "$tool" >/dev/null 2>&1; then
    command -v "$tool"
    return 0
  fi
  local vswhere="C:/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
  [ -f "$vswhere" ] || return 1
  local root
  root="$("$vswhere" -latest -products '*' \
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 \
    -property installationPath 2>/dev/null | tr -d '\r')"
  [ -n "$root" ] || return 1
  ls -d "$root"/VC/Tools/MSVC/*/bin/HostX64/x64/"$tool".exe 2>/dev/null | sort | tail -1
}

# The shared Rust build links onnxruntime.dll directly; the archive ships the
# DLL but no import library, so synthesize one from its exports.
if lv_is_windows && [ ! -f "$LIB/onnxruntime.lib" ]; then
  DUMPBIN="$(find_msvc dumpbin)" || {
    echo "dumpbin.exe not found; install the Visual Studio C++ build tools" >&2
    exit 1
  }
  LIBTOOL="$(find_msvc lib)" || {
    echo "lib.exe not found; install the Visual Studio C++ build tools" >&2
    exit 1
  }
  lv_stage "Create onnxruntime.lib"
  DEF="$DEST/onnxruntime.def"
  {
    echo "LIBRARY onnxruntime.dll"
    echo "EXPORTS"
    "$DUMPBIN" -exports "$LIB/onnxruntime.dll" \
      | awk '/^ +[0-9]+ +[0-9A-Fa-f]+ +[0-9A-Fa-f]+ +/ { print "    " $4 }'
  } >"$DEF"
  "$LIBTOOL" -def:"$DEF" -machine:x64 -out:"$LIB/onnxruntime.lib" >/dev/null
  rm -f "$DEF" "$DEST"/*.exp
fi

LIB_NATIVE="$(lv_native_path "$LIB")"
echo
echo "Runtime ready. Build with:"
echo "  SHERPA_ONNX_LIB_DIR=$LIB_NATIVE \\"
echo "    cargo build -p laverock-server --no-default-features --features onnx-cuda"
echo
echo "At run time, the CUDA 13 and cuDNN 9 DLLs must be on PATH."
