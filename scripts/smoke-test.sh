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
curl -sf "$BASE/api/engines" | grep -q "\"$MODEL\"" \
  && ok "engine $MODEL registered" || bad "engine $MODEL missing"

echo "ui"
curl -sf "$BASE/" | grep -q "Laverock" && ok "index page" || bad "index page"

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

curl -sf "$BASE/api/profiles" | grep -q "$pid" \
  && ok "profile listed" || bad "profile not listed"
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

echo "remote access"
PORT=$(echo "$BASE" | sed -n 's|.*:\([0-9][0-9]*\).*|\1|p')
PORT=${PORT:-8787}
LAN_IP=$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null || hostname -I 2>/dev/null | awk '{print $1}')
if [ -n "$LAN_IP" ]; then
  curl -s -X POST "$BASE/api/settings/lan" -H 'Content-Type: application/json' \
    -d '{"enabled":true}' >/dev/null
  sleep 0.5

  code=$(curl -s -o /dev/null -w '%{http_code}' "http://$LAN_IP:$PORT/api/engines")
  [ "$code" = "401" ] && ok "LAN requires pairing" || bad "LAN without token -> $code"

  pair=$(curl -s -X POST "$BASE/api/pair/start")
  pcode=$(echo "$pair" | sed -n 's/.*"code":"\([0-9]*\)".*/\1/p')
  jar=$(mktemp)
  curl -s -c "$jar" -X POST "http://$LAN_IP:$PORT/api/pair/claim" \
    -H 'Content-Type: application/json' \
    -d "{\"code\":\"$pcode\",\"device_name\":\"smoke-test\"}" >/dev/null
  code=$(curl -s -b "$jar" -o /dev/null -w '%{http_code}' "http://$LAN_IP:$PORT/api/engines")
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
