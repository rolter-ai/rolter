"""bring the docker-compose stack up/down and wait for it to be reachable.

kept dependency-free (subprocess + httpx + tenacity) so it can run in CI without
a docker python SDK. every ``up`` starts from a clean database (no named pg volume
in the compose file), so tests never see leftover state.
"""

from __future__ import annotations

import base64
import os
import pathlib
import subprocess

import httpx
from tenacity import retry, stop_after_delay, wait_fixed

HERE = pathlib.Path(__file__).resolve().parent.parent
COMPOSE_FILE = HERE / "docker-compose.e2e.yml"

# host-published endpoints (see docker-compose.e2e.yml port mappings)
CONTROL_URL = os.environ.get("ROLTER_E2E_CONTROL_URL", "http://localhost:4001")
GATEWAY_URL = os.environ.get("ROLTER_E2E_GATEWAY_URL", "http://localhost:4000")
ADMIN_TOKEN = os.environ.get("ROLTER_E2E_ADMIN_TOKEN", "e2e-superadmin-token")

# throwaway AES-256-GCM envelope key for provider api_key sealing. derived from a
# readable 32-byte phrase so no key-shaped base64 blob lives in git (keeps secret
# scanners quiet); the compose references ${ROLTER_KEK} and we inject this.
_TEST_KEK = base64.b64encode(b"rolter-e2e-test-kek-not-secret!!").decode()


def _compose_env() -> dict[str, str]:
    env = dict(os.environ)
    env.setdefault("ROLTER_KEK", _TEST_KEK)
    return env


class Stack:
    """thin wrapper over ``docker compose`` for the e2e topology."""

    def __init__(self, compose_file: pathlib.Path = COMPOSE_FILE):
        self.compose_file = compose_file

    def _compose(self, *args: str, check: bool = True) -> subprocess.CompletedProcess:
        cmd = ["docker", "compose", "-f", str(self.compose_file), *args]
        return subprocess.run(cmd, check=check, text=True, capture_output=True, env=_compose_env())

    def up(self, *, build: bool = True) -> None:
        args = ["up", "-d", "--wait", "--wait-timeout", "300"]
        if build:
            args.append("--build")
        self._compose(*args)
        self.wait_healthy()

    def down(self) -> None:
        # -v drops the (anonymous) volumes; leak-free between runs
        self._compose("down", "-v", "--remove-orphans", check=False)

    def logs(self, service: str | None = None) -> str:
        args = ["logs", "--no-color"]
        if service:
            args.append(service)
        return self._compose(*args, check=False).stdout

    @retry(stop=stop_after_delay(120), wait=wait_fixed(2), reraise=True)
    def wait_healthy(self) -> None:
        """block until both planes answer /healthz (compose --wait covers most of
        this, but the gateway needs its first snapshot fetch to settle)."""
        for url in (CONTROL_URL, GATEWAY_URL):
            resp = httpx.get(f"{url}/healthz", timeout=5)
            resp.raise_for_status()
