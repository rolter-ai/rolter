"""multi-cluster balancing verification (#616).

proves the balancer actually balances and that strategies produce *observably
different* target distributions, using the fake-vLLM fleet (sim-a/-b/-c) as
distinct targets. every proxied response carries an ``x-rolter-provider`` header
naming the target that served it, so a burst of requests can be tallied into a
per-target distribution and asserted against each strategy's contract.

distribution assertions are statistical but stable: fixed request counts and
documented tolerance bands, driven at content-identical sims so the only
variable is the balancer's choice.
"""

from __future__ import annotations

import uuid
from collections import Counter

import pytest

from rolter_e2e.bootstrap import SIM_ENGINES, UPSTREAM_MODEL, new_tenant
from rolter_e2e.client import ControlClient, GatewayClient
from rolter_e2e.stack import GATEWAY_URL

# a target whose upstream never answers — used for the unhealthy-skip case. the
# host does not resolve/connect on the compose network, so requests to it fail
# and the gateway cools it down and routes around it.
DEAD_ENGINE = "http://sim-dead.invalid:9999"


def _rand(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:6]}"


def _served_by(resp) -> str:
    """the target that served a proxied response (provider slug/name)."""
    return resp.headers.get("x-rolter-provider", "?")


class _Balancer:
    """helper that builds a single multi-target route under one strategy and
    fires deterministic bursts at it, returning the per-target tally."""

    def __init__(self, admin: ControlClient, tenant):
        self.admin = admin
        self.tenant = tenant

    def build(self, strategy: str, *, engines=None, weights=None) -> tuple[str, GatewayClient, list[str]]:
        engines = engines or SIM_ENGINES
        tok = uuid.uuid4().hex[:6]
        model = f"bal-{strategy}-{tok}"
        providers = [
            self.admin.create_provider(self.tenant.org_id, api_base=base, name=f"{model}-p{i}")
            for i, base in enumerate(engines)
        ]
        route = self.admin.create_route(self.tenant.project_id, model, strategy=strategy)
        for i, p in enumerate(providers):
            w = weights[i] if weights else 1
            self.admin.add_route_target(route["id"], p["id"], upstream_model=UPSTREAM_MODEL, weight=w)
        # scope a key to this model so the burst works regardless of whether other
        # tests have flipped the gateway into key-enforcing mode
        key = self.admin.create_virtual_key(self.tenant.project_id, models=[model])
        gw = GatewayClient(GATEWAY_URL, virtual_key=key["key"])
        names = [p["name"] for p in providers]
        _warmup(gw, model)
        return model, gw, names

    def fire(self, gw: GatewayClient, model: str, n: int, *, prompt: str = "ping",
             distinct: bool = False) -> Counter:
        tally: Counter = Counter()
        for i in range(n):
            body = f"{prompt}-{i}" if distinct else prompt
            resp = gw.chat(model, body)
            assert resp.status_code == 200, f"chat {i} failed: {resp.status_code} {resp.text}"
            tally[_served_by(resp)] += 1
        return tally


def _warmup(gw: GatewayClient, model: str, tries: int = 30) -> None:
    import time
    for _ in range(tries):
        if gw.chat(model, "warmup").status_code == 200:
            return
        time.sleep(1)
    raise AssertionError(f"model {model} never became routable")


@pytest.fixture
def bal(admin: ControlClient) -> _Balancer:
    return _Balancer(admin, new_tenant(admin))


def test_round_robin_is_near_uniform(bal: _Balancer) -> None:
    """round_robin rotates evenly across all targets."""
    model, gw, names = bal.build("round_robin")
    tally = bal.fire(gw, model, 30)
    gw.close()

    assert set(tally) == set(names), f"not all targets used: {tally}"
    lo, hi = min(tally.values()), max(tally.values())
    # smooth round-robin over 30/3: expect ~10 each; allow a small band for
    # concurrent in-flight bookkeeping
    assert hi - lo <= 3, f"round_robin not uniform: {dict(tally)}"


def test_weighted_tracks_target_weight(bal: _Balancer) -> None:
    """weighted skews the distribution toward higher-weight targets."""
    model, gw, names = bal.build("weighted", weights=[1, 1, 4])
    tally = bal.fire(gw, model, 60)
    gw.close()

    heavy = names[2]
    light = [names[0], names[1]]
    # weight 4 vs 1+1: the heavy target should clearly dominate each light one
    assert all(tally[heavy] > 2 * tally[l] for l in light), f"weight not honored: {dict(tally)}"
    assert all(tally[l] > 0 for l in light), f"light targets starved: {dict(tally)}"


def test_cache_aware_pins_same_prefix(bal: _Balancer) -> None:
    """cache_aware sends the same prompt prefix to the same target (affinity)."""
    model, gw, names = bal.build("cache_aware")
    same = bal.fire(gw, model, 20, prompt="the-same-prefix", distinct=False)
    gw.close()

    top = max(same.values())
    # identical prefix must stick: the dominant target should take the vast
    # majority of the burst (affinity), not a round-robin spread
    assert top >= 0.8 * sum(same.values()), f"cache_aware did not pin the prefix: {dict(same)}"


def test_strategies_produce_different_distributions(bal: _Balancer) -> None:
    """the core claim: same input, different strategy → different distribution.

    round_robin spreads a fixed prompt across all targets; cache_aware pins the
    same prompt to one. the two distributions must differ.
    """
    rr_model, rr_gw, _ = bal.build("round_robin")
    ca_model, ca_gw, _ = bal.build("cache_aware")
    rr = bal.fire(rr_gw, rr_model, 24, prompt="identical")
    ca = bal.fire(ca_gw, ca_model, 24, prompt="identical")
    rr_gw.close()
    ca_gw.close()

    # round_robin uses all 3; cache_aware concentrates — distinct spread counts
    assert len(rr) >= 2, f"round_robin unexpectedly concentrated: {dict(rr)}"
    assert max(ca.values()) > max(rr.values()), (
        f"cache_aware not more concentrated than round_robin: rr={dict(rr)} ca={dict(ca)}"
    )


def test_power_of_two_distributes(bal: _Balancer) -> None:
    """power_of_two spreads load rather than pinning one target.

    with content-identical, instant sims there is no standing load imbalance to
    skew toward, so this asserts the weaker, stable property — traffic reaches
    more than one target. deterministic load-skew under real concurrency is
    covered by the crate-level balancer unit tests.
    """
    model, gw, names = bal.build("power_of_two")
    tally = bal.fire(gw, model, 30)
    gw.close()
    assert len(tally) >= 2, f"power_of_two pinned a single target: {dict(tally)}"


def test_unhealthy_target_is_skipped(bal: _Balancer) -> None:
    """a target whose upstream fails is cooled down; traffic routes around it.

    builds a route over two healthy sims plus one dead engine, warms up, then
    fires a burst and asserts every response is served (failover works) and the
    dead target serves none of them.
    """
    model, gw, names = bal.build("round_robin", engines=[SIM_ENGINES[0], SIM_ENGINES[1], DEAD_ENGINE])
    dead = names[2]
    healthy = {names[0], names[1]}

    tally = bal.fire(gw, model, 30)
    gw.close()

    assert tally[dead] == 0, f"dead target still served traffic: {dict(tally)}"
    assert set(tally).issubset(healthy | {dead}), f"unexpected target: {dict(tally)}"
    assert sum(tally.values()) == 30, f"lost requests to the dead target: {dict(tally)}"
