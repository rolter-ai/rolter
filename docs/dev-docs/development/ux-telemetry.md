# Dashboard UX telemetry

The dashboard emits a structural-only event stream to `POST /api/v1/ui-events`,
which lands in the `ui_events` ClickHouse table beside `request_logs`. It
answers the questions spans cannot: which screens are slow to become usable,
where people back out, which forms get abandoned, which error and empty states
are actually reached.

The rules the stream lives by are in the ADR
([dashboard UX telemetry](../adr/2026-08-06-dashboard-ux-telemetry.md)) and, in
code, at the top of [`ui/src/lib/ux.ts`](../../../ui/src/lib/ux.ts) and
[`ui/src/lib/ux-react.tsx`](../../../ui/src/lib/ux-react.tsx). This page is the
practical half: where the instrumentation lives and what a new screen has to do.

## Instrument the shared component, never the screen

This is the one rule that decides whether the data is complete. A hook wired
into a screen is instrumentation that stops at whoever remembered it; a hook
wired into the component every screen renders is instrumentation that cannot be
forgotten.

| Shared component | Hook               | What it records                                               |
| ---------------- | ------------------ | ------------------------------------------------------------- |
| `EmptyState`     | `useEmptyState`    | a zero-data placeholder was reached                           |
| `LoadError`      | `useErrorState`    | an error placeholder was reached                              |
| `EditorSheet`    | `useFormTelemetry` | a create/edit form was submitted, saved, refused or abandoned |
| `ConfirmDialog`  | `useFormTelemetry` | a destructive action was confirmed or thought better of       |

`EditorSheet` backs thirteen screens and `ConfirmDialog` backs fifteen, so the
two of them cover most of what the dashboard does. Both take a **required**
`name`, which is what stops a new call site from being silently uninstrumented:
adding one without a name is a type error rather than a gap nobody notices until
the data is queried.

The screen key is not a prop. It travels through `UxScreenProvider`, mounted
once by the app shell, so a sheet rendered outside one is silent rather than
mislabelled.

## Names are keys, not content

`name` and `target` are stable slugs — `virtual-key-create`,
`alert-channel-delete`, `sso-connection-edit`. Never a title, never a row's own
name, never anything derived from what was typed. `sanitizeKey` in `ux.ts`
rejects anything that is not a plausible key rather than truncating it, because
a truncated secret is still a secret; the cost of getting this wrong is one
missing event, and that is deliberate.

Where a sheet serves both create and edit, name both — `custom-role-create` and
`custom-role-edit` are different findings, and an abandon rate that mixes them
answers neither.

## Adding a screen

A new screen calls `useScreenReady(ready)` with its own primary-data loading
flag. Only the screen knows which of its several queries is the one the user is
waiting on, so this stays with the screen rather than being inferred centrally —
a guess would produce a number that looks authoritative and is not. Take care
with a query that is `enabled: false`: it stays pending forever, so gate it the
way `Playground` does rather than letting it suppress the event.

Everything else — forms, confirmations, empty and error placeholders — comes
free from the shared components, provided the screen uses them.

## Asserting the events

The events are asserted in stories, through the recording helpers in
[`ui/src/pages/story-harness.tsx`](../../../ui/src/pages/story-harness.tsx):

- `recordUxEvents` in the meta's `beforeEach`, so a story neither inherits the
  previous one's events nor leaves its own behind
- `expectUxEvent(action, target?)` waits for one event and hands it back, so the
  story can go on to assert the screen, the outcome or the duration
- `expectNoUxEvent(action, target?)` is the other half, and it is the one that
  matters for a cancel: a dialog that emitted `form_submit` on the way out
  passes every positive assertion and still makes the data say the delete went
  through
- `uxEvents()` for the cases where the _sequence_ is the assertion — a refused
  save is `["ok", "error"]` on `form_submit`, two rows, not one

The queue is never flushed under a story, so it can be read directly.

Run them with `bun run test:stories <file>` from `ui/`.

## Proving the pipeline end to end

Stories assert what the dashboard _emits_. They say nothing about whether a row
ever reaches ClickHouse, and the client is built so that it never will say:
`ux.ts` swallows every transport failure on purpose, because a dashboard whose
save button breaks when analytics 500s is worse than a dashboard with no
analytics. A pipeline that has quietly stopped working is therefore
indistinguishable, from every screen and every log, from a pipeline nobody is
using.

[`crates/rolter-control/tests/ux_pipeline.rs`](../../../crates/rolter-control/tests/ux_pipeline.rs)
closes that gap: it posts the payload `ux.ts` builds to the real
`POST /api/v1/ui-events`, then reads the rows back out of ClickHouse and asserts
screen, action and `ts`. Reading `ts` back is the only place #1224 — a batch
stamped at flush time, collapsing a whole session onto one instant — is
observable at all, since a row stamped at ingest is otherwise a perfectly valid
row.

It runs wherever both `ROLTER_TEST_DATABASE_URL` and `ROLTER_TEST_CLICKHOUSE_URL`
are set, and self-skips otherwise. CI sets both: the `nextest / doctests` job in
`quality.yml` runs a ClickHouse service beside its Postgres one. Locally:

```bash
docker run -d --name ux-ch -p 18123:8123 clickhouse/clickhouse-server:24-alpine
ROLTER_TEST_CLICKHOUSE_URL=http://127.0.0.1:18123   cargo test -p rolter-control --features postgres --test ux_pipeline
```

The test creates the `ui_events` table itself, from the shipped
`clickhouse/008_ui_events.sql` rather than from a copy — a private copy would
let the table under test drift away from the one a deployment gets, which is the
class of failure it exists to catch.

## Failure modes, and who notices

Every row below was produced by running it, not by reading the code. The middle
column is what decides the blast radius: `ux.ts` treats 401, 403, 404 and 405 as
terminal and stops sending for the life of the tab, and treats everything else
as transient — dropping the batch it was holding and trying again on the next
flush.

| What went wrong                                              | Server answers | Client does                    | Cost                                                                    | Who notices  |
| ------------------------------------------------------------ | -------------- | ------------------------------ | ----------------------------------------------------------------------- | ------------ |
| ClickHouse is down or unreachable                            | `500`          | drops the batch, keeps sending | the events queued during the outage, and nothing after it               | nobody       |
| `ui_events` table missing (a data volume older than the DDL) | `500`          | drops the batch, keeps sending | **every event, for the whole run** — the condition never clears itself  | nobody       |
| one malformed event in a batch                               | `400`          | drops the batch, keeps sending | every event that shared that flush, not just the bad one                | nobody       |
| batch over 100 events                                        | `400`          | drops the batch, keeps sending | that flush; only reachable if the client's cap drifts from the server's | nobody       |
| session lapsed, or no session                                | `401`          | **disables itself**            | everything from that moment until the tab is reloaded                   | nobody       |
| control plane older than #805, or a proxy dropping the route | `404`          | **disables itself**            | everything, from the first flush onward                                 | nobody       |
| a proxy rewriting the method                                 | `405`          | **disables itself**            | everything, from the first flush onward                                 | nobody       |
| `CLICKHOUSE_URL` unset on the control plane                  | `500`          | drops the batch, keeps sending | everything, and one request per flush forever                           | nobody       |
| `logging.ui_events = false`                                  | `202`          | nothing — this is success      | everything                                                              | the operator |

"Nobody" is literal in every row: the dashboard shows nothing, and the control
plane logs nothing either — a failed insert returns an `ApiError` and is never
traced. That is the right call for the _client_ and a poor one for the server,
which is why it is filed separately.

The last row is the only deliberate one, and it is only reachable from a
bootstrap TOML: `logging.ui_events` is not projected out of the Postgres store,
so a database-backed deployment always runs with it on.

## What is not here yet

A clean abandon and a dirty one are different findings — one is a misclick, the
other is somebody who filled the form in and gave up — and `EditorSheet` knows
which it was. Recording it needs a new `action` enum value and therefore a
ClickHouse migration, so it is tracked separately in
[#1731](https://github.com/rolter-ai/rolter/issues/1731) rather than smuggled in
as a routine change.
