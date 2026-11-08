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

# Feature groups can be run in isolation while a lane is being developed:
#   SMOKE_ONLY=synthesis,mp3 ./scripts/smoke-test.sh
# `all` (the default) runs everything. `health` and `engines` always run:
# every other group needs the engine list and a reachable server.
ONLY="${SMOKE_ONLY:-all}"
want() {
  [ "$ONLY" = "all" ] && return 0
  case ",$ONLY," in *",$1,"*) return 0 ;; *) return 1 ;; esac
}

# Windows shells cannot carry non-ASCII through a command line: Git Bash
# replaces every CJK character with '?' before curl sees it, which turns a
# Chinese transcript into punctuation and makes ZipVoice's duration predictor
# run away. Hand those values over in a file and let curl read the bytes.
field() { # field <name> <value> -> one -F argument
  local f
  f="$(mktemp "$OUT/field.XXXXXX")"
  printf '%s' "$2" >"$f"
  printf '%s=<%s' "$1" "$f"
}

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

if want ui; then
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
fi

if want settings; then
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
fi

if want models; then
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
fi

synth() { # name text wav prompt_text [extra...]
  local name="$1" text="$2" wav="$3" ptext="$4"; shift 4
  local code
  code=$(curl -s -o "$OUT/$name.wav" -w '%{http_code}' -X POST \
    "$BASE/api/engines/$MODEL/synthesize" \
    -F "$(field text "$text")" -F "prompt=@$DIR/test_wavs/$wav" \
    -F "$(field prompt_text "$ptext")" "$@")
  if [ "$code" = "200" ] && head -c 4 "$OUT/$name.wav" | grep -q RIFF; then
    ok "$name ($(wc -c <"$OUT/$name.wav" | tr -d ' ') bytes)"
  else
    bad "$name (HTTP $code)"
  fi
}

# The MP3 check reuses the WAV from the synthesis group.
if want synthesis || want mp3; then
echo "synthesis"
synth zh "你好，我是 Laverock。" leijun-1.wav \
  "那还是36年前, 1987年. 我呢考上了武汉大学的计算机系."
synth en "Hello, this is Laverock." news-female.wav \
  "各位村民, 大家新年好! 近期, 湖北省武汉市等多个地区"
synth mixed "人工智能 artificial intelligence 正在改变生活。" news-female-2.wav \
  "本台消息, 中共中央国务院, 近日印发关于构建数据基础制度, 更好发挥数据要素作用的意见." \
  -F 'speed=1.1'
fi

if want jobs; then
echo "jobs"
job_json=$(curl -s -X POST "$BASE/api/engines/$MODEL/synthesize" \
  -H 'x-laverock-job: 1' \
  -F "$(field text '你好，我是 Laverock 的后台任务。')" \
  -F "prompt=@$DIR/test_wavs/leijun-1.wav" \
  -F "$(field prompt_text '那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.')")
job_id=$(printf '%s' "$job_json" | sed -n 's/.*"job_id":"\([^"]*\)".*/\1/p')
[ -n "$job_id" ] && ok "start job ($job_id)" || bad "start job: $job_json"

job_status() { # job_status <id>
  curl -s "$BASE/api/jobs/$1" | sed -n 's/.*"status":"\([^"]*\)".*/\1/p'
}
status=""
for _ in $(seq 1 300); do
  status=$(job_status "$job_id")
  case "$status" in done|error|cancelled) break ;; esac
  sleep 1
done
[ "$status" = "done" ] && ok "job finished" || bad "job status: $status"

# A finished job replays its buffered events to a late subscriber.
events=$(curl -sN --max-time 10 "$BASE/api/jobs/$job_id/events")
case "$events" in *"event: snapshot"*) ok "event snapshot" ;; *) bad "event snapshot" ;; esac
case "$events" in *"event: done"*) ok "event done" ;; *) bad "event done" ;; esac

code=$(curl -s -o "$OUT/job-take.wav" -w '%{http_code}' \
  "$BASE/api/jobs/$job_id/files/take-0.wav")
[ "$code" = "200" ] && head -c 4 "$OUT/job-take.wav" | grep -q RIFF \
  && ok "job artifact" || bad "job artifact (HTTP $code)"

code=$(curl -s -o /dev/null -w '%{http_code}' \
  "$BASE/api/jobs/$job_id/files/..%2F..%2Fsettings.json")
case "$code" in
  400|404) ok "artifact traversal rejected" ;;
  *) bad "artifact traversal -> $code" ;;
esac

code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/jobs/does-not-exist")
[ "$code" = "404" ] && ok "unknown job -> 404" || bad "unknown job -> $code"

# Cancelling stops the run between takes: with two requested, the job
# must end `cancelled` (possibly after the take already in flight).
cancel_json=$(curl -s -X POST "$BASE/api/engines/$MODEL/synthesize" \
  -H 'x-laverock-job: 1' \
  -F "$(field text '这个任务会被取消。')" \
  -F "prompt=@$DIR/test_wavs/leijun-1.wav" \
  -F "$(field prompt_text '那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.')" \
  -F 'samples=2')
cancel_id=$(printf '%s' "$cancel_json" | sed -n 's/.*"job_id":"\([^"]*\)".*/\1/p')
if [ -n "$cancel_id" ]; then
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/jobs/$cancel_id/cancel")
  [ "$code" = "202" ] && ok "cancel accepted" || bad "cancel -> $code"
  status=""
  for _ in $(seq 1 300); do
    status=$(job_status "$cancel_id")
    case "$status" in done|error|cancelled) break ;; esac
    sleep 1
  done
  [ "$status" = "cancelled" ] && ok "job cancelled" || bad "cancel status: $status"
else
  bad "start job to cancel: $cancel_json"
fi
fi

if want stream; then
echo "streaming synthesis"
# Four sentences, over the server's 100-character split budget, so the
# first audio frame must arrive before the whole text is done.
stream_text='你好，我是 Laverock 的流式合成演示。这是一段比较长的文字，用来验证第一句生成后就能开始播放，而不是等整段完成。流式合成会按句子切分文本，每生成一段就立刻发送一个音频帧，浏览器收到后按顺序排入播放队列。这样长文本的首字节延迟就缩短到了第一句的生成时间。'
stream_out=$(curl -sN --max-time 600 -X POST \
  "$BASE/api/engines/$MODEL/synthesize" \
  -H 'X-Laverock-Stream: 1' \
  -F "$(field text "$stream_text")" \
  -F "prompt=@$DIR/test_wavs/leijun-1.wav" \
  -F "$(field prompt_text '那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.')" \
  -F 'stream_audio=1')
frames=$(printf '%s\n' "$stream_out" | grep -c '^event: audio$' || true)
[ "$frames" -ge 2 ] && ok "streamed $frames audio frames" \
  || bad "streamed only $frames audio frames"
case "$stream_out" in
  *"event: wav"*) ok "stream final wav" ;;
  *) bad "stream final wav missing" ;;
esac
case "$stream_out" in
  *"event: done"*) ok "stream finished" ;;
  *) bad "stream did not finish" ;;
esac
fi

if want resources; then
echo "resources"
# Synthesis above leaves the engine loaded; unloading is what the Models
# page and the Settings resource panel drive.
synth resources "你好，我是 Laverock。" leijun-1.wav \
  "那还是36年前, 1987年. 我呢考上了武汉大学的计算机系."
engine_entry() { # engine_entry -> the JSON fields after $MODEL's id
  local line
  line=$(curl -sf "$BASE/api/engines")
  case "$line" in
    *"\"id\":\"$MODEL\""*) ;;
    *) echo missing; return ;;
  esac
  printf '%s' "${line#*\"id\":\"$MODEL\"}"
}
loaded_state() { # loaded_state -> true / false / missing
  local rest
  rest=$(engine_entry)
  case "$rest" in
    missing) echo missing; return ;;
  esac
  rest=${rest#*\"loaded\":}
  case "$rest" in
    true*) echo true ;;
    false*) echo false ;;
    *) echo missing ;;
  esac
}
engine_backend() { # engine_backend -> onnx / audiocpp / llama.cpp / python
  local rest
  rest=$(engine_entry)
  rest=${rest#*\"backend\":\"}
  printf '%s' "${rest%%\"*}"
}
state=$(loaded_state)
[ "$state" = "true" ] && ok "engine loaded after synthesis" \
  || bad "engine loaded state: $state"
backend=$(engine_backend)
case "$backend" in
  onnx|audiocpp|llama.cpp|python) ok "engine backend ($backend)" ;;
  *) bad "engine backend: $backend" ;;
esac
system_json=$(curl -sf "$BASE/api/system")
case "$system_json" in
  *'"rss":'*) ok "system memory snapshot" ;;
  *) bad "system memory missing" ;;
esac
case "$system_json" in
  *'"backends":['*']'*) ok "system backends listed" ;;
  *) bad "system backends missing" ;;
esac
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/engines/$MODEL/unload")
[ "$code" = "204" ] && ok "unload accepted" || bad "unload -> $code"
state=$(loaded_state)
[ "$state" = "false" ] && ok "engine unloaded" || bad "engine still loaded: $state"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/engines/nope/unload")
[ "$code" = "404" ] && ok "unknown engine unload -> 404" || bad "unknown unload -> $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/engines/unload")
[ "$code" = "204" ] && ok "unload all accepted" || bad "unload all -> $code"
fi

if want mp3; then
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
fi

if want validation; then
echo "validation"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  "$BASE/api/engines/$MODEL/synthesize" \
  -F 'text=no transcript' -F "prompt=@$DIR/test_wavs/leijun-1.wav")
[ "$code" = "400" ] && ok "missing prompt_text -> 400" || bad "missing prompt_text -> $code"
fi

if want profiles; then
echo "profiles"
pid=$(curl -s -X POST "$BASE/api/profiles" \
  -F 'name=smoke-test-voice' \
  -F "$(field prompt_text '那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.')" \
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
  -F "$(field text '你好，我是 Laverock。')" -F "profile_id=$pid")
[ "$code" = "200" ] && head -c 4 "$OUT/from-profile.wav" | grep -q RIFF \
  && ok "synthesize from profile" || bad "synthesize from profile (HTTP $code)"

code=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "$BASE/api/profiles/$pid")
[ "$code" = "204" ] && ok "delete profile" || bad "delete profile -> $code"
curl -sf "$BASE/api/profiles" | grep -q "$pid" \
  && bad "deleted profile still listed" || ok "profile removed"
# A saved voice's clip is also a library take, and takes outlive voices by
# design. Clean it up so later groups (and the browser test) see a library
# with only the takes they created.
for cid in $(curl -s "$BASE/api/captures" | sed 's/},{/}\n{/g' \
  | grep -F '"name":"smoke-test-voice"' \
  | sed -n 's/.*"id":"\([^"]*\)".*/\1/p'); do
  curl -s -o /dev/null -X DELETE "$BASE/api/captures/$cid"
done
fi

if want captures; then
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
# Which of two byte-identical takes is canonical depends on their shared
# creation second and the directory read order, so only require that the
# pair reports the duplication at all.
c3_line=$(printf '%s\n' "$captures_json" | sed 's/},{/}\n{/g' \
  | grep -E "\"($c1|$c3)\"")
case "$c3_line" in
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
    -F "$(field text '这是用两段素材拼成的声音。')" -F "profile_id=$duo")
  [ "$code" = "200" ] && head -c 4 "$OUT/duo-synth.wav" | grep -q RIFF \
    && ok "synthesize from a two-take voice" \
    || bad "synthesize from a two-take voice (HTTP $code)"
  # The OpenAI-compatible endpoint resolves the same voice by name, so it
  # must join the takes too (not just hand over the single fallback clip).
  printf '{"model":"%s","voice":"smoke-test-duo","input":"通过 OpenAI 接口合成。"}' \
    "$MODEL" >"$OUT/oai.json"
  code=$(curl -s -o "$OUT/duo-oai.wav" -w '%{http_code}' -X POST \
    "$BASE/v1/audio/speech" -H 'Content-Type: application/json' \
    --data-binary @"$OUT/oai.json")
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
fi

if want subtitles; then
echo "subtitles"
if curl -s "$BASE/api/health" | grep -q '"transcribe":true'; then
  take_id() { sed -n 's/.*"id":"\([^"]*\)".*/\1/p'; }
  details=$(curl -s -X POST "$BASE/api/transcribe/details" \
    -F "audio=@$DIR/test_wavs/leijun-1.wav")
  case "$details" in
    *'"segments":[{'*) ok "detailed transcript segments" ;;
    *) bad "details: $(printf '%s' "$details" | head -c 160)" ;;
  esac
  case "$details" in
    *武汉*) ok "detailed transcript text" ;;
    *) bad "detailed transcript text" ;;
  esac
  case "$details" in
    *'"duration":'*) ok "detailed transcript duration" ;;
    *) bad "details duration missing" ;;
  esac

  srt=$(curl -s -X POST "$BASE/api/transcribe/subtitles?format=srt" \
    -F "audio=@$DIR/test_wavs/leijun-1.wav")
  case "$srt" in
    *' --> '*) ok "uploaded subtitle cues" ;;
    *) bad "srt: $(printf '%s' "$srt" | head -c 120)" ;;
  esac
  vtt=$(curl -s -X POST "$BASE/api/transcribe/subtitles?format=vtt" \
    -F "audio=@$DIR/test_wavs/leijun-1.wav")
  case "$vtt" in
    WEBVTT*) ok "vtt header" ;;
    *) bad "vtt: $(printf '%s' "$vtt" | head -c 120)" ;;
  esac
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST \
    "$BASE/api/transcribe/subtitles?format=ass" \
    -F "audio=@$DIR/test_wavs/leijun-1.wav")
  [ "$code" = "400" ] && ok "unknown subtitle format -> 400" \
    || bad "unknown subtitle format -> $code"

  sub=$(curl -s -X POST "$BASE/api/captures" -F 'name=smoke-subs' \
    -F "audio=@$DIR/test_wavs/leijun-1.wav" | take_id)
  heard=$(curl -s -X POST "$BASE/api/captures/$sub/transcribe")
  case "$heard" in
    *'"transcript_segments":[{'*) ok "take stores timed cues" ;;
    *) bad "take cues missing: $(printf '%s' "$heard" | head -c 160)" ;;
  esac
  take_srt=$(curl -s "$BASE/api/captures/$sub/subtitles?format=srt")
  case "$take_srt" in
    *' --> '*) ok "take subtitle download" ;;
    *) bad "take subtitles: $(printf '%s' "$take_srt" | head -c 120)" ;;
  esac
  curl -s -o /dev/null -X DELETE "$BASE/api/captures/$sub"
else
  echo "  skip (no speech-to-text model)"
fi
fi

if want tools; then
echo "tools"
tools_json=$(curl -sf "$BASE/api/tools")
case "$tools_json" in
  *'"denoise":'*) ok "tool availability" ;;
  *) bad "tool availability" ;;
esac
case "$tools_json" in
  *'"separation":'*) ok "separation availability" ;;
  *) bad "separation availability" ;;
esac
case "$tools_json" in
  *'"mp3":true'*) ok "mp3 availability" ;;
  *) bad "mp3 availability" ;;
esac

# The tools page is WAV-only; a renamed text file is rejected before a job
# is created, and an unknown mode is a client error too.
printf 'not a wav\n' >"$OUT/not-a-wav.txt"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/tools/enhance" \
  -F 'mode=denoise' -F "audio=@$OUT/not-a-wav.txt;filename=note.wav")
[ "$code" = "400" ] && ok "non-WAV -> 400" || bad "non-WAV -> $code"

code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/tools/enhance" \
  -F 'mode=loud' -F "audio=@$DIR/test_wavs/leijun-1.wav")
[ "$code" = "400" ] && ok "unknown tool mode -> 400" || bad "unknown tool mode -> $code"

tool_job() { # tool_job <mode> -> job id
  curl -s -X POST "$BASE/api/tools/enhance" \
    -F "mode=$1" -F "audio=@$DIR/test_wavs/leijun-1.wav" \
    | sed -n 's/.*"job_id":"\([^"]*\)".*/\1/p'
}
tool_wait() { # tool_wait <id> -> terminal status
  local id="$1" status=""
  for _ in $(seq 1 300); do
    status=$(curl -s "$BASE/api/jobs/$id" \
      | sed -n 's/.*"status":"\([^"]*\)".*/\1/p')
    case "$status" in done|error|cancelled) break ;; esac
    sleep 1
  done
  printf '%s' "$status"
}

if case "$tools_json" in *'"denoise":true'*) true ;; *) false ;; esac; then
  tid=$(tool_job denoise)
  [ -n "$tid" ] && ok "start denoise job ($tid)" || bad "start denoise job"
  status=$(tool_wait "$tid")
  [ "$status" = "done" ] && ok "denoise finished" || bad "denoise status: $status"
  events=$(curl -sN --max-time 10 "$BASE/api/jobs/$tid/events")
  case "$events" in
    *"event: stage"*) ok "tool stage events" ;;
    *) bad "tool stage events missing" ;;
  esac
  code=$(curl -s -o "$OUT/denoised.wav" -w '%{http_code}' \
    "$BASE/api/jobs/$tid/files/denoised.wav")
  [ "$code" = "200" ] && head -c 4 "$OUT/denoised.wav" | grep -q RIFF \
    && ok "denoise artifact" || bad "denoise artifact (HTTP $code)"
else
  echo "  skip denoise job (model not installed)"
fi

if case "$tools_json" in *'"separation":true'*) true ;; *) false ;; esac; then
  tid=$(tool_job both)
  [ -n "$tid" ] && ok "start stem job ($tid)" || bad "start stem job"
  status=$(tool_wait "$tid")
  [ "$status" = "done" ] && ok "stem separation finished" \
    || bad "stem separation status: $status"
  for name in vocals.wav instrumental.wav; do
    code=$(curl -s -o "$OUT/$name" -w '%{http_code}' \
      "$BASE/api/jobs/$tid/files/$name")
    [ "$code" = "200" ] && head -c 4 "$OUT/$name" | grep -q RIFF \
      && ok "stem artifact $name" || bad "stem artifact $name (HTTP $code)"
  done
  code=$(curl -s -o "$OUT/stems.zip" -w '%{http_code}' \
    "$BASE/api/jobs/$tid/files/stems.zip")
  [ "$code" = "200" ] && head -c 2 "$OUT/stems.zip" | grep -q PK \
    && ok "stems zip" || bad "stems zip (HTTP $code)"

  # Separation runs at about twice real time, so cancelling a six-second
  # clip always lands while the separator is still working.
  cid=$(tool_job instrumental)
  code=$(curl -s -o /dev/null -w '%{http_code}' \
    -X POST "$BASE/api/jobs/$cid/cancel")
  [ "$code" = "202" ] && ok "tool cancel accepted" || bad "tool cancel -> $code"
  status=$(tool_wait "$cid")
  [ "$status" = "cancelled" ] && ok "tool job cancelled" \
    || bad "tool cancel status: $status"
else
  echo "  skip stem separation (model not installed)"
  # A missing pack is a setup problem, not a server error.
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/tools/enhance" \
    -F 'mode=vocals' -F "audio=@$DIR/test_wavs/leijun-1.wav")
  [ "$code" = "400" ] && ok "missing separation model -> 400" \
    || bad "missing separation model -> $code"
fi
fi

if want projects; then
echo "projects"
project_id() { sed -n 's/^{"id":"\([^"]*\)".*/\1/p'; }
printf '1\n00:00:00,000 --> 00:00:01,000\nhello\n\n2\n00:00:01,200 --> 00:00:02,400\nworld\n' \
  >"$OUT/cues.srt"
pj=$(curl -s -X POST "$BASE/api/projects" -F 'name=smoke-project' \
  -F "audio=@$DIR/test_wavs/leijun-1.wav" -F "subtitles=<$OUT/cues.srt" | project_id)
[ -n "$pj" ] && ok "create project ($pj)" || bad "create project"
case "$(curl -s "$BASE/api/projects")" in
  *"\"$pj\""*) ok "project listed" ;;
  *) bad "project missing from list" ;;
esac
pj_json=$(curl -s "$BASE/api/projects/$pj")
case "$pj_json" in
  *'"segments":[{"id":"s1","start":0.0,"end":1.0,"text":"hello"'*)
    ok "imported cues" ;;
  *) bad "imported cues: $(printf '%s' "$pj_json" | head -c 160)" ;;
esac
curl -sf "$BASE/api/projects/$pj/source" -o "$OUT/project.wav" \
  && head -c 4 "$OUT/project.wav" | grep -q RIFF \
  && ok "project source" || bad "project source"

# PATCH replaces the whole cue list, as the editor sends it.
printf '{"name":"smoke-project-2","segments":[{"id":"s1","start":0.0,"end":1.5,"text":"edited"}]}' \
  >"$OUT/project.json"
code=$(curl -s -o "$OUT/project-patched.json" -w '%{http_code}' -X PATCH \
  "$BASE/api/projects/$pj" -H 'Content-Type: application/json' \
  --data-binary @"$OUT/project.json")
[ "$code" = "200" ] && grep -q '"name":"smoke-project-2"' "$OUT/project-patched.json" \
  && ok "patch project" || bad "patch project (HTTP $code)"
code=$(curl -s -o /dev/null -w '%{http_code}' -X PATCH "$BASE/api/projects/$pj" \
  -H 'Content-Type: application/json' \
  -d '{"segments":[{"id":"s1","start":2.0,"end":1.0,"text":"bad"}]}')
[ "$code" = "400" ] && ok "bad cue range -> 400" || bad "bad cue range -> $code"

srt=$(curl -s "$BASE/api/projects/$pj/subtitles?format=srt")
case "$srt" in
  *' --> '*edited*) ok "project srt export" ;;
  *) bad "project srt: $(printf '%s' "$srt" | head -c 120)" ;;
esac
vtt=$(curl -s "$BASE/api/projects/$pj/subtitles?format=vtt")
case "$vtt" in
  WEBVTT*edited*) ok "project vtt export" ;;
  *) bad "project vtt: $(printf '%s' "$vtt" | head -c 120)" ;;
esac
code=$(curl -s -o /dev/null -w '%{http_code}' \
  "$BASE/api/projects/$pj/subtitles?format=ass")
[ "$code" = "400" ] && ok "unknown project subtitle format -> 400" \
  || bad "unknown project subtitle format -> $code"

if curl -s "$BASE/api/health" | grep -q '"transcribe":true'; then
  heard=$(curl -s -X POST "$BASE/api/projects/$pj/transcribe")
  case "$heard" in
    *武汉*) ok "transcribe project" ;;
    *) bad "transcribe project: $(printf '%s' "$heard" | head -c 160)" ;;
  esac

  # Dubbing: one WAV per cue from the saved engine and voice.
  dub_wait() { # dub_wait <id> -> terminal status
    local id="$1" status=""
    for _ in $(seq 1 300); do
      status=$(curl -s "$BASE/api/jobs/$id" \
        | sed -n 's/.*"status":"\([^"]*\)".*/\1/p')
      case "$status" in done|error|cancelled) break ;; esac
      sleep 1
    done
    printf '%s' "$status"
  }
  pv=$(curl -s -X POST "$BASE/api/profiles" -F 'name=smoke-dub-voice' \
    -F "prompt=@$DIR/test_wavs/leijun-1.wav" \
    -F "$(field prompt_text '那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.')" \
    | project_id)
  [ -n "$pv" ] && ok "dub voice created" || bad "dub voice"
  code=$(curl -s -o /dev/null -w '%{http_code}' -X PATCH "$BASE/api/projects/$pj" \
    -H 'Content-Type: application/json' \
    -d "{\"engine_id\":\"$MODEL\",\"voice_id\":\"$pv\"}")
  [ "$code" = "200" ] && ok "dubbing voice saved" || bad "dubbing voice -> $code"
  code=$(curl -s -o /dev/null -w '%{http_code}' -X PATCH "$BASE/api/projects/$pj" \
    -H 'Content-Type: application/json' -d '{"engine_id":"no-such-engine"}')
  [ "$code" = "400" ] && ok "unknown dubbing engine -> 400" \
    || bad "unknown dubbing engine -> $code"
  jid=$(curl -s -X POST "$BASE/api/projects/$pj/synthesize" -F 'all=1' \
    | sed -n 's/.*"job_id":"\([^"]*\)".*/\1/p')
  [ -n "$jid" ] && ok "start dubbing job ($jid)" || bad "start dubbing job"
  status=$(dub_wait "$jid")
  [ "$status" = "done" ] && ok "dubbing finished" || bad "dubbing status: $status"
  pj_json=$(curl -s "$BASE/api/projects/$pj")
  case "$pj_json" in
    *'"tts_path":"segments/s1.wav"'*) ok "cue audio stored" ;;
    *) bad "cue audio missing: $(printf '%s' "$pj_json" | head -c 160)" ;;
  esac
  curl -sf "$BASE/api/projects/$pj/segments/s1/audio" -o "$OUT/dub-s1.wav" \
    && head -c 4 "$OUT/dub-s1.wav" | grep -q RIFF \
    && ok "cue audio download" || bad "cue audio download"

  # Editing a cue's wording drops its stale audio.
  printf '{"segments":[{"id":"s1","start":0.0,"end":1.0,"text":"changed"}]}' \
    >"$OUT/dub.json"
  curl -s -o /dev/null -X PATCH "$BASE/api/projects/$pj" \
    -H 'Content-Type: application/json' --data-binary @"$OUT/dub.json"
  code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/projects/$pj/segments/s1/audio")
  [ "$code" = "404" ] && ok "editing a cue drops its audio" \
    || bad "stale cue audio -> $code"

  # One cue can be re-run without redoing the project.
  jid=$(curl -s -X POST "$BASE/api/projects/$pj/synthesize" -F 'segment=s1' \
    | sed -n 's/.*"job_id":"\([^"]*\)".*/\1/p')
  status=$(dub_wait "$jid")
  [ "$status" = "done" ] && ok "single-cue dubbing" \
    || bad "single-cue dubbing: $status"

  # Mix: the dubbed cue over the source, with ducking.
  jid=$(curl -s -X POST "$BASE/api/projects/$pj/mix" \
    -F 'background_volume=0.4' -F 'voice_volume=1.0' -F 'ducking=1' \
    | sed -n 's/.*"job_id":"\([^"]*\)".*/\1/p')
  [ -n "$jid" ] && ok "start mix job ($jid)" || bad "start mix job"
  status=$(dub_wait "$jid")
  [ "$status" = "done" ] && ok "mix finished" || bad "mix status: $status"
  code=$(curl -s -o "$OUT/mix.wav" -w '%{http_code}' \
    "$BASE/api/projects/$pj/files/mix.wav")
  [ "$code" = "200" ] && head -c 4 "$OUT/mix.wav" | grep -q RIFF \
    && ok "mix download" || bad "mix download (HTTP $code)"
  case "$(curl -s "$BASE/api/projects/$pj")" in
    *'"name":"mix.wav"'*) ok "mix recorded as an export" ;;
    *) bad "mix export missing" ;;
  esac
  code=$(curl -s -o /dev/null -w '%{http_code}' \
    "$BASE/api/projects/$pj/files/nope.wav")
  [ "$code" = "404" ] && ok "unknown project file -> 404" \
    || bad "unknown project file -> $code"
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/projects/$pj/mix" \
    -F 'background_volume=9' -F 'voice_volume=1' -F 'ducking=1')
  [ "$code" = "400" ] && ok "bad mix level -> 400" || bad "bad mix level -> $code"

  curl -s -o /dev/null -X DELETE "$BASE/api/profiles/$pv"
  for cid in $(curl -s "$BASE/api/captures" | sed 's/},{/}\n{/g' \
    | grep -F '"name":"smoke-dub-voice"' \
    | sed -n 's/.*"id":"\([^"]*\)".*/\1/p'); do
    curl -s -o /dev/null -X DELETE "$BASE/api/captures/$cid"
  done
else
  echo "  skip transcribing project (no speech-to-text model)"
fi

# An empty project has nothing to export, and a renamed text file is not a
# source clip.
empty=$(curl -s -X POST "$BASE/api/projects" -F 'name=smoke-empty' \
  -F "audio=@$DIR/test_wavs/leijun-1.wav" | project_id)
code=$(curl -s -o /dev/null -w '%{http_code}' \
  "$BASE/api/projects/$empty/subtitles?format=srt")
[ "$code" = "400" ] && ok "empty project has no subtitles" \
  || bad "empty project export -> $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  "$BASE/api/projects/$empty/synthesize" -F 'all=1')
[ "$code" = "400" ] && ok "dubbing needs an engine" \
  || bad "dubbing without an engine -> $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  "$BASE/api/projects/$empty/mix" -F 'ducking=1')
[ "$code" = "400" ] && ok "mixing needs a dub" \
  || bad "mixing without a dub -> $code"
printf 'not a wav\n' >"$OUT/not-a-wav-project.txt"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/projects" \
  -F 'name=bad' -F "audio=@$OUT/not-a-wav-project.txt;filename=note.wav")
[ "$code" = "400" ] && ok "unreadable media -> 400" || bad "unreadable media -> $code"

# Video ingestion: streamed to disk, probed with ffprobe, and its first
# audio track extracted. Skipped when this machine has no ffmpeg.
if command -v ffmpeg >/dev/null 2>&1 && command -v ffprobe >/dev/null 2>&1 \
  && curl -s "$BASE/api/health" | grep -q '"ffmpeg":true'; then
  ffmpeg -y -v error -f lavfi -i testsrc=size=160x120:rate=10:duration=1 \
    -f lavfi -i sine=frequency=440:duration=1 -shortest \
    -c:v mpeg4 -c:a aac "$OUT/clip.mp4"
  vp=$(curl -s -X POST "$BASE/api/projects" -F 'name=smoke-video' \
    -F "media=@$OUT/clip.mp4" | project_id)
  [ -n "$vp" ] && ok "create video project ($vp)" || bad "create video project"
  vjson=$(curl -s "$BASE/api/projects/$vp")
  case "$vjson" in
    *'"kind":"video"'*) ok "video source recorded" ;;
    *) bad "video kind: $(printf '%s' "$vjson" | head -c 160)" ;;
  esac
  case "$vjson" in
    *'"width":160'*'"height":120'*) ok "video dimensions probed" ;;
    *) bad "video dimensions: $(printf '%s' "$vjson" | head -c 160)" ;;
  esac
  curl -sf "$BASE/api/projects/$vp/source" -o "$OUT/video-audio.wav" \
    && head -c 4 "$OUT/video-audio.wav" | grep -q RIFF \
    && ok "video audio extracted" || bad "video audio extracted"
  curl -sf "$BASE/api/projects/$vp/media" -o "$OUT/video-copy.mp4" \
    && [ -s "$OUT/video-copy.mp4" ] && ok "original media served" \
    || bad "original media served"
  code=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "$BASE/api/projects/$vp")
  [ "$code" = "204" ] && ok "delete video project" \
    || bad "delete video project -> $code"
else
  echo "  skip video project (no ffmpeg)"
fi

for id in "$pj" "$empty"; do
  curl -s -o /dev/null -X DELETE "$BASE/api/projects/$id"
done
code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/projects/$pj")
[ "$code" = "404" ] && ok "delete project" || bad "deleted project -> $code"
fi

if want voxcpm2; then
if case "$engines_json" in *'"id":"voxcpm2"'*) true ;; *) false ;; esac; then
  echo "voxcpm2"
  code=$(curl -s -o "$OUT/vox-zh.wav" -w '%{http_code}' -X POST \
    "$BASE/api/engines/voxcpm2/synthesize" \
    -F "$(field text '你好，我是 VoxCPM2，这里不需要参考文本。')" \
    -F "prompt=@$DIR/test_wavs/leijun-1.wav")
  [ "$code" = "200" ] && head -c 4 "$OUT/vox-zh.wav" | grep -q RIFF \
    && ok "voxcpm2 clone without transcript" || bad "voxcpm2 (HTTP $code)"
else
  echo "  skip voxcpm2 (not installed)"
fi
fi

if want qwen3tts; then
if case "$engines_json" in *'"id":"qwen3tts"'*) true ;; *) false ;; esac; then
  echo "qwen3tts"
  code=$(curl -s -o "$OUT/qwen.wav" -w '%{http_code}' -X POST \
    "$BASE/api/engines/qwen3tts/synthesize" \
    -F "$(field text '你好，我是 Qwen3-TTS，通过 laverock 调用。')" \
    -F "prompt=@$DIR/test_wavs/leijun-1.wav")
  [ "$code" = "200" ] && head -c 4 "$OUT/qwen.wav" | grep -q RIFF \
    && ok "qwen3tts clone" || bad "qwen3tts (HTTP $code)"
else
  echo "  skip qwen3tts (not installed)"
fi
fi

if want cosyvoice; then
if case "$engines_json" in *'"id":"cosyvoice"'*) true ;; *) false ;; esac; then
  echo "cosyvoice"
  code=$(curl -s -o "$OUT/cosy.wav" -w '%{http_code}' --max-time 600 -X POST \
    "$BASE/api/engines/cosyvoice/synthesize" \
    -F "$(field text '你好，我是 CosyVoice3，通过 laverock 调用。')" \
    -F "prompt=@$DIR/test_wavs/leijun-1.wav" \
    -F "$(field prompt_text '那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.')")
  [ "$code" = "200" ] && head -c 4 "$OUT/cosy.wav" | grep -q RIFF \
    && ok "cosyvoice clone" || bad "cosyvoice (HTTP $code)"
else
  echo "  skip cosyvoice (not installed)"
fi
fi

if want dots; then
if case "$engines_json" in *'"id":"dots"'*) true ;; *) false ;; esac; then
  echo "dots"
  code=$(curl -s -o "$OUT/dots.wav" -w '%{http_code}' --max-time 600 -X POST \
    "$BASE/api/engines/dots/synthesize" \
    -F "$(field text '你好，我是 dots.tts，通过 laverock 调用。')" \
    -F "prompt=@$DIR/test_wavs/leijun-1.wav")
  [ "$code" = "200" ] && head -c 4 "$OUT/dots.wav" | grep -q RIFF \
    && ok "dots clone" || bad "dots (HTTP $code)"
else
  echo "  skip dots (not installed)"
fi
fi

if want longcat; then
if case "$engines_json" in *'"id":"longcat"'*) true ;; *) false ;; esac; then
  echo "longcat"
  code=$(curl -s -o "$OUT/longcat.wav" -w '%{http_code}' --max-time 600 -X POST \
    "$BASE/api/engines/longcat/synthesize" \
    -F "$(field text '你好，我是 LongCat AudioDiT，通过 laverock 调用。')" \
    -F "prompt=@$DIR/test_wavs/leijun-1.wav" \
    -F "$(field prompt_text '那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.')")
  [ "$code" = "200" ] && head -c 4 "$OUT/longcat.wav" | grep -q RIFF \
    && ok "longcat clone" || bad "longcat (HTTP $code)"
else
  echo "  skip longcat (not installed)"
fi
fi

if want kokoro; then
if case "$engines_json" in *'"kind":"kokoro"'*) true ;; *) false ;; esac; then
  echo "kokoro"
  code=$(curl -s -o "$OUT/kokoro.wav" -w '%{http_code}' --max-time 120 -X POST \
    "$BASE/api/engines/kokoro-int8-multi-lang-v1_0/synthesize" \
    -F "$(field text '你好，我是 Kokoro，通过 laverock 调用。')" \
    -F 'voice=47')
  [ "$code" = "200" ] && head -c 4 "$OUT/kokoro.wav" | grep -q RIFF \
    && ok "kokoro preset voice (no reference audio)" || bad "kokoro (HTTP $code)"
else
  echo "  skip kokoro (not installed)"
fi
fi

if want remote; then
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
fi

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
