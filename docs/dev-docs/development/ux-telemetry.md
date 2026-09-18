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

## What is not here yet

A clean abandon and a dirty one are different findings — one is a misclick, the
other is somebody who filled the form in and gave up — and `EditorSheet` knows
which it was. Recording it needs a new `action` enum value and therefore a
ClickHouse migration, so it is tracked separately in
[#1731](https://github.com/rolter-ai/rolter/issues/1731) rather than smuggled in
as a routine change.
