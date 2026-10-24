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
