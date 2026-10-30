#!/usr/bin/env bash
# End-to-end test of the chain a user actually walks:
#
#   record      a WAV on disk stands in for the microphone
#   transcribe  POST /api/transcribe turns it into a transcript
#   synthesize  that transcript goes straight back in as prompt_text, so the
#               clone runs on machine output, not on a hand-written string
#   export      history WAV/MP3 plus a voice package that is re-imported
#
# Requires ZipVoice and SenseVoice installed; everything else is skipped.
# Unlike smoke-test.sh, which sweeps features, this fails when any link in
# the chain is missing rather than skipping it.
#
#   ./scripts/e2e-test.sh [base_url] [model_id] [reference.wav]
set -euo pipefail

BASE="${1:-http://127.0.0.1:8787}"
MODEL="${2:-sherpa-onnx-zipvoice-distill-int8-zh-en-emilia}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/models/$MODEL"
REF="${3:-$DIR/test_wavs/leijun-1.wav}"
OUT="$(mktemp -d)"
NAME="e2e-voice-$$"
trap 'rm -rf "$OUT"' EXIT

pass=0
fail=0
ok()  { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }
say() { echo "$1"; }
# Ids currently in history; a failed grep must not abort the script.
ids() {
  curl -s "$BASE/api/history" | grep -o '"id":"[^"]*"' | cut -d'"' -f4 | sort || true
}
# Blank lines from an empty list would otherwise look like a new entry.
listed() { printf '%s\n' "$1" | sed '/^$/d'; }
# An MP3 starts with an ID3v2 tag ("ID3") or an MPEG frame sync (0xFFE).
mp3_valid() {
  local f="$1" b1 b2
  head -c 3 "$f" | grep -q ID3 && return 0
  read -r b1 b2 <<<"$(head -c 2 "$f" | od -An -tu1)"
  [ "${b1:-0}" = "255" ] && [ $(( ${b2:-0} & 224 )) -eq 224 ]
}

[ -f "$REF" ] || { echo "reference clip not found: $REF" >&2; exit 1; }

# ---------------------------------------------------------------- preflight
say "preflight"
health=$(curl -sf "$BASE/api/health") || { echo "server not reachable at $BASE" >&2; exit 1; }
case "$health" in
  *'"transcribe":true'*) ok "speech-to-text available" ;;
  *) bad "speech-to-text missing (run ./scripts/download-stt.sh)" ;;
esac
case "$(curl -sf "$BASE/api/engines")" in
  *"\"$MODEL\""*) ok "engine $MODEL registered" ;;
  *) bad "engine $MODEL missing" ;;
esac
[ "$fail" -eq 0 ] || { echo; echo "cannot continue without the models above" >&2; exit 1; }

# ---------------------------------------------------------------- transcribe
say "record -> transcribe"
code=$(curl -s -o "$OUT/asr.json" -w '%{http_code}' -X POST "$BASE/api/transcribe" \
  -F "audio=@$REF")
text=""
if [ "$code" = "200" ]; then
  text=$(sed -e 's/^.*"text":"//' -e 's/"}$//' "$OUT/asr.json")
fi
if [ "$code" = "200" ] && [ "${#text}" -ge 5 ]; then
  ok "POST /api/transcribe (${#text} chars)"
  say "       heard: $text"
else
  bad "POST /api/transcribe (HTTP $code) $(head -c 200 "$OUT/asr.json")"
fi
# The clip is Li Jun talking about 武汉大学; a transcript that lacks any of
# it means the recogniser or the upload path is wrong.
case "$text" in
  *武汉*|*武漢*) ok "transcript matches the recording" ;;
  *) bad "transcript does not match the recording" ;;
esac
[ "$fail" -eq 0 ] || { echo; echo "transcription failed; stopping here" >&2; exit 1; }

# ---------------------------------------------------------------- synthesize
say "transcribe -> synthesize"
before=$(ids)
code=$(curl -s -o "$OUT/chain.wav" -w '%{http_code}' -X POST \
  "$BASE/api/engines/$MODEL/synthesize" \
  -F 'text=这是一次端到端测试：录音、转写、合成、导出。' \
  -F "prompt=@$REF" -F "prompt_text=$text")
if [ "$code" = "200" ] && head -c 4 "$OUT/chain.wav" | grep -q RIFF; then
  ok "clone synthesized from the ASR transcript ($(wc -c <"$OUT/chain.wav" | tr -d ' ') bytes)"
else
  bad "clone synthesis (HTTP $code)"
fi

# ---------------------------------------------------------------- history
say "history"
hid=""
if [ "$code" = "200" ]; then
  after=$(ids)
  hid=$(comm -13 <(listed "$before") <(listed "$after") | head -1)
fi
if [ -n "$hid" ]; then
  ok "entry recorded ($hid)"
else
  bad "history has no new entry"
fi

if [ -n "$hid" ]; then
  curl -sf "$BASE/api/history/$hid/audio" -o "$OUT/hist.wav" \
    && head -c 4 "$OUT/hist.wav" | grep -q RIFF \
    && ok "history WAV" || bad "history WAV"
  code=$(curl -s -o "$OUT/hist.mp3" -w '%{http_code}' "$BASE/api/history/$hid/audio?format=mp3")
  if [ "$code" = "200" ] && mp3_valid "$OUT/hist.mp3"; then
    ok "history MP3"
  else
    bad "history MP3 (HTTP $code)"
  fi

  say "export -> import"
  pid=$(curl -s -X POST "$BASE/api/profiles" \
    -F "name=$NAME" -F "prompt_text=$text" -F "prompt=@$REF" \
    | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  [ -n "$pid" ] && ok "voice saved ($pid)" || bad "voice save"

  if [ -n "$pid" ]; then
    code=$(curl -s -o "$OUT/voice.zip" -w '%{http_code}' "$BASE/api/profiles/$pid/export")
    if [ "$code" = "200" ] && [ "$(head -c 2 "$OUT/voice.zip")" = "PK" ]; then
      ok "voice package exported ($(wc -c <"$OUT/voice.zip" | tr -d ' ') bytes)"
    else
      bad "voice export (HTTP $code)"
    fi

    curl -s -o /dev/null -X DELETE "$BASE/api/profiles/$pid"
    report=$(curl -s -X POST "$BASE/api/profiles/import" -F "file=@$OUT/voice.zip")
    case "$report" in
      *'"imported":1'*) ok "voice package imported" ;;
      *) bad "voice import: $report" ;;
    esac

    # Clean up the re-imported copy (its id is new).
    pid2=$(curl -s "$BASE/api/profiles" | sed 's/},{/}\n{/g' \
      | grep "\"$NAME\"" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
    [ -z "$pid2" ] || curl -s -o /dev/null -X DELETE "$BASE/api/profiles/$pid2"
  fi

  curl -s -o /dev/null -X DELETE "$BASE/api/history/$hid"
  ok "cleaned up"
fi

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
