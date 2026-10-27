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

# lv_is_windows: succeeds under Git-Bash / MSYS2 / Cygwin.
lv_is_windows() {
  case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) return 0 ;; *) return 1 ;; esac
}

# lv_has_nvidia: succeeds when an NVIDIA driver is visible to the OS.
# nvidia-smi ships with the driver, so it is present on machines that can
# use a CUDA runtime even though no CUDA toolkit is installed.
lv_has_nvidia() {
  command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi >/dev/null 2>&1
}

# lv_native_path <path>: path form that the platform's own binaries accept.
# Rust (std::process) and python.exe reject MSYS /e/.. mounts, so on Windows
# we emit E:/.. (forward slashes stay valid in JSON and for the Win32 API).
lv_native_path() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -m "$1"
  else
    printf '%s' "$1"
  fi
}

# lv_venv_python <venv_dir>: interpreter path inside a venv for this OS.
# Windows venvs use Scripts/ instead of bin/.
lv_venv_python() {
  if lv_is_windows; then
    printf '%s/Scripts/python.exe' "$1"
  else
    printf '%s/bin/python' "$1"
  fi
}

# lv_venv_cli <venv_dir> <command>: console-script executable in a venv.
# e.g. lv_venv_cli "$venv" huggingface-cli
lv_venv_cli() {
  local venv="$1" name="$2"
  if lv_is_windows; then
    printf '%s/Scripts/%s.exe' "$venv" "$name"
  else
    printf '%s/bin/%s' "$venv" "$name"
  fi
}

# lv_unzip <archive> <dest_dir>: extract a zip, falling back to PowerShell
# on Git-Bash for Windows (which ships without Info-ZIP unzip).
lv_unzip() {
  local zip="$1" dest="$2"
  mkdir -p "$dest"
  if command -v unzip >/dev/null 2>&1; then
    unzip -q "$zip" -d "$dest"
  else
    powershell -NoProfile -Command \
      "Expand-Archive -LiteralPath '$(cygpath -w "$zip")' -DestinationPath '$(cygpath -w "$dest")' -Force"
  fi
}

# lv_audiocpp_bin <tools_dir> <version> <models_dir>
#
# Ensure the audio.cpp native runtime is available and print its path.
# The runtime is shared by every audio.cpp-backed engine (dots.tts,
# CosyVoice3, …) and lives under <tools>/audiocpp[-cuda<ver>] so it is
# downloaded once. On Windows with an NVIDIA GPU we fetch the CUDA build
# plus the matching cudart bundle (CUDA/cuBLAS/cuFFT DLLs); the target
# machine needs only the driver, not a CUDA toolkit. Override detection
# with AUDIOCPP_CUDA=off (force CPU) or AUDIOCPP_CUDA_VERSION=<ver>.
# The printed path is in the platform-native form (E:/.. on Windows),
# which is what Rust and the engine.json manifests require.
lv_audiocpp_bin() {
  local tools="$1" version="$2" models="$3"
  local platform
  platform="$(uname -s)"

  # Choose the runtime flavor up front so CPU and CUDA installs can live
  # side by side without either masquerading as the other.
  local asset ext dir ac_cuda=0 cuda_ver=""
  case "$platform" in
    Darwin)
      case "$(uname -m)" in
        arm64) asset="bin-macos-arm64-metal" ;;
        *)     asset="bin-macos-x64-metal" ;;
      esac
      ext=".tar.gz"
      dir="$tools/audiocpp" ;;
    Linux)
      asset="bin-ubuntu-x64-cpu-portable"; ext=".tar.gz"
      dir="$tools/audiocpp" ;;
    MINGW*|MSYS*|CYGWIN*)
      ext=".zip"
      if [ "${AUDIOCPP_CUDA:-auto}" != "off" ] && lv_has_nvidia; then
        ac_cuda=1
        cuda_ver="${AUDIOCPP_CUDA_VERSION:-12.4}"
        asset="bin-windows-x64-cuda${cuda_ver}"
        dir="$tools/audiocpp-cuda${cuda_ver}"
      else
        asset="bin-windows-x64-cpu-portable"
        dir="$tools/audiocpp"
      fi ;;
    *)
      echo "unsupported platform for audio.cpp: $platform" >&2
      return 1 ;;
  esac

  local bin="$dir/audiocpp_cli" exe="$dir/audiocpp_cli.exe"
  if [ -x "$bin" ] || [ -x "$exe" ]; then
    if [ -x "$bin" ]; then lv_native_path "$bin"; else lv_native_path "$exe"; fi
    return 0
  fi

  # A runtime installed by an earlier per-model engine still works; copy
  # it (and any backend DLLs beside it) into the shared location so removing
  # that model cannot break us.
  local legacy ldir
  legacy=$(find "$models" -maxdepth 4 -name audiocpp_cli -type f -perm -u+x 2>/dev/null | head -1)
  if [ -z "$legacy" ]; then
    legacy=$(find "$models" -maxdepth 4 -name audiocpp_cli.exe -type f 2>/dev/null | head -1)
  fi
  if [ -n "$legacy" ]; then
    ldir="$(dirname "$legacy")"
    mkdir -p "$dir"
    cp "$legacy" "$dir/"
    cp "$ldir"/*.dll "$dir/" 2>/dev/null || true
    [ -f "$ldir/LICENSE" ] && cp "$ldir/LICENSE" "$dir/"
    chmod +x "$dir/audiocpp_cli" "$exe" 2>/dev/null || true
    lv_native_path "$dir/$(basename "$legacy")"
    return 0
  fi

  local base tmp
  base="https://github.com/0xShug0/audio.cpp/releases/download/$version"
  tmp="$(mktemp -d)"
  mkdir -p "$tmp/ex" "$dir"

  lv_stage "下载 audio.cpp 运行时（${asset}）"
  lv_download "$base/audio-$version-$asset$ext" "$tmp/runtime$ext" 0
  if [ "$ext" = ".zip" ]; then
    lv_unzip "$tmp/runtime.zip" "$tmp/ex"
    # The CUDA build splits the exe and each backend into its own DLL; the
    # OS loads those from the exe's directory, so copy the whole tree rather
    # than the single binary the portable CPU build yields.
    cp -a "$tmp/ex/." "$dir/"
    if [ "$ac_cuda" = 1 ]; then
      lv_stage "下载 CUDA 运行库（cudart ${cuda_ver}，免安装 CUDA）"
      lv_download "$base/audio-$version-cudart-windows-x64-cuda${cuda_ver}.zip" \
        "$tmp/cudart.zip" 0
      mkdir -p "$tmp/cu"
      lv_unzip "$tmp/cudart.zip" "$tmp/cu"
      find "$tmp/cu" -name '*.dll' -exec cp -f {} "$dir/" \;
    fi
  else
    tar -xzf "$tmp/runtime.tar.gz" -C "$tmp/ex"
    cp "$tmp/ex/audiocpp_cli" "$dir/" 2>/dev/null || true
    [ -f "$tmp/ex/LICENSE" ] && cp "$tmp/ex/LICENSE" "$dir/"
  fi
  chmod +x "$dir/audiocpp_cli" "$exe" 2>/dev/null || true
  rm -rf "$tmp"

  if [ -x "$bin" ]; then lv_native_path "$bin"; else lv_native_path "$exe"; fi
}

# lv_audiocpp_backend: the --backend value matching the runtime flavor that
# lv_audiocpp_bin selects above (explicit cuda for the CUDA build, so we do
# not rely on the runtime's own "best" probing; metal on Apple Silicon).
lv_audiocpp_backend() {
  case "$(uname -s)" in
    Darwin) echo metal ;;
    MINGW*|MSYS*|CYGWIN*)
      if [ "${AUDIOCPP_CUDA:-auto}" != "off" ] && lv_has_nvidia; then
        echo cuda
      else
        echo best
      fi ;;
    *) echo best ;;
  esac
}

# lv_sha256 <path>: hex digest, via whichever tool ships on the platform.
lv_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# lv_voxcpm2_runner <dest_dir> <tools_dir>
#
# Install the prebuilt voxcpm2-cli published on this repo's rolling
# "voxcpm2-runner" GitHub release into <dest_dir>/voxcpm2-cli[.exe] (next
# to the GGUF weights, where the engine manifest looks for it). The
# extracted binary is cached under <tools_dir>/voxcpm2-runner so removing
# and reinstalling the model costs nothing.
#
# LAVEROCK_GITHUB_RELEASES overrides the download base (mirror). Returns
# non-zero when the platform has no prebuilt asset or the download or
# checksum check fails; the caller then falls back to a source build.
lv_voxcpm2_runner() {
  local dest="$1" tools="$2"
  local cache="$tools/voxcpm2-runner"
  local platform asset ext inner
  platform="$(uname -s)"
  case "$platform" in
    Darwin)
      case "$(uname -m)" in
        arm64) asset="voxcpm2-cli-macos-arm64.tar.gz" ;;
        x86_64) asset="voxcpm2-cli-macos-x64.tar.gz" ;;
        *) return 1 ;;
      esac
      ext=".tar.gz"; inner="voxcpm2-cli" ;;
    Linux)
      [ "$(uname -m)" = "x86_64" ] || return 1
      asset="voxcpm2-cli-linux-x64.tar.gz"; ext=".tar.gz"; inner="voxcpm2-cli" ;;
    MINGW*|MSYS*|CYGWIN*)
      asset="voxcpm2-cli-windows-x64.zip"; ext=".zip"; inner="voxcpm2-cli.exe" ;;
    *)
      return 1 ;;
  esac

  mkdir -p "$cache" "$dest"
  if [ ! -x "$cache/$inner" ]; then
    local base manifest want got extract
    base="${LAVEROCK_GITHUB_RELEASES:-https://github.com/chen3feng/laverock/releases/download}"

    # Manifest first: its sha256 line catches a truncated archive. The
    # layout is one line per file by design (see voxcpm2-runner.yml).
    lv_stage "校验 voxcpm2-cli 预编译版本"
    manifest="$cache/manifest.json"
    curl -fsSL --retry 2 "$base/voxcpm2-runner/voxcpm2-runner.json" -o "$manifest" || return 1
    want="$(sed -n "s/.*\"$asset\".*\"sha256\": *\"\([0-9a-fA-F]\{64\}\)\".*/\1/p" "$manifest" | head -1 | tr 'A-F' 'a-f')"
    [ -n "$want" ] || { echo "no prebuilt $asset in runner manifest" >&2; return 1; }

    lv_download "$base/voxcpm2-runner/$asset" "$cache/$asset" 0 || return 1
    got="$(lv_sha256 "$cache/$asset" | tr 'A-F' 'a-f')"
    if [ "$got" != "$want" ]; then
      echo "checksum mismatch for $asset" >&2
      rm -f "$cache/$asset"
      return 1
    fi

    # The archive is a flat directory holding the static runner plus
    # LICENSE and COMMIT.
    if [ "$ext" = ".zip" ]; then
      if command -v unzip >/dev/null 2>&1; then
        unzip -q -o "$cache/$asset" -d "$cache"
      else
        powershell -NoProfile -Command \
          "Expand-Archive -LiteralPath '$(cygpath -w "$cache/$asset")' -DestinationPath '$(cygpath -w "$cache")' -Force"
      fi
    else
      tar -xzf "$cache/$asset" -C "$cache"
    fi
    [ -f "$cache/$inner" ] || { echo "$inner missing from $asset" >&2; return 1; }
    if [ -f "$cache/LICENSE" ]; then
      cp "$cache/LICENSE" "$cache/voxcpm2-cli-LICENSE.txt"
    fi
    chmod +x "$cache/$inner" 2>/dev/null || true
    rm -f "$cache/$asset" "$manifest"
  fi

  cp "$cache/$inner" "$dest/$inner"
  chmod +x "$dest/$inner" 2>/dev/null || true
  lv_stage "voxcpm2-cli 预编译版本就绪"
}

# lv_fetch_parallel <url> <dest> <total_bytes> [workers]
#
# Split a large file into ranged requests and resume per slice (Hugging
# Face's single-connection download is often ~5x slower than parallel
# ranges through a mirror). Emits @@LV progress; verifies the final size.
lv_fetch_parallel() {
  local url="$1" dest="$2" total="$3" workers="${4:-8}"
  local i s e want have got alive pid attempt
  local chunk=$(( (total + workers - 1) / workers ))
  local pids=()

  lv_slice_size() { # index -> expected byte count
    local n="$1" start end
    start=$((n * chunk)); end=$((start + chunk - 1))
    [ "$end" -ge "$total" ] && end=$((total - 1))
    echo $((end - start + 1))
  }

  # Up to three passes: a transient reset (HTTP/2 stream cancel, flaky
  # proxy) only costs the unfinished bytes of one slice.
  for attempt in 1 2 3; do
    pids=()
    for i in $(seq 0 $((workers - 1))); do
      s=$((i * chunk)); e=$((s + chunk - 1))
      [ "$e" -ge "$total" ] && e=$((total - 1))
      want=$(lv_slice_size "$i")
      have=$(lv_filesize "$dest.part$i")
      if [ "$have" -gt "$want" ]; then
        have=0
        : >"$dest.part$i"
      fi
      [ "$have" -eq "$want" ] && continue
      curl -sSL --fail --retry 5 --retry-delay 2 --retry-all-errors \
        -r "$((s + have))-$e" "$url" >>"$dest.part$i" &
      pids+=($!)
    done

    while :; do
      alive=0
      for pid in ${pids[@]+"${pids[@]}"}; do
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
    # bash 3.2 (macOS) errors on an empty array under `set -u`.
    for pid in ${pids[@]+"${pids[@]}"}; do
      wait "$pid" 2>/dev/null || true
    done

    got=0
    for i in $(seq 0 $((workers - 1))); do
      got=$((got + $(lv_filesize "$dest.part$i")))
    done
    [ "$got" -ge "$total" ] && break
    [ "$attempt" -lt 3 ] && lv_stage "下载不完整，正在续传（第 $((attempt + 1)) 次）"
  done

  : >"$dest"
  for i in $(seq 0 $((workers - 1))); do
    want=$(lv_slice_size "$i")
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
