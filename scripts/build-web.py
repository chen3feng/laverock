#!/usr/bin/env python3
"""Assemble the web UI into the single file the server embeds.

The pages ship as one self-contained HTML file (no second request for
styles or scripts, no bundler at build time). Editing a 3,000-line HTML
file is miserable, so the sources live in `crates/laverock-server/web/`:

  index.html    shell; `{{styles.css}}`, `{{body.html}}`, `{{js}}`, `{{favicon}}`
  styles.css    the stylesheet
  body.html     the markup outside the pages (sidebar, overlays, templates);
                `{{nav}}` and `{{pages}}` are filled from pages.json
  pages.json    page index: id, i18n key, fallback label and nav icon
  pages/*.html  one `<section class="page">` per page, in pages.json order
  i18n/*.js     translation fragments, concatenated before the runtime
  js/NN-*.js    one file per section, concatenated in filename order
  favicon.png   32px tab icon, inlined as a data URI so the tab needs no request

The JavaScript is a plain script, not ES modules: the sections share
top-level declarations, and joining them in order reproduces exactly the
file that used to be written by hand.

  python3 scripts/build-web.py           rewrite assets/index.html
  python3 scripts/build-web.py --check   exit 1 if it is out of date
"""

from __future__ import annotations

import argparse
import base64
import difflib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "crates/laverock-server/web"
OUT = ROOT / "crates/laverock-server/assets/index.html"


def pages_index() -> list[dict]:
    """The page list from pages.json, validated so a typo fails the build."""
    raw = json.loads((WEB / "pages.json").read_text(encoding="utf-8"))
    if not raw:
        raise SystemExit(f"{WEB / 'pages.json'} is empty")
    for entry in raw:
        for key in ("id", "i18n", "label", "icon"):
            if not entry.get(key):
                raise SystemExit(f"pages.json entry {entry!r} is missing {key!r}")
        if not (WEB / "pages" / f"{entry['id']}.html").is_file():
            raise SystemExit(f"missing page source: web/pages/{entry['id']}.html")
    return raw


def build() -> str:
    shell = (WEB / "index.html").read_text(encoding="utf-8")
    styles = (WEB / "styles.css").read_text(encoding="utf-8").strip("\n")
    pages = pages_index()
    nav = "\n".join(
        "    <a href=\"#/{id}\" data-page=\"{id}\">\n"
        "      <svg viewBox=\"0 0 24 24\">{icon}</svg>\n"
        "      <span data-i18n=\"{i18n}\">{label}</span>\n"
        "    </a>".format(**entry)
        for entry in pages
    )
    page_html = "\n\n".join(
        (WEB / "pages" / f"{entry['id']}.html")
        .read_text(encoding="utf-8")
        .rstrip("\n")
        for entry in pages
    )
    body_src = (WEB / "body.html").read_text(encoding="utf-8")
    for token in ("{{nav}}", "{{pages}}"):
        if token not in body_src:
            raise SystemExit(f"body.html is missing {token}")
    body = body_src.replace("{{nav}}", nav).replace("{{pages}}", page_html)
    scripts = sorted((WEB / "i18n").glob("*.js")) + sorted((WEB / "js").glob("*.js"))
    if not scripts:
        raise SystemExit(f"no JavaScript sources under {WEB / 'js'}")
    page_ids = ", ".join(json.dumps(entry["id"]) for entry in pages)
    js = "\n\n".join(
        p.read_text(encoding="utf-8")
        .rstrip("\n")
        .replace("{{page_ids}}", page_ids)
        for p in scripts
    )
    favicon = "data:image/png;base64," + base64.b64encode(
        (WEB / "favicon.png").read_bytes()).decode("ascii")

    for token in ("{{styles.css}}", "{{body.html}}", "{{js}}", "{{favicon}}"):
        if token not in shell:
            raise SystemExit(f"index.html is missing {token}")
    return (
        shell.replace("{{styles.css}}", styles)
        .replace("{{body.html}}", body)
        .replace("{{js}}", js)
        .replace("{{favicon}}", favicon)
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true",
                        help="verify assets/index.html matches web/ instead of writing it")
    args = parser.parse_args()

    generated = build()
    current = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
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

    OUT.write_text(generated, encoding="utf-8", newline="\n")
    print(f"wrote {OUT.relative_to(ROOT)} ({len(generated.splitlines())} lines)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
