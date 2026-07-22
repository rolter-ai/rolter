"""chaos / fault-injection reliability contracts (#620).

drives a dedicated static-config gateway (chaos/rolter-chaos.toml, tuned with a
short request timeout, a short cooldown, and the circuit breaker enabled) against
mock upstreams whose failure mode is fixed by their FAULT env var
(mock/faulty_upstream.py). the gateway and mocks live behind the compose
``chaos`` profile so they never slow the main e2e suite.

what is asserted, over the wire:
- a transient upstream failure (5xx / 429) is shed to a healthy sibling — the
  request still succeeds, and the failing target is parked on a cooldown
- when every target is down the gateway returns a clean 5xx contract (OpenAI-
  style JSON), never a 200 and never a panic
- a slow upstream trips the request timeout within a bound, not the full 60s
  default — no hang
- sustained failure trips the circuit breaker OPEN (asserted via the gateway's
  own ``rolter_breaker_opened_total`` metric — a real state transition, not just
  a status code)
- a flapping target degrades gracefully and is re-admitted (recovery), never
  permanently parked

not covered here (need signals this black-box harness can't drive): graceful
SIGTERM drain of in-flight requests, and per-provider bounded-queue backpressure
under OOM pressure — tracked as a follow-up.
"""

from __future__ import annotations

import os
import pathlib
import re
import subprocess
import time
from collections.abc import Iterator

import httpx
import pytest
from tenacity import retry, stop_after_delay, wait_fixed

HERE = pathlib.Path(__file__).resolve().parent.parent
COMPOSE_FILE = HERE / "docker-compose.e2e.yml"
CHAOS_URL = os.environ.get("ROLTER_E2E_CHAOS_URL", "http://localhost:4002")

# the chaos services (compose `chaos` profile) this suite manages — named
# explicitly so teardown never touches the main stack sharing the project
_CHAOS_SERVICES = [
    "gateway-chaos",
    "faulty-ok",
    "faulty-500",
    "faulty-500b",
    "faulty-429",
    "faulty-slow",
    "faulty-flap",
]

# throwaway KEK: compose interpolates ${ROLTER_KEK} across the whole file even
# when only chaos services are targeted, so it must be defined
import base64

_TEST_KEK = base64.b64encode(b"rolter-e2e-test-kek-not-secret!!").decode()


def _compose(*args: str, check: bool = True) -> subprocess.CompletedProcess:
    env = dict(os.environ)
    env.setdefault("ROLTER_KEK", _TEST_KEK)
    cmd = ["docker", "compose", "-f", str(COMPOSE_FILE), "--profile", "chaos", *args]
    return subprocess.run(cmd, check=check, text=True, capture_output=True, env=env)


@retry(stop=stop_after_delay(180), wait=wait_fixed(2), reraise=True)
def _wait_ready() -> None:
    httpx.get(f"{CHAOS_URL}/healthz", timeout=5).raise_for_status()


@retry(stop=stop_after_delay(120), wait=wait_fixed(2), reraise=True)
def _wait_upstreams() -> None:
    """the gateway answers /healthz before the python mock upstreams finish
    booting (compose depends_on waits only for container start), so poll a route
    whose success proves the healthy mock is actually accepting connections."""
    r = _chat(CHAOS_URL, "chaos-shed")
    if r.status_code != 200:
        raise AssertionError(f"upstreams not ready yet: chaos-shed -> {r.status_code}")


@pytest.fixture(scope="module")
def chaos() -> Iterator[str]:
    """bring up the chaos gateway + mock upstreams (unless NO_MANAGE), yield the
    gateway base url, and tear down only the chaos services on exit."""
    manage = os.environ.get("ROLTER_E2E_NO_MANAGE") != "1"
    if manage:
        _compose("up", "-d", "--build", "gateway-chaos")
    try:
        _wait_ready()
        _wait_upstreams()
        yield CHAOS_URL
    finally:
        if manage:
            _compose("rm", "-sfv", *_CHAOS_SERVICES, check=False)


def _chat(url: str, model: str, *, timeout: float = 20.0) -> httpx.Response:
    body = {"model": model, "messages": [{"role": "user", "content": "ping"}]}
    with httpx.Client(base_url=url, timeout=timeout) as c:
        return c.post("/v1/chat/completions", json=body)


def _metric(url: str, name: str) -> float:
    """sum the samples of a prometheus counter/gauge by metric name (labels
    ignored). missing metric → 0.0."""
    text = httpx.get(f"{url}/metrics", timeout=5).text
    total = 0.0
    pat = re.compile(rf"^{re.escape(name)}(?:\{{[^}}]*\}})?\s+([0-9eE.+-]+)$")
    for line in text.splitlines():
        if line.startswith("#"):
            continue
        m = pat.match(line.strip())
        if m:
            total += float(m.group(1))
    return total


def _assert_openai_error(resp: httpx.Response) -> None:
    """a rejection carries the OpenAI-style ``{"error": {"message": ...}}``
    envelope, not an empty body or an upstream stack trace."""
    body = resp.json()
    assert "error" in body and "message" in body["error"], f"not an openai error envelope: {body}"


# --------------------------------------------------------------------------- #
# a transient 5xx is shed to a healthy sibling                                 #
# --------------------------------------------------------------------------- #
def test_transient_5xx_sheds_to_healthy_target(chaos: str) -> None:
    """chaos-shed = [bad500, healthy]: retry + cooldown fail over so the caller
    still gets a 200, and the failing target is parked (cooldown metric moves)."""
    before = _metric(chaos, "rolter_cooldowns_tripped_total")
    # a few requests so the round-robin certainly hits the bad target at least once
    statuses = [_chat(chaos, "chaos-shed").status_code for _ in range(6)]
    assert all(s == 200 for s in statuses), f"failover did not hold: {statuses}"
    after = _metric(chaos, "rolter_cooldowns_tripped_total")
    assert after > before, f"a failing target was never parked on a cooldown ({before} -> {after})"


# --------------------------------------------------------------------------- #
# a rate-limited (429) target is shed to a healthy sibling                     #
# --------------------------------------------------------------------------- #
def test_rate_limited_target_fails_over(chaos: str) -> None:
    """chaos-429 = [bad429, healthy]: a 429 is transient — honor the cooldown and
    fail over so the caller still succeeds."""
    statuses = [_chat(chaos, "chaos-429").status_code for _ in range(6)]
    assert all(s == 200 for s in statuses), f"429 was not shed to the healthy target: {statuses}"


# --------------------------------------------------------------------------- #
# every target down → a clean 5xx contract, never a 200 or a panic             #
# --------------------------------------------------------------------------- #
def test_all_targets_down_returns_clean_5xx(chaos: str) -> None:
    """chaos-alldown = [bad500, bad500b]: with no healthy sibling the gateway
    exhausts retries and returns a bounded 5xx carrying the OpenAI error shape."""
    resp = _chat(chaos, "chaos-alldown")
    assert resp.status_code >= 500, f"expected a 5xx when all targets are down, got {resp.status_code}"
    assert resp.status_code < 600
    _assert_openai_error(resp)


# --------------------------------------------------------------------------- #
# sustained failure trips the circuit breaker OPEN (observed via metrics)      #
# --------------------------------------------------------------------------- #
def test_circuit_breaker_opens_on_sustained_failure(chaos: str) -> None:
    """hammering an all-down route past the breaker's failure_threshold trips the
    per-target breaker OPEN — asserted through the gateway's own
    ``rolter_breaker_opened_total`` counter, i.e. a real state transition.

    ``breaker_opened_total`` counts the closed→open transition, so on a long-
    lived gateway a target that is already open would not re-increment. to assert
    a fresh transition deterministically regardless of prior state, we drive the
    breaker open, let the open window (open_secs=3) lapse into half-open, then
    fail the half-open probe again — which re-opens it and moves the counter."""
    # phase 1: ensure the targets are open (or already open — either way)
    for _ in range(9):
        _chat(chaos, "chaos-alldown")
    # phase 2: let the open window lapse so the next failure is a fresh transition
    time.sleep(4)  # > open_secs (3)
    before = _metric(chaos, "rolter_breaker_opened_total")
    # phase 3: the half-open probes fail again → a new closed/half-open→open
    # transition per target → the counter must move
    for _ in range(9):
        _chat(chaos, "chaos-alldown")
    after = _metric(chaos, "rolter_breaker_opened_total")
    assert after > before, f"breaker never re-tripped open under sustained failure ({before} -> {after})"


# --------------------------------------------------------------------------- #
# a slow upstream trips the request timeout within a bound (no 60s hang)       #
# --------------------------------------------------------------------------- #
def test_slow_upstream_times_out_within_bound(chaos: str) -> None:
    """chaos-slow = [slow] (sleeps 5s) with request_secs=2: the request must fail
    cleanly and quickly — not hang for the 60s default — and every attempt
    (retries included) is bounded, so total elapsed stays well under a minute."""
    start = time.monotonic()
    resp = _chat(chaos, "chaos-slow", timeout=30.0)
    elapsed = time.monotonic() - start
    assert resp.status_code >= 500, f"a stalled upstream should surface a 5xx, got {resp.status_code}"
    _assert_openai_error(resp)
    # 1 initial + up to 2 retries, each bounded by the ~2s request timeout, plus
    # backoff — comfortably under 20s and nowhere near the 60s default
    assert elapsed < 20, f"slow upstream was not bounded by the timeout: {elapsed:.1f}s"


# --------------------------------------------------------------------------- #
# a flapping target degrades gracefully and is re-admitted (recovery)          #
# --------------------------------------------------------------------------- #
def test_flapping_target_degrades_and_recovers(chaos: str) -> None:
    """chaos-flap = [flap] (alternates 500/200): a single oscillating target is
    never permanently parked — over a spaced series of calls the gateway keeps
    re-admitting it, so we observe successes recovering after failures, and every
    response is a clean status (200 or a bounded 5xx), never a panic."""
    seen_ok = False
    seen_fail = False
    recovered_after_fail = False
    # space calls past the 2s cooldown so a parked target is re-probed
    for _ in range(8):
        status = _chat(chaos, "chaos-flap").status_code
        assert status == 200 or 500 <= status < 600, f"unexpected status {status}"
        if status == 200:
            if seen_fail:
                recovered_after_fail = True
            seen_ok = True
        else:
            seen_fail = True
        time.sleep(2.5)
    assert seen_ok, "a flapping target never served a single success (never re-admitted)"
    assert recovered_after_fail, "the target never recovered after a failure"
