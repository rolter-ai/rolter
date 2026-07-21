# End-to-end test harness: Python/uv project driving a black-box stack

## Metadata

| Field | Value |
| --- | --- |
| Product | rolter |
| Date | 21 Jul 2026 |
| Status | ACCEPTED |
| Issues | [#613](https://github.com/rolter-ai/rolter/issues/613), [#614](https://github.com/rolter-ai/rolter/issues/614) |

## Context

Per-crate unit tests and the two narrow suites we already have (#414 in-process
integration, #449 compose smoke) never exercise rolter as a whole: a freshly
booted stack, multiple tenants, live RBAC changes, a fleet of upstream engines,
and balancing across clusters. #613 needs a broad governance/routing E2E suite,
and #614 is the harness every other scenario (RBAC matrix, balancing, key
lifecycle, security) builds on.

Two decisions had to be made: what *drives* the stack, and how the driver code
is packaged.

## Options considered

1. **Rust in-process integration test** — spin control+gateway in-process against
   testcontainers. Fast and CI-native, but not a real deployment: it bypasses the
   HTTP edge, the compose network, and the container boundaries where isolation
   and auth actually matter.
2. **Bash script driving docker-compose** — closest to "run fresh rolter", but
   bash grows unmaintainable for a table-driven RBAC matrix with JSON assertions.
3. **Python driver over docker-compose** — real HTTP against a real composed
   stack, with a proper test framework for the matrix.

For packaging the Python driver:

- **PEP 723 inline script metadata** (`uv add --script`) — deps live in a
  single file header, `uv run file.py` auto-installs. Ideal for one-file scripts.
- **A uv project** (`pyproject.toml` + `uv.lock`) — deps and a lockfile for a
  multi-file package.

## Decision

Adopt **option 3**: a Python driver, latest CPython managed by `uv`, hitting the
real HTTP APIs of a stack brought up by docker-compose (postgres, redis,
clickhouse, control, gateway, and N `llm-d-inference-sim` fake-vLLM engines). No
in-process shortcuts — the tests see exactly what an operator/tenant sees.

Package it as a **uv project** under `integration/e2e/`, not inline PEP 723
script metadata. The harness is a pytest package: a shared helper/client library
plus multiple scenario modules (`test_rbac.py`, `test_balancing.py`,
`test_keys.py`, `test_security.py`) sharing fixtures. Inline metadata is
per-single-file and cannot express cross-module fixtures or a lockfile, so it is
the wrong grain here. Inline PEP 723 remains fine for any standalone one-off
helper run by hand.

The harness project is kept **separate from the root maturin `pyproject.toml`**:
that one packages the shipping wheel at `requires-python >=3.9`; the test harness
has no such floor and pins the latest stable CPython via `uv python pin`.

RBAC only enforces when `ROLTER_ADMIN_TOKEN` is set (otherwise the control plane
runs in open/superadmin mode, per #454/ROL-250), so the harness compose sets the
admin token and shares `ROLTER_KEY_PEPPER`/`ROLTER_SESSION_PEPPER` between
control and gateway. Bootstrap uses the admin token as superadmin; per-role
scenarios create local accounts and drive them via session tokens.

The suite is heavy, so it is **gated** (manual dispatch + nightly), never on the
default per-PR gate.

## Consequences

- One harness backs every #613 sub-issue; scenarios are Python modules, not new
  bespoke rigs.
- Tests run against the real container edge, so cross-tenant isolation, auth, and
  balancing are exercised where they actually live.
- Fake engines only (`llm-d-inference-sim`) — offline/air-gapped safe, no secrets,
  no model downloads, deterministic.
- A lockfile makes the harness reproducible; the cost is a second Python project
  in the tree, deliberately isolated from the wheel's packaging metadata.
- Full stack-boot verification requires Docker and building the rolter images, so
  it lives in CI (dispatch/nightly), not the fast PR path.
