"""drive a real Keycloak (compose profile ``idp``) for the SSO suite (#714).

Two URLs for one server, on purpose:

* ``INTERNAL_ISSUER`` — what the *control plane* uses. Discovery, the token
  endpoint and the JWKS are all fetched container-to-container, so they resolve
  ``keycloak`` on the compose network.
* ``HOST_URL`` — what *this process* uses. The harness stands in for a browser,
  and a browser on the host cannot resolve ``keycloak``.

Keycloak mints front-channel URLs from ``KC_HOSTNAME`` (the internal one), so
the authorize URL the control plane hands back points at ``keycloak:8080``. The
harness rewrites the host to reach the same server through the published port;
Keycloak does not care which Host header the request arrived with, and the
issuer both sides verify stays identical either way.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass

import httpx
from tenacity import retry, stop_after_delay, wait_fixed

HOST_URL = os.environ.get("ROLTER_E2E_KEYCLOAK_URL", "http://localhost:8081")
INTERNAL_URL = os.environ.get("ROLTER_E2E_KEYCLOAK_INTERNAL_URL", "http://keycloak:8080")
ADMIN_USER = os.environ.get("ROLTER_E2E_KEYCLOAK_USER", "admin")
ADMIN_PASSWORD = os.environ.get("ROLTER_E2E_KEYCLOAK_PASSWORD", "admin")

REALM = "rolter-e2e"
CLIENT_ID = "rolter-control"
CLIENT_SECRET = "e2e-keycloak-client-secret"


def internal_issuer(realm: str = REALM) -> str:
    return f"{INTERNAL_URL}/realms/{realm}"


def to_host_url(url: str) -> str:
    """Rewrite a Keycloak-minted front-channel URL so this process can follow it."""
    return url.replace(INTERNAL_URL, HOST_URL, 1)


@dataclass
class KeycloakUser:
    username: str
    email: str
    password: str
    # keycloak's VERIFY_PROFILE required action interrupts the login with a
    # "complete your account" form when either name is missing, so a test user
    # always carries both
    first_name: str = "Test"
    last_name: str = "User"


class Keycloak:
    """the slice of Keycloak's admin REST API this suite needs."""

    def __init__(self, base_url: str = HOST_URL):
        self.base_url = base_url
        self._http = httpx.Client(base_url=base_url, timeout=30)

    def close(self) -> None:
        self._http.close()

    @retry(stop=stop_after_delay(180), wait=wait_fixed(2), reraise=True)
    def wait_ready(self) -> None:
        """Keycloak boots slowly and answers 404s while it is still coming up;
        the master realm's openid configuration is the first honest signal."""
        resp = self._http.get("/realms/master/.well-known/openid-configuration")
        resp.raise_for_status()

    def _admin_token(self) -> str:
        resp = self._http.post(
            "/realms/master/protocol/openid-connect/token",
            data={
                "grant_type": "password",
                "client_id": "admin-cli",
                "username": ADMIN_USER,
                "password": ADMIN_PASSWORD,
            },
        )
        resp.raise_for_status()
        return resp.json()["access_token"]

    def _admin(self, method: str, path: str, **kwargs) -> httpx.Response:
        resp = self._http.request(
            method,
            f"/admin/realms{path}",
            headers={"Authorization": f"Bearer {self._admin_token()}"},
            **kwargs,
        )
        if resp.status_code >= 400:
            raise AssertionError(f"keycloak {method} {path} -> {resp.status_code}: {resp.text}")
        return resp

    # -- realm bootstrap ----------------------------------------------------
    def provision_realm(
        self,
        *,
        realm: str,
        redirect_uris: list[str],
        groups: list[str],
        user: KeycloakUser | None = None,
        member_of: list[str] | None = None,
    ) -> None:
        """Create (or reset) a realm with one confidential client and its groups,
        and a user in `member_of` when one is given (more come from `add_user`).
        Idempotent: the realm is deleted first, so a rerun against a kept stack
        starts clean."""
        self._http.request(
            "DELETE",
            f"/admin/realms/{realm}",
            headers={"Authorization": f"Bearer {self._admin_token()}"},
        )
        self._http.post(
            "/admin/realms",
            headers={"Authorization": f"Bearer {self._admin_token()}"},
            json={"realm": realm, "enabled": True},
        ).raise_for_status()

        self._admin(
            "POST",
            f"/{realm}/clients",
            json={
                "clientId": CLIENT_ID,
                "secret": CLIENT_SECRET,
                "publicClient": False,
                "standardFlowEnabled": True,
                "redirectUris": redirect_uris,
                # the group claim rolter reads is not in keycloak's default
                # token; a group-membership mapper puts it there
                "protocolMappers": [
                    {
                        "name": "groups",
                        "protocol": "openid-connect",
                        "protocolMapper": "oidc-group-membership-mapper",
                        "config": {
                            "claim.name": "groups",
                            "id.token.claim": "true",
                            "access.token.claim": "true",
                            "full.path": "true",
                        },
                    }
                ],
            },
        )

        for group in groups:
            self._admin("POST", f"/{realm}/groups", json={"name": group})

        if user is not None:
            self.add_user(realm, user, member_of or [])

    def add_user(self, realm: str, user: KeycloakUser, member_of: list[str]) -> None:
        """Create a user in an existing realm and put them in `member_of`."""
        self._admin(
            "POST",
            f"/{realm}/users",
            json={
                "username": user.username,
                "email": user.email,
                "firstName": user.first_name,
                "lastName": user.last_name,
                "emailVerified": True,
                "enabled": True,
                "credentials": [{"type": "password", "value": user.password, "temporary": False}],
            },
        )
        for group in member_of:
            self.set_group_membership(realm, user.username, group, member=True)

    def _user_id(self, realm: str, username: str) -> str:
        users = self._admin(
            "GET", f"/{realm}/users", params={"username": username, "exact": True}
        ).json()
        assert users, f"no keycloak user {username!r} in realm {realm}"
        return users[0]["id"]

    def _group_id(self, realm: str, name: str) -> str:
        groups = self._admin("GET", f"/{realm}/groups", params={"search": name}).json()
        for group in groups:
            if group["name"] == name:
                return group["id"]
        raise AssertionError(f"no keycloak group {name!r} in realm {realm}")

    def set_group_membership(self, realm: str, username: str, group: str, *, member: bool) -> None:
        """Add or remove a user from a group — the operation an IdP admin
        performs to grant or revoke access."""
        path = (
            f"/{realm}/users/{self._user_id(realm, username)}/groups/{self._group_id(realm, group)}"
        )
        self._admin("PUT" if member else "DELETE", path)


class BrowserSession:
    """the minimum browser: follows redirects, keeps cookies, and can submit
    Keycloak's login form. Not a browser engine — the login page is a plain HTML
    form, and posting it is exactly what a user clicking "Sign in" does.

    Cookies are tracked by hand rather than through httpx's jar. Keycloak sets
    ``KC_RESTART`` with ``SameSite=None``, which obliges it to also set
    ``Secure``; httpx then refuses to send it back over plain http and the login
    fails with "Restart login cookie not found". Browsers do send it, because
    ``localhost`` counts as a secure context — so replaying every cookie is the
    faithful behaviour here, not a workaround. Cookies are kept per host so the
    identity provider's and rolter's never mix.
    """

    def __init__(self) -> None:
        # redirects are followed manually so the harness can assert on each hop
        self._http = httpx.Client(follow_redirects=False, timeout=30)
        self._cookies: dict[str, dict[str, str]] = {}

    def close(self) -> None:
        self._http.close()

    def _remember(self, url: str, resp: httpx.Response) -> None:
        jar = self._cookies.setdefault(httpx.URL(url).host, {})
        for raw in resp.headers.get_list("set-cookie"):
            name, _, rest = raw.partition("=")
            value = rest.split(";", 1)[0]
            # an empty value with an expiry in the past is a deletion
            if value:
                jar[name.strip()] = value
            else:
                jar.pop(name.strip(), None)

    def _headers(self, url: str, extra: dict[str, str] | None = None) -> dict[str, str]:
        headers = dict(extra or {})
        jar = self._cookies.get(httpx.URL(url).host, {})
        if jar:
            headers["cookie"] = "; ".join(f"{k}={v}" for k, v in jar.items())
        return headers

    def get(self, url: str) -> httpx.Response:
        resp = self._http.get(url, headers=self._headers(url))
        self._remember(url, resp)
        return resp

    def log_in(self, authorize_url: str, username: str, password: str) -> httpx.Response:
        """Walk the login form and return Keycloak's redirect back to rolter.

        Keycloak may bounce through extra internal hops after the credentials
        are accepted (a required action, a consent screen). Those are followed
        here so the caller always receives the redirect that finally leaves the
        identity provider — which is the only hop the tests care about.
        """
        page = self._follow_internal(self.get(to_host_url(authorize_url)))
        # this session may already hold a keycloak sso cookie from an earlier
        # login, in which case authorize answers with the code straight away and
        # there is no form to fill — exactly what a returning browser sees
        if page.status_code in (302, 303):
            return page
        assert page.status_code == 200, f"keycloak login page: {page.status_code}"
        action = to_host_url(_form_action(page.text))
        resp = self._http.post(
            action,
            data={"username": username, "password": password, "credentialId": ""},
            headers=self._headers(action, {"content-type": "application/x-www-form-urlencoded"}),
        )
        self._remember(action, resp)
        return self._follow_internal(resp)

    def _follow_internal(self, resp: httpx.Response) -> httpx.Response:
        """Follow redirects that stay inside Keycloak, and stop at the one that
        leaves it — the hop back into rolter is what the tests assert on."""
        for _ in range(5):
            location = resp.headers.get("location", "")
            # keycloak sends absolute urls minted from KC_HOSTNAME, but a
            # relative Location is legal and also stays on the same server
            if location.startswith("/"):
                location = f"{HOST_URL}{location}"
            if not location or not location.startswith((INTERNAL_URL, HOST_URL)):
                return resp
            resp = self.get(to_host_url(location))
        raise AssertionError("keycloak kept redirecting to itself; login never completed")


def _form_action(html: str) -> str:
    match = re.search(r'<form[^>]+action="([^"]+)"', html)
    assert match, "keycloak login page carried no form action"
    return match.group(1).replace("&amp;", "&")
