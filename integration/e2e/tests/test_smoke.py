"""foundation smoke test (#614): a freshly booted stack registers the fleet and
serves a chat call end-to-end through the gateway.

this is the acceptance gate for the harness itself; the RBAC/balancing/keys/
security scenarios (#615-#618) build on the same fixtures and helpers.
"""

from __future__ import annotations

import time

from rolter_e2e.bootstrap import register_fleet
from rolter_e2e.client import ControlClient, GatewayClient
from rolter_e2e.stack import GATEWAY_URL


def test_planes_healthy(admin: ControlClient) -> None:
    # superadmin can read the composed snapshot the gateway consumes; it must be
    # a valid versioned config, not a validation-error envelope.
    snap = admin.snapshot()
    assert isinstance(snap, dict), snap
    assert "error" not in snap, snap
    assert "config" in snap and "version" in snap, snap


def test_fleet_bootstrap_and_chat(admin: ControlClient) -> None:
    # small fleet keeps the smoke fast; balancing scale lives in #616
    fleet = register_fleet(admin, models=3)
    assert fleet.virtual_key
    assert len(fleet.models) == 3

    gw = GatewayClient(GATEWAY_URL, virtual_key=fleet.virtual_key)

    # the gateway polls the snapshot every 2s; give the new routes a moment to
    # propagate before the first call.
    _wait_for_model(gw, fleet.models[0])

    resp = gw.chat(fleet.models[0], "hello from the e2e smoke test")
    assert resp.status_code == 200, resp.text
    body = resp.json()
    assert body["choices"][0]["message"]["content"]
    gw.close()


def test_anthropic_surface(admin: ControlClient) -> None:
    fleet = register_fleet(admin, models=1)
    gw = GatewayClient(GATEWAY_URL, virtual_key=fleet.virtual_key)
    _wait_for_model(gw, fleet.models[0])
    resp = gw.messages(fleet.models[0], "hello")
    assert resp.status_code == 200, resp.text
    gw.close()


def _wait_for_model(gw: GatewayClient, model: str, timeout: float = 20.0) -> None:
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        resp = gw.chat(model, "warmup")
        if resp.status_code == 200:
            return
        last = resp
        time.sleep(1.0)
    raise AssertionError(f"model {model} never became routable: {last.status_code if last else 'n/a'} "
                         f"{last.text if last else ''}")
