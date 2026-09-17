# Dashboard UX telemetry as a structural-only event stream

**Status:** Accepted · **Date:** 6 Aug 2026 · **Issues:** [#805](https://github.com/rolter-ai/rolter/issues/805)
**Relates:** ADR-0023 (access-policy propagation)

## Context

Browser tracing ([#807](https://github.com/rolter-ai/rolter/pull/807)) tells us
how long a dashboard screen took and whether it threw. It does not tell us
whether the screen was *usable*: which screens are slow to become interactive,
where people back out, which forms get abandoned, which error and empty states
are actually reached. Those are the questions that decide what to build next,
and spans cannot answer them.

Collecting that is ordinarily where a product-analytics vendor is added. For a
self-hosted gateway that sits in the request path of its operators' LLM traffic,
a second data processor is a hard sell: a different retention policy, a
different redaction story, a different jurisdiction, and an outbound connection
operators did not ask for. Several rolter deployments are air-gapped, where it
simply does not work at all.

There is also a real hazard specific to this kind of telemetry. UX event streams
are where free text leaks. An "abandoned form" event is one careless field away
from carrying what was typed into the form, and the dashboard's forms hold
provider API keys, virtual keys and prompts.

## Decision

Collect UX events in-house, into a `ui_events` ClickHouse table beside
`request_logs`, and make the *schema* the privacy guarantee.

**Structural only.** Every column is a key, an enum, a duration or an id. There
is deliberately no column a form value, prompt or free-text body could be
written into. `target` names the form, control or validation rule; it never
carries what failed the rule. A future column that could hold free text is a
change to this ADR, not a routine migration.

This is chosen over the usual alternative — accept arbitrary payloads and redact
on write — because redaction is a policy that has to keep being correct as the
dashboard grows, while a schema with nowhere to put a value is correct by
construction. `logging.payload_capture` already exists for the case where
someone genuinely wants raw bodies, and it is off by default; this stream is
deliberately not that.

**Server-side attribution.** `user_id` is taken from the authenticated session
and a client-supplied one is ignored, so a caller cannot file events against
another user.

**Authentication, not authorization.** The ingest endpoint is guarded by
`CurrentUser` alone, following `me.rs`, and has no row in the capability table.
The RBAC model holds that a viewer writes nothing — `rbac_matrix.rs` asserts
exactly that — and a `ui_event:create` capability granted to any authenticated
caller breaks the invariant. A viewer who could not file their own screen views
would be absent from every funnel, which makes the data quietly wrong rather
than absent. Keeping this off the capability table states plainly that it is not
an operator surface.

**On by default, with two off switches.** Absent `clickhouse_url` the stream is
inert. `logging.ui_events = false` is a deployment-level opt-out. It defaults on
— unlike `payload_capture` — precisely because the schema cannot carry sensitive
content, so the usual reason to make telemetry opt-in does not apply.

## Consequences

Operators get usability data with no new vendor, no new egress and no new
retention policy: same deployment, same TTL, same 90-day partitioning as request
logs. Because events carry `trace_id`, a slow screen and the gateway request
behind it are one join rather than two systems.

The cost is expressiveness. Any question that needs a value rather than a key is
unanswerable by design, and some genuinely useful analyses are foreclosed. That
is the intended trade: the class of incident this prevents — dashboard secrets
in an analytics table — is worse than the analyses it forbids.

Adding a screen means adding a stable screen key rather than letting a URL
through. `screen`, `target` and `from_screen` are `LowCardinality` columns
capped at 96 characters, which a real URL does not fit; sending one would
degrade the table, so the bound fails the write instead.

Per-tenant routing of this stream is out of scope and tracked separately
([#812](https://github.com/rolter-ai/rolter/issues/812)).
