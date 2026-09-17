# rolter e2e harness

Black-box end-to-end tests: a freshly booted docker-compose stack
(postgres/redis/clickhouse + control + gateway + a fleet of fake-vLLM engines)
driven through the **real HTTP APIs**. No in-process shortcuts — the tests see
exactly what an operator and a tenant see.

Foundation for the #613 epic. Scenario suites (RBAC matrix #615, balancing #616,
key lifecycle #617, security #618) build on the fixtures and client here.

See the decision record: [`docs/dev-docs/adr/2026-07-21-e2e-test-harness.md`](../../docs/adr/2026-07-21-e2e-test-harness.md).

## Layout

| Path | Purpose |
| --- | --- |
| `docker-compose.e2e.yml` | full stack; gateway runs in DB-snapshot mode, RBAC on (`ROLTER_ADMIN_TOKEN` set) |
| `rolter_e2e/client.py` | `ControlClient` / `GatewayClient` — typed wrappers over every endpoint used |
| `rolter_e2e/stack.py` | `docker compose up/down/--wait` + health polling |
| `rolter_e2e/bootstrap.py` | tenant + fake-vLLM fleet bootstrap (`register_fleet`) |
| `conftest.py` | session fixtures: `stack`, `admin`, `gateway` |
| `tests/test_smoke.py` | the harness acceptance gate (#614) |
| `rolter_e2e/keycloak.py` | admin-API driver for the real identity provider (#714) |
| `tests/test_sso.py` | OIDC login + SCIM against Keycloak; needs `--idp` |

## Run

Requires Docker + [`uv`](https://docs.astral.sh/uv/). Heavy — **not** on the
per-PR gate; run on demand or nightly.

```bash
cd integration/e2e
uv sync                 # resolve + lock deps into .venv
uv run pytest           # boots the stack, runs, tears down
```

Or from the repo root: `just e2e`.

### Single sign-on against a real IdP

The SSO suite needs Keycloak, which roughly doubles stack boot time, so it is
opt-in behind both a pytest flag and a compose profile:

```bash
cd integration/e2e
uv run pytest tests/test_sso.py --idp
```

`--idp` starts the `idp` compose profile and unskips the `idp`-marked tests.
Keycloak runs in dev mode with throwaway credentials and no persistence.

Two URLs point at that one server on purpose: the control plane resolves
`keycloak:8080` on the compose network (discovery, token, JWKS), while the
harness stands in for a browser and reaches the published `localhost:8081`.
Keycloak mints front-channel URLs from the internal name, so the harness
rewrites the host on its own requests — see `rolter_e2e/keycloak.py`.

### Useful env toggles

| Var | Effect |
| --- | --- |
| `ROLTER_E2E_NO_MANAGE=1` | don't manage compose; test an already-running stack |
| `ROLTER_E2E_NO_BUILD=1` | `up` without `--build` (reuse existing images) |
| `ROLTER_E2E_KEEP=1` | leave the stack up after the run (for debugging) |
| `ROLTER_E2E_CONTROL_URL` / `ROLTER_E2E_GATEWAY_URL` | point at non-default hosts |
| `ROLTER_E2E_KEYCLOAK_URL` / `ROLTER_E2E_KEYCLOAK_INTERNAL_URL` | the IdP's host-published and compose-internal URLs |

## Notes

- Fake engines only (`ghcr.io/llm-d/llm-d-inference-sim`) — offline/air-gapped
  safe, deterministic, no model downloads, no secrets.
- Every `up` starts from a clean database (the compose file uses no named
  postgres volume), so runs don't leak state into each other.
- RBAC enforces only because `ROLTER_ADMIN_TOKEN` is set; bootstrap acts as
  superadmin via that token, per-role scenarios drive session tokens from
  `ControlClient.login`.
