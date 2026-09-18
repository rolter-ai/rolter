# Dashboard error states

A screen that cannot load its data has to say _why_. Before #962 every screen
rendered the same sentence — `Failed to load X.` — for causes needing entirely
different responses, so it pointed at none of them.

That is not a hypothetical cost. During the #924 dogfooding pass the Keys screen
showed "Failed to load your keys." while the real cause was every
`/api/v1/me/*` route returning 401 (#942). The message sent the operator to
check their key configuration; the actual cause was found afterwards by reading
traces. An error that cannot separate _you are not signed in_ from _the server
is down_ costs more time than no error at all, because it invites a wrong
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

| kind              | cause                                                                                                                         | recovery offered                 |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------- | -------------------------------- |
| `unauthenticated` | 401                                                                                                                           | sign in again                    |
| `forbidden`       | 403                                                                                                                           | none — ask an administrator      |
| `openMode`        | 401 with code `open_mode_no_session`                                                                                          | none — set `ROLTER_ADMIN_TOKEN`  |
| `noStore`         | 404 with code `no_such_endpoint` (or that message prefix from an older control plane)                                         | none — set `ROLTER_DATABASE_URL` |
| `noAnalytics`     | an `AnalyticsUnavailableError`: 503 from a control plane with no `clickhouse_url`, or 404 from one too old to serve the route | none — set `CLICKHOUSE_URL`      |
| `unreachable`     | the thrown value is not an `ApiError`, so `fetch` never connected                                                             | retry                            |
| `server`          | 5xx                                                                                                                           | retry                            |
| `unknown`         | any other non-ok status                                                                                                       | retry                            |

`noStore` is the third one that looks like something else. Every CRUD and
settings route is mounted only when the control plane runs with a database, so
a config-file-only deployment answers Users, Keys, Providers and every settings
screen with the API's JSON 404. That is the deployment's shape, not a wrong URL
and not a failure a retry can change (#1204).

`noAnalytics` is its sibling and the fourth (#1236). The analytics routes _are_
mounted; they just have no ClickHouse behind them, so they answer 503 —
`getAnalytics` turns that, and the 404 an older control plane gives, into an
`AnalyticsUnavailableError` rather than an `ApiError`. Without a kind of its
own that error carried no status, so the status-less rule read it as
`unreachable` and told the operator the control plane could not be reached
while it was answering every request. Logs, McpLogs and Dashboard each used to
hand-roll their own paragraph for it — two of them untranslated, one shaped as
an empty state — which is three different answers to one deployment setting.
CostAttribution's spend strip and Account's usage figures followed in #1270:
the strip had the Dashboard's old empty state, and Account now states any usage
failure once above the key cards instead of letting each card read "no usage".

Two of these are easy to collapse and must not be. A plain 401 is fixed by
signing in; `open_mode_no_session` is a control plane running with no admin
token, which has no accounts to sign into at all — signing in again is exactly
the wrong advice. And a retry button on a 403 suggests the failure was transient
when it was a permission, so `isRetryable` withholds it.

## A 401 is handled once, not per screen

Since #1196 the shell owns the expired session. `api.ts` calls the handler
`AuthProvider` registered through `setSessionExpiredHandler` whenever a request
that _carried the session token_ is answered 401 — the token and the cached
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

## A screen that makes one request per row

A screen whose list read is followed by a detail read per row has a failure the
single-query screens do not: the list arrives, some of the detail reads do not,
and the rows they belong to still have to render as _something_. Defaulting
them to the empty answer is the bug #1461 was filed over — the Complexity
Router mapped `policyQueries[i]?.data?.tiers ?? []` and so drew a route whose
policy had 500'd, 403'd or simply not landed yet in the group headed "No policy
yet", beside a button offering to create the policy it already had.

Keep the four states apart and let each one say what it is:

- **loading** — a skeleton for that row, not an empty answer;
- **failed** — one `LoadError` for the group, the affected rows named under it,
  and _no way in_: an editor seeded from a read that failed saves a fresh draft
  over contents nobody has seen;
- **configured** and **unconfigured** — the two real answers.

Two consequences fall out of this. A count in the screen's summary counts only
the rows that resolved, because a denominator that includes the unread ones
states them as empty. And `useScreenReady` / `useErrorState` follow the detail
reads too — a screen that reports itself interactive while every row is still
a skeleton is measuring the wrong moment.

An editor opened from such a row re-reads its own record (`refetchOnMount:
"always"`) rather than seeding from the list's cache, so it has the same two
ways of holding nothing and has to render both.

## Adding a screen

Add the resource noun to `errors.resources.*` in **every** catalog under
`ui/src/lib/i18n/locales/` (see [i18n](i18n.md)) and use it as above. The six
`errors.load.*` kinds already exist; a new screen needs no new error copy.

A _mutation_ that fails is a different surface: it is reported where the action
was taken, not where the data would have been. For a destructive action that
means inside the confirmation, which stays open so the message has somewhere to
live — see [destructive actions](destructive-actions.md).

## One-shot feedback: toasts

Inline messages are for what stays on screen: a field that failed validation, a
load that failed. Feedback about something that just _happened_ — a save that
went through, a delete the control plane refused, a row toggle that bounced —
goes through the toast queue (`useToast()` in `ui/src/lib/toast.tsx`, rendered
once by `<Toaster />` in the shell). A success is a polite `role="status"`
announcement that dismisses itself; a failure is an assertive `role="alert"`
that stays longer, carries the control plane's own message as its detail line,
and can be dismissed by hand. Outside the provider — a story, a test — the hook
is a no-op, so a screen never has to know whether the shell is around it.
Use `t("toast.*")` for the titles so every screen says "saved" the same way.

### Which outcomes toast

Every `useMutation` reports its outcome. The rule is _where_, not _whether_:
the toast carries what the surface that triggered the action cannot, because
that surface is gone by the time the answer arrives.

| the action                                       | success                                                    | failure                                                          |
| ------------------------------------------------ | ---------------------------------------------------------- | ---------------------------------------------------------------- |
| a settings screen's Save                         | toast — the sticky footer's "…updated." flash is gone      | toast; the footer no longer keeps a copy                         |
| a sheet or dialog that closes on success         | toast                                                      | toast, plus the inline line the still-open sheet already carried |
| a delete behind a `ConfirmDialog`                | toast                                                      | toast, plus the dialog's own `error` line                        |
| a row toggle                                     | nothing — the switch staying flipped _is_ the confirmation | toast; a switch that bounces back says nothing at all            |
| a reveal-once secret (mint a key, issue a token) | nothing — the secret on screen is the confirmation         | the inline line beside the button                                |

Field-level validation never moves: a value that will not parse belongs next to
the field, on screen for as long as it is wrong.

### The settings screens invalidate

`setQueryData` alone left every other reader of the key on the value it already
had — the screen looked saved and the rest of the dashboard did not agree. The
eleven settings screens now write the response _and_ invalidate the query, so
the save is what the next read sees (#1197).
