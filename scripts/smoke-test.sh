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

echo "mp3 export"
if curl -s "$BASE/api/health" | grep -q '"mp3":true'; then
  code=$(curl -s -o "$OUT/zh.mp3" -w '%{http_code}' -X POST "$BASE/api/audio/mp3" \
    --data-binary @"$OUT/zh.wav")
  [ "$code" = "200" ] && head -c 3 "$OUT/zh.mp3" | grep -q ID3 \
    && ok "wav -> mp3" || bad "wav -> mp3 (HTTP $code)"

  hid=$(curl -s "$BASE/api/history" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
  code=$(curl -s -o "$OUT/hist.mp3" -w '%{http_code}' \
    "$BASE/api/history/$hid/audio?format=mp3")
  [ "$code" = "200" ] && head -c 3 "$OUT/hist.mp3" | grep -q ID3 \
    && ok "history mp3" || bad "history mp3 (HTTP $code)"
else
  echo "  skip (no ffmpeg/lame)"
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
