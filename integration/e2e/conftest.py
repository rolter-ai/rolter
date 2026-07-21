"""shared fixtures: bring the stack up once per session, expose admin clients and
a fleet-bootstrap helper the scenario modules consume.

set ROLTER_E2E_NO_MANAGE=1 to test against an already-running stack (e.g. one you
brought up by hand, or a CI job that manages compose lifecycle itself).
"""

from __future__ import annotations

import os
from collections.abc import Iterator

import pytest

from rolter_e2e.client import ControlClient, GatewayClient
from rolter_e2e.stack import ADMIN_TOKEN, CONTROL_URL, GATEWAY_URL, Stack


def pytest_collection_modifyitems(items: list[pytest.Item]) -> None:
    # everything under tests/ needs the live stack
    for item in items:
        item.add_marker(pytest.mark.e2e)


@pytest.fixture(scope="session")
def stack() -> Iterator[Stack]:
    s = Stack()
    manage = os.environ.get("ROLTER_E2E_NO_MANAGE") != "1"
    if manage:
        s.up(build=os.environ.get("ROLTER_E2E_NO_BUILD") != "1")
    else:
        s.wait_healthy()
    try:
        yield s
    finally:
        if manage and os.environ.get("ROLTER_E2E_KEEP") != "1":
            s.down()


@pytest.fixture(scope="session")
def admin(stack: Stack) -> Iterator[ControlClient]:
    """superadmin control client, authenticated with the bootstrap admin token."""
    c = ControlClient(CONTROL_URL, token=ADMIN_TOKEN)
    try:
        yield c
    finally:
        c.close()


@pytest.fixture
def gateway() -> Iterator[GatewayClient]:
    """unauthenticated gateway client; bind a key via .`_http` or construct one
    directly in a test with the minted virtual key."""
    g = GatewayClient(GATEWAY_URL)
    try:
        yield g
    finally:
        g.close()


def gateway_for(virtual_key: str) -> GatewayClient:
    """gateway client authenticated with a specific minted virtual key."""
    return GatewayClient(GATEWAY_URL, virtual_key=virtual_key)
