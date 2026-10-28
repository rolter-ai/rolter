#!/usr/bin/env python3
"""Check the rendered SigNoz Compose startup contract; no daemon is needed."""

import json
import sys


def check(config):
    services = config["services"]
    migrator = services["signoz-schema-migrator"]
    command = migrator["command"][-1]
    steps = [
        "migrate ready",
        "migrate bootstrap",
        "migrate sync up",
        "migrate async up",
    ]
    positions = [command.index(step) for step in steps]
    assert positions == sorted(positions), "migration readiness must precede schema creation"
    for name in ("signoz-otel-collector", "signoz"):
        service = services[name]
        dependency = service["depends_on"]["signoz-schema-migrator"]
        assert dependency["condition"] == "service_completed_successfully", name
    collector = services["signoz-otel-collector"]
    assert collector["entrypoint"] == ["/bin/sh"]
    command = collector["command"][-1]
    assert "migrate sync check &&" in command
    assert command.index("migrate sync check") < command.index(
        "exec /signoz-otel-collector --config="
    )


if __name__ == "__main__":
    check(json.load(sys.stdin))
    print("SigNoz startup contract passed")
