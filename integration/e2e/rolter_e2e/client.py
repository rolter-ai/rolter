"""thin, typed HTTP clients for the control plane and the gateway.

every method maps to a real endpoint; nothing reaches into the process. the
control client authenticates either with the bootstrap admin token (superadmin)
or a per-user session bearer token, so the same helpers drive both the operator
and the tenant side of a scenario.
"""

from __future__ import annotations

import uuid
from dataclasses import dataclass
from typing import Any

import httpx


class ApiError(Exception):
    """a non-2xx control/gateway response, carrying status + parsed body."""

    def __init__(self, method: str, url: str, status: int, body: Any):
        self.method = method
        self.url = url
        self.status = status
        self.body = body
        super().__init__(f"{method} {url} -> {status}: {body!r}")


def _rand(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:8]}"


class _Http:
    def __init__(self, base_url: str, token: str | None, timeout: float = 30.0):
        self._base = base_url.rstrip("/")
        self._client = httpx.Client(base_url=self._base, timeout=timeout)
        self._token = token

    def _headers(self, extra: dict[str, str] | None = None) -> dict[str, str]:
        h: dict[str, str] = {}
        if self._token:
            h["Authorization"] = f"Bearer {self._token}"
        if extra:
            h.update(extra)
        return h

    def request(
        self,
        method: str,
        path: str,
        *,
        json: Any = None,
        headers: dict[str, str] | None = None,
        expect: int | tuple[int, ...] | None = None,
    ) -> httpx.Response:
        resp = self._client.request(method, path, json=json, headers=self._headers(headers))
        if expect is not None:
            ok = (resp.status_code == expect) if isinstance(expect, int) else (resp.status_code in expect)
            if not ok:
                raise ApiError(method, path, resp.status_code, _safe_body(resp))
        return resp

    def json(self, method: str, path: str, *, json: Any = None, expect: int | tuple[int, ...] = 200) -> Any:
        return _safe_body(self.request(method, path, json=json, expect=expect))

    def close(self) -> None:
        self._client.close()


def _safe_body(resp: httpx.Response) -> Any:
    try:
        return resp.json()
    except Exception:
        return resp.text


class ControlClient:
    """drives the rolter-control CRUD + auth API.

    construct with the bootstrap admin token for superadmin, or with a session
    token (from :meth:`login`) to act as a specific local account.
    """

    def __init__(self, base_url: str, token: str | None = None):
        self.base_url = base_url
        self.token = token
        self._http = _Http(base_url, token)

    # -- auth ---------------------------------------------------------------
    def login(self, email: str, password: str) -> "ControlClient":
        """log in and return a *new* client bound to the issued session token."""
        body = self._http.json("POST", "/api/v1/auth/login", json={"email": email, "password": password})
        return ControlClient(self.base_url, token=body["token"])

    def me(self) -> Any:
        return self._http.json("GET", "/api/v1/auth/me")

    # -- tenancy ------------------------------------------------------------
    def create_org(self, name: str | None = None, slug: str | None = None) -> Any:
        name = name or _rand("org")
        slug = slug or name.lower().replace(" ", "-")
        return self._http.json("POST", "/api/v1/orgs", json={"name": name, "slug": slug}, expect=(200, 201))

    def create_team(self, org_id: str, name: str | None = None) -> Any:
        return self._http.json(
            "POST", f"/api/v1/orgs/{org_id}/teams", json={"name": name or _rand("team")}, expect=(200, 201)
        )

    def create_project(self, team_id: str, name: str | None = None) -> Any:
        return self._http.json(
            "POST", f"/api/v1/teams/{team_id}/projects", json={"name": name or _rand("proj")}, expect=(200, 201)
        )

    # -- users & rbac -------------------------------------------------------
    def create_user(self, org_id: str, email: str, password: str, role: str = "member") -> Any:
        """invite/create a local account and grant it ``role`` at ``org_id``."""
        return self._http.json(
            "POST",
            f"/api/v1/orgs/{org_id}/users",
            json={"email": email, "password": password, "role": role},
            expect=(200, 201),
        )

    def create_membership(self, org_id: str, user_id: str, scope_type: str, scope_id: str, role: str) -> Any:
        return self._http.json(
            "POST",
            f"/api/v1/orgs/{org_id}/memberships",
            json={"user_id": user_id, "scope_type": scope_type, "scope_id": scope_id, "role": role},
            expect=(200, 201),
        )

    def update_user(self, user_id: str, **fields: Any) -> Any:
        return self._http.json("PUT", f"/api/v1/users/{user_id}", json=fields)

    def delete_membership(self, membership_id: str, *, expect: int | tuple[int, ...] = (200, 204)) -> httpx.Response:
        return self._http.request("DELETE", f"/api/v1/memberships/{membership_id}", expect=expect)

    # -- providers, groups, routes -----------------------------------------
    def create_provider(self, org_id: str, api_base: str, *, kind: str = "openai_compatible", name: str | None = None,
                         api_key: str | None = "sk-fake") -> Any:
        payload: dict[str, Any] = {"name": name or _rand("prov"), "kind": kind, "api_base": api_base}
        if api_key is not None:
            payload["api_key"] = api_key
        return self._http.json("POST", f"/api/v1/orgs/{org_id}/providers", json=payload, expect=(200, 201))

    def create_provider_group(self, org_id: str, members: list[dict[str, Any]], *, strategy: str = "round_robin",
                              name: str | None = None) -> Any:
        return self._http.json(
            "POST",
            f"/api/v1/orgs/{org_id}/provider-groups",
            json={"name": name or _rand("grp"), "strategy": strategy, "members": members},
            expect=(200, 201),
        )

    def create_route(self, project_id: str, model: str, *, strategy: str = "round_robin") -> Any:
        return self._http.json(
            "POST", f"/api/v1/projects/{project_id}/routes", json={"model": model, "strategy": strategy},
            expect=(200, 201),
        )

    def add_route_target(self, route_id: str, provider_id: str, *, upstream_model: str | None = None,
                         weight: int = 1) -> Any:
        payload: dict[str, Any] = {"provider_id": provider_id, "weight": weight}
        if upstream_model is not None:
            payload["upstream_model"] = upstream_model
        return self._http.json("POST", f"/api/v1/routes/{route_id}/targets", json=payload, expect=(200, 201))

    def set_route_strategy(self, route_id: str, strategy: str) -> Any:
        return self._http.json("PUT", f"/api/v1/routes/{route_id}/advanced", json={"strategy": strategy})

    def delete_route(self, route_id: str, *, expect: int | tuple[int, ...] = (200, 204)) -> httpx.Response:
        return self._http.request("DELETE", f"/api/v1/routes/{route_id}", expect=expect)

    # -- virtual keys -------------------------------------------------------
    def create_virtual_key(self, project_id: str, *, models: list[str] | None = None,
                           providers: list[str] | None = None, name: str | None = None) -> Any:
        """returns the created row plus the one-time plaintext ``key``."""
        return self._http.json(
            "POST",
            f"/api/v1/projects/{project_id}/virtual-keys",
            json={"name": name or _rand("vk"), "models": models or [], "providers": providers or []},
            expect=(200, 201),
        )

    def revoke_virtual_key(self, key_id: str, *, expect: int | tuple[int, ...] = (200, 204)) -> httpx.Response:
        return self._http.request("DELETE", f"/api/v1/virtual-keys/{key_id}", expect=expect)

    # -- raw escape hatch (negative tests assert arbitrary status) ----------
    def raw(self, method: str, path: str, *, json: Any = None,
            expect: int | tuple[int, ...] | None = None) -> httpx.Response:
        return self._http.request(method, path, json=json, expect=expect)

    def snapshot(self) -> Any:
        return self._http.json("GET", "/internal/snapshot")

    def close(self) -> None:
        self._http.close()


class GatewayClient:
    """drives the data-plane OpenAI/Anthropic surface with a virtual key."""

    def __init__(self, base_url: str, virtual_key: str | None = None):
        self.base_url = base_url
        self._http = _Http(base_url, virtual_key)

    def chat(self, model: str, prompt: str = "ping", *,
             expect: int | tuple[int, ...] | None = None) -> httpx.Response:
        return self._http.request(
            "POST",
            "/v1/chat/completions",
            json={"model": model, "messages": [{"role": "user", "content": prompt}]},
            expect=expect,
        )

    def messages(self, model: str, prompt: str = "ping", *,
                 expect: int | tuple[int, ...] | None = None) -> httpx.Response:
        return self._http.request(
            "POST",
            "/v1/messages",
            json={"model": model, "max_tokens": 16, "messages": [{"role": "user", "content": prompt}]},
            expect=expect,
        )

    def models(self) -> Any:
        return self._http.json("GET", "/v1/models")

    def close(self) -> None:
        self._http.close()


@dataclass
class Tenant:
    """a bootstrapped org with a team, project, and admin handle — the unit most
    scenarios start from."""

    org_id: str
    team_id: str
    project_id: str
