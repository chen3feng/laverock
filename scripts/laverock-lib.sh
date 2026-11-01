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

# Python subprocesses (huggingface-cli, runners) print emoji/CJK on
# Windows; force UTF-8 stdout so cp1252 never raises UnicodeEncodeError.
export PYTHONIOENCODING=utf-8

# lv_init [model_dir]
#
# Standard download-script preamble. Call right after sourcing this file.
# Sets SCRIPT_DIR/ROOT/MODELS_DIR/TOOLS from the calling script's location
# (scripts/ sits one level below the repo root); with a model_dir argument
# also sets DEST=$MODELS_DIR/<model_dir> and creates it.
lv_init() {
  local caller="${BASH_SOURCE[1]:-${0:-}}"
  SCRIPT_DIR="$(cd "$(dirname "$caller")" && pwd)"
  ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
  MODELS_DIR="${LAVEROCK_MODELS_DIR:-$ROOT/models}"
  TOOLS="${LAVEROCK_TOOLS_DIR:-$ROOT/.tools}"
  if [ -n "${1:-}" ]; then
    DEST="$MODELS_DIR/$1"
    mkdir -p "$DEST"
  else
    mkdir -p "$MODELS_DIR"
  fi
}

# lv_stage <label>
lv_stage() {
  # stderr: these markers are progress output for the UI, and helpers such as
  # lv_audiocpp_bin/lv_mosstts_bin are called in $(...) — stdout is the
  # return value. The server merges both streams into the install log.
  printf '@@LV stage %s\n' "$*" >&2
}

# lv_stage_key <key> [name=value ...]
#
# Emit a *translatable* stage: the key and its arguments are rendered by
# the server in the UI language (see laverock_core::i18n). Values must be
# single tokens; lv_stage_key escapes spaces as %20.
lv_stage_key() {
  local key="$1"
  shift
  local out="$key"
  local pair
  for pair in "$@"; do
    out="$out ${pair// /%20}"
  done
  printf '@@LV stage %s\n' "$out" >&2
}

# lv_progress <percent 0-100>
lv_progress() {
  local pct="$1"
  case "$pct" in
    ''|*[!0-9.]*) return ;;
  esac
  awk -v p="$pct" 'BEGIN { if (p < 0) p = 0; if (p > 100) p = 100; printf "@@LV progress %.1f\n", p }' >&2
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

# lv_python_supported <interpreter>: true for Python 3.10-3.12.
lv_python_supported() {
  "$1" -c 'import sys; sys.exit(0 if (3, 10) <= sys.version_info[:2] <= (3, 12) else 1)' \
    >/dev/null 2>&1
}

# lv_find_python [explicit]: echo a usable Python 3.10-3.12 interpreter.
# Prefers the explicit override, then versioned names (newest first); on
# Windows plain `python` comes before the often-broken `python3` alias.
#
# Check the version instead of trusting the name: a GUI-launched macOS app
# inherits only /usr/bin:/bin:/usr/sbin:/sbin, where `python3` is 3.9 and
# Homebrew's 3.12 is not on PATH at all. Looking in the usual prefixes and
# rejecting unsupported interpreters keeps pip from failing later with an
# opaque dependency-resolution error.
lv_find_python() {
  local explicit="${1:-}" candidate dir
  if [ -n "$explicit" ]; then
    if command -v "$explicit" >/dev/null 2>&1; then
      if lv_python_supported "$explicit"; then
        printf '%s\n' "$explicit"
        return 0
      fi
      echo "$explicit is $("$explicit" --version 2>&1), but Python 3.10-3.12 is required" >&2
      return 1
    fi
  fi
  local candidates="python3.12 python3.11 python3.10"
  if lv_is_windows; then
    candidates="$candidates python python3 py"
  else
    candidates="$candidates python3"
  fi
  for candidate in $candidates; do
    if command -v "$candidate" >/dev/null 2>&1 && lv_python_supported "$candidate"; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  # Common install prefixes that a desktop-launched process does not
  # inherit in PATH (Homebrew on Apple Silicon/Intel, MacPorts).
  local prefixes=""
  if ! lv_is_windows; then
    prefixes="/opt/homebrew/bin /usr/local/bin /opt/local/bin"
  fi
  for dir in $prefixes; do
    for candidate in $candidates; do
      if [ -x "$dir/$candidate" ] && lv_python_supported "$dir/$candidate"; then
        printf '%s\n' "$dir/$candidate"
        return 0
      fi
    done
  done
  return 1
}

# lv_python_at_least <interpreter> <major.minor>: true when the interpreter
# is at least that version.
lv_python_at_least() {
  "$1" -c 'import sys
maj, minor = map(int, sys.argv[1].split("."))
sys.exit(0 if sys.version_info[:2] >= (maj, minor) else 1)' "$2" >/dev/null 2>&1
}

# lv_prepare_python_venv <venv_dir> <python> [min_version]
#
# Create the isolated venv if needed, export LV_PY to its interpreter (the
# download helpers parse JSON with it), upgrade pip, and on Windows with an
# NVIDIA GPU pre-install the CUDA torch build so the default CPU-only PyPI
# wheel is never silently used. Sets VPY in the caller for its own pip calls.
# With min_version (e.g. 3.10), a venv left by an older interpreter is
# rebuilt: the desktop app once picked macOS's 3.9 from a minimal PATH, and
# pip would then fail on packages that require 3.10+.
lv_prepare_python_venv() {
  local venv="$1" python="$2" min="${3:-}"
  VPY="$(lv_venv_python "$venv")"
  if [ -f "$VPY" ] && [ -n "$min" ] && ! lv_python_at_least "$VPY" "$min"; then
    echo "Recreating $venv: $("$VPY" --version 2>&1), but Python $min+ is required" >&2
    rm -rf "$venv"
  fi
  if [ ! -f "$VPY" ]; then
    lv_stage_key stage_venv
    "$python" -m venv "$venv"
  fi
  export LV_PY="$VPY"
  # Progress bars on even though pip sees a pipe, not a terminal.
  export PIP_PROGRESS_BAR=on
  lv_stage_key stage_pip_upgrade
  "$VPY" -m pip install --upgrade pip
  if lv_is_windows && lv_has_nvidia \
     && ! "$VPY" -c 'import torch, torchaudio, sys; sys.exit(0 if torch.version.cuda else 1)' 2>/dev/null
  then
    # The default PyPI torch wheel is CPU-only on Windows (version.cuda is
    # None); install a CUDA build first so the engine's own requirements
    # leave torch satisfied. A CUDA-enabled torch already present is left
    # alone, so reruns do not re-resolve the multi-GB package.
    lv_stage_key stage_torch_cuda
    "$VPY" -m pip install torch torchaudio \
      --index-url https://download.pytorch.org/whl/cu130
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

  # The resident server beside the CLI keeps weights loaded between
  # requests; an install that only has the CLI is refreshed so both are
  # present.
  local bin="$dir/audiocpp_cli" exe="$dir/audiocpp_cli.exe"
  local srv="$dir/audiocpp_server" sexe="$dir/audiocpp_server.exe"
  if { [ -x "$bin" ] || [ -x "$exe" ]; } && { [ -x "$srv" ] || [ -x "$sexe" ]; }; then
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
    # Without the server binary the copy would silently lose the resident
    # path, so only reuse a legacy install that carries both.
    if [ ! -f "$ldir/audiocpp_server" ] && [ ! -f "$ldir/audiocpp_server.exe" ]; then
      legacy=""
    fi
  fi
  if [ -n "$legacy" ]; then
    ldir="$(dirname "$legacy")"
    mkdir -p "$dir"
    cp "$legacy" "$dir/"
    [ -f "$ldir/audiocpp_server" ] && cp "$ldir/audiocpp_server" "$dir/"
    [ -f "$ldir/audiocpp_server.exe" ] && cp "$ldir/audiocpp_server.exe" "$dir/"
    cp "$ldir"/*.dll "$dir/" 2>/dev/null || true
    [ -f "$ldir/LICENSE" ] && cp "$ldir/LICENSE" "$dir/"
    chmod +x "$dir/audiocpp_cli" "$exe" "$dir/audiocpp_server" "$dir/audiocpp_server.exe" 2>/dev/null || true
    lv_native_path "$dir/$(basename "$legacy")"
    return 0
  fi

  local base tmp
  base="https://github.com/0xShug0/audio.cpp/releases/download/$version"
  tmp="$(mktemp -d)"
  mkdir -p "$tmp/ex" "$dir"

  lv_stage_key stage_audiocpp "asset=${asset}"
  lv_download "$base/audio-$version-$asset$ext" "$tmp/runtime$ext" 0
  if [ "$ext" = ".zip" ]; then
    lv_unzip "$tmp/runtime.zip" "$tmp/ex"
    # The CUDA build splits the exe and each backend into its own DLL; the
    # OS loads those from the exe's directory, so copy the whole tree rather
    # than the single binary the portable CPU build yields.
    cp -a "$tmp/ex/." "$dir/"
    if [ "$ac_cuda" = 1 ]; then
      lv_stage_key stage_cuda_runtime "version=${cuda_ver}"
      lv_download "$base/audio-$version-cudart-windows-x64-cuda${cuda_ver}.zip" \
        "$tmp/cudart.zip" 0
      mkdir -p "$tmp/cu"
      lv_unzip "$tmp/cudart.zip" "$tmp/cu"
      find "$tmp/cu" -name '*.dll' -exec cp -f {} "$dir/" \;
    fi
  else
    tar -xzf "$tmp/runtime.tar.gz" -C "$tmp/ex"
    if [ ! -f "$tmp/ex/audiocpp_cli" ] || [ ! -f "$tmp/ex/audiocpp_server" ]; then
      echo "audio.cpp archive is incomplete (missing audiocpp_cli or audiocpp_server); rerun the installer" >&2
      rm -rf "$tmp"
      return 1
    fi
    cp "$tmp/ex/audiocpp_cli" "$dir/"
    cp "$tmp/ex/audiocpp_server" "$dir/"
    [ -f "$tmp/ex/LICENSE" ] && cp "$tmp/ex/LICENSE" "$dir/"
  fi
  chmod +x "$dir/audiocpp_cli" "$exe" "$dir/audiocpp_server" "$dir/audiocpp_server.exe" 2>/dev/null || true
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
    lv_stage_key stage_verify_vox
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
  lv_stage_key stage_vox_ready
}

# lv_mosstts_bin <tools_dir>
#
# Ensure the prebuilt llama-moss-tts runtime is available and print its
# path in the platform-native form (E:/... on Windows). It is built by
# .github/workflows/mosstts-runner.yml from the OpenMOSS llama.cpp fork and
# published on this repo's rolling "mosstts-runner" release, so installing
# MOSS-TTS no longer needs cmake or a C++ toolchain. The binary is cached
# under <tools_dir>/mosstts-runtime and stamped with the upstream commit
# from the manifest, so a rebuilt release replaces a stale binary.
#
# LAVEROCK_GITHUB_RELEASES overrides the download base (mirror). Returns
# non-zero when the platform has no prebuilt asset or the download or
# checksum check fails; the caller then falls back to a source build.
lv_mosstts_bin() {
  local tools="$1"
  local cache="$tools/mosstts-runtime"
  local asset ext inner
  case "$(uname -s)" in
    Darwin)
      case "$(uname -m)" in
        arm64) asset="llama-moss-tts-macos-arm64.tar.gz" ;;
        x86_64) asset="llama-moss-tts-macos-x64.tar.gz" ;;
        *) return 1 ;;
      esac
      ext=".tar.gz"; inner="llama-moss-tts" ;;
    Linux)
      [ "$(uname -m)" = "x86_64" ] || return 1
      asset="llama-moss-tts-linux-x64.tar.gz"; ext=".tar.gz"; inner="llama-moss-tts" ;;
    MINGW*|MSYS*|CYGWIN*)
      asset="llama-moss-tts-windows-x64.zip"; ext=".zip"; inner="llama-moss-tts.exe" ;;
    *)
      return 1 ;;
  esac

  mkdir -p "$cache"
  local base manifest want got commit extract
  base="${LAVEROCK_GITHUB_RELEASES:-https://github.com/chen3feng/laverock/releases/download}"
  manifest="$cache/manifest.json"

  # Manifest first: its sha256 line catches a truncated archive, and its
  # upstream commit tells us whether a cached binary is still current.
  # Offline, keep using whatever is cached.
  if curl -fsSL --retry 2 --max-time 60 \
      "$base/mosstts-runner/mosstts-runner.json" -o "$manifest.tmp" 2>/dev/null; then
    mv "$manifest.tmp" "$manifest"
  elif [ -x "$cache/$inner" ]; then
    lv_native_path "$cache/$inner"
    return 0
  else
    return 1
  fi
  commit="$(sed -n 's/.*"upstream": *"\([0-9a-fA-F]*\)".*/\1/p' "$manifest" \
    | head -1 | tr 'A-F' 'a-f')"

  if [ -x "$cache/$inner" ] && [ -n "$commit" ] \
     && [ "$(cat "$cache/COMMIT" 2>/dev/null)" = "$commit" ]; then
    lv_native_path "$cache/$inner"
    return 0
  fi

  lv_stage_key stage_verify_moss
  want="$(sed -n "s/.*\"$asset\".*\"sha256\": *\"\([0-9a-fA-F]\{64\}\)\".*/\1/p" "$manifest" \
    | head -1 | tr 'A-F' 'a-f')"
  [ -n "$want" ] || { echo "no prebuilt $asset in the mosstts-runner manifest" >&2; return 1; }

  lv_download "$base/mosstts-runner/$asset" "$cache/$asset" 0 || return 1
  got="$(lv_sha256 "$cache/$asset" | tr 'A-F' 'a-f')"
  if [ "$got" != "$want" ]; then
    echo "checksum mismatch for $asset" >&2
    rm -f "$cache/$asset"
    return 1
  fi

  # The archive is a flat directory holding the static binary plus LICENSE
  # and COMMIT.
  extract="$cache/extract"
  rm -rf "$extract"
  mkdir -p "$extract"
  if [ "$ext" = ".zip" ]; then
    lv_unzip "$cache/$asset" "$extract"
  else
    tar -xzf "$cache/$asset" -C "$extract"
  fi
  [ -f "$extract/$inner" ] || { echo "$inner missing from $asset" >&2; return 1; }
  cp "$extract/$inner" "$cache/$inner"
  [ -f "$extract/LICENSE" ] && cp "$extract/LICENSE" "$cache/LICENSE"
  if [ -f "$extract/COMMIT" ]; then
    cp "$extract/COMMIT" "$cache/COMMIT"
  else
    printf '%s\n' "$commit" > "$cache/COMMIT"
  fi
  chmod +x "$cache/$inner" 2>/dev/null || true
  rm -rf "$extract" "$cache/$asset" "$manifest"

  lv_stage_key stage_moss_ready
  lv_native_path "$cache/$inner"
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
      # No --retry-all-errors: it makes curl re-request a fixed `-r` range
      # after a mid-transfer reset (error 18) and append it again, so the
      # .part file grows past `want`. The three-pass loop above is what
      # resumes a partial slice, by re-computing `have` and the range.
      curl -sSL --fail --retry 5 --retry-delay 2 \
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
    [ "$attempt" -lt 3 ] && lv_stage_key stage_resume "attempt=$((attempt + 1))"
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

# lv_pybin: a usable interpreter for the tiny JSON/size helpers below.
# Callers that already built a venv set LV_PY to its (validated) python;
# otherwise fall back to a system one (on Windows `python3` may be only the
# non-functional Store alias, so prefer plain `python` there).
lv_pybin() {
  if [ -n "${LV_PY:-}" ]; then
    printf '%s\n' "$LV_PY"
  elif lv_is_windows; then
    command -v python 2>/dev/null || command -v python3 2>/dev/null
  else
    command -v python3 2>/dev/null || command -v python 2>/dev/null
  fi
}

# lv_hf_base: HuggingFace base URL honoring HF_ENDPOINT (e.g. an
# hf-mirror.com override), trailing slash removed.
lv_hf_base() {
  local base="${HF_ENDPOINT:-https://huggingface.co}"
  printf '%s' "${base%/}"
}

# lv_ms_base: ModelScope base URL (override with MODELSCOPE_ENDPOINT).
lv_ms_base() {
  local base="${MODELSCOPE_ENDPOINT:-https://modelscope.cn}"
  printf '%s' "${base%/}"
}

# lv_ms_download_file <repo> <rel> <dest> [workers] [label]
#
# Download one file from a ModelScope model repo via its API endpoint,
# which redirects to a CDN that supports ranged requests. Used for the
# MOSS-TTS weights, whose Hugging Face mirror is gated but ModelScope is
# open. Skips the download when <dest> already exists non-empty.
lv_ms_download_file() {
  local repo="$1" rel="$2" dest="$3" workers="${4:-8}" label="${5:-Download $rel}"
  if [ -s "$dest" ]; then
    return 0
  fi
  local enc="${rel//\//%2F}"
  enc="${enc// /%20}"
  local url; url="$(lv_ms_base)/api/v1/models/$repo/repo?Revision=master&FilePath=$enc"
  local total; total="$(lv_content_length "$url")"
  if [ "${total:-0}" -le 0 ]; then
    echo "cannot determine the download size for $url" >&2
    return 1
  fi
  # Small files (tokenizer JSON) do not benefit from slicing and the
  # per-worker range math degenerates below `workers` bytes.
  [ "$total" -lt 1048576 ] && workers=1
  lv_stage "$label"
  mkdir -p "$(dirname "$dest")"
  lv_fetch_parallel "$url" "$dest" "$total" "$workers"
}

# lv_hf_repo_files <repo> [hf_base]
#
# Print `size<TAB>path` for every regular file on a repo's main branch via
# the plain tree API (a GET of JSON; no hub metadata headers). Mirrors such
# as hf-mirror.com serve this even though huggingface_hub 0.36's metadata
# HEAD negotiation rejects their redirects (the X-Repo-Commit header is
# dropped -> FileMetadataError).
lv_hf_repo_files() {
  local repo="$1" base="${2:-$(lv_hf_base)}"
  local py; py="$(lv_pybin)"
  if [ -z "$py" ]; then
    echo "python3 is required to list $repo" >&2
    return 1
  fi
  local listing err errfile
  errfile="${TMPDIR:-/tmp}/lv-hf-list.$$.err"
  listing="$(curl -fsSL --retry 3 --retry-delay 2 --connect-timeout 20 \
    "$base/api/models/$repo/tree/main?recursive=true" 2>"$errfile")" || {
    err="$(tr '\n' ' ' <"$errfile" 2>/dev/null | cut -c1-200)"
    echo "Cannot reach the model repository ${base} (${err:-connection timed out})." >&2
    lv_hf_network_hint "$base" >&2
    return 1
  }
  printf '%s' "$listing" | "$py" -c '
import json, sys
try:
    entries = json.load(sys.stdin)
except Exception:
    sys.exit(2)
if not isinstance(entries, list):
    sys.exit(2)
for e in entries:
    if e.get("type") == "file" and e.get("path"):
        print("%s\t%s" % (e.get("size", 0), e["path"]))
'
}

# Actionable guidance when the model host is unreachable: the official
# endpoint is often blocked where hf-mirror.com works, and the user must
# pick a mirror in the app's download settings.
lv_hf_network_hint() {
  local base="${1:-$(lv_hf_base)}"
  case "$base" in
    *hf-mirror.com*|*modelscope*)
      echo "Hint: check the mirror/proxy under Settings → Downloads, then retry." ;;
    *)
      echo "Hint: if huggingface.co is unreachable, set the Hugging Face mirror to
https://hf-mirror.com (or configure a proxy) under Settings → Downloads,
then reinstall from the Models page." ;;
  esac
}

# lv_hf_download_repo <repo> <dest_dir> [workers] [file ...]
#
# Download an HF repo revision into <dest_dir>, preserving subdirectories,
# using only resolve-URL GET/range requests (works against hf-mirror.com or
# the official endpoint behind a proxy; honors HF_ENDPOINT and *_proxy).
# Files >= 8 MiB go through lv_fetch_parallel (resumable ranged slices);
# smaller files use a single curl. A file already present with the exact
# expected size is skipped, so re-runs resume cheaply. With extra `file`
# arguments only those repo paths are fetched; a caller-defined
# `lv_hf_skip <path>` function (returning 0) excludes paths otherwise.
lv_hf_download_repo() {
  local repo="$1" dest="$2" workers="${3:-8}"
  shift 3 2>/dev/null || true
  local wants=("$@")
  local base; base="$(lv_hf_base)"
  local parallel_min=$((8 * 1024 * 1024))

  local listing
  listing="$(lv_hf_repo_files "$repo" "$base")" || return 1
  if [ -z "$listing" ]; then
    echo "No downloadable files listed for ${repo} (${base})" >&2
    lv_hf_network_hint "$base" >&2
    return 1
  fi
  mkdir -p "$dest"

  # A here-string (not a pipe) keeps this loop in the current shell so
  # `return` propagates and downloads run in the foreground.
  local size rel fdest url tmp ok w
  while IFS=$'\t' read -r size rel; do
    # Native-Windows Python writes CRLF when piped; drop the trailing CR
    # so the allowlist string compare matches on every line.
    rel="${rel%$'\r'}"
    [ -n "${rel:-}" ] || continue
    if declare -F lv_hf_skip >/dev/null 2>&1 && lv_hf_skip "$rel"; then
      continue
    fi
    if [ "${#wants[@]}" -gt 0 ]; then
      ok=0
      for w in "${wants[@]}"; do
        [ "$rel" = "$w" ] && ok=1 && break
      done
      [ "$ok" = 1 ] || continue
    fi
    fdest="$dest/$rel"
    mkdir -p "$(dirname "$fdest")"
    if [ -f "$fdest" ] && [ "$(lv_filesize "$fdest")" = "${size:-0}" ]; then
      continue
    fi
    # A few espeak data files contain spaces (e.g. "!v/Mr serious"); the
    # local name keeps them, only the URL is percent-encoded.
    url="$base/$repo/resolve/main/${rel// /%20}"
    lv_stage_key stage_download_file "name=${rel}" "size=$(awk -v b="${size:-0}" 'BEGIN { printf "%.1f MB", b / 1048576 }')"
    if [ "${size:-0}" -ge "$parallel_min" ]; then
      lv_fetch_parallel "$url" "$fdest" "$size" "$workers"
    else
      tmp="$fdest.tmp.$$"
      curl -fsSL --retry 5 --retry-delay 2 -o "$tmp" "$url"
      if [ "$(lv_filesize "$tmp")" != "${size:-0}" ]; then
        echo "$rel: expected ${size:-0} bytes, got $(lv_filesize "$tmp")" >&2
        rm -f "$tmp"
        return 1
      fi
      mv -f "$tmp" "$fdest"
    fi
  done <<< "$listing"

  if [ "${#wants[@]}" -gt 0 ]; then
    for w in "${wants[@]}"; do
      if [ ! -f "$dest/$w" ]; then
        echo "$repo: $w missing after download" >&2
        return 1
      fi
    done
  fi
}

# lv_hf_download_file <repo> <repo_path> <dest> [workers] [label]
#
# Download one LFS file from <repo>'s main branch via a resolve URL, using
# lv_fetch_parallel with the server-advertised size (resumable ranged
# slices). A non-empty existing <dest> is left untouched.
lv_hf_download_file() {
  local repo="$1" rel="$2" dest="$3" workers="${4:-8}" label="${5:-Download $rel}"
  if [ -s "$dest" ]; then
    return 0
  fi
  local url; url="$(lv_hf_base)/$repo/resolve/main/${rel// /%20}"
  local total; total="$(lv_content_length "$url")"
  if [ "${total:-0}" -le 0 ]; then
    echo "cannot determine the download size for $url" >&2
    return 1
  fi
  lv_stage "$label"
  mkdir -p "$(dirname "$dest")"
  lv_fetch_parallel "$url" "$dest" "$total" "$workers"
}

# lv_extract_tarball <url> <tar_path> <extract_dir> [label]
#
# Download a .tar.bz2 release asset to <tar_path> and extract it into
# <extract_dir> (created). Non-zero on a failed/empty download.
lv_extract_tarball() {
  local url="$1" tar_path="$2" extract_dir="$3" label="${4:-Download archive}"
  lv_stage "$label"
  lv_download "$url" "$tar_path" || return 1
  [ -s "$tar_path" ] || { echo "downloaded archive is empty: $url" >&2; return 1; }
  lv_stage_key stage_extract
  mkdir -p "$extract_dir"
  tar -xjf "$tar_path" -C "$extract_dir"
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
  lv_stage_key stage_downloading "name=$(basename "$dest")"

  # A proxy that drops the connection mid-stream leaves a short file that
  # curl still reports as success, and a truncated archive extracts to
  # something plausible-looking but incomplete. Compare against the
  # advertised length and resume until it matches.
  local attempt=0
  while :; do
    have="$(lv_filesize "$dest")"
    if [ "${total:-0}" -gt 0 ] && [ "$have" -ge "$total" ]; then
      return 0
    fi
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
        lv_stage_key stage_download_size "name=$(basename "$dest")" "size=$(awk -v s="$size" 'BEGIN { printf "%.0f MB", s / 1048576 }')"
      fi
    done
    wait "$cpid" || true

    have="$(lv_filesize "$dest")"
    if [ "${total:-0}" -le 0 ]; then
      [ "$have" -gt 0 ] && return 0
    elif [ "$have" -ge "$total" ]; then
      return 0
    fi
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 3 ]; then
      echo "download incomplete after $attempt attempts: $url ($have of $total bytes)" >&2
      return 1
    fi
    echo "download stopped at $have of $total bytes; resuming" >&2
  done
}
