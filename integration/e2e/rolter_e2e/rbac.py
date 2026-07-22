"""helpers for the RBAC permission-matrix scenario (#615).

models the four principals under test — global ``superadmin`` plus the three
scoped roles ``admin`` / ``member`` / ``viewer`` — as :class:`Actor` handles,
each carrying a control client already authenticated *as that principal*. The
matrix tests then drive the same endpoint through every actor and assert the
allowed ranks succeed while the rest get a ``403`` with the OpenAI-style error
envelope.
"""

from __future__ import annotations

import uuid
from dataclasses import dataclass

import httpx

from .client import ControlClient

# total order over the roles the control plane enforces (rbac.rs role_rank plus
# the global superadmin bit, which outranks every scoped grant).
ROLE_RANK = {"viewer": 0, "member": 1, "admin": 2, "superadmin": 3}


def rank(role: str) -> int:
    return ROLE_RANK[role]


@dataclass
class Actor:
    """a principal plus a control client authenticated as it."""

    role: str
    client: ControlClient
    #: the local account's user id (``None`` for the token-based superadmin)
    user_id: str | None = None
    #: the org-scoped membership id, so a test can demote/revoke it live
    membership_id: str | None = None
    email: str | None = None
    password: str | None = None

    @property
    def rank(self) -> int:
        return ROLE_RANK[self.role]


def make_user_actor(admin: ControlClient, org_id: str, role: str) -> Actor:
    """create a local account with ``role`` at ``org_id`` and log it in.

    ``role`` is one of ``admin`` / ``member`` / ``viewer``; the returned actor
    carries a client bound to that user's freshly issued session token.
    """
    email = f"{role}-{uuid.uuid4().hex[:8]}@e2e.test"
    password = uuid.uuid4().hex
    created = admin.create_user(org_id, email=email, password=password, role=role)
    session = admin.login(email, password)
    return Actor(
        role=role,
        client=session,
        user_id=created["user"]["id"],
        membership_id=created["membership"]["id"],
        email=email,
        password=password,
    )


def superadmin_actor(admin: ControlClient) -> Actor:
    """wrap the bootstrap admin-token client as the superadmin principal."""
    return Actor(role="superadmin", client=admin)


def assert_forbidden(resp: httpx.Response) -> None:
    """assert an exact ``403`` carrying the control plane's error envelope.

    the acceptance criteria require the negative path to prove the precise
    status *and* the OpenAI-style ``{"error": {"message": ...}}`` shape, not
    merely "not 2xx".
    """
    assert resp.status_code == 403, f"expected 403, got {resp.status_code}: {resp.text}"
    body = resp.json()
    assert isinstance(body, dict) and "error" in body, f"missing error envelope: {body!r}"
    assert "message" in body["error"], f"error object has no message: {body!r}"
