#!/usr/bin/env bash
# Install Seed-VC (zero-shot voice conversion) as a Laverock engine.
#
# Seed-VC turns an existing recording into a target voice while keeping the
# words and timing. It runs through audio.cpp's native `vc` task, on the
# same shared runtime as the speech engines — no Python/PyTorch stack.
#
#   SEEDVC_QUANT=q8_0|f16|orig   weight precision (default q8_0, ~3.1 GB)
#   SEEDVC_VERSION=...           pin a different audio.cpp release
#
# Disk: GGUF 3.1 GB (Q8_0) or 3.6 GB (F16), plus the shared ~80 MB runtime.
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"
lv_init seed-vc

AC_VERSION="${SEEDVC_VERSION:-v0.9.4}"
case "${SEEDVC_QUANT:-q8_0}" in
  f16)  FLAVOR="f16";   SIZE="~3.6 GB"; NAME="Seed-VC (F16)" ;;
  orig) FLAVOR="orig";  SIZE="~7.0 GB"; NAME="Seed-VC (original dtype)" ;;
  q8|q8_0|*) FLAVOR="q8_0"; SIZE="~3.1 GB"; NAME="Seed-VC (Q8_0)" ;;
esac

PACKAGE="SeedVC-MLX-GGUF/seed-vc-mlx-${FLAVOR}.gguf"
GGUF="$DEST/$(basename "$PACKAGE")"

# 1. Shared audio.cpp runtime (also used by the speech and music engines).
BIN="$(lv_audiocpp_bin "$TOOLS" "$AC_VERSION" "$MODELS_DIR")"
BACKEND="$(lv_audiocpp_backend)"

# 2. Weights from Hugging Face, in parallel ranged requests.
lv_hf_download_file audio-cpp/audio.cpp-gguf "$PACKAGE" "$GGUF" 8 \
  "Downloading Seed-VC weights ($SIZE)"

# 3. Request contract. Laverock reads these options for the conversion
# page's controls and validates them before calling audio.cpp.
cat > "$DEST/model_spec.json" <<'EOF'
{
  "schema_version": 1,
  "family": "seed_vc",
  "display_name": "Seed-VC",
  "tasks": ["vc", "svc"],
  "capabilities": { "vc": ["speaker_reference"], "svc": ["speaker_reference", "singing"] },
  "options": {
    "request": [
      {
        "name": "route",
        "type": "enum",
        "values": ["v2_vc", "v1_whisper_bigvgan_vc", "v1_xlsr_hift_vc", "v1_svc"],
        "default": "v2_vc",
        "label": { "en": "Conversion route", "zh": "转换路线" },
        "description": {
          "en": "V2 is the recommended speech route; the V1 routes trade quality for different vocoders, and v1_svc is for singing.",
          "zh": "V2 是推荐的语音路线；V1 路线使用不同的声码器，v1_svc 用于歌声转换。"
        }
      },
      {
        "name": "num_inference_steps",
        "type": "int",
        "min": 1,
        "max": 100,
        "default": 30,
        "label": { "en": "Diffusion steps", "zh": "扩散步数" },
        "description": {
          "en": "More steps are steadier but slower; 30 is the model default.",
          "zh": "越多越稳但越慢；30 是模型默认值。"
        }
      },
      {
        "name": "intelligibility_guidance_scale",
        "type": "float",
        "min": 0.0,
        "max": 3.0,
        "step": 0.05,
        "default": 0.7,
        "label": { "en": "Intelligibility guidance", "zh": "清晰度引导" },
        "description": {
          "en": "How strongly the output follows the source words (V2 only).",
          "zh": "输出遵循源音频内容的强度（仅 V2）。"
        }
      },
      {
        "name": "similarity_guidance_scale",
        "type": "float",
        "min": 0.0,
        "max": 3.0,
        "step": 0.05,
        "default": 0.7,
        "label": { "en": "Similarity guidance", "zh": "相似度引导" },
        "description": {
          "en": "How strongly the output follows the target voice (V2 only).",
          "zh": "输出贴近目标音色的强度（仅 V2）。"
        }
      },
      {
        "name": "seed",
        "type": "int",
        "min": 0,
        "max": 2147483647,
        "default": 42,
        "label": { "en": "Random seed", "zh": "随机种子" },
        "description": {
          "en": "Same seed and inputs reproduce the same conversion.",
          "zh": "相同种子与输入得到相同的转换结果。"
        }
      }
    ],
    "session": [],
    "load": []
  }
}
EOF

# 4. Manifest.
cat > "$DEST/engine.json" <<EOF
{
  "id": "seed-vc",
  "kind": "audio_cpp",
  "name": "$NAME",
  "task": "vc",
  "languages": ["language-agnostic"],
  "capabilities": ["vc"],
  "audio_cpp": {
    "binary": "$BIN",
    "model": "$(basename "$GGUF")",
    "backend": "$BACKEND",
    "family": "seed_vc",
    "task": "vc",
    "input": "audio",
    "voice_ref": true,
    "spec": "model_spec.json",
    "seed": 42
  },
  "defaults": { "speed": 1.0, "num_steps": 30 }
}
EOF

echo
echo "Seed-VC installed at $DEST"
echo "Reload the Models page (or restart the server) to pick it up."
