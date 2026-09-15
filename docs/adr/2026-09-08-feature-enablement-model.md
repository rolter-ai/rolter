# One model for turning a subsystem off

**Status:** Proposed · **Date:** 8 Sep 2026 · **Issues:** [#1073](https://github.com/rolter-ai/rolter/issues/1073), [#535](https://github.com/rolter-ai/rolter/issues/535)
**Relates:** ADR-0030 (served OpenAPI document)

## Context

rolter ships a growing set of optional subsystems, and an operator who wants to
turn one off has to find out, per subsystem, which of several unrelated
mechanisms applies. #1073 describes this as three mechanisms coexisting without
written boundaries. The survey below found **eight**.

### What exists today

| # | Mechanism | Where | Subsystems |
|---|---|---|---|
| 1 | Deployment-wide DB-backed flag | `FeatureFlagsConfig`, `config.rs:171`; applied by `apply_feature_flags()`, `config.rs:201` | `response_cache`, `cache_aware_routing`, `circuit_breaker`, `active_health_checks`, `complexity_routing`, `guardrails` |
| 2 | Per-subsystem `enabled: bool` in config | the subsystem's own config struct | cache, health, breaker, guardrails, guardrail webhook, PII sanitizer, prompt templates, payload capture |
| 3 | `Option<T>` presence | `RouteConfig.semantic`, `config.rs:1868` | semantic cache (per route) |
| 4 | Empty collection | `PluginsConfig.instances`, `GatewayConfig.mcp_servers` | plugins, MCP servers |
| 5 | Numeric sentinel | `RealtimeConfig` (`0` = no cap), `usage_recording.sample_rate = 0` | realtime caps, request-log rows |
| 6 | Per-object `enabled` row | provider, route, guardrail rule, MCP server, SSO provider tables | those objects individually |
| 7 | Compile-time cargo feature | `rolter-control{ldap,postgres}`, `rolter-core{otlp}`, `rolter-store{postgres}`, `rolter{postgres}` | LDAP, postgres store, OTLP export |
| 8 | Implicit infrastructure capability | `unavailable_flags()`, `feature_flags.rs:44`; `ui_events` "inert anyway without `clickhouse_url`" | response cache (needs Redis), cache-aware routing (needs a provider publishing KV/LMCache signals), UI events (needs ClickHouse) |

Mechanisms 1 and 2 are not alternatives — 1 *drives* 2. `apply_feature_flags()`
writes the deployment flag into the subsystem's own `enabled` field, and for two
flags it does something else entirely: `cache_aware_routing` off rewrites every
cache-aware route's strategy to `PowerOfTwo`, and `complexity_routing` off
removes a route param. So "off" already means three different things in one
function: clear a boolean, substitute a fallback behaviour, and delete
configuration.

### The three gaps

**Coverage.** Ten subsystems have no deployment-wide off switch: MCP gateway,
PII sanitizer, guardrail webhooks, semantic cache, budgets, rate limits,
plugins, payload capture, realtime, analytics. Several have a per-object
`enabled` (mechanism 6), which is not the same thing — an operator disabling a
subsystem should not have to edit every row.

**The dashboard does not react.** `ui/src/lib/nav.tsx:167` and
`ui/src/App.tsx:131` reference feature flags exactly once each, and only to
register the FeatureFlags *screen itself*. No other nav entry or route consults
flag state, so disabling a subsystem leaves its nav entry and screen in place,
reachable, backed by an API that now returns nothing useful.

**No shared definition of "off".** Nothing says whether a disabled subsystem
should reject, bypass, 404, or merely hide. Each subsystem answers
independently, which is why mechanisms 3, 4 and 5 exist at all: each was a
locally reasonable choice made without a rule to follow.

## Decision

### A. Two layers, and only two

- **Capability** (compile-time or infrastructure): can this deployment run the
  subsystem at all? Sources are cargo features (7) and infrastructure presence
  (8). Not operator-settable at runtime; surfaced read-only.
- **Enablement** (deployment-wide, DB-backed, hot-reloadable): should this
  deployment run it? One boolean per subsystem in `FeatureFlagsConfig`.

Per-object `enabled` rows (6) stay, and are explicitly **not** part of this
model: they answer "should this provider be used", not "does this deployment
run provider health checks". The ADR's rule is that a subsystem must never be
disabled *only* by emptying its collection or unsetting an `Option` —
mechanisms 3, 4 and 5 are demoted from enablement mechanisms to configuration
details, and each gains a real flag.

A cargo feature can never be a dashboard switch. That is the whole reason the
capability layer is separate, and the reason `unavailable_flags()` already
exists: it is the precedent for "the switch exists, but this deployment cannot
honour it", and it should be extended rather than duplicated.

### B. Off semantics by class

| Class | Subsystems | "Off" means |
|---|---|---|
| Request-path filter | PII sanitizer, guardrails, guardrail webhook, plugins, prompt templates | **Bypass silently.** The request proceeds unmodified. A filter that fails closed when disabled would make the switch an outage. |
| Optimisation | response cache, semantic cache, cache-aware routing, complexity routing, circuit breaker, active health checks | **Bypass, fall back to the simple path.** Already the behaviour `apply_feature_flags()` implements for cache-aware routing. |
| Gateway surface | MCP gateway, realtime | **`501 Not Implemented`** with a body naming the flag. Not 404: the path exists in this build and a 404 is indistinguishable from a typo, which turns an operator's deliberate choice into a debugging session. |
| Enforcement | budgets, rate limits | **Bypass, and say so.** Disabling enforcement is a security-relevant act, so it is audited on write and reported by `rolter check`. |
| Recording | usage recording, payload capture, analytics, UI events | **Drop at ingest.** The hot path must not pay for a disabled recorder. |
| UI-only surface | any screen whose whole subsystem is off | **Hide the nav entry and route.** |

The rule behind the table: **a disabled subsystem never changes an answer the
client already had a right to.** Filters and optimisations bypass; surfaces that
would otherwise lie say `501`.

### C. Propagation

Flags the data plane reads travel the existing path: control plane → `/internal/snapshot`
→ `ArcSwap` in the gateway. Per AGENTS.md any new flag column needs a
`bump_config_version()` trigger migration, or the gateway serves stale config
forever.

The dashboard learns state from the existing `GET /api/v1/feature-flags`,
extended with the capability layer, rather than a second bootstrap payload —
the response already carries `unavailable`, which is the same shape.

### D. Config-file reconciliation

`rolter.toml`'s `[feature_flags]` is the **boot default**; the DB row wins once
the control plane has one, matching how every other config table already
behaves. A store-less gateway uses the file alone. `rolter check` warns when the
file disagrees with the DB, because silently ignoring a file an operator edited
is how a deployment ends up with a flag nobody can explain.

### E. Naming

`<area>.<subsystem>`, grouped for the dashboard by area: routing, safety,
caching, observability, integrations. Flags are permanent operational toggles,
not release flags — so no flag is ever added "temporarily", and removing a
subsystem removes its flag in the same change.

## Consequences

- Every optional subsystem gets one switch in one place, and the dashboard can
  finally hide what is off.
- **Default-on is guaranteed by construction**: every new flag defaults `true`
  via `#[serde(default = "default_true")]`, so an upgrade that adds flags cannot
  change behaviour. This wants a test asserting `FeatureFlagsConfig::default()`
  is all-true, so the guarantee cannot rot as flags are added.
- Ten subsystems need a flag, a migration with a `bump_config_version()`
  trigger, snapshot plumbing, an off-semantic implementation and a dashboard
  switch. That is the follow-up work, sized per subsystem rather than as one
  change.
- Mechanisms 3, 4 and 5 do not disappear; they stop being *the* way to disable
  something. Existing behaviour is unchanged — an empty plugin list is still
  inert — but it is no longer the documented control.
- The `501` choice for gateway surfaces is a wire-visible decision. It is the
  one part of this ADR a client can observe, so it belongs in the OpenAPI
  document (ADR-0030) when implemented.

## Rejected

- **One `enabled` per subsystem config and nothing else.** Simplest, but it is
  mechanism 2 as it stands, and it cannot express the capability layer — an
  operator flipping `response_cache` on with no Redis gets silence rather than
  "this deployment cannot run that".
- **A generic `features: HashMap<String, bool>`.** Removes the compile-time
  guarantee that a flag name is real, and makes the dashboard's grouping and the
  default-on test impossible to enforce.
- **Folding flags into the SPA bootstrap payload.** `/api/v1/feature-flags`
  already exists, already carries `unavailable`, and is already superadmin-gated
  for writes while readable for the nav gating; a second payload is a second
  thing to keep in step.
