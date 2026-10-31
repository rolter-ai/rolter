#!/usr/bin/env python3
"""Append the control plane's deprecations to the newest changelog entry.

ADR-0032 promises that a deprecation is announced in the release notes, with
each sunset date. release-plz writes the changelog from commit messages and
cannot read the list in `crates/rolter-control/src/deprecation.rs`, so the
release PR gets the section from the rendered copy checked in beside it
(`crates/rolter-control/deprecations.md`, kept current by a unit test).

    scripts/release-deprecations.py            # append to the newest entry
    scripts/release-deprecations.py --check    # exit 1 if it would change

Does nothing while nothing is deprecated, and does nothing twice.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NOTES = ROOT / "crates" / "rolter-control" / "deprecations.md"
CHANGELOG = ROOT / "crates" / "rolter-gateway" / "CHANGELOG.md"

# a released entry, `## [1.2.0](...) - 2027-01-15`; `## [Unreleased]` has no digit
ENTRY = re.compile(r"^## \[\d", re.MULTILINE)
SECTION = "### Deprecations"


def updated(changelog: str, notes: str) -> str:
    """`changelog` with `notes` at the end of its newest released entry."""
    notes = notes.strip()
    if not notes:
        return changelog
    first = ENTRY.search(changelog)
    if not first:
        return changelog
    following = ENTRY.search(changelog, first.end())
    end = following.start() if following else len(changelog)
    if SECTION in changelog[first.start() : end]:
        return changelog
    entry = changelog[first.start() : end].rstrip("\n")
    rest = changelog[end:].lstrip("\n")
    out = changelog[: first.start()] + entry + "\n\n" + notes + "\n"
    return out + "\n" + rest if rest else out


def main() -> int:
    check = "--check" in sys.argv[1:]
    notes = NOTES.read_text() if NOTES.exists() else ""
    current = CHANGELOG.read_text()
    new = updated(current, notes)
    if new == current:
        return 0
    if check:
        print("::error::the newest changelog entry lacks the deprecations section")
        return 1
    CHANGELOG.write_text(new)
    print(f"added the deprecations section to {CHANGELOG.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
