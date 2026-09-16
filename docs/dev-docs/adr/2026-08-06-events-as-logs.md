# Events as logs, not span events

**Status:** Accepted · **Date:** 6 Aug 2026 · **Issues:** [#814](https://github.com/rolter-ai/rolter/issues/814), [#808](https://github.com/rolter-ai/rolter/issues/808), [#809](https://github.com/rolter-ai/rolter/issues/809), [#805](https://github.com/rolter-ai/rolter/issues/805)
**Relates:** ADR-0024 (dashboard UX telemetry)

## Context

OpenTelemetry is [deprecating the Span Events
API](https://opentelemetry.io/blog/2026/deprecating-span-events/) — `Span.AddEvent`,
`Span.RecordException` and friends. Maintaining two overlapping ways to emit a
correlated event (span events via the Tracing API, log events via the Logs API)
produced split guidance for instrumentation authors and a slower-evolving event
model. The replacement is **events as logs**: emitted through the Logs API,
named, and correlated to the active span through context rather than attached to
it. Semantic-convention authors are now explicitly told to document events as
log-based events.

Nothing is being removed soon. The deprecation is phased, existing data stays
valid in the OTLP trace model, and compatibility layers are a stated priority.
This is direction-setting, not an outage.

### What rolter emits today

A grep for the deprecated API finds nothing:

```
$ grep -rn "add_event\|record_exception" --include='*.rs' crates/
(no matches)
```

That result is misleading, and reading the bridge is what the audit actually
required. rolter instruments through `tracing` + `tracing-opentelemetry`, and
that layer's `on_event` turns **every `tracing` event fired inside an active span
into an OTel span event** (`tracing-opentelemetry-0.33.0/src/layer.rs:1468`,
`span.add_event(...)`; the pre-export path pushes onto `builder.events`
identically). An `ERROR`-level event additionally sets the span status to error.

So rolter has zero explicit uses of the deprecated API and is, in practice, an
exclusive user of it: the ~57 `tracing` event macros across `rolter-gateway`,
`rolter-proxy` and `rolter-core` become span events whenever they fire inside one
of the pipeline spans. The migration surface is the bridge, not our call sites.

### Why the ordering between #808 and #809 changes

The GenAI semantic conventions ([#808](https://github.com/rolter-ai/rolter/issues/808))
are precisely the area moving to log-based events. Implementing them against span
events would be building onto a deprecated API on the day it is written.

Emitting log-based events requires exporting logs at all, and rolter does not:
there is no OTLP logs exporter in the workspace and `signoz_logs` is empty by
construction. [#809](https://github.com/rolter-ai/rolter/issues/809) therefore
stops being an independent nice-to-have and becomes a prerequisite.

## Decision

**Log-based events are rolter's event model. New instrumentation targets the Logs
API; span events are legacy, produced only incidentally by the `tracing` bridge.**

Concretely:

1. **No new explicit span events.** `add_event` and `record_exception` are not to
   be called directly. Nothing calls them today and nothing should start.
2. **#809 (OTLP log export) sequences before #808 (GenAI conventions).** The
   conventions #808 implements are specified as log-based events, so the log
   pipeline has to exist before they can be emitted correctly. #808's acceptance
   criteria adopt log-based events as the target.
3. **`tracing` stays.** The bridge is idiomatic Rust and the ecosystem's own
   integration, not a bespoke house wrapper — the same distinction drawn in the
   [#815 wrapping audit](../architecture/observability.md). It is where the
   span-event mapping lives, so it is also the single place a future migration
   happens: when `tracing-opentelemetry` routes events through the Logs API,
   rolter follows by upgrading, without touching 57 call sites.
4. **Correlation is the acceptance bar.** A log-based event is only a replacement
   for a span event if it carries `trace_id` and `span_id`, which is already
   called out as most of the value in #809. An exported log that cannot be joined
   back to its span is a regression against what the bridge does today.
5. **Content stays off.** The GenAI opt-in content attributes
   (`gen_ai.input.messages` / `gen_ai.output.messages`) remain disabled whether
   they are carried as span events or as log records. Moving event model does not
   move the privacy line.

## Consequences

- The migration is a dependency upgrade rather than a code sweep, because rolter
  never adopted the deprecated API directly. This is the payoff of having gone
  through `tracing` instead of calling OTel by hand.
- #809 gains a second justification beyond "logs are missing": it unblocks #808.
  Its `trace_id`/`span_id` correlation requirement is now load-bearing rather
  than a nicety, and should be treated as a blocking acceptance criterion.
- Until #809 lands, error detail continues to reach backends as span events via
  the bridge. That is fine — the deprecation is phased and the data stays valid —
  but it means the GenAI work should not start early and improvise its own event
  channel.
- Anything that wants a *new* event type before #809 has no correct home. The
  answer is to wait rather than to add a span event that will have to be removed.

## Alternatives considered

**Implement #808 now against span events, migrate later.** Rejected. The
conventions are specified as log-based events, so this builds a second migration
into work that has not shipped yet, on the exact attributes most likely to be
re-specified.

**Emit both span events and log events during a transition.** Rejected. Duplicate
correlated events on both signals is the split-guidance problem OpenTelemetry is
deprecating span events to escape, and it doubles egress for data that already
joins on `trace_id`.

**Drop `tracing` and call the OTel Logs API directly.** Rejected. It is a large
rewrite of every call site to avoid a bridge that will itself be updated
upstream, and it contradicts the #815 finding that the ecosystem bridge is not
the wrapping the "don't wrap" guidance targets.
