#!/usr/bin/env bash
# Shared helpers for the model download scripts.
#
# Progress protocol for the desktop UI: scripts print one-line markers
# that the server parses out of the install log:
#
#   @@LV stage <text>     what is happening (indeterminate step)
#   @@LV progress <pct>   0-100 for the current download
#
# The markers are stripped from the visible log; everything else the
# scripts print is shown as before.

# lv_stage <label>
lv_stage() {
  printf '@@LV stage %s\n' "$*"
}

# lv_progress <percent 0-100>
lv_progress() {
  local pct="$1"
  case "$pct" in
    ''|*[!0-9.]*) return ;;
  esac
  awk -v p="$pct" 'BEGIN { if (p < 0) p = 0; if (p > 100) p = 100; printf "@@LV progress %.1f\n", p }'
}

# lv_filesize <path>  (portable byte count, 0 if missing)
lv_filesize() {
  if [ -f "$1" ]; then
    wc -c <"$1" | tr -d '[:space:]'
  else
    echo 0
  fi
}

# lv_content_length <url>: final Content-Length after redirects, 0 on failure.
lv_content_length() {
  curl -sIL "$1" | tr -d '\r' | awk '
    tolower($1) == "content-length:" { v = $2 }
    END { printf "%d", v + 0 }'
}

# lv_fetch_parallel <url> <dest> <total_bytes> [workers]
#
# Split a large file into ranged requests and resume per slice (Hugging
# Face's single-connection download is often ~5x slower than parallel
# ranges through a mirror). Emits @@LV progress; verifies the final size.
lv_fetch_parallel() {
  local url="$1" dest="$2" total="$3" workers="${4:-8}"
  local i s e want have got alive pid
  local chunk=$(( (total + workers - 1) / workers ))
  local pids=()

  for i in $(seq 0 $((workers - 1))); do
    s=$((i * chunk)); e=$((s + chunk - 1))
    [ "$e" -ge "$total" ] && e=$((total - 1))
    want=$((e - s + 1))
    have=$(lv_filesize "$dest.part$i")
    if [ "$have" -gt "$want" ]; then
      have=0
      : >"$dest.part$i"
    fi
    if [ "$have" -lt "$want" ]; then
      curl -sSL --fail --retry 3 --retry-delay 2 \
        -r "$((s + have))-$e" "$url" >>"$dest.part$i" &
      pids+=($!)
    fi
  done

  while :; do
    alive=0
    for pid in "${pids[@]}"; do
      kill -0 "$pid" 2>/dev/null && alive=1
    done
    [ "$alive" -eq 0 ] && break
    sleep 2
    got=0
    for i in $(seq 0 $((workers - 1))); do
      got=$((got + $(lv_filesize "$dest.part$i")))
    done
    lv_progress "$(awk -v s="$got" -v t="$total" 'BEGIN { if (t > 0) printf "%.1f", s * 100 / t }')"
  done
  for pid in "${pids[@]}"; do
    wait "$pid" || return 1
  done

  : >"$dest"
  for i in $(seq 0 $((workers - 1))); do
    s=$((i * chunk)); e=$((s + chunk - 1))
    [ "$e" -ge "$total" ] && e=$((total - 1))
    want=$((e - s + 1))
    have=$(lv_filesize "$dest.part$i")
    if [ "$have" -ne "$want" ]; then
      echo "part $i is $have bytes, expected $want" >&2
      return 1
    fi
    cat "$dest.part$i" >>"$dest"
    rm -f "$dest.part$i"
  done
  if [ "$(lv_filesize "$dest")" -ne "$total" ]; then
    echo "assembled file has the wrong size" >&2
    return 1
  fi
  lv_progress 100
}

# lv_track <dir> <total_bytes> <command...>
#
# Run a command that fills <dir> (e.g. `hf download --local-dir`) and
# report growth against the expected total. Tools like huggingface-cli
# only draw progress bars on a TTY, so the UI would otherwise sit on a
# stage label for the whole multi-GB download.
lv_track() {
  local dir="$1" total="$2"
  shift 2
  "$@" &
  local pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    sleep 2
    local have_kb
    have_kb=$(du -sk "$dir" 2>/dev/null | awk '{print $1}')
    if [ "${total:-0}" -gt 0 ]; then
      lv_progress "$(awk -v s="${have_kb:-0}" -v t="$((total / 1024))" 'BEGIN { if (t > 0) printf "%.1f", s * 100 / t }')"
    fi
  done
  wait "$pid"
}

# lv_download <url> <dest> [resume]
#
# curl with the progress meter silenced; a background watcher reports the
# growing file size against the server-advertised length. resume=1 (the
# default) appends to a partial file, 0 starts over.
lv_download() {
  local url="$1" dest="$2" resume="${3:-1}"
  local total have
  total="$(lv_content_length "$url")"
  have="$(lv_filesize "$dest")"
  lv_stage "下载 $(basename "$dest")"

  local args=(-sSL --fail --retry 3 --retry-delay 2 -o "$dest")
  if [ "$resume" = "1" ] && [ "$have" -gt 0 ]; then
    args+=(-C -)
  fi
  curl "${args[@]}" "$url" &
  local cpid=$!

  while kill -0 "$cpid" 2>/dev/null; do
    sleep 1
    local size; size="$(lv_filesize "$dest")"
    if [ "${total:-0}" -gt 0 ]; then
      lv_progress "$(awk -v s="$size" -v t="$total" 'BEGIN { printf "%.1f", s * 100 / t }')"
    else
      lv_stage "下载 $(basename "$dest") · $(awk -v s="$size" 'BEGIN { printf "%.0f MB", s / 1048576 }')"
    fi
  done
  wait "$cpid"
}
