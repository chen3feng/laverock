#!/usr/bin/env bash
# Lint the install scripts for a macOS-only trap.
#
# macOS ships bash 3.2, which parses `$var` followed directly by a
# multibyte character as part of the variable name: in
#
#     lv_stage "下载 $rel（…）"
#
# the name becomes `rel<U+FF08>`, and `set -u` aborts with
# "rel…: unbound variable". Every script uses `set -euo pipefail`, so a
# single Chinese progress line right after an unbraced variable kills an
# install. Require `${var}` whenever the next byte is non-ASCII.
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
status=0

# 1. Syntax check every script.
for f in "$DIR"/*.sh; do
  bash -n "$f" || { echo "syntax error: $f" >&2; status=1; }
done

# 2. No unbraced variable immediately followed by a non-ASCII byte.
python3 - "$DIR" <<'PY' || status=1
import glob
import re
import sys

root = sys.argv[1]
pattern = re.compile(r"\$([A-Za-z_][A-Za-z0-9_]*)(?=[^\x00-\x7f])")
bad = []
for path in sorted(glob.glob(f"{root}/*.sh")):
    with open(path, encoding="utf-8") as handle:
        for number, line in enumerate(handle, 1):
            if line.lstrip().startswith("#"):
                continue  # comments may quote the bad pattern
            if pattern.search(line):
                bad.append(f"{path}:{number}: {line.strip()}")
if bad:
    print("unbraced $var followed by a non-ASCII character (bash 3.2 misparses it):", file=sys.stderr)
    for entry in bad:
        print("  " + entry, file=sys.stderr)
    sys.exit(1)
PY

if [ "$status" -eq 0 ]; then
  echo "shell scripts OK"
fi
exit "$status"
