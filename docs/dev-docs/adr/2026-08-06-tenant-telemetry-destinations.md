# Client control over telemetry: a kill switch now, collector-routed tenant destinations later

**Status:** Accepted · **Date:** 6 Aug 2026 · **Issues:** [#812](https://github.com/rolter-ai/rolter/issues/812), [#805](https://github.com/rolter-ai/rolter/issues/805), [#809](https://github.com/rolter-ai/rolter/issues/809)
**Relates:** ADR-0019 (egress proxy pools), ADR-0025 (events as logs)

## Context

Telemetry is currently an *operator* decision baked into the deployment:
`OTEL_EXPORTER_OTLP_ENDPOINT` for the backend exporter and
`ROLTER_UI_OTEL_ENDPOINT` for the dashboard. A client of the gateway has no say
in either — they cannot turn tracing off for their own traffic, and they cannot
have their spans delivered to a backend they own.

[#812](https://github.com/rolter-ai/rolter/issues/812) raises two things of very
different size, and filed them together so the relationship is on the record.
This ADR splits them.

**The switch.** "Off" is implicit today: you achieve it by leaving an endpoint
unset. That works but is not discoverable, does not survive somebody setting the
endpoint for one signal, and gives an operator nothing to point at in a security
review.

**Per-tenant destinations.** rolter is multi-tenant (org / team / project,
virtual keys, RBAC) while telemetry is single-destination, which does not match.
A team running rolter as shared infrastructure wants its spans in *its* backend;
some tenants want no export at all for data-residency reasons; a customer
debugging their own integration should not have to ask the operator to fetch
traces. This half raises questions — scope, fan-out, egress safety, cardinality —
that are design work rather than implementation.

## Decision

**Ship the kill switch now as an environment variable. Do not build per-tenant
fan-out in rolter; route it in the collector when it is built.**

### 1. `ROLTER_TELEMETRY_ENABLED`

One switch covering traces, metrics, the dashboard's browser tracing, and logs
once [#809](https://github.com/rolter-ai/rolter/issues/809) lands.

- **It can only subtract.** When false, no exporter is built regardless of which
  `OTEL_*` endpoints are set. It is never a second way to turn export *on*.
- **Unset means enabled**, so no existing deployment changes behaviour. That is
  not a weaker default than it sounds: with no endpoint configured nothing is
  exported anyway, so the effective default remains "exports nothing".
- **An unrecognized value leaves export on.** A typo must not silently blind a
  deployment; only the explicit falsy spellings (`0`, `false`, `no`, `off`)
  disable it.
- **Environment-only, with no config-file key**, despite #812 asking for "the
  matching config key". `telemetry::init` installs the subscriber in `main`
  before any config file is read — the gateway has to be able to log a config
  parse failure — so a config-file switch could not gate trace export at all.
  Honouring one only for the signals initialized later would mean the same key
  meant different things depending on which signal you asked about, which is
  worse than not having it. The `OTEL_*` contract this composes with is
  environment-based for the same reason.

### 2. Per-tenant destinations belong in the collector

When the second half is built, it is configuration of an OpenTelemetry Collector
that sits in front of the tenants' backends, not a fan-out inside rolter.

- **Scope: org.** It is the easiest to administer and the coarsest unit anyone
  actually asks about ("my team's spans in my team's Honeycomb"). Virtual key is
  finer and matches how traffic is attributed, but a destination per key is a
  cardinality problem with no matching demand. rolter's job is to *stamp* the
  attribute; the routing key can be refined later without changing where fan-out
  happens.
- **Fan-out: the collector's routing processor.** Multiple SDK exporters
  in-process does not scale with tenant count — each is a connection, a queue and
  a retry buffer, all inside the request-serving process. rolter emits one OTLP
  stream to one collector with a resource attribute naming the tenant, and the
  collector's `routing` processor sends each tenant's data onward. This keeps
  exactly one egress path in the gateway and makes per-tenant destinations an
  operator's configuration change rather than a rolter deploy.
- **Egress safety follows from that.** A tenant-supplied endpoint is an arbitrary
  URL, and the deciding argument against in-process fan-out is that it would make
  the gateway POST to one. Terminating tenant endpoints in the collector keeps
  that SSRF surface out of the data plane entirely. If a tenant-supplied endpoint
  is ever accepted through the API, it must go through the same `EgressPolicy`
  that provider `api_base` values do (ADR-0019), and its credentials through the
  encrypted-secret path — never plaintext config.
- **Opt-out is the switch, one level down.** A tenant wanting no export at all is
  a routing rule that drops, not a second mechanism.

## Consequences

- An operator gets a single, documentable, reviewable "no telemetry leaves this
  deployment" today, without waiting on the design work.
- The gateway keeps exactly one exporter and one egress path no matter how many
  tenants exist. Cardinality and buffer growth become the collector's problem,
  where they are a sizing question rather than a data-plane risk.
- The cost is that per-tenant destinations require running a collector. That is
  already the recommended topology (`docs/architecture/observability.md`), so it
  is not a new dependency for anyone following it — but a deployment exporting
  straight to a vendor endpoint would have to add one.
- Nothing here is blocked on #809, but when OTLP log export lands it inherits
  both decisions automatically: the switch covers it, and its records carry the
  same resource attributes the routing processor keys on. That is why the switch
  is worded per-deployment rather than per-signal.
- `ROLTER_TELEMETRY_ENABLED` is now part of the public configuration surface and
  cannot quietly change meaning. In particular it must stay "can only subtract" —
  making it required-true would break every existing deployment on upgrade.

## Alternatives considered

**A config-file `[telemetry] enabled` key.** Rejected on initialization order, as
above. Worth restating because it is the obvious request and the reason it does
not work is not obvious.

**Per-signal switches (`ROLTER_TRACES_ENABLED`, …).** Rejected. The problem #812
names is that "off" is currently spread across several variables; adding more
variables reproduces it. Per-signal control already exists through the
signal-specific `OTEL_*_ENDPOINT` variables.

**In-process per-tenant exporters.** Rejected on all three of scaling, egress
safety and cardinality, as above.

**Reflecting a tenant-supplied endpoint straight from a virtual key.** Rejected
outright: it turns every key holder into someone who can aim the gateway's
egress, which is the SSRF primitive ADR-0019 exists to prevent.
