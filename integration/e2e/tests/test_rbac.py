"""RBAC permission matrix over live HTTP (#615).

drives every guarded control-plane action through all four principals
(superadmin / admin / member / viewer) and asserts the allowed ranks succeed
while the rest get an exact ``403`` with the OpenAI-style error envelope, then
proves a *live* grant→revoke→retry loop on both planes:

* control: a membership flip takes effect on the very next request (authorize
  reads memberships per-request, so there is no stale-allow window), and
* gateway: revoking a virtual key propagates to the data plane within the
  snapshot poll interval, after which the key ``401``\\s.

the matrix is table-driven over ``(endpoint, required_rank)`` and the actor
ranks, so coverage is exhaustive by construction rather than per-endpoint
copy-paste.
"""

from __future__ import annotations

import time
import uuid
from dataclasses import dataclass

import pytest

from rolter_e2e.bootstrap import SIM_ENGINES, UPSTREAM_MODEL
from rolter_e2e.client import ControlClient, GatewayClient
from rolter_e2e.rbac import Actor, assert_forbidden, make_user_actor, superadmin_actor
from rolter_e2e.stack import GATEWAY_URL


def _rand(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:8]}"


# --------------------------------------------------------------------------- #
# world: two orgs, each with a team + project, plus one actor per scoped role  #
# and a secondary org-admin used to drive the live rule flips.                 #
# --------------------------------------------------------------------------- #
@dataclass
class Org:
    org_id: str
    team_id: str
    team_id_sibling: str
    project_id: str
    admin: Actor
    admin2: Actor  # secondary admin — performs the demotions/revocations
    member: Actor
    viewer: Actor


@dataclass
class World:
    sa: Actor  # superadmin (bootstrap admin token)
    admin_client: ControlClient
    org1: Org
    org2: Org


def _build_org(admin: ControlClient) -> Org:
    org = admin.create_org()
    team = admin.create_team(org["id"])
    team_sib = admin.create_team(org["id"])
    project = admin.create_project(team["id"])
    return Org(
        org_id=org["id"],
        team_id=team["id"],
        team_id_sibling=team_sib["id"],
        project_id=project["id"],
        admin=make_user_actor(admin, org["id"], "admin"),
        admin2=make_user_actor(admin, org["id"], "admin"),
        member=make_user_actor(admin, org["id"], "member"),
        viewer=make_user_actor(admin, org["id"], "viewer"),
    )


@pytest.fixture(scope="module")
def world(admin: ControlClient) -> World:
    return World(
        sa=superadmin_actor(admin),
        admin_client=admin,
        org1=_build_org(admin),
        org2=_build_org(admin),
    )


# --------------------------------------------------------------------------- #
# endpoint catalogue: each entry drives one guarded action with a *fresh*,     #
# valid body (so repeated allowed-role calls never collide) and declares the   #
# minimum role rank the control plane enforces for it (see crates/             #
# rolter-control/src/crud.rs authorize() / require_superadmin call sites).     #
# --------------------------------------------------------------------------- #
# rank: viewer=0, member=1, admin=2, superadmin=3
VIEWER, MEMBER, ADMIN, SUPERADMIN = 0, 1, 2, 3


def _endpoints(sa: ControlClient, org: Org):
    """build the (name, required_rank, call) table bound to ``org``.

    ``call(actor)`` issues the request through the actor's own client with
    ``expect=None`` so the raw status is returned for assertion. setup that must
    exist for a call to reach 2xx (e.g. a target user for a membership grant) is
    minted via ``sa`` (the superadmin client) inside the factory.
    """

    def provider(actor: Actor):
        return actor.client.raw(
            "POST", f"/api/v1/orgs/{org.org_id}/providers",
            json={"name": _rand("prov"), "kind": "openai_compatible",
                  "api_base": SIM_ENGINES[0], "api_key": "sk-fake"},
        )

    def team(actor: Actor):
        return actor.client.raw("POST", f"/api/v1/orgs/{org.org_id}/teams", json={"name": _rand("team")})

    def project(actor: Actor):
        return actor.client.raw(
            "POST", f"/api/v1/teams/{org.team_id}/projects", json={"name": _rand("proj")}
        )

    # a real provider so successful route creates can be given a target — a route
    # with no targets fails snapshot validation ("neither targets nor variants")
    # and 500s the WHOLE gateway snapshot, freezing every polling gateway. the
    # matrix is about the authz decision, not leaving half-built config behind.
    route_provider = sa.create_provider(org.org_id, api_base=SIM_ENGINES[0], name=_rand("route-prov"))

    def route(actor: Actor):
        resp = actor.client.raw(
            "POST", f"/api/v1/projects/{org.project_id}/routes",
            json={"model": _rand("model"), "strategy": "round_robin"},
        )
        if 200 <= resp.status_code < 300:
            # keep the created route valid so it doesn't poison the shared snapshot
            sa.add_route_target(resp.json()["id"], route_provider["id"], upstream_model=UPSTREAM_MODEL)
        return resp

    def virtual_key(actor: Actor):
        return actor.client.raw(
            "POST", f"/api/v1/projects/{org.project_id}/virtual-keys",
            json={"name": _rand("vk"), "models": [], "providers": []},
        )

    def budget(actor: Actor):
        return actor.client.raw(
            "POST", "/api/v1/budgets",
            json={"scope_type": "org", "scope_id": org.org_id, "limit_usd": "100", "period": "30d"},
        )

    def create_user(actor: Actor):
        return actor.client.raw(
            "POST", f"/api/v1/orgs/{org.org_id}/users",
            json={"email": f"{_rand('u')}@e2e.test", "password": uuid.uuid4().hex, "role": "viewer"},
        )

    def create_membership(actor: Actor):
        # grant viewer at org scope to a throwaway account minted for this call,
        # so superadmin and admin can both exercise it without a 409 collision
        target = sa.create_user(org.org_id, email=f"{_rand('m')}@e2e.test", password=uuid.uuid4().hex, role="viewer")
        # the account already carries an org membership from create_user; grant a
        # second, team-scoped one to keep the write unambiguous and conflict-free
        return actor.client.raw(
            "POST", f"/api/v1/orgs/{org.org_id}/memberships",
            json={"user_id": target["user"]["id"], "scope_type": "team",
                  "scope_id": org.team_id, "role": "viewer"},
        )

    def list_providers(actor: Actor):
        return actor.client.raw("GET", f"/api/v1/orgs/{org.org_id}/providers")

    def list_routes(actor: Actor):
        return actor.client.raw("GET", f"/api/v1/projects/{org.project_id}/routes")

    def list_teams(actor: Actor):
        return actor.client.raw("GET", f"/api/v1/orgs/{org.org_id}/teams")

    def model_price(actor: Actor):
        return actor.client.raw(
            "PUT", "/api/v1/model-prices",
            json={"model": _rand("price-model"), "input_per_mtok": "1", "output_per_mtok": "2"},
        )

    def create_org(actor: Actor):
        name = _rand("org")
        return actor.client.raw("POST", "/api/v1/orgs", json={"name": name, "slug": name})

    return [
        # scoped writes → Admin
        ("create_provider", ADMIN, provider),
        ("create_team", ADMIN, team),
        ("create_project", ADMIN, project),
        ("create_route", ADMIN, route),
        ("create_virtual_key", ADMIN, virtual_key),
        ("create_budget", ADMIN, budget),
        ("create_user", ADMIN, create_user),
        ("create_membership", ADMIN, create_membership),
        # scoped reads → Viewer
        ("list_providers", VIEWER, list_providers),
        ("list_routes", VIEWER, list_routes),
        ("list_teams", VIEWER, list_teams),
        # global resources → superadmin
        ("upsert_model_price", SUPERADMIN, model_price),
        ("create_org", SUPERADMIN, create_org),
    ]


def _actors(org: Org, sa: Actor) -> dict[str, Actor]:
    return {"superadmin": sa, "admin": org.admin, "member": org.member, "viewer": org.viewer}


def test_permission_matrix(world: World) -> None:
    """(endpoint × role) → allowed ranks succeed (2xx), the rest get 403.

    this is the exhaustive positive/negative sweep: every guarded endpoint is
    driven by every principal and the outcome is derived purely from the rank
    order, so a regression that loosens or tightens any single gate fails here.
    """
    org = world.org1
    actors = _actors(org, world.sa)
    failures: list[str] = []

    for name, required, call in _endpoints(world.admin_client, org):
        for role, actor in actors.items():
            resp = call(actor)
            allowed = actor.rank >= required
            if allowed:
                if not (200 <= resp.status_code < 300):
                    failures.append(f"{name} as {role}: expected 2xx, got {resp.status_code} {resp.text}")
            else:
                if resp.status_code != 403:
                    failures.append(f"{name} as {role}: expected 403, got {resp.status_code} {resp.text}")
                else:
                    body = resp.json()
                    if not (isinstance(body, dict) and body.get("error", {}).get("message")):
                        failures.append(f"{name} as {role}: 403 without error envelope: {body!r}")

    assert not failures, "permission matrix violations:\n" + "\n".join(failures)


def test_live_grant_revoke_control(world: World) -> None:
    """membership flips take effect on the next request, no stale-allow window.

    a plain org member cannot create a project; a secondary admin grants them
    Admin *on the team*; the very next call succeeds; the admin revokes the
    grant; the next call is forbidden again — all without any propagation wait,
    because control authorizes against live membership rows per request.
    """
    org = world.org1
    subject = make_user_actor(world.admin_client, org.org_id, "member")

    def create_project():
        return subject.client.raw(
            "POST", f"/api/v1/teams/{org.team_id}/projects", json={"name": _rand("proj")}
        )

    # 1. denied: member has no write authority on the team
    assert_forbidden(create_project())

    # 2. secondary admin grants Admin at team scope
    grant = org.admin2.client.raw(
        "POST", f"/api/v1/orgs/{org.org_id}/memberships",
        json={"user_id": subject.user_id, "scope_type": "team", "scope_id": org.team_id, "role": "admin"},
        expect=(200, 201),
    )
    membership_id = grant.json()["id"]

    # 3. allowed immediately — no snapshot/poll wait on the control path
    ok = create_project()
    assert 200 <= ok.status_code < 300, f"post-grant create should succeed, got {ok.status_code} {ok.text}"

    # 4. secondary admin revokes the grant
    org.admin2.client.raw("DELETE", f"/api/v1/memberships/{membership_id}", expect=(200, 204))

    # 5. forbidden again on the next request — the flip is not cached
    assert_forbidden(create_project())


def test_live_gateway_key_revoke(world: World) -> None:
    """revoking a virtual key propagates to the gateway within the poll window.

    mint a working route with **two** keys — a ``target`` and a ``guard`` —
    confirm the target drives a chat, revoke it, then poll the data plane until
    the target ``401``\\s while the guard keeps working, proving the revocation
    reaches the gateway snapshot within a bounded window (snapshot poll is 2s in
    compose; observed ~1s here).

    the guard key is load-bearing, not incidental: the gateway's virtual-key
    auth is a *global* on/off — ``authenticate()`` treats an empty key set as
    "auth disabled" and lets every request through keyless (handlers.rs). so
    revoking the *only* key would open the gateway rather than reject the
    revoked key; keeping a second key in the snapshot holds auth enforced so we
    actually exercise the revoked-key rejection path. (that open-when-empty
    footgun is tracked separately.)
    """
    admin = world.admin_client
    org = world.org1
    model = _rand("revoke-model")

    provider = admin.create_provider(org.org_id, api_base=SIM_ENGINES[0], name=_rand("prov"))
    rt = admin.create_route(org.project_id, model, strategy="round_robin")
    admin.add_route_target(rt["id"], provider["id"], upstream_model=UPSTREAM_MODEL)
    guard = admin.create_virtual_key(org.project_id, models=[model])
    target = admin.create_virtual_key(org.project_id, models=[model])

    gw_target = GatewayClient(GATEWAY_URL, virtual_key=target["key"])
    gw_guard = GatewayClient(GATEWAY_URL, virtual_key=guard["key"])

    # wait for the new route+keys to reach the gateway snapshot (bounded)
    deadline = time.monotonic() + 30
    while True:
        resp = gw_target.chat(model)
        if resp.status_code == 200:
            break
        assert time.monotonic() < deadline, f"key never became usable: {resp.status_code} {resp.text}"
        time.sleep(1)

    # revoke the target and poll until the gateway rejects it
    admin.revoke_virtual_key(target["id"])
    revoked_at = time.monotonic()
    deadline = revoked_at + 30
    while True:
        resp = gw_target.chat(model)
        if resp.status_code in (401, 403):
            break
        assert time.monotonic() < deadline, (
            f"revoked key still accepted after {time.monotonic() - revoked_at:.1f}s: {resp.status_code}"
        )
        time.sleep(1)

    # the guard key must keep working: only the revoked key lost access
    guard_resp = gw_guard.chat(model)
    assert guard_resp.status_code == 200, f"guard key should still work, got {guard_resp.status_code} {guard_resp.text}"

    gw_target.close()
    gw_guard.close()


def test_scope_escalation_negatives(world: World) -> None:
    """a team-scoped Admin cannot reach up to the org or sideways to a sibling.

    grants Admin on exactly one team, then asserts the classic escalation
    attempts all ``403``: mutating the parent org, creating a sibling team, and
    writing into a sibling team — while the in-scope write still succeeds.
    """
    org = world.org1
    # viewer at org scope, then Admin on team_id only (no org-level write power)
    subject = make_user_actor(world.admin_client, org.org_id, "viewer")
    world.admin_client.create_membership(org.org_id, subject.user_id, "team", org.team_id, "admin")

    # up: cannot create a provider on the parent org (Admin@org required)
    assert_forbidden(subject.client.raw(
        "POST", f"/api/v1/orgs/{org.org_id}/providers",
        json={"name": _rand("prov"), "kind": "openai_compatible", "api_base": SIM_ENGINES[0], "api_key": "sk-fake"},
    ))
    # up: cannot create a sibling team (Admin@org required)
    assert_forbidden(subject.client.raw(
        "POST", f"/api/v1/orgs/{org.org_id}/teams", json={"name": _rand("team")}
    ))
    # sideways: cannot write into the sibling team
    assert_forbidden(subject.client.raw(
        "POST", f"/api/v1/teams/{org.team_id_sibling}/projects", json={"name": _rand("proj")}
    ))
    # in scope: creating a project under the granted team still works
    ok = subject.client.raw(
        "POST", f"/api/v1/teams/{org.team_id}/projects", json={"name": _rand("proj")}
    )
    assert 200 <= ok.status_code < 300, f"in-scope create should succeed, got {ok.status_code} {ok.text}"


def test_cross_org_isolation(world: World) -> None:
    """an Admin in one org has no authority in another org."""
    org1, org2 = world.org1, world.org2
    # org1's admin tries to create a provider in org2 → forbidden
    assert_forbidden(org1.admin.client.raw(
        "POST", f"/api/v1/orgs/{org2.org_id}/providers",
        json={"name": _rand("prov"), "kind": "openai_compatible", "api_base": SIM_ENGINES[0], "api_key": "sk-fake"},
    ))
    # and cannot list org2's providers either (read is Viewer@org2, which they lack)
    assert_forbidden(org1.admin.client.raw("GET", f"/api/v1/orgs/{org2.org_id}/providers"))
