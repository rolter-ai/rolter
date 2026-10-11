#!/usr/bin/env python3
# /// script
# requires-python = ">=3.9"
# dependencies = ["pyyaml==6.0.3"]
# ///
"""Run the rendered preflight init containers' `rolter check` against their own env.

A default `helm install` once could never get past its own strict preflight: the
chart's `0.0.0.0` bind warned, and the gateway pod was held to the control
plane's rules (#1939). Rendering the chart proves neither, so this takes the
rendered manifests on stdin, finds every init container that runs `rolter
check`, and runs the real binary with exactly that container's args and env.

    helm template ci charts/rolter --set ... \\
      | uv run --script scripts/check-chart-preflight.py target/debug/rolter

The chart's config file is written out of its ConfigMap, the way the pod mounts
it. The container's env is the only environment the binary sees, so a variable
the chart forgot to pass is missing here too.
"""

import os
import subprocess
import sys
import tempfile

import yaml

LAUNCHER = "/usr/local/bin/rolter"
CONFIG_MOUNT = "/etc/rolter/rolter.toml"


def main(binary):
    docs = [d for d in yaml.safe_load_all(sys.stdin) if isinstance(d, dict)]
    config = next(
        (d["data"]["rolter.toml"] for d in docs if d.get("kind") == "ConfigMap" and "rolter.toml" in (d.get("data") or {})),
        None,
    )
    if config is None:
        sys.exit("no rendered ConfigMap carries rolter.toml")

    failures = []
    ran = []
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "rolter.toml")
        with open(path, "w") as f:
            f.write(config)
        for doc in docs:
            spec = ((doc.get("spec") or {}).get("template") or {}).get("spec") or {}
            for c in spec.get("initContainers") or []:
                if LAUNCHER not in (c.get("command") or []):
                    continue
                args = [path if a == CONFIG_MOUNT else a for a in c.get("args") or []]
                env = {e["name"]: e["value"] for e in c.get("env") or [] if "value" in e}
                # the update notice dials github; the chart's own value is
                # covered by check-chart-update-check.py
                env["ROLTER_UPDATE_CHECK"] = "false"
                where = f"{doc['kind']}/{doc['metadata']['name']}"
                result = subprocess.run([binary, *args], env=env, capture_output=True, text=True)
                ran.append(where)
                print(f"--- {where}: rolter {' '.join(args)} (exit {result.returncode})")
                print(result.stdout + result.stderr)
                if result.returncode != 0:
                    failures.append(where)

    if len(ran) < 2:
        failures.append(f"ran {len(ran)} preflight containers, want control and gateway")
    if failures:
        sys.exit("preflight failed for: " + ", ".join(failures))
    print(f"preflight passed for: {', '.join(ran)}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: check-chart-preflight.py <path to the rolter binary>")
    main(sys.argv[1])
