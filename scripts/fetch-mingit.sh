#!/usr/bin/env bash
# Fetch a portable Git-for-Windows runtime for the Windows desktop bundle.
#
# The model installers are POSIX shell scripts (scripts/download-*.sh), so a
# machine without Git for Windows / WSL could not run them. Rather than asking
# the user to install anything, the Windows installer carries a portable Git:
# a redistributable, no-admin subset providing bash, coreutils, tar and curl.
# The server runs <root>/bin/bash.exe and prepends its bin dirs to PATH.
#
# Note: the slimmer "MinGit" artifacts (as of Git 2.56) no longer contain
# bash/curl/tar, so this fetches the full PortableGit self-extracting 7z and
# unpacks it silently. The extracted tree is gitignored, never committed.
#
# Runs at bundle-time (CI release job and locally); no-op outside Windows
# unless MINGIT_FORCE=1.
#
#   MINGIT_VERSION=2.56.0.4   override the pinned asset version
#   MINGIT_WINDOWS_N=4        override the pinned .windows.N release suffix
#   MINGIT_URL=...            override the whole download URL
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST="$ROOT/crates/laverock-desktop/mingit"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) ;;
  *)
    if [ "${MINGIT_FORCE:-0}" != "1" ]; then
      echo "Portable Git is only bundled for Windows; skipping fetch on $(uname -s)."
      exit 0
    fi
    ;;
esac

if [ -x "$DEST/bin/bash.exe" ]; then
  echo "Portable Git already present at $DEST"
  exit 0
fi

VERSION="${MINGIT_VERSION:-2.56.0.4}"
WIN="${MINGIT_WINDOWS_N:-4}"
ASSET="PortableGit-${VERSION}-64-bit.7z.exe"
URL="${MINGIT_URL:-https://github.com/git-for-windows/git/releases/download/v${VERSION}.windows.${WIN}/${ASSET}}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
sfx="$tmp/$ASSET"
echo "Downloading $ASSET"
download() { # url dest
  curl -fSL --retry 3 --retry-all-errors "$1" -o "$2"
}
if ! download "$URL" "$sfx"; then
  # The asset's four-digit version does not encode the release's .windows.N
  # suffix; probe neighbouring revisions before giving up.
  for n in 1 2 3 5 6; do
    cand="https://github.com/git-for-windows/git/releases/download/v${VERSION}.windows.${n}/${ASSET}"
    if download "$cand" "$sfx"; then URL="$cand"; WIN="$n"; break; fi
  done
fi
[ -s "$sfx" ] || { echo "could not download PortableGit from $URL" >&2; exit 1; }

# The release asset is a 7-Zip self-extracting archive; its silent switches
# are `-y` (assume yes) and `-o<dir>` (no space before the path).
echo "Extracting portable Git into $DEST"
mkdir -p "$DEST"
chmod +x "$sfx" 2>/dev/null || true
"$sfx" -y "-o$(cygpath -w "$DEST")"

[ -x "$DEST/bin/bash.exe" ] || { echo "bash.exe missing after extracting PortableGit" >&2; exit 1; }
echo "Portable Git $VERSION (windows.$WIN) ready at $DEST"
