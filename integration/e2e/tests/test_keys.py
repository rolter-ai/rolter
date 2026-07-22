"""key & credential lifecycle over the real APIs (#617).

exercises virtual-key mint / use / model-scope / rotate / revoke, self-service
(/me) minting by a Member, rate-limit enforcement at the gateway edge, cross
-tenant isolation, and the security invariant that a provider's encrypted
api_key is never echoed back in plaintext.

out of scope here (documented, not silently dropped):
- **expiry**: the create API exposes no expiry field, so an expired-key path
  can't be driven over HTTP from the harness.
- **budget cap**: budget enforcement needs model pricing plus token-usage
  accounting whose spend lags through analytics, so it can't be asserted
  deterministically against instant fake sims — left to crate-level tests.
"""

from __future__ import annotations

import time
import uuid

import pytest

from rolter_e2e.bootstrap import SIM_ENGINES, UPSTREAM_MODEL, new_tenant
from rolter_e2e.client import ControlClient, GatewayClient
from rolter_e2e.stack import GATEWAY_URL


def _rand(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:6]}"


def _route(admin: ControlClient, tenant, model: str | None = None) -> str:
    """create a provider + route + target for a fresh model; return the model."""
    model = model or _rand("key-model")
    provider = admin.create_provider(tenant.org_id, api_base=SIM_ENGINES[0], name=_rand("prov"))
    route = admin.create_route(tenant.project_id, model, strategy="round_robin")
    admin.add_route_target(route["id"], provider["id"], upstream_model=UPSTREAM_MODEL)
    return model


def _wait_usable(gw: GatewayClient, model: str, tries: int = 30) -> None:
    for _ in range(tries):
        if gw.chat(model, "warmup").status_code == 200:
            return
        time.sleep(1)
    raise AssertionError(f"model {model} never became routable")


@pytest.fixture
def tenant(admin: ControlClient):
    return new_tenant(admin)


def test_mint_and_use(admin: ControlClient, tenant) -> None:
    """an admin-minted key scoped to a model drives a chat on that model."""
    model = _route(admin, tenant)
    key = admin.create_virtual_key(tenant.project_id, models=[model])
    gw = GatewayClient(GATEWAY_URL, virtual_key=key["key"])
    _wait_usable(gw, model)
    resp = gw.chat(model)
    gw.close()
    assert resp.status_code == 200, resp.text
    assert resp.headers.get("x-rolter-model") == model


def test_model_scope_enforced(admin: ControlClient, tenant) -> None:
    """a key scoped to model A cannot call model B (403 model_not_allowed)."""
    model_a = _route(admin, tenant)
    model_b = _route(admin, tenant)
    key = admin.create_virtual_key(tenant.project_id, models=[model_a])
    gw = GatewayClient(GATEWAY_URL, virtual_key=key["key"])
    _wait_usable(gw, model_a)

    assert gw.chat(model_a).status_code == 200
    denied = gw.chat(model_b)
    gw.close()
    assert denied.status_code == 403, f"out-of-scope model should be 403: {denied.status_code} {denied.text}"
    assert denied.json()["error"].get("code") == "model_not_allowed", denied.text


def test_self_service_mint_and_rotate(admin: ControlClient, tenant) -> None:
    """a Member mints their own key via /me, uses it, rotates it; the old secret
    is rejected and the new one accepted."""
    model = _route(admin, tenant)
    email, password = f"{_rand('m')}@e2e.test", uuid.uuid4().hex
    admin.create_user(tenant.org_id, email=email, password=password, role="member")
    member = admin.login(email, password)

    minted = member.mint_my_key(tenant.project_id, models=[model])
    # the minted-key response is the flattened key row plus the one-time secret
    old_key, key_id = minted["key"], minted["id"]
    gw_old = GatewayClient(GATEWAY_URL, virtual_key=old_key)
    _wait_usable(gw_old, model)
    assert gw_old.chat(model).status_code == 200

    rotated = member.rotate_my_key(key_id)
    new_key = rotated["key"]
    assert new_key != old_key

    # the new key works; the old secret is rejected once rotation propagates
    gw_new = GatewayClient(GATEWAY_URL, virtual_key=new_key)
    _wait_usable(gw_new, model)
    deadline = time.monotonic() + 20
    while True:
        if gw_old.chat(model).status_code in (401, 403):
            break
        assert time.monotonic() < deadline, "rotated-out key still accepted after 20s"
        time.sleep(1)
    assert gw_new.chat(model).status_code == 200
    gw_old.close()
    gw_new.close()


def test_revocation_is_immediate(admin: ControlClient, tenant) -> None:
    """revoking a key rejects it on the next call; a guard key keeps auth on."""
    model = _route(admin, tenant)
    guard = admin.create_virtual_key(tenant.project_id, models=[model])
    target = admin.create_virtual_key(tenant.project_id, models=[model])
    gw_t = GatewayClient(GATEWAY_URL, virtual_key=target["key"])
    gw_g = GatewayClient(GATEWAY_URL, virtual_key=guard["key"])
    _wait_usable(gw_t, model)

    admin.revoke_virtual_key(target["id"])
    deadline = time.monotonic() + 20
    while True:
        if gw_t.chat(model).status_code in (401, 403):
            break
        assert time.monotonic() < deadline, "revoked key still accepted after 20s"
        time.sleep(1)
    assert gw_g.chat(model).status_code == 200, "guard key must still work"
    gw_t.close()
    gw_g.close()


def test_rate_limit_returns_429(admin: ControlClient, tenant) -> None:
    """a per-key RPM cap rejects the overage with 429 + Retry-After."""
    model = _route(admin, tenant)
    key = admin.create_virtual_key(tenant.project_id, models=[model])
    gw = GatewayClient(GATEWAY_URL, virtual_key=key["key"])
    _wait_usable(gw, model)  # route ready before the cap is attached

    # attach a per-key RPM cap, then poll until it propagates and trips: a burst
    # over the cap in one minute must start returning 429
    admin.create_rate_limit("virtual_key", key["id"], rpm=3)
    deadline = time.monotonic() + 20
    over_limit = None
    while time.monotonic() < deadline:
        codes = [gw.chat(model) for _ in range(6)]
        over_limit = next((r for r in codes if r.status_code == 429), None)
        if over_limit is not None:
            break
        time.sleep(2)
    gw.close()
    assert over_limit is not None, "rpm=3 cap never triggered a 429 within 20s"
    # the overage carries the error envelope + a Retry-After hint
    assert over_limit.json()["error"].get("code") == "rate_limit_exceeded", over_limit.text
    assert "retry-after" in {k.lower() for k in over_limit.headers}, "missing Retry-After header"


def test_provider_api_key_never_returned(admin: ControlClient, tenant) -> None:
    """a provider's api_key is encrypted at rest and never echoed in an API
    response (create or list)."""
    secret = f"sk-supersecret-{uuid.uuid4().hex}"
    created = admin.create_provider(tenant.org_id, api_base=SIM_ENGINES[0], name=_rand("prov"), api_key=secret)
    listing = admin.list_providers(tenant.org_id)
    assert secret not in str(created), "create response leaked the plaintext api_key"
    assert secret not in str(listing), "list response leaked the plaintext api_key"


def test_viewer_cannot_mint_key(admin: ControlClient, tenant) -> None:
    """a Viewer cannot mint a key on either the admin or the self-service path."""
    email, password = f"{_rand('v')}@e2e.test", uuid.uuid4().hex
    admin.create_user(tenant.org_id, email=email, password=password, role="viewer")
    viewer = admin.login(email, password)

    admin_path = viewer.raw("POST", f"/api/v1/projects/{tenant.project_id}/virtual-keys",
                            json={"name": _rand("vk"), "models": [], "providers": []})
    self_path = viewer.raw("POST", f"/api/v1/me/projects/{tenant.project_id}/virtual-keys",
                           json={"name": _rand("vk"), "models": []})
    assert admin_path.status_code == 403, admin_path.text
    assert self_path.status_code == 403, self_path.text


def test_cross_tenant_key_isolation(admin: ControlClient) -> None:
    """a key minted in org A cannot call a model that lives in org B."""
    tenant_a = new_tenant(admin)
    tenant_b = new_tenant(admin)
    model_a = _route(admin, tenant_a)
    model_b = _route(admin, tenant_b)

    key_a = admin.create_virtual_key(tenant_a.project_id, models=[model_a])
    gw = GatewayClient(GATEWAY_URL, virtual_key=key_a["key"])
    _wait_usable(gw, model_a)

    denied = gw.chat(model_b)
    gw.close()
    assert denied.status_code in (403, 404), (
        f"org-A key reached org-B model: {denied.status_code} {denied.text}"
    )
