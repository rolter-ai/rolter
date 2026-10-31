"""Single sign-on and SCIM against a real identity provider (#714).

The unit suite proves rolter's own logic against a stub IdP. This proves
interoperability: a genuine Keycloak discovery document, a real JWKS, a real
login form, and Keycloak's `/`-prefixed realm groups — the exact shape the
group normalizer exists for.

Keycloak is not a SCIM client (it has no built-in SCIM push), so the SCIM half
replays the request shapes Okta and Entra actually send, against the same live
control plane. Both halves share one stack, which is the point: provisioning
and login have to agree about who a user is.

Requires the ``idp`` compose profile. Run with::

    cd integration/e2e && uv run pytest tests/test_sso.py --idp
"""

from __future__ import annotations

import uuid

import httpx
import pytest

from rolter_e2e.client import ControlClient
from rolter_e2e.keycloak import (
    CLIENT_ID,
    CLIENT_SECRET,
    REALM,
    BrowserSession,
    Keycloak,
    KeycloakUser,
    internal_issuer,
)
from rolter_e2e.stack import CONTROL_URL

pytestmark = pytest.mark.idp

GROUP_PLATFORM = "platform"
GROUP_UNMAPPED = "contractors"


def _rand(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:8]}"


def _new_user(keycloak: Keycloak, *, member_of: list[str]) -> KeycloakUser:
    """A user no other test has signed in as.

    Rolter refuses to adopt an account that already holds roles in another org
    (`account_outside_org`), so a user shared across tests, each signing in
    through its own org's provider, would be refused from the second test on.
    """
    name = _rand("user")
    user = KeycloakUser(username=name, email=f"{name}@example.com", password=f"{name}-password")
    keycloak.add_user(REALM, user, member_of)
    return user


@pytest.fixture(scope="module")
def keycloak(stack) -> Keycloak:
    kc = Keycloak()
    kc.wait_ready()
    kc.provision_realm(
        realm=REALM,
        redirect_uris=[f"{CONTROL_URL}/*"],
        groups=[GROUP_PLATFORM, GROUP_UNMAPPED],
    )
    try:
        yield kc
    finally:
        kc.close()


def _start_login(slug: str) -> tuple[str, httpx.Response]:
    resp = httpx.get(f"{CONTROL_URL}/auth/sso/{slug}/start", follow_redirects=False, timeout=30)
    assert resp.status_code == 303, resp.text
    return resp.headers["location"], resp


def _finish_login(
    browser: BrowserSession, authorize_url: str, user: KeycloakUser
) -> httpx.Response:
    """Log in at Keycloak and follow its redirect back into rolter."""
    redirect = browser.log_in(authorize_url, user.username, user.password)
    assert redirect.status_code in (302, 303), redirect.text
    callback = redirect.headers["location"]
    assert callback.startswith(CONTROL_URL), f"keycloak redirected off-site: {callback}"
    return browser.get(callback)


def test_sso_login_grants_the_role_the_idp_group_maps_to(
    admin: ControlClient, keycloak: Keycloak
) -> None:
    org = admin.create_org()
    team = admin.create_team(org["id"])
    slug = _rand("kc")
    user = _new_user(keycloak, member_of=[GROUP_PLATFORM])

    provider = admin._http.json(
        "POST",
        f"/api/v1/orgs/{org['id']}/sso-providers",
        json={
            "name": "Keycloak",
            "slug": slug,
            "issuer": internal_issuer(),
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
        },
    )
    # the secret went in sealed and does not come back out
    assert CLIENT_SECRET not in str(provider)

    admin._http.json(
        "POST",
        f"/api/v1/sso-providers/{provider['id']}/group-mappings",
        json={"group_name": GROUP_PLATFORM, "role": "admin", "team_id": team["id"]},
    )

    authorize_url, _ = _start_login(slug)
    assert authorize_url.startswith(f"{internal_issuer()}/protocol/openid-connect/auth")
    assert "code_challenge_method=S256" in authorize_url

    browser = BrowserSession()
    try:
        landed = _finish_login(browser, authorize_url, user)
    finally:
        browser.close()
    assert landed.status_code == 200, landed.text
    body = landed.json()
    assert body["user"]["email"] == user.email
    assert body["granted_roles"] == ["admin"]

    # the session is a real one, and the mapped role really works: keycloak's
    # `/platform` group became team-scoped admin in rolter
    session = ControlClient(CONTROL_URL, token=body["token"])
    try:
        assert session.me()["user"]["email"] == user.email
        project = session.create_project(team["id"], name=_rand("from-sso"))
        assert project["team_id"] == team["id"]
        # ...and nothing above that scope
        denied = session._http.request(
            "POST", f"/api/v1/orgs/{org['id']}/teams", json={"name": _rand("nope")}
        )
        assert denied.status_code == 403, denied.text
    finally:
        session.close()


def test_leaving_the_idp_group_revokes_the_role_it_granted(
    admin: ControlClient, keycloak: Keycloak
) -> None:
    org = admin.create_org()
    team = admin.create_team(org["id"])
    slug = _rand("kc")
    user = _new_user(keycloak, member_of=[GROUP_PLATFORM])
    provider = admin._http.json(
        "POST",
        f"/api/v1/orgs/{org['id']}/sso-providers",
        json={
            "name": "Keycloak",
            "slug": slug,
            "issuer": internal_issuer(),
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
        },
    )
    admin._http.json(
        "POST",
        f"/api/v1/sso-providers/{provider['id']}/group-mappings",
        json={"group_name": GROUP_PLATFORM, "role": "admin", "team_id": team["id"]},
    )

    browser = BrowserSession()
    try:
        authorize_url, _ = _start_login(slug)
        first = _finish_login(browser, authorize_url, user)
        assert first.status_code == 200
        user_id = first.json()["user"]["id"]

        # an operator grant made by hand must survive what follows
        admin._http.json(
            "POST",
            f"/api/v1/orgs/{org['id']}/memberships",
            json={
                "user_id": user_id,
                "scope_type": "org",
                "scope_id": org["id"],
                "role": "viewer",
            },
        )

        # the IdP admin removes them from the group; the next login is refused
        # because nothing maps any more, and the role that group granted is gone
        keycloak.set_group_membership(REALM, user.username, GROUP_PLATFORM, member=False)
        authorize_url, _ = _start_login(slug)
        refused = _finish_login(browser, authorize_url, user)
        assert refused.status_code == 403, refused.text
    finally:
        browser.close()

    roles = {
        (m["role"], m["source"])
        for m in admin._http.json("GET", f"/api/v1/orgs/{org['id']}/memberships")
        if m["user_id"] == user_id
    }
    assert roles == {("viewer", "manual")}, f"sso reconciliation touched a manual grant: {roles}"


def test_the_authorize_url_is_rejected_when_the_state_is_replayed(
    admin: ControlClient, keycloak: Keycloak
) -> None:
    org = admin.create_org()
    slug = _rand("kc")
    user = _new_user(keycloak, member_of=[])
    provider = admin._http.json(
        "POST",
        f"/api/v1/orgs/{org['id']}/sso-providers",
        json={
            "name": "Keycloak",
            "slug": slug,
            "issuer": internal_issuer(),
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
            "default_role": "member",
        },
    )
    assert provider["enabled"] is True

    browser = BrowserSession()
    try:
        authorize_url, _ = _start_login(slug)
        redirect = browser.log_in(authorize_url, user.username, user.password)
        callback = redirect.headers["location"]
        first = browser.get(callback)
        assert first.status_code == 200, first.text
        # the very same code+state pair, replayed
        replayed = browser.get(callback)
        assert replayed.status_code == 400, replayed.text
    finally:
        browser.close()


def test_scim_provisions_and_deactivates_against_the_live_stack(admin: ControlClient) -> None:
    """SCIM with the payloads Okta and Entra actually send. Independent of
    Keycloak, but pointed at the same control plane: an account provisioned by
    SCIM and one adopted by SSO must be the same account."""
    org = admin.create_org()
    token = admin._http.json("POST", f"/api/v1/orgs/{org['id']}/scim-tokens", json={"name": "okta"})
    scim = httpx.Client(
        base_url=CONTROL_URL,
        # the bearer value is returned once, at creation, and never again
        headers={"Authorization": f"Bearer {token['secret']}"},
        timeout=30,
    )
    email = f"{_rand('scim')}@example.com"
    try:
        created = scim.post(
            "/scim/v2/Users",
            json={
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
                "userName": email,
                "name": {"givenName": "Grace", "familyName": "Hopper"},
                "emails": [{"primary": True, "value": email, "type": "work"}],
                "active": True,
            },
        )
        assert created.status_code == 201, created.text
        scim_id = created.json()["id"]

        # a repeat create is a uniqueness conflict, not a duplicate account
        again = scim.post(
            "/scim/v2/Users",
            json={"schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"], "userName": email},
        )
        assert again.status_code == 409, again.text
        assert again.json()["scimType"] == "uniqueness"

        listed = scim.get("/scim/v2/Users", params={"filter": f'userName eq "{email}"'})
        assert listed.status_code == 200
        assert listed.json()["totalResults"] == 1

        # entra deactivates with a PATCH; okta with a PUT carrying active=false
        patched = scim.patch(
            f"/scim/v2/Users/{scim_id}",
            json={
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "active", "value": False}],
            },
        )
        assert patched.status_code == 200, patched.text
        assert patched.json()["active"] is False

        # a deactivated account cannot log in, whichever door it tries
        login = httpx.post(
            f"{CONTROL_URL}/api/v1/auth/login",
            json={"email": email, "password": "does-not-matter"},
            timeout=30,
        )
        assert login.status_code in (401, 403)
    finally:
        scim.close()
