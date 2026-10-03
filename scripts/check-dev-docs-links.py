#!/usr/bin/env python3
"""Fail when a relative link in docs/dev-docs/ points at nothing.

The dev docs link to source files and to each other with relative paths, and
the depth is easy to get wrong: a page under docs/dev-docs/<section>/ reaches
the repository root with `../../../`, not `../../` (#1926). mdBook does not
check links to files outside its book, so nothing else catches a dead one.

Checks every `](target)` and `[ref]: target` in each .md file under
docs/dev-docs/ whose target is not a URL or a pure #anchor. The #anchor is
stripped and the rest must resolve to an existing file or directory. Fenced
code blocks and inline code are skipped. Pure standard library; run from
anywhere:

    python3 scripts/check-dev-docs-links.py
"""

import re
import sys
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs" / "dev-docs"

INLINE = re.compile(r"\]\(\s*(<[^>]*>|[^)\s]*)")
REFDEF = re.compile(r"^\s{0,3}\[[^\]]+\]:\s*(<[^>]*>|\S+)")
CODE_SPAN = re.compile(r"`[^`]*`")
SCHEME = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")


def targets(text):
    fence = None
    for number, line in enumerate(text.splitlines(), 1):
        stripped = line.lstrip()
        if fence:
            if stripped.startswith(fence):
                fence = None
            continue
        if stripped.startswith("```") or stripped.startswith("~~~"):
            fence = stripped[:3]
            continue
        line = CODE_SPAN.sub("", line)
        found = [m.group(1) for m in INLINE.finditer(line)]
        ref = REFDEF.match(line)
        if ref:
            found.append(ref.group(1))
        for target in found:
            yield number, target.strip("<>")


def main():
    broken = []
    for page in sorted(DOCS.rglob("*.md")):
        for number, target in targets(page.read_text(encoding="utf-8")):
            path = target.split("#", 1)[0].split("?", 1)[0]
            if not path or path.startswith("//") or SCHEME.match(path):
                continue
            base = ROOT if path.startswith("/") else page.parent
            if not (base / unquote(path).lstrip("/")).resolve().exists():
                broken.append(f"{page.relative_to(ROOT)}:{number}: broken link {target}")
    for line in broken:
        print(line)
    if broken:
        print(f"\n{len(broken)} broken relative link(s) under docs/dev-docs/", file=sys.stderr)
        return 1
    print("docs/dev-docs relative links ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
