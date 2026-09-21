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

## The struggle signals

Three of the actions record what the operator _could not_ do (#1731). They are
the reason the stream is worth pointing at a dogfood week at all — everything
else says what happened, and these say where it went wrong.

| Action          | Emitted from                                    | What it means                                         |
| --------------- | ----------------------------------------------- | ----------------------------------------------------- |
| `retry_submit`  | `useFormTelemetry` — a submit after a failure   | somebody did not understand why the first save failed |
| `refused_click` | `useRefusedClick` — the disabled-control path   | somebody reached for a control RBAC denies            |
| `abandon_dirty` | `useFormTelemetry` — a close with a draft in it | somebody filled the form in and gave up               |

`form_abandon` keeps its old meaning and is now only the clean case — a form
closed untouched, which is a misclick rather than a design problem. The two are
separate actions rather than one action with a flag precisely so the existing
series is still comparable across the change.

A retry is state the hook carries: `failed()` arms it, the next `submitted()`
spends it. A third save after a success is a fresh attempt, not a retry of a
failure two saves ago.

### Why a refused click takes a wrapper

A `disabled` button is inert. The HTML spec has the user agent withhold the
`click` event from a disabled form control, so the one interaction worth
measuring is the one the DOM refuses to report — and every alternative to
`disabled` is worse. `aria-disabled` with a swallowed handler makes a screen
reader announce a control that is not one; removing the control takes away the
explanation of why it is missing.

So the control stays genuinely disabled and `useRefusedClick` catches
`pointerdown` in the **capture** phase on a `display: contents` wrapper beside
it. Capture runs on every node on the event's path before the target, and a
pointer event is dispatched to a disabled control where a mouse or click event
is not, so the wrapper sees the reach the button never will. The wrapper
generates no box, so the control keeps its place in the parent's layout.

A disabled control is not focusable, so there is no keyboard path to miss, and
nothing is deduplicated: reaching for the same refused control four times is
the signal, not noise.

`GatedButton`, `GatedSwitch` and `RowIconButton` are wired, which is every
shared gated control. Each takes an optional `control` — a stable slug such as
`provider-new`, never the label — and falls back to the control's kind, so an
un-named call site still records the capability and the screen.

### What a refused row carries, and why it is on by default

This one is a privacy-shaped call, so the reasoning is written down rather than
left in a review thread. The row records that a _control_ was reached for and
refused:

```
screen  = providers
action  = refused_click
target  = provider-new:provider:create
outcome = error
```

plus the `session_id`, `org_id`/`team_id`/`project_id` and `app_version` every
event in this stream already carries, and the `user_id` the server fills from
the session for every event alike. No label, no message, no identity beyond
that. It says "this permission boundary is in somebody's way", not "this person
tried to do something they should not have" — which is the same structural-only
bar the rest of the stream is held to, and what makes shipping it on by default
defensible rather than surveillance.

`sanitizeKey` is what holds that line, and it is deliberately asymmetric. The
capability is the signal, so one that is not a plausible key drops the event
entirely; a control key that is not one — a call site that passed the button's
text — drops only the control and still records which boundary was hit.

### Adding an action

The `action` enum is a ClickHouse `Enum8`, where the ordinal _is_ the stored
value. It is append-only: renumbering an existing value rewrites the meaning of
every row already written rather than migrating it. Adding one means

1. a new `clickhouse/NNN_*.sql` that re-declares the enum with the existing
   ordinals untouched and the new value appended,
2. the value appended to `ACTIONS` in
   [`crates/rolter-control/src/ui_events.rs`](../../../crates/rolter-control/src/ui_events.rs),
   in the same order — `the_action_list_is_the_enum8_in_ordinal_order` pins it,
   so a value sorted into the middle is a failing test rather than a silent
   remapping,
3. the value added to the `UiEvent["action"]` union in `ui/src/lib/api.ts` and
   a named emitter in `ux.ts`, so a typo at a call site is a type error.

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

| What went wrong                                              | Server answers | Client does                    | Cost                                                                    | Who notices                        |
| ------------------------------------------------------------ | -------------- | ------------------------------ | ----------------------------------------------------------------------- | ---------------------------------- |
| ClickHouse is down or unreachable                            | `500`          | drops the batch, keeps sending | the events queued during the outage, and nothing after it               | the operator: a warn and a counter |
| `ui_events` table missing (a data volume older than the DDL) | `500`          | drops the batch, keeps sending | **every event, for the whole run** — the condition never clears itself  | the operator: a warn and a counter |
| one malformed event in a batch                               | `400`          | drops the batch, keeps sending | every event that shared that flush, not just the bad one                | nobody                             |
| batch over 100 events                                        | `400`          | drops the batch, keeps sending | that flush; only reachable if the client's cap drifts from the server's | nobody                             |
| session lapsed, or no session                                | `401`          | **disables itself**            | everything from that moment until the tab is reloaded                   | nobody                             |
| control plane older than #805, or a proxy dropping the route | `404`          | **disables itself**            | everything, from the first flush onward                                 | nobody                             |
| a proxy rewriting the method                                 | `405`          | **disables itself**            | everything, from the first flush onward                                 | nobody                             |
| `CLICKHOUSE_URL` unset on the control plane                  | `500`          | drops the batch, keeps sending | everything, and one request per flush forever                           | the operator: a warn and a counter |
| `logging.ui_events = false`                                  | `202`          | nothing — this is success      | everything                                                              | the operator                       |

"Nobody" is literal where it appears: the dashboard shows nothing, and the
control plane does not log a rejected _request_. A store that cannot take the
write is different, because it is a deployment fault rather than a client bug,
and since #1747 the server reports it even though the client stays quiet:

- `rolter_control_ingest_failures` adds one per lost batch, labelled `stream`
  (`ui_events`, or `mcp_logs` for the MCP ingest endpoint, which shares the
  path) and `reason` (`insert` for a refused or unreachable store,
  `unconfigured` for a missing `CLICKHOUSE_URL`) — alert on any sustained rate;
- a `telemetry ingest failed` warning names the store's own error, at most once
  a minute per stream, with a `suppressed` count of the failures since the
  previous one. Once a minute rather than once a batch because this is a
  per-flush path from every open tab, and a warn per batch would bury the log
  it is meant to be found in;
- the `500` body says only that the event store did not accept the write. It no
  longer quotes ClickHouse's error or the insert URL, which are internal
  topology on a client-facing response; any credentials in the URL are masked
  in the log too.

The status stays `500` on purpose: `ux.ts` reads it as transient, so capture
resumes by itself once the store is back. The logic lives in
[`crates/rolter-control/src/ingest_failure.rs`](../../../crates/rolter-control/src/ingest_failure.rs).

The last row is the only deliberate one, and it is only reachable from a
bootstrap TOML: `logging.ui_events` is not projected out of the Postgres store,
so a database-backed deployment always runs with it on.
