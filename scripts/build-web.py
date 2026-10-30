#!/usr/bin/env python3
"""Assemble the web UI into the single file the server embeds.

The pages ship as one self-contained HTML file (no second request for
styles or scripts, no bundler at build time). Editing a 3,000-line HTML
file is miserable, so the sources live in `crates/laverock-server/web/`:

  index.html    shell; `{{styles.css}}`, `{{body.html}}`, `{{js}}`
  styles.css    the stylesheet
  body.html     the markup, including <template> blocks
  js/NN-*.js    one file per section, concatenated in filename order

The JavaScript is a plain script, not ES modules: the sections share
top-level declarations, and joining them in order reproduces exactly the
file that used to be written by hand.

  python3 scripts/build-web.py           rewrite assets/index.html
  python3 scripts/build-web.py --check   exit 1 if it is out of date
"""

from __future__ import annotations

import argparse
import difflib
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "crates/laverock-server/web"
OUT = ROOT / "crates/laverock-server/assets/index.html"


def build() -> str:
    shell = (WEB / "index.html").read_text()
    styles = (WEB / "styles.css").read_text().strip("\n")
    body = (WEB / "body.html").read_text()
    scripts = sorted((WEB / "js").glob("*.js"))
    if not scripts:
        raise SystemExit(f"no JavaScript sources under {WEB / 'js'}")
    js = "\n\n".join(p.read_text().rstrip("\n") for p in scripts)

    for token in ("{{styles.css}}", "{{body.html}}", "{{js}}"):
        if token not in shell:
            raise SystemExit(f"index.html is missing {token}")
    return (
        shell.replace("{{styles.css}}", styles)
        .replace("{{body.html}}", body)
        .replace("{{js}}", js)
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true",
                        help="verify assets/index.html matches web/ instead of writing it")
    args = parser.parse_args()

    generated = build()
    current = OUT.read_text() if OUT.exists() else ""
    if args.check:
        if generated == current:
            return 0
        diff = difflib.unified_diff(
            current.splitlines(), generated.splitlines(),
            fromfile=str(OUT.relative_to(ROOT)), tofile="web/ (rebuilt)",
            lineterm="", n=2)
        sys.stderr.write(
            "assets/index.html is out of date; run `python3 scripts/build-web.py`\n")
        sys.stderr.write("\n".join(list(diff)[:60]) + "\n")
        return 1

    OUT.write_text(generated)
    print(f"wrote {OUT.relative_to(ROOT)} ({len(generated.splitlines())} lines)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
