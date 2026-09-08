# A served OpenAPI document for the control-plane API

**Status:** Accepted · **Date:** 8 Sep 2026 · **Issues:** [#1040](https://github.com/rolter-ai/rolter/issues/1040), [#421](https://github.com/rolter-ai/rolter/issues/421), [#422](https://github.com/rolter-ai/rolter/issues/422), [#851](https://github.com/rolter-ai/rolter/issues/851)
**Relates:** ADR-0027 (end-to-end test harness)

## Context

The gateway describes itself. `crates/rolter-gateway/src/openapi.rs` hand-builds
an OpenAPI 3.1 document, serves it at `GET /openapi.json`, and renders it with a
Scalar bundle embedded in the binary at `GET /docs`. A client can point a
generator at a running gateway and get the OpenAI/Anthropic-compatible surface.

The control plane does not. Its management API — roughly 160 paths and 240
operations across tenancy, users, RBAC, providers, routes, virtual keys,
budgets, guardrails, MCP, SSO, SCIM, alerting and the deployment settings
singletons — exists only as an Axum router assembled in
`crates/rolter-control/src/lib.rs` from about thirty `router()` functions. The
only machine-readable description of it is the dashboard's own `ui/src/lib/api.ts`,
which is a client, not a contract.

Three SDKs are planned: Rust (#421), Python (#422) and JS/TS (#851). Without a
schema each of them hand-writes the same 240 operations, and each of them
re-hand-writes them every time an endpoint is added. Nothing tells an SDK author
that a new endpoint exists, so the three clients drift from the server and from
each other, independently and silently.

There is a second, sharper problem: even *with* a document, a hand-maintained
one goes stale the first time somebody adds an endpoint and forgets. A schema
that is 95% right is worse than none, because a generated SDK reports the
missing 5% as "not part of the API" rather than as "not documented yet".

## Options considered

1. **Accept hand-written SDK surfaces.** Cheapest today. It moves the drift
   problem into three repositories instead of solving it once, and puts the cost
   on whoever adds the 241st endpoint, three times over.
2. **Derive the document from the handlers with `utoipa`.** Attractive because
   the annotation lives next to the code it describes. It means adding a
   proc-macro dependency and its whole derive graph to a crate that currently
   has neither, annotating every handler and every request/response type in one
   change, and accepting that the derived schema describes rolter's *Rust types*
   rather than its wire contract — the two diverge wherever a handler flattens,
   renames or hides a field, which several of them do (`VirtualKey.key_hash`,
   `User.password_hash`, `CreatedVirtualKey`'s flattened plaintext).
3. **Hand-build the document, and make a test enforce completeness.** The
   gateway's existing pattern, plus the missing half: a coverage gate that fails
   the build when a mounted route has no row in the document.

## Decision

Serve it, option 3.

`crates/rolter-control/src/openapi.rs` hand-builds an OpenAPI 3.1 document from
one table of operations and serves it at `GET /openapi.json`, with the same
embedded-Scalar reference at `GET /docs` the gateway already ships. This matches
the gateway's approach rather than introducing a second OpenAPI toolchain into
the workspace, and it keeps the document describing the wire contract rather
than the Rust types behind it.

Completeness is enforced, not hoped for. `every_registered_route_is_documented`
walks every route registration under `crates/rolter-control/src/` — reading the
source, so it cannot drift from what is actually mounted — and fails naming any
`(path, method)` with no row in the table. `every_documented_route_is_registered`
runs the check in reverse, so a deleted endpoint cannot linger in the schema.
This is the same shape of drift guard as
`the_matrix_lists_every_capability_exactly_once` in `rbac_matrix.rs` and
`the_gateway_never_uses_a_poisonable_mutex` in the gateway's `lock_discipline`
test: a list that must not silently fall behind the code gets a test that
compares it to the code.

Body schemas are deliberately uneven, and the gate is what makes that safe.
Endpoints whose handlers already have a stable `serde` shape — tenancy, users
and memberships, providers and provider groups, routes and route targets,
virtual keys, budgets, rate limits and model prices — are typed against 47
entries in `components/schemas`. Surfaces whose payload is genuinely dynamic —
settings singletons, guardrail configs, SCIM envelopes, analytics rows — are
documented as open objects. Every one of them still appears with its path,
method, tag, path parameters and error response, so a generated SDK has the call
even where it does not yet have the struct, and tightening one later is a local
change rather than a discovery problem.

## Consequences

- The three SDK issues can generate from a schema instead of transcribing a
  router, and a fourth client gets the same starting point for free.
- Adding a control-plane endpoint now costs one row in `operations()`. Forgetting
  it is a test failure that names the route, not a silent gap — which is the
  whole point, and the reason a hand-built document is defensible here.
- `rolter-control` gains the `scalar_api_reference` dependency the gateway
  already carries. The bundle is embedded, so `/docs` still works air-gapped.
- The document describes the surface the binary can serve, not the subset a
  given process happens to have mounted: the CRUD routes only mount with a
  postgres pool, but the schema lists them unconditionally. A schema that
  changed shape with deployment configuration would be useless to generate from.
- The coverage gate reads source text rather than introspecting the router,
  because Axum exposes no way to enumerate registered routes. It skips comment
  lines and `#[cfg(test)]` items, and a route registered through anything other
  than a literal path would escape it. That is a known limit, and the crate
  registers every route as a literal today.
- Loose bodies are a debt that is now visible and enumerable rather than
  invisible: every operation carrying an open object is one grep away.
