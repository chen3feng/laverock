#!/usr/bin/env bash
# Install MOSS-TTS-Delay (OpenMOSS) as a Laverock engine.
#
# MOSS-TTS ships a first-class GGUF export plus a native llama.cpp runtime.
# Instead of a Python/PyTorch stack we run the OpenMOSS llama.cpp fork's
# `llama-moss-tts` binary, which drives the Qwen3-delay backbone and the
# audio encoder/decoder. That binary is prebuilt by .github/workflows/
# mosstts-runner.yml and published on this repo's rolling "mosstts-runner"
# release, so installing no longer needs cmake or a C++ toolchain; we only
# compile from source as a fallback for unreleased platforms. The audio
# tokenizer is converted to GGUF with a small numpy-only helper, so
# inference itself needs no Python.
#
# Weights come from ModelScope (open); the HF mirror of the GGUF repo is
# gated behind a login.
#
#   MOSS_MODEL_QUANT=Q8_0     pick a larger first-class backbone quant
#   MOSS_LLAMA_REF=...        pin a different fork branch/tag (also the
#                             source of the tokenizer converter)
#   MOSS_LLAMA_REPO=...       override the fork URL (source build only)
#   MOSS_RUNTIME_BUILD=1      always build the runtime from source
#
# Requirements: a Python 3 with pip (one-time tokenizer conversion; not
# needed at inference time), plus git + cmake + a C++ compiler only when
# the source build runs.
#
# Disk: backbone ~5–9 GB + tokenizer ~7 GB (freed after conversion). The
# fallback build adds a compiler and a build tree; budget ~20 GB free then.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init mosstts

ENGINE_ID="mosstts"
WEIGHTS="$DEST/weights"
WORK="$DEST/build"
FORK="$WORK/llama.cpp"
BUILD="$FORK/build"
AT="$WORK/audio-tokenizer"
SRC="$WORK/src"
QUANT="${MOSS_MODEL_QUANT:-Q4_K_M}"
BACKBONE_FILE="MOSS_TTS_FIRST_CLASS_${QUANT}.gguf"
FORK_URL="${MOSS_LLAMA_REPO:-https://github.com/OpenMOSS/llama.cpp}"
# Pinned to the commit the prebuilt runner is built from
# (.github/workflows/mosstts-runner.yml); bump both together.
FORK_REF="${MOSS_LLAMA_REF:-b785003ba497794ecfa337c3e47f01af79489888}"
GGUF_REPO="OpenMOSS/MOSS-TTS-GGUF"
AT_REPO="OpenMOSS/MOSS-Audio-Tokenizer"

die() { echo "$*" >&2; exit 1; }

mkdir -p "$WEIGHTS" "$WORK"

# 1. Native runtime: prebuilt first, source build only as a fallback.
BIN=""
if [ "${MOSS_RUNTIME_BUILD:-0}" != "1" ]; then
  BIN="$(lv_mosstts_bin "$TOOLS")" || BIN=""
fi
if [ -z "$BIN" ]; then
  echo "No prebuilt llama-moss-tts for this platform; building from source." >&2
  command -v cmake >/dev/null 2>&1 || die "cmake is required to build the MOSS-TTS runtime"
  command -v git >/dev/null 2>&1 || die "git is required to build the MOSS-TTS runtime"
  if ! command -v c++ >/dev/null 2>&1 && ! command -v g++ >/dev/null 2>&1 \
     && ! command -v clang++ >/dev/null 2>&1 && ! lv_is_windows; then
    die "a C++ compiler (c++/g++/clang++) is required to build the MOSS-TTS runtime"
  fi

  # A shallow clone of the pinned commit is enough. The TTS CLI has no use
  # for TLS or the HTTP server, and disabling them avoids pulling OpenSSL in.
  if [ ! -d "$FORK/.git" ]; then
    lv_stage "Fetching the MOSS-TTS llama.cpp runtime"
    git clone --depth 1 "$FORK_URL" "$FORK"
    git -C "$FORK" fetch --depth 1 origin "$FORK_REF" 2>/dev/null || true
    git -C "$FORK" checkout --detach "$FORK_REF" 2>/dev/null || true
  fi

  lv_stage "Building llama-moss-tts (several minutes)"
  if [ ! -x "$BUILD/bin/llama-moss-tts" ] && [ ! -x "$BUILD/bin/llama-moss-tts.exe" ]; then
    jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
    gen=()
    if command -v ninja >/dev/null 2>&1; then gen=(-G Ninja); fi
    # On macOS the default clang may resolve the CommandLineTools SDK, which
    # can be newer than the compiler understands; pin the Xcode SDK instead.
    sys=()
    if [ "$(uname -s)" = "Darwin" ]; then
      sr="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null || true)"
      if [ -n "$sr" ]; then sys=("-DCMAKE_OSX_SYSROOT=$sr"); fi
    fi
    cmake -S "$FORK" -B "$BUILD" ${gen[@]+"${gen[@]}"} -DCMAKE_BUILD_TYPE=Release \
      -DLLAMA_CURL=OFF -DLLAMA_OPENSSL=OFF -DLLAMA_BUILD_SERVER=OFF \
      -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF \
      ${sys[@]+"${sys[@]}"}
    cmake --build "$BUILD" --target llama-moss-tts -j "$jobs"
  fi
  BIN="$BUILD/bin/llama-moss-tts"
  if [ -x "${BIN}.exe" ]; then BIN="${BIN}.exe"; fi
  [ -x "$BIN" ] || die "the build did not produce $BIN"
  BIN="$(lv_native_path "$BIN")"
fi
echo "Runtime: $BIN"

# 2. Weights (ModelScope, open).
lv_ms_download_file "$GGUF_REPO" "first_class/$BACKBONE_FILE" "$WEIGHTS/$BACKBONE_FILE" 8 \
  "Downloading the backbone ($QUANT, ~5 GB)"
for f in config.json model.safetensors.index.json \
         model-00001-of-00002.safetensors model-00002-of-00002.safetensors; do
  lv_ms_download_file "$AT_REPO" "$f" "$AT/$f" 8 \
    "Downloading the audio tokenizer ($(basename "$f"))"
done

# 3. Source of the audio-tokenizer converter. The build above already has
#    it; otherwise fetch the repo tarball at the same pinned commit, which
#    keeps git out of the prebuilt path.
CONVERT_DIR="$FORK"
if [ ! -f "$CONVERT_DIR/convert_moss_audio_tokenizer_split_to_gguf.py" ]; then
  CONVERT_DIR="$SRC/llama.cpp"
  if [ ! -f "$CONVERT_DIR/convert_moss_audio_tokenizer_split_to_gguf.py" ]; then
    lv_stage "Fetching the MOSS-TTS converter scripts"
    tarball="$WORK/llama.cpp-src.tar.gz"
    lv_download "https://codeload.github.com/OpenMOSS/llama.cpp/tar.gz/$FORK_REF" "$tarball" 0 \
      || die "could not download $FORK_REF from OpenMOSS/llama.cpp"
    rm -rf "$SRC"
    mkdir -p "$SRC/extract"
    tar -xzf "$tarball" -C "$SRC/extract"
    root="$(find "$SRC/extract" -maxdepth 1 -mindepth 1 -type d | head -1)"
    mkdir -p "$CONVERT_DIR"
    cp "$root/convert_moss_audio_tokenizer_split_to_gguf.py" \
       "$root/convert_moss_audio_tokenizer_to_gguf.py" "$CONVERT_DIR/"
    cp -R "$root/gguf-py" "$CONVERT_DIR/"
    cp "$root/LICENSE" "$CONVERT_DIR/" 2>/dev/null || true
    rm -rf "$SRC/extract" "$tarball"
  fi
fi

# 4. Convert the audio tokenizer to the native encoder/decoder GGUFs.
lv_stage "Converting the audio tokenizer to GGUF"
ENC="$WEIGHTS/moss_tts_audio_encoder_f16.gguf"
DEC="$WEIGHTS/moss_tts_audio_decoder_f16.gguf"
if [ ! -f "$ENC" ] || [ ! -f "$DEC" ]; then
  PYTHON="$(command -v python3.12 2>/dev/null || command -v python3.11 2>/dev/null \
    || command -v python3.10 2>/dev/null || command -v python3 2>/dev/null \
    || command -v python 2>/dev/null || true)"
  [ -n "$PYTHON" ] || die "a Python 3 interpreter is required for the one-time tokenizer conversion"
  VENV="$WORK/pyvenv"
  VPY="$(lv_venv_python "$VENV")"
  if [ ! -f "$VPY" ]; then
    lv_stage_key stage_venv
    "$PYTHON" -m venv "$VENV"
    "$VPY" -m pip install --quiet --upgrade pip
  fi
  # The gguf-py helpers need numpy and PyYAML; nothing else. Do not go
  # through lv_prepare_python_venv, which would pull multi-GB CUDA torch on
  # Windows+NVIDIA for no reason.
  "$VPY" -m pip install --quiet --upgrade numpy pyyaml
  "$VPY" "$CONVERT_DIR/convert_moss_audio_tokenizer_split_to_gguf.py" \
    "$AT" --encoder-outfile "$ENC" --decoder-outfile "$DEC" --outtype f16
fi
[ -f "$ENC" ] && [ -f "$DEC" ] || die "audio tokenizer conversion produced no GGUF"
# The safetensors are only needed for the conversion.
rm -rf "$AT"

# 5. Manifest (absolute paths are allowed and used as-is).
BIN_NATIVE="$(lv_native_path "$BIN")"
# Rust resolves manifest paths itself, so MSYS /e/.. mounts must be emitted
# as E:/..; only the runtime binary used to be converted.
BACKBONE_NATIVE="$(lv_native_path "$WEIGHTS/$BACKBONE_FILE")"
ENC_NATIVE="$(lv_native_path "$ENC")"
DEC_NATIVE="$(lv_native_path "$DEC")"
cat > "$DEST/engine.json" <<EOF
{
  "id": "$ENGINE_ID",
  "kind": "mosstts",
  "name": "MOSS-TTS ($QUANT, native)",
  "languages": ["zh", "en", "de", "es", "fr", "ja", "it", "he", "ko", "ru", "fa", "ar", "pl", "pt", "cs", "da", "sv", "hu", "el", "tr"],
  "capabilities": ["tts", "clone", "multilingual"],
  "mosstts": {
    "binary": "$BIN_NATIVE",
    "backbone": "$BACKBONE_NATIVE",
    "audio_encoder": "$ENC_NATIVE",
    "audio_decoder": "$DEC_NATIVE",
    "language": "zh",
    "n_gpu_layers": -1,
    "max_new_tokens": 3072,
    "env": { "PYTHONIOENCODING": "utf-8" }
  },
  "parameters": [
    {
      "name": "language", "label": {"en": "Language", "zh": "语言"}, "type": "choice",
      "options": [
        { "value": "zh", "label": {"en": "Chinese 中文", "zh": "中文 Chinese"} },
        { "value": "en", "label": {"en": "English 英语", "zh": "英语 English"} },
        { "value": "de", "label": {"en": "German 德语", "zh": "德语 German"} },
        { "value": "es", "label": {"en": "Spanish 西班牙语", "zh": "西班牙语 Spanish"} },
        { "value": "fr", "label": {"en": "French 法语", "zh": "法语 French"} },
        { "value": "ja", "label": {"en": "Japanese 日语", "zh": "日语 Japanese"} },
        { "value": "it", "label": {"en": "Italian 意大利语", "zh": "意大利语 Italian"} },
        { "value": "he", "label": {"en": "Hebrew 希伯来语", "zh": "希伯来语 Hebrew"} },
        { "value": "ko", "label": {"en": "Korean 韩语", "zh": "韩语 Korean"} },
        { "value": "ru", "label": {"en": "Russian 俄语", "zh": "俄语 Russian"} },
        { "value": "fa", "label": {"en": "Persian 波斯语", "zh": "波斯语 Persian"} },
        { "value": "ar", "label": {"en": "Arabic 阿拉伯语", "zh": "阿拉伯语 Arabic"} },
        { "value": "pl", "label": {"en": "Polish 波兰语", "zh": "波兰语 Polish"} },
        { "value": "pt", "label": {"en": "Portuguese 葡萄牙语", "zh": "葡萄牙语 Portuguese"} },
        { "value": "cs", "label": {"en": "Czech 捷克语", "zh": "捷克语 Czech"} },
        { "value": "da", "label": {"en": "Danish 丹麦语", "zh": "丹麦语 Danish"} },
        { "value": "sv", "label": {"en": "Swedish 瑞典语", "zh": "瑞典语 Swedish"} },
        { "value": "hu", "label": {"en": "Hungarian 匈牙利语", "zh": "匈牙利语 Hungarian"} },
        { "value": "el", "label": {"en": "Greek 希腊语", "zh": "希腊语 Greek"} },
        { "value": "tr", "label": {"en": "Turkish 土耳其语", "zh": "土耳其语 Turkish"} }
      ],
      "default": "zh",
      "hint": {"en": "Optional: clone your reference voice. Without a clip the model uses a default voice.", "zh": "可选：克隆参考声音；不提供参考音频则使用默认音色。"}
    }
  ],
  "defaults": { "speed": 1.0 }
}
EOF

echo
echo "MOSS-TTS ($QUANT) installed at $DEST"
echo "Restart the server (or reload the Models page) to pick it up."
