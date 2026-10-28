#!/usr/bin/env bash
# Download Kokoro-82M (multi-language v1.0, INT8) for sherpa-onnx.
# ~132 MB compressed: ONNX model, voices.bin, espeak data, jieba dict.
# Kokoro has preset speakers only (54 style vectors, no cloning), so the
# manifest we write carries the full sid -> voice choice list.
#
# Primary source is the sherpa-onnx GitHub release (one tarball). When it
# is unreachable (or KOKORO_SOURCE=hf is set), files are fetched one by
# one from the HuggingFace mirror repo instead; HF_ENDPOINT is honored
# (e.g. https://hf-mirror.com).
set -euo pipefail

# shellcheck source=laverock-lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/laverock-lib.sh"

MODEL="kokoro-int8-multi-lang-v1_0"
HF_REPO="csukuangfj/kokoro-int8-multi-lang-v1_0"
GH_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/${MODEL}.tar.bz2"

# The GitHub tarball expands to this $MODEL directory under MODELS_DIR.
lv_init "$MODEL"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Files not needed for inference; lv_hf_download_repo honors this hook.
lv_hf_skip() {
  case "$1" in
    .gitattributes|LICENSE|README.md|dict/README.md|dict/generate_user_dict.py|lexicon-gb-en.txt) return 0 ;;
    *) return 1 ;;
  esac
}

download_hf() {
  lv_stage "从 HuggingFace 镜像逐文件下载（$(lv_hf_base)）"
  lv_hf_download_repo "$HF_REPO" "$DEST" 8
}

# Primary: one GitHub tarball. When it is unreachable (or KOKORO_SOURCE=hf),
# files are fetched one by one from the HuggingFace mirror repo instead.
if [ "${KOKORO_SOURCE:-}" = "hf" ]; then
  download_hf
elif ! lv_extract_tarball "$GH_URL" "$TMP/model.tar.bz2" "$MODELS_DIR" \
      "下载 Kokoro-82M INT8 打包文件 · 约 132 MB"; then
  echo "GitHub 打包下载失败，改用 HuggingFace 镜像逐文件下载" >&2
  download_hf
fi

# Fail loudly before writing a manifest that points at missing files
# (sherpa-onnx itself calls exit() on such misconfiguration).
for f in model.int8.onnx voices.bin tokens.txt espeak-ng-data/phondata \
         dict/jieba.dict.utf8 lexicon-us-en.txt lexicon-zh.txt \
         date-zh.fst number-zh.fst phone-zh.fst; do
  if [ ! -s "$DEST/$f" ]; then
    echo "missing expected model file after download: $f" >&2
    exit 1
  fi
done

# Speaker ids follow the order baked into the ONNX metadata
# (`speaker_names`), not alphabetic order — em_santa is sid 53.
# af_* 美式英语 | bf_*/bm_* 英式英语 | zf_*/zm_* 中文（普通话）
lv_stage "写入引擎清单"
cat > "$DEST/engine.json" <<EOF
{
  "id": "$MODEL",
  "kind": "kokoro",
  "name": "Kokoro 82M (INT8)",
  "languages": ["zh", "en", "ja", "fr", "es", "it", "pt", "hi"],
  "capabilities": ["tts", "preset-voices"],
  "kokoro": {
    "model": "model.int8.onnx",
    "voices": "voices.bin",
    "tokens": "tokens.txt",
    "data_dir": "espeak-ng-data",
    "dict_dir": "dict",
    "lexicon": "lexicon-us-en.txt,lexicon-zh.txt",
    "rule_fsts": "date-zh.fst,number-zh.fst,phone-zh.fst",
    "length_scale": 1.0,
    "num_threads": 4,
    "default_sid": 3
  },
  "defaults": { "speed": 1.0, "num_steps": 4 },
  "parameters": [
    {
      "name": "voice",
      "label": "音色",
      "type": "choice",
      "options": [
        { "label": "Alloy · 美式英语女声 (af_alloy)", "value": "0" },
        { "label": "Aoede · 美式英语女声 (af_aoede)", "value": "1" },
        { "label": "Bella · 美式英语女声 (af_bella)", "value": "2" },
        { "label": "Heart · 美式英语女声 (af_heart)", "value": "3" },
        { "label": "Jessica · 美式英语女声 (af_jessica)", "value": "4" },
        { "label": "Kore · 美式英语女声 (af_kore)", "value": "5" },
        { "label": "Nicole · 美式英语女声 (af_nicole)", "value": "6" },
        { "label": "Nova · 美式英语女声 (af_nova)", "value": "7" },
        { "label": "River · 美式英语女声 (af_river)", "value": "8" },
        { "label": "Sarah · 美式英语女声 (af_sarah)", "value": "9" },
        { "label": "Sky · 美式英语女声 (af_sky)", "value": "10" },
        { "label": "Adam · 美式英语男声 (am_adam)", "value": "11" },
        { "label": "Echo · 美式英语男声 (am_echo)", "value": "12" },
        { "label": "Eric · 美式英语男声 (am_eric)", "value": "13" },
        { "label": "Fenrir · 美式英语男声 (am_fenrir)", "value": "14" },
        { "label": "Liam · 美式英语男声 (am_liam)", "value": "15" },
        { "label": "Michael · 美式英语男声 (am_michael)", "value": "16" },
        { "label": "Onyx · 美式英语男声 (am_onyx)", "value": "17" },
        { "label": "Puck · 美式英语男声 (am_puck)", "value": "18" },
        { "label": "Santa · 美式英语男声 (am_santa)", "value": "19" },
        { "label": "Alice · 英式英语女声 (bf_alice)", "value": "20" },
        { "label": "Emma · 英式英语女声 (bf_emma)", "value": "21" },
        { "label": "Isabella · 英式英语女声 (bf_isabella)", "value": "22" },
        { "label": "Lily · 英式英语女声 (bf_lily)", "value": "23" },
        { "label": "Daniel · 英式英语男声 (bm_daniel)", "value": "24" },
        { "label": "Fable · 英式英语男声 (bm_fable)", "value": "25" },
        { "label": "George · 英式英语男声 (bm_george)", "value": "26" },
        { "label": "Lewis · 英式英语男声 (bm_lewis)", "value": "27" },
        { "label": "Dora · 西班牙语女声 (ef_dora)", "value": "28" },
        { "label": "Alex · 西班牙语男声 (em_alex)", "value": "29" },
        { "label": "Siwis · 法语女声 (ff_siwis)", "value": "30" },
        { "label": "Alpha · 印地语女声 (hf_alpha)", "value": "31" },
        { "label": "Beta · 印地语女声 (hf_beta)", "value": "32" },
        { "label": "Omega · 印地语男声 (hm_omega)", "value": "33" },
        { "label": "Psi · 印地语男声 (hm_psi)", "value": "34" },
        { "label": "Sara · 意大利语女声 (if_sara)", "value": "35" },
        { "label": "Nicola · 意大利语男声 (im_nicola)", "value": "36" },
        { "label": "Alpha · 日语女声 (jf_alpha)", "value": "37" },
        { "label": "Gongitsune · 日语女声 (jf_gongitsune)", "value": "38" },
        { "label": "Nezumi · 日语女声 (jf_nezumi)", "value": "39" },
        { "label": "Tebukuro · 日语女声 (jf_tebukuro)", "value": "40" },
        { "label": "Kumo · 日语男声 (jm_kumo)", "value": "41" },
        { "label": "Dora · 葡萄牙语女声 (pf_dora)", "value": "42" },
        { "label": "Alex · 葡萄牙语男声 (pm_alex)", "value": "43" },
        { "label": "Santa · 葡萄牙语男声 (pm_santa)", "value": "44" },
        { "label": "小贝 · 普通话女声 (zf_xiaobei)", "value": "45" },
        { "label": "小妮 · 普通话女声 (zf_xiaoni)", "value": "46" },
        { "label": "晓晓 · 普通话女声 (zf_xiaoxiao)", "value": "47" },
        { "label": "小伊 · 普通话女声 (zf_xiaoyi)", "value": "48" },
        { "label": "云健 · 普通话男声 (zm_yunjian)", "value": "49" },
        { "label": "云希 · 普通话男声 (zm_yunxi)", "value": "50" },
        { "label": "云夏 · 普通话男声 (zm_yunxia)", "value": "51" },
        { "label": "云扬 · 普通话男声 (zm_yunyang)", "value": "52" },
        { "label": "Santa · 西班牙语男声 (em_santa)", "value": "53" }
      ],
      "default": "3",
      "hint": "Kokoro 只提供预置音色，不支持用自己的音频克隆。"
    },
    {
      "name": "speed",
      "label": "语速",
      "type": "float",
      "min": 0.5,
      "max": 2.0,
      "step": 0.1,
      "default": 1.0,
      "hint": "越大语速越快，不改变音色"
    }
  ]
}
EOF

echo "Installed to $DEST"
echo "Try it: cargo run -p laverock-server"
