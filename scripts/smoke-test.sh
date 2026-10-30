#!/usr/bin/env bash
# End-to-end smoke test against a running server.
#   ./scripts/smoke-test.sh [base_url] [model_id]
set -euo pipefail

BASE="${1:-http://127.0.0.1:8787}"
MODEL="${2:-sherpa-onnx-zipvoice-distill-int8-zh-en-emilia}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/models/$MODEL"
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

pass=0
fail=0
ok()   { echo "  ok   $1"; pass=$((pass + 1)); }
bad()  { echo "  FAIL $1"; fail=$((fail + 1)); }

# An MP3 starts with an ID3v2 tag ("ID3") or an MPEG frame sync (0xFFE).
# The built-in encoder writes a LAME-style Info tag frame, so there is no
# ID3v2 header: accept either shape.
mp3_valid() {
  local f="$1" b1 b2
  head -c 3 "$f" | grep -q ID3 && return 0
  read -r b1 b2 <<<"$(head -c 2 "$f" | od -An -tu1)"
  [ "${b1:-0}" = "255" ] && [ $(( ${b2:-0} & 224 )) -eq 224 ]
}

echo "health"
curl -sf "$BASE/api/health" >/dev/null && ok "GET /api/health" || bad "GET /api/health"

echo "engines"
# Captured once: piping curl into `grep -q` trips pipefail with SIGPIPE.
engines_json=$(curl -sf "$BASE/api/engines")
case "$engines_json" in
  *"\"$MODEL\""*) ok "engine $MODEL registered" ;;
  *) bad "engine $MODEL missing" ;;
esac

echo "ui"
index_html=$(curl -sf "$BASE/")
case "$index_html" in
  *Laverock*) ok "index page" ;;
  *) bad "index page" ;;
esac
case "$index_html" in
  *'id="page-settings"'*) ok "sidebar pages" ;;
  *) bad "sidebar pages" ;;
esac

echo "settings"
settings_json=$(curl -s "$BASE/api/settings")
curl -sf "$BASE/api/settings" >/dev/null && ok "GET /api/settings" || bad "GET /api/settings"
case "$settings_json" in
  *'"proxy_mode":"system"'*|*'"proxy_mode":"manual"'*|*'"proxy_mode":"direct"'*)
    ok "proxy mode reported" ;;
  *) bad "proxy mode missing" ;;
esac

code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/settings" \
  -H 'Content-Type: application/json' -d '{"proxy_mode":"manual","proxy":"not-a-url"}')
[ "$code" = "400" ] && ok "invalid proxy -> 400" || bad "invalid proxy -> $code"

code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/settings" \
  -H 'Content-Type: application/json' \
  -d '{"proxy_mode":"manual","proxy":"http://127.0.0.1:10808"}')
[ "$code" = "200" ] && ok "save settings" || bad "save settings -> $code"

# Put the user's settings back.
curl -s -o /dev/null -X POST "$BASE/api/settings" \
  -H 'Content-Type: application/json' -d "$settings_json"

echo "models"
models_json=$(curl -sf "$BASE/api/models")
case "$models_json" in
  *'"id":"zipvoice"'*) ok "model catalog" ;;
  *) bad "model catalog" ;;
esac
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/models/nope/install")
[ "$code" = "400" ] && ok "unknown model -> 400" || bad "unknown model -> $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/models/zipvoice/install")
[ "$code" = "400" ] && ok "installed model -> 400" || bad "installed model -> $code"
curl -sf -X POST "$BASE/api/models/reload" >/dev/null \
  && ok "reload engines" || bad "reload engines"

synth() { # name text wav prompt_text [extra...]
  local name="$1" text="$2" wav="$3" ptext="$4"; shift 4
  local code
  code=$(curl -s -o "$OUT/$name.wav" -w '%{http_code}' -X POST \
    "$BASE/api/engines/$MODEL/synthesize" \
    -F "text=$text" -F "prompt=@$DIR/test_wavs/$wav" -F "prompt_text=$ptext" "$@")
  if [ "$code" = "200" ] && head -c 4 "$OUT/$name.wav" | grep -q RIFF; then
    ok "$name ($(wc -c <"$OUT/$name.wav" | tr -d ' ') bytes)"
  else
    bad "$name (HTTP $code)"
  fi
}

echo "synthesis"
synth zh "你好，我是 Laverock。" leijun-1.wav \
  "那还是36年前, 1987年. 我呢考上了武汉大学的计算机系."
synth en "Hello, this is Laverock." news-female.wav \
  "各位村民, 大家新年好! 近期, 湖北省武汉市等多个地区"
synth mixed "人工智能 artificial intelligence 正在改变生活。" news-female-2.wav \
  "本台消息, 中共中央国务院, 近日印发关于构建数据基础制度, 更好发挥数据要素作用的意见." \
  -F 'speed=1.1'

echo "mp3 export (built-in encoder)"
if curl -s "$BASE/api/health" | grep -q '"mp3":true'; then
  code=$(curl -s -o "$OUT/zh.mp3" -w '%{http_code}' -X POST "$BASE/api/audio/mp3" \
    --data-binary @"$OUT/zh.wav")
  [ "$code" = "200" ] && mp3_valid "$OUT/zh.mp3" \
    && ok "wav -> mp3" || bad "wav -> mp3 (HTTP $code)"

  hid=$(curl -s "$BASE/api/history" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
  code=$(curl -s -o "$OUT/hist.mp3" -w '%{http_code}' \
    "$BASE/api/history/$hid/audio?format=mp3")
  [ "$code" = "200" ] && mp3_valid "$OUT/hist.mp3" \
    && ok "history mp3" || bad "history mp3 (HTTP $code)"
else
  bad "health lacks mp3 support (built-in encoder should always be available)"
fi

echo "validation"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  "$BASE/api/engines/$MODEL/synthesize" \
  -F 'text=no transcript' -F "prompt=@$DIR/test_wavs/leijun-1.wav")
[ "$code" = "400" ] && ok "missing prompt_text -> 400" || bad "missing prompt_text -> $code"

echo "profiles"
pid=$(curl -s -X POST "$BASE/api/profiles" \
  -F 'name=smoke-test-voice' \
  -F 'prompt_text=那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.' \
  -F "prompt=@$DIR/test_wavs/leijun-1.wav" \
  | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
[ -n "$pid" ] && ok "create profile ($pid)" || bad "create profile"

case "$(curl -sf "$BASE/api/profiles")" in
  *"$pid"*) ok "profile listed" ;;
  *) bad "profile not listed" ;;
esac
curl -sf "$BASE/api/profiles/$pid/audio" -o "$OUT/ref.wav" \
  && head -c 4 "$OUT/ref.wav" | grep -q RIFF \
  && ok "profile audio" || bad "profile audio"

code=$(curl -s -o "$OUT/from-profile.wav" -w '%{http_code}' -X POST \
  "$BASE/api/engines/$MODEL/synthesize" \
  -F 'text=你好，我是 Laverock。' -F "profile_id=$pid")
[ "$code" = "200" ] && head -c 4 "$OUT/from-profile.wav" | grep -q RIFF \
  && ok "synthesize from profile" || bad "synthesize from profile (HTTP $code)"

code=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "$BASE/api/profiles/$pid")
[ "$code" = "204" ] && ok "delete profile" || bad "delete profile -> $code"
curl -sf "$BASE/api/profiles" | grep -q "$pid" \
  && bad "deleted profile still listed" || ok "profile removed"

echo "captures"
# Two takes, and a byte-identical copy of the first.
take_id() { sed -n 's/.*"id":"\([^"]*\)".*/\1/p'; }
c1=$(curl -s -X POST "$BASE/api/captures" -F 'name=smoke-a' \
  -F "audio=@$DIR/test_wavs/leijun-1.wav" | take_id)
c2=$(curl -s -X POST "$BASE/api/captures" -F 'name=smoke-b' \
  -F "audio=@$DIR/test_wavs/news-female.wav" | take_id)
c3=$(curl -s -X POST "$BASE/api/captures" -F 'name=smoke-a-copy' \
  -F "audio=@$DIR/test_wavs/leijun-1.wav" | take_id)
if [ -n "$c1" ] && [ -n "$c2" ] && [ -n "$c3" ]; then
  ok "create takes"
else
  bad "create takes"
fi
captures_json=$(curl -sf "$BASE/api/captures")
case "$captures_json" in
  *"{\"code\":\"no_transcript\"}"*) ok "untranscribed take flagged" ;;
  *) bad "take readiness check" ;;
esac
# Content de-duplication is checked in the unit tests; a global clean-up
# here would touch takes that belong to a real library, so this only
# verifies that the duplicate is reported.
c3_line=$(printf '%s\n' "$captures_json" | sed 's/},{/}\n{/g' | grep "\"$c3\"")
case "$c3_line" in
  *'"duplicate_of":null'*|'') bad "duplicate not flagged" ;;
  *'"duplicate_of":"c'*) ok "duplicate take flagged" ;;
  *) bad "duplicate not flagged" ;;
esac
curl -sf "$BASE/api/captures/$c1/audio" -o "$OUT/take-a.wav" \
  && head -c 4 "$OUT/take-a.wav" | grep -q RIFF \
  && ok "take audio" || bad "take audio"


if curl -s "$BASE/api/health" | grep -q '"transcribe":true'; then
  heard=$(curl -s -X POST "$BASE/api/captures/$c1/transcribe")
  case "$heard" in
    *武汉*) ok "re-transcribe take" ;;
    *) bad "re-transcribe take: $(printf '%s' "$heard" | head -c 160)" ;;
  esac
else
  echo "  skip re-transcribing (no speech-to-text model)"
fi

# A voice made of both takes hands the engine their audio joined.
duo_json=$(curl -s -X POST "$BASE/api/profiles" \
  -F 'name=smoke-test-duo' -F "capture=$c1" -F "capture=$c2")
duo=$(printf '%s' "$duo_json" | take_id)
[ -n "$duo" ] && ok "voice from two takes ($duo)" || bad "voice from two takes"
if [ -n "$duo" ]; then
  case "$duo_json" in
    *'"captures":["'$c1'","'$c2'"]'*) ok "takes recorded on the voice" ;;
    *) bad "take list missing from the new voice" ;;
  esac
  curl -sf "$BASE/api/profiles/$duo/audio" -o "$OUT/duo.wav"
  a=$(wc -c <"$OUT/take-a.wav" | tr -d ' ')
  d=$(wc -c <"$OUT/duo.wav" | tr -d ' ')
  [ "$d" -gt "$a" ] && ok "joined reference covers both takes" \
    || bad "joined reference too short ($d <= $a)"
  code=$(curl -s -o "$OUT/duo-synth.wav" -w '%{http_code}' -X POST \
    "$BASE/api/engines/$MODEL/synthesize" \
    -F 'text=这是用两段素材拼成的声音。' -F "profile_id=$duo")
  [ "$code" = "200" ] && head -c 4 "$OUT/duo-synth.wav" | grep -q RIFF \
    && ok "synthesize from a two-take voice" \
    || bad "synthesize from a two-take voice (HTTP $code)"
  # The OpenAI-compatible endpoint resolves the same voice by name, so it
  # must join the takes too (not just hand over the single fallback clip).
  code=$(curl -s -o "$OUT/duo-oai.wav" -w '%{http_code}' -X POST \
    "$BASE/v1/audio/speech" -H 'Content-Type: application/json' \
    -d "{\"model\":\"$MODEL\",\"voice\":\"smoke-test-duo\",\"input\":\"通过 OpenAI 接口合成。\"}")
  [ "$code" = "200" ] && head -c 4 "$OUT/duo-oai.wav" | grep -q RIFF \
    && ok "openai speech from a two-take voice" \
    || bad "openai speech from a two-take voice (HTTP $code)"
  curl -s -o /dev/null -X DELETE "$BASE/api/profiles/$duo"
fi

# Deleting a take drops it from the voices using it.
solo=$(curl -s -X POST "$BASE/api/profiles" \
  -F 'name=smoke-test-solo' -F "capture=$c1" -F "capture=$c2" | take_id)
curl -s -o /dev/null -X DELETE "$BASE/api/captures/$c2"
if [ -z "$solo" ]; then
  bad "voice from takes for the delete check"
else
  solo_line=$(curl -sf "$BASE/api/profiles" | sed 's/},{/}\n{/g' | grep "\"$solo\"")
  case "$solo_line" in
    *'"captures":["'$c1'"]'*) ok "deleting a take updates voices" ;;
    *) bad "voice still references a deleted take" ;;
  esac
fi
# Only our own takes go; a real library keeps whatever it had.
curl -s -o /dev/null -X DELETE "$BASE/api/captures/$c1"
curl -s -o /dev/null -X DELETE "$BASE/api/captures/$c3"
curl -s -o /dev/null -X DELETE "$BASE/api/profiles/$solo"
ok "cleaned up takes"

if case "$engines_json" in *'"id":"voxcpm2"'*) true ;; *) false ;; esac; then
  echo "voxcpm2"
  code=$(curl -s -o "$OUT/vox-zh.wav" -w '%{http_code}' -X POST \
    "$BASE/api/engines/voxcpm2/synthesize" \
    -F 'text=你好，我是 VoxCPM2，这里不需要参考文本。' \
    -F "prompt=@$DIR/test_wavs/leijun-1.wav")
  [ "$code" = "200" ] && head -c 4 "$OUT/vox-zh.wav" | grep -q RIFF \
    && ok "voxcpm2 clone without transcript" || bad "voxcpm2 (HTTP $code)"
else
  echo "  skip voxcpm2 (not installed)"
fi

if case "$engines_json" in *'"id":"qwen3tts"'*) true ;; *) false ;; esac; then
  echo "qwen3tts"
  code=$(curl -s -o "$OUT/qwen.wav" -w '%{http_code}' -X POST \
    "$BASE/api/engines/qwen3tts/synthesize" \
    -F 'text=你好，我是 Qwen3-TTS，通过 laverock 调用。' \
    -F "prompt=@$DIR/test_wavs/leijun-1.wav")
  [ "$code" = "200" ] && head -c 4 "$OUT/qwen.wav" | grep -q RIFF \
    && ok "qwen3tts clone" || bad "qwen3tts (HTTP $code)"
else
  echo "  skip qwen3tts (not installed)"
fi

if case "$engines_json" in *'"id":"cosyvoice"'*) true ;; *) false ;; esac; then
  echo "cosyvoice"
  code=$(curl -s -o "$OUT/cosy.wav" -w '%{http_code}' --max-time 600 -X POST \
    "$BASE/api/engines/cosyvoice/synthesize" \
    -F 'text=你好，我是 CosyVoice3，通过 laverock 调用。' \
    -F "prompt=@$DIR/test_wavs/leijun-1.wav" \
    -F 'prompt_text=那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.')
  [ "$code" = "200" ] && head -c 4 "$OUT/cosy.wav" | grep -q RIFF \
    && ok "cosyvoice clone" || bad "cosyvoice (HTTP $code)"
else
  echo "  skip cosyvoice (not installed)"
fi

if case "$engines_json" in *'"id":"dots"'*) true ;; *) false ;; esac; then
  echo "dots"
  code=$(curl -s -o "$OUT/dots.wav" -w '%{http_code}' --max-time 600 -X POST \
    "$BASE/api/engines/dots/synthesize" \
    -F 'text=你好，我是 dots.tts，通过 laverock 调用。' \
    -F "prompt=@$DIR/test_wavs/leijun-1.wav")
  [ "$code" = "200" ] && head -c 4 "$OUT/dots.wav" | grep -q RIFF \
    && ok "dots clone" || bad "dots (HTTP $code)"
else
  echo "  skip dots (not installed)"
fi

if case "$engines_json" in *'"id":"longcat"'*) true ;; *) false ;; esac; then
  echo "longcat"
  code=$(curl -s -o "$OUT/longcat.wav" -w '%{http_code}' --max-time 600 -X POST \
    "$BASE/api/engines/longcat/synthesize" \
    -F 'text=你好，我是 LongCat AudioDiT，通过 laverock 调用。' \
    -F "prompt=@$DIR/test_wavs/leijun-1.wav" \
    -F 'prompt_text=那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.')
  [ "$code" = "200" ] && head -c 4 "$OUT/longcat.wav" | grep -q RIFF \
    && ok "longcat clone" || bad "longcat (HTTP $code)"
else
  echo "  skip longcat (not installed)"
fi

if case "$engines_json" in *'"kind":"kokoro"'*) true ;; *) false ;; esac; then
  echo "kokoro"
  code=$(curl -s -o "$OUT/kokoro.wav" -w '%{http_code}' --max-time 120 -X POST \
    "$BASE/api/engines/kokoro-int8-multi-lang-v1_0/synthesize" \
    -F 'text=你好，我是 Kokoro，通过 laverock 调用。' \
    -F 'voice=47')
  [ "$code" = "200" ] && head -c 4 "$OUT/kokoro.wav" | grep -q RIFF \
    && ok "kokoro preset voice (no reference audio)" || bad "kokoro (HTTP $code)"
else
  echo "  skip kokoro (not installed)"
fi

echo "remote access"
PORT=$(echo "$BASE" | sed -n 's|.*:\([0-9][0-9]*\).*|\1|p')
PORT=${PORT:-8787}
LAN_IP=$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null || hostname -I 2>/dev/null | awk '{print $1}')
if [ -n "$LAN_IP" ]; then
  curl -s -X POST "$BASE/api/settings/lan" -H 'Content-Type: application/json' \
    -d '{"enabled":true}' >/dev/null
  sleep 0.5

  code=$(curl -sk -o /dev/null -w '%{http_code}' "https://$LAN_IP:$PORT/api/engines")
  [ "$code" = "401" ] && ok "LAN requires pairing" || bad "LAN without token -> $code"

  pair=$(curl -s -X POST "$BASE/api/pair/start")
  pcode=$(echo "$pair" | sed -n 's/.*"code":"\([0-9]*\)".*/\1/p')
  jar=$(mktemp)
  curl -sk -c "$jar" -X POST "https://$LAN_IP:$PORT/api/pair/claim" \
    -H 'Content-Type: application/json' \
    -d "{\"code\":\"$pcode\",\"device_name\":\"smoke-test\"}" >/dev/null
  code=$(curl -sk -b "$jar" -o /dev/null -w '%{http_code}' "https://$LAN_IP:$PORT/api/engines")
  [ "$code" = "200" ] && ok "paired LAN access" || bad "paired LAN access -> $code"
  rm -f "$jar"

  did=$(curl -s "$BASE/api/devices" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
  code=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "$BASE/api/devices/$did")
  [ "$code" = "204" ] && ok "revoke device" || bad "revoke device -> $code"

  curl -s -X POST "$BASE/api/settings/lan" -H 'Content-Type: application/json' \
    -d '{"enabled":false}' >/dev/null
else
  echo "  skip (no LAN address found)"
fi

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
