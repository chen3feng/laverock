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
  lv_stage "Fetching file list from HuggingFace ($(lv_hf_base))"
  lv_hf_download_repo "$HF_REPO" "$DEST" 8
}

# Primary: one GitHub tarball. When it is unreachable (or KOKORO_SOURCE=hf),
# files are fetched one by one from the HuggingFace mirror repo instead.
if [ "${KOKORO_SOURCE:-}" = "hf" ]; then
  download_hf
elif ! lv_extract_tarball "$GH_URL" "$TMP/model.tar.bz2" "$MODELS_DIR" \
      "Downloading Kokoro-82M INT8 tarball (~132 MB)"; then
  echo "GitHub tarball unavailable, falling back to per-file HuggingFace download" >&2
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
lv_stage "Writing engine manifest"
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
      "label": {"en": "Voice", "zh": "音色"},
      "type": "choice",
      "options": [
        { "label": {"en": "Alloy · American English (female) (af_alloy)", "zh": "Alloy · 美式英语女声 (af_alloy)"}, "value": "0" },
        { "label": {"en": "Aoede · American English (female) (af_aoede)", "zh": "Aoede · 美式英语女声 (af_aoede)"}, "value": "1" },
        { "label": {"en": "Bella · American English (female) (af_bella)", "zh": "Bella · 美式英语女声 (af_bella)"}, "value": "2" },
        { "label": {"en": "Heart · American English (female) (af_heart)", "zh": "Heart · 美式英语女声 (af_heart)"}, "value": "3" },
        { "label": {"en": "Jessica · American English (female) (af_jessica)", "zh": "Jessica · 美式英语女声 (af_jessica)"}, "value": "4" },
        { "label": {"en": "Kore · American English (female) (af_kore)", "zh": "Kore · 美式英语女声 (af_kore)"}, "value": "5" },
        { "label": {"en": "Nicole · American English (female) (af_nicole)", "zh": "Nicole · 美式英语女声 (af_nicole)"}, "value": "6" },
        { "label": {"en": "Nova · American English (female) (af_nova)", "zh": "Nova · 美式英语女声 (af_nova)"}, "value": "7" },
        { "label": {"en": "River · American English (female) (af_river)", "zh": "River · 美式英语女声 (af_river)"}, "value": "8" },
        { "label": {"en": "Sarah · American English (female) (af_sarah)", "zh": "Sarah · 美式英语女声 (af_sarah)"}, "value": "9" },
        { "label": {"en": "Sky · American English (female) (af_sky)", "zh": "Sky · 美式英语女声 (af_sky)"}, "value": "10" },
        { "label": {"en": "Adam · American English (male) (am_adam)", "zh": "Adam · 美式英语男声 (am_adam)"}, "value": "11" },
        { "label": {"en": "Echo · American English (male) (am_echo)", "zh": "Echo · 美式英语男声 (am_echo)"}, "value": "12" },
        { "label": {"en": "Eric · American English (male) (am_eric)", "zh": "Eric · 美式英语男声 (am_eric)"}, "value": "13" },
        { "label": {"en": "Fenrir · American English (male) (am_fenrir)", "zh": "Fenrir · 美式英语男声 (am_fenrir)"}, "value": "14" },
        { "label": {"en": "Liam · American English (male) (am_liam)", "zh": "Liam · 美式英语男声 (am_liam)"}, "value": "15" },
        { "label": {"en": "Michael · American English (male) (am_michael)", "zh": "Michael · 美式英语男声 (am_michael)"}, "value": "16" },
        { "label": {"en": "Onyx · American English (male) (am_onyx)", "zh": "Onyx · 美式英语男声 (am_onyx)"}, "value": "17" },
        { "label": {"en": "Puck · American English (male) (am_puck)", "zh": "Puck · 美式英语男声 (am_puck)"}, "value": "18" },
        { "label": {"en": "Santa · American English (male) (am_santa)", "zh": "Santa · 美式英语男声 (am_santa)"}, "value": "19" },
        { "label": {"en": "Alice · British English (female) (bf_alice)", "zh": "Alice · 英式英语女声 (bf_alice)"}, "value": "20" },
        { "label": {"en": "Emma · British English (female) (bf_emma)", "zh": "Emma · 英式英语女声 (bf_emma)"}, "value": "21" },
        { "label": {"en": "Isabella · British English (female) (bf_isabella)", "zh": "Isabella · 英式英语女声 (bf_isabella)"}, "value": "22" },
        { "label": {"en": "Lily · British English (female) (bf_lily)", "zh": "Lily · 英式英语女声 (bf_lily)"}, "value": "23" },
        { "label": {"en": "Daniel · British English (male) (bm_daniel)", "zh": "Daniel · 英式英语男声 (bm_daniel)"}, "value": "24" },
        { "label": {"en": "Fable · British English (male) (bm_fable)", "zh": "Fable · 英式英语男声 (bm_fable)"}, "value": "25" },
        { "label": {"en": "George · British English (male) (bm_george)", "zh": "George · 英式英语男声 (bm_george)"}, "value": "26" },
        { "label": {"en": "Lewis · British English (male) (bm_lewis)", "zh": "Lewis · 英式英语男声 (bm_lewis)"}, "value": "27" },
        { "label": {"en": "Dora · Spanish (female) (ef_dora)", "zh": "Dora · 西班牙语女声 (ef_dora)"}, "value": "28" },
        { "label": {"en": "Alex · Spanish (male) (em_alex)", "zh": "Alex · 西班牙语男声 (em_alex)"}, "value": "29" },
        { "label": {"en": "Siwis · French (female) (ff_siwis)", "zh": "Siwis · 法语女声 (ff_siwis)"}, "value": "30" },
        { "label": {"en": "Alpha · Hindi (female) (hf_alpha)", "zh": "Alpha · 印地语女声 (hf_alpha)"}, "value": "31" },
        { "label": {"en": "Beta · Hindi (female) (hf_beta)", "zh": "Beta · 印地语女声 (hf_beta)"}, "value": "32" },
        { "label": {"en": "Omega · Hindi (male) (hm_omega)", "zh": "Omega · 印地语男声 (hm_omega)"}, "value": "33" },
        { "label": {"en": "Psi · Hindi (male) (hm_psi)", "zh": "Psi · 印地语男声 (hm_psi)"}, "value": "34" },
        { "label": {"en": "Sara · Italian (female) (if_sara)", "zh": "Sara · 意大利语女声 (if_sara)"}, "value": "35" },
        { "label": {"en": "Nicola · Italian (male) (im_nicola)", "zh": "Nicola · 意大利语男声 (im_nicola)"}, "value": "36" },
        { "label": {"en": "Alpha · Japanese (female) (jf_alpha)", "zh": "Alpha · 日语女声 (jf_alpha)"}, "value": "37" },
        { "label": {"en": "Gongitsune · Japanese (female) (jf_gongitsune)", "zh": "Gongitsune · 日语女声 (jf_gongitsune)"}, "value": "38" },
        { "label": {"en": "Nezumi · Japanese (female) (jf_nezumi)", "zh": "Nezumi · 日语女声 (jf_nezumi)"}, "value": "39" },
        { "label": {"en": "Tebukuro · Japanese (female) (jf_tebukuro)", "zh": "Tebukuro · 日语女声 (jf_tebukuro)"}, "value": "40" },
        { "label": {"en": "Kumo · Japanese (male) (jm_kumo)", "zh": "Kumo · 日语男声 (jm_kumo)"}, "value": "41" },
        { "label": {"en": "Dora · Portuguese (female) (pf_dora)", "zh": "Dora · 葡萄牙语女声 (pf_dora)"}, "value": "42" },
        { "label": {"en": "Alex · Portuguese (male) (pm_alex)", "zh": "Alex · 葡萄牙语男声 (pm_alex)"}, "value": "43" },
        { "label": {"en": "Santa · Portuguese (male) (pm_santa)", "zh": "Santa · 葡萄牙语男声 (pm_santa)"}, "value": "44" },
        { "label": {"en": "Xiaobei · Mandarin (female) (zf_xiaobei)", "zh": "小贝 · 普通话女声 (zf_xiaobei)"}, "value": "45" },
        { "label": {"en": "Xiaoni · Mandarin (female) (zf_xiaoni)", "zh": "小妮 · 普通话女声 (zf_xiaoni)"}, "value": "46" },
        { "label": {"en": "Xiaoxiao · Mandarin (female) (zf_xiaoxiao)", "zh": "晓晓 · 普通话女声 (zf_xiaoxiao)"}, "value": "47" },
        { "label": {"en": "Xiaoyi · Mandarin (female) (zf_xiaoyi)", "zh": "小伊 · 普通话女声 (zf_xiaoyi)"}, "value": "48" },
        { "label": {"en": "Yunjian · Mandarin (male) (zm_yunjian)", "zh": "云健 · 普通话男声 (zm_yunjian)"}, "value": "49" },
        { "label": {"en": "Yunxi · Mandarin (male) (zm_yunxi)", "zh": "云希 · 普通话男声 (zm_yunxi)"}, "value": "50" },
        { "label": {"en": "Yunxia · Mandarin (male) (zm_yunxia)", "zh": "云夏 · 普通话男声 (zm_yunxia)"}, "value": "51" },
        { "label": {"en": "Yunyang · Mandarin (male) (zm_yunyang)", "zh": "云扬 · 普通话男声 (zm_yunyang)"}, "value": "52" },
        { "label": {"en": "Santa · Spanish (male) (em_santa)", "zh": "Santa · 西班牙语男声 (em_santa)"}, "value": "53" }
      ],
      "default": "3",
      "hint": {"en": "Kokoro offers preset voices only; it cannot clone your own audio.", "zh": "Kokoro 只提供预置音色，不支持用自己的音频克隆。"}
    },
    {
      "name": "speed",
      "label": {"en": "Speed", "zh": "语速"},
      "type": "float",
      "min": 0.5,
      "max": 2.0,
      "step": 0.1,
      "default": 1.0,
      "hint": {"en": "Higher is faster; the voice itself is unchanged.", "zh": "越大语速越快，不改变音色"}
    }
  ]
}
EOF

echo "Installed to $DEST"
echo "Try it: cargo run -p laverock-server"
