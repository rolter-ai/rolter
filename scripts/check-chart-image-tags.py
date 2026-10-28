#!/usr/bin/env python3
# /// script
# requires-python = ">=3.9"
# dependencies = ["pyyaml==6.0.3"]
# ///
"""Fail when a Deployment's containers run another image tag than expected.

The chart tags the control plane and the gateways apart (#3352): each
component's `<component>.image.tag` wins, then `image.tag`, then the chart's
appVersion. The preflight initContainer of each Deployment runs the same image
as the workload beside it, so it is held to the same tag.

Reads a multi-document manifest on stdin and takes the tag each component
should run:

    helm template ci charts/rolter --set gateway.image.tag=1.2.0 \\
      | uv run --script scripts/check-chart-image-tags.py 1.3.0 1.2.0
"""

import sys

import yaml

# the Deployment a component's pods belong to, by its name's suffix
COMPONENTS = {"control": "-control", "gateway": "-gateway"}


def containers(doc):
    spec = ((doc.get("spec") or {}).get("template") or {}).get("spec") or {}
    yield from spec.get("initContainers") or []
    yield from spec.get("containers") or []


def main(control, gateway):
    expected = {"control": control, "gateway": gateway}
    failures = []
    seen = {name: 0 for name in COMPONENTS}
    for doc in yaml.safe_load_all(sys.stdin):
        if not isinstance(doc, dict) or doc.get("kind") != "Deployment":
            continue
        name = (doc.get("metadata") or {}).get("name") or ""
        component = next((c for c, suffix in COMPONENTS.items() if name.endswith(suffix)), None)
        if component is None:
            continue
        for c in containers(doc):
            seen[component] += 1
            tag = (c.get("image") or "").rsplit(":", 1)[-1]
            if tag != expected[component]:
                failures.append(
                    f"Deployment/{name} container {c.get('name')}: image {c.get('image')!r}, "
                    f"want tag {expected[component]!r}"
                )
    for component, count in seen.items():
        if count == 0:
            failures.append(f"no {component} container rendered; the check proves nothing")
    for f in failures:
        print(f"error: {f}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print("usage: check-chart-image-tags.py <control tag> <gateway tag>", file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1], sys.argv[2]))
