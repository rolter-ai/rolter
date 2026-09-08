# Dashboard error states

A screen that cannot load its data has to say *why*. Before #962 every screen
rendered the same sentence — `Failed to load X.` — for causes needing entirely
different responses, so it pointed at none of them.

That is not a hypothetical cost. During the #924 dogfooding pass the Keys screen
showed "Failed to load your keys." while the real cause was every
`/api/v1/me/*` route returning 401 (#942). The message sent the operator to
check their key configuration; the actual cause was found afterwards by reading
traces. An error that cannot separate *you are not signed in* from *the server
is down* costs more time than no error at all, because it invites a wrong
hypothesis and the operator spends their attention there first.

## The rule

Never render a load failure by hand. Use `LoadError`:

```tsx
{keys.error && (
  <LoadError
    error={keys.error}
    resource={t("errors.resources.virtualKeys")}
    onRetry={() => keys.refetch()}
  />
)}
```

`resource` is the translated noun for what failed — it is interpolated into
both the title and the body, so each reads as a sentence in every locale. Five
of the eight bodies name it, so a call site that filled only the title showed
the reader a raw `{{resource}}` (#1362); catalog parity cannot catch that,
because the placeholder is present in every locale and it is the render that
drops it. `src/lib/load-error.test.ts` holds the copy to the one variable the
component passes, and the `EveryKind` story asserts no `{{` survives to the DOM. Pass `onRetry` whenever the
caller holds a query handle; the component decides whether offering it is
honest.

## What it distinguishes

`classifyLoadError` in `ui/src/lib/load-error.ts` maps a thrown value to one of
eight kinds. `ApiError` already carries `status` and the control plane's `code`,
so no screen has to parse a message to find out what happened.

| kind | cause | recovery offered |
| --- | --- | --- |
| `unauthenticated` | 401 | sign in again |
| `forbidden` | 403 | none — ask an administrator |
| `openMode` | 401 with code `open_mode_no_session` | none — set `ROLTER_ADMIN_TOKEN` |
| `noStore` | 404 with code `no_such_endpoint` (or that message prefix from an older control plane) | none — set `ROLTER_DATABASE_URL` |
| `noAnalytics` | an `AnalyticsUnavailableError`: 503 from a control plane with no `clickhouse_url`, or 404 from one too old to serve the route | none — set `CLICKHOUSE_URL` |
| `unreachable` | the thrown value is not an `ApiError`, so `fetch` never connected | retry |
| `server` | 5xx | retry |
| `unknown` | any other non-ok status | retry |

`noStore` is the third one that looks like something else. Every CRUD and
settings route is mounted only when the control plane runs with a database, so
a config-file-only deployment answers Users, Keys, Providers and every settings
screen with the API's JSON 404. That is the deployment's shape, not a wrong URL
and not a failure a retry can change (#1204).

`noAnalytics` is its sibling and the fourth (#1236). The analytics routes *are*
mounted; they just have no ClickHouse behind them, so they answer 503 —
`getAnalytics` turns that, and the 404 an older control plane gives, into an
`AnalyticsUnavailableError` rather than an `ApiError`. Without a kind of its
own that error carried no status, so the status-less rule read it as
`unreachable` and told the operator the control plane could not be reached
while it was answering every request. Logs, McpLogs and Dashboard each used to
hand-roll their own paragraph for it — two of them untranslated, one shaped as
an empty state — which is three different answers to one deployment setting.

Two of these are easy to collapse and must not be. A plain 401 is fixed by
signing in; `open_mode_no_session` is a control plane running with no admin
token, which has no accounts to sign into at all — signing in again is exactly
the wrong advice. And a retry button on a 403 suggests the failure was transient
when it was a permission, so `isRetryable` withholds it.

## A 401 is handled once, not per screen

Since #1196 the shell owns the expired session. `api.ts` calls the handler
`AuthProvider` registered through `setSessionExpiredHandler` whenever a request
that *carried the session token* is answered 401 — the token and the cached
account are dropped and the sign-in screen explains why. A screen still renders
`LoadError` for the request that failed, but it no longer does so with a dead
token attached and no way out except signing out by hand.

Three 401s are deliberately not that: `open_mode_no_session` (there is no
account to sign in to), a refused `POST /api/v1/auth/login` (the password was
wrong, the session was not), and an expired invitation token. None of them says
anything about the session in localStorage.

## Two things that are not this component

**An empty result is not a failure.** A successful request returning zero rows
renders an empty state. Routing it here would tell an operator something is
broken when nothing is. The placeholder for that, and the one for a request
still in flight, are in [loading and empty states](loading-and-empty-states.md).

**The control plane's own message is never swallowed.** `LoadError` prints it
beneath the summary. The dashboard's classification is a helpful gloss, not a
replacement — #962 happened because the gloss was the only thing on screen and
it was wrong.

## Adding a screen

Add the resource noun to `errors.resources.*` in **every** catalog under
`ui/src/lib/i18n/locales/` (see [i18n](i18n.md)) and use it as above. The six
`errors.load.*` kinds already exist; a new screen needs no new error copy.

A *mutation* that fails is a different surface: it is reported where the action
was taken, not where the data would have been. For a destructive action that
means inside the confirmation, which stays open so the message has somewhere to
live — see [destructive actions](destructive-actions.md).

## One-shot feedback: toasts

Inline messages are for what stays on screen: a field that failed validation, a
load that failed. Feedback about something that just *happened* — a save that
went through, a delete the control plane refused, a row toggle that bounced —
goes through the toast queue (`useToast()` in `ui/src/lib/toast.tsx`, rendered
once by `<Toaster />` in the shell). A success is a polite `role="status"`
announcement that dismisses itself; a failure is an assertive `role="alert"`
that stays longer, carries the control plane's own message as its detail line,
and can be dismissed by hand. Outside the provider — a story, a test — the hook
is a no-op, so a screen never has to know whether the shell is around it.
Use `t("toast.*")` for the titles so every screen says "saved" the same way.

### Which outcomes toast

Every `useMutation` reports its outcome. The rule is *where*, not *whether*:
the toast carries what the surface that triggered the action cannot, because
that surface is gone by the time the answer arrives.

| the action | success | failure |
| --- | --- | --- |
| a settings screen's Save | toast — the sticky footer's "…updated." flash is gone | toast; the footer no longer keeps a copy |
| a sheet or dialog that closes on success | toast | toast, plus the inline line the still-open sheet already carried |
| a delete behind a `ConfirmDialog` | toast | toast, plus the dialog's own `error` line |
| a row toggle | nothing — the switch staying flipped *is* the confirmation | toast; a switch that bounces back says nothing at all |
| a reveal-once secret (mint a key, issue a token) | nothing — the secret on screen is the confirmation | the inline line beside the button |

Field-level validation never moves: a value that will not parse belongs next to
the field, on screen for as long as it is wrong.

### The settings screens invalidate

`setQueryData` alone left every other reader of the key on the value it already
had — the screen looked saved and the rest of the dashboard did not agree. The
eleven settings screens now write the response *and* invalidate the query, so
the save is what the next read sees (#1197).
