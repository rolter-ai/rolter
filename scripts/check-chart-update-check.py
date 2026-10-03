#!/usr/bin/env python3
# /// script
# requires-python = ">=3.9"
# dependencies = ["pyyaml==6.0.3"]
# ///
"""Fail when a container that runs the `rolter` launcher lacks ROLTER_UPDATE_CHECK.

The launcher spawns its release check beside every subcommand, so the preflight
initContainer (`rolter check`) of each Deployment asks GitHub for a release
unless the variable reaches it. `control.updateCheck` was written into the
control plane's environment only, so an air-gapped install still dialled
api.github.com on every gateway pod start (#2382).

Reads a multi-document manifest on stdin and takes the expected value:

    helm template ci charts/rolter --set control.updateCheck=false \\
      | uv run --script scripts/check-chart-update-check.py false
"""

import sys

import yaml

LAUNCHER = "/usr/local/bin/rolter"


def containers(doc):
    spec = ((doc.get("spec") or {}).get("template") or {}).get("spec") or {}
    yield from spec.get("initContainers") or []
    yield from spec.get("containers") or []


def main(expected):
    failures = []
    seen = 0
    for doc in yaml.safe_load_all(sys.stdin):
        if not isinstance(doc, dict):
            continue
        for c in containers(doc):
            if LAUNCHER not in (c.get("command") or []):
                continue
            seen += 1
            values = [e.get("value") for e in c.get("env") or [] if e.get("name") == "ROLTER_UPDATE_CHECK"]
            where = f"{doc.get('kind')}/{(doc.get('metadata') or {}).get('name')} container {c.get('name')}"
            if not values or values[-1] != expected:
                failures.append(f"{where}: ROLTER_UPDATE_CHECK is {values or 'unset'}, want {expected!r}")
    # one launcher container per Deployment; none means the check proves nothing
    if seen < 2:
        failures.append(f"found {seen} launcher containers, want at least 2 (control and gateway preflight)")
    for f in failures:
        print(f"error: {f}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: check-chart-update-check.py <expected value>")
    sys.exit(main(sys.argv[1]))
