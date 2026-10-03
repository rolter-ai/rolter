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
both the title and the body, so each reads as a sentence in every locale. Some
bodies name it, so a call site that filled only the title showed
the reader a raw `{{resource}}` (#1362); catalog parity cannot catch that,
because the placeholder is present in every locale and it is the render that
drops it. `src/lib/load-error.test.ts` holds the copy to the one variable the
component passes, and the `EveryKind` story asserts no `{{` survives to the DOM. Pass `onRetry` whenever the
caller holds a query handle; the component decides whether offering it is
honest.

## What it distinguishes

`classifyLoadError` in `ui/src/lib/load-error.ts` maps a thrown value to one of
seven kinds. `ApiError` already carries `status` and the control plane's `code`,
so no screen has to parse a message to find out what happened.

| kind              | cause                                                                                 | recovery offered                 |
| ----------------- | ------------------------------------------------------------------------------------- | -------------------------------- |
| `unauthenticated` | 401                                                                                   | sign in again                    |
| `forbidden`       | 403                                                                                   | none — ask an administrator      |
| `openMode`        | 401 with code `open_mode_no_session`                                                  | none — set `ROLTER_ADMIN_TOKEN`  |
| `noStore`         | 404 with code `no_such_endpoint` (or that message prefix from an older control plane) | none — set `ROLTER_DATABASE_URL` |
| `unreachable`     | the thrown value is not an `ApiError`, so `fetch` never connected                     | retry                            |
| `server`          | 5xx                                                                                   | retry                            |
| `unknown`         | any other non-ok status                                                               | retry                            |

`noStore` is the third one that looks like something else. Every CRUD and
settings route is mounted only when the control plane runs with a database, so
a config-file-only deployment answers Users, Keys, Providers and every settings
screen with the API's JSON 404. That is the deployment's shape, not a wrong URL
and not a failure a retry can change (#1204).

## No analytics store is not a load error

A control plane with no ClickHouse is a deployment shape rolter supports, so
`LoadError` has no kind for it. The analytics routes _are_ mounted; they just
have no ClickHouse behind them, so they answer 503, and a control plane too old
to serve a route answers 404. `getAnalytics` turns both into an
`AnalyticsUnavailableError` rather than an `ApiError`, which carries no status.
Read as a load failure it would be wrong twice over: the status-less rule calls
it `unreachable` and tells the operator the control plane could not be reached
while it was answering every request (#1236), and a red `role="alert"` puts a
deployment shape in the voice of a 500 that a screen reader announces as urgent
on every visit (#1984, #1976, #2016).

Every screen that reads analytics states the answer in the shared
`AnalyticsUnavailable` panel in `ui/src/components/`, an informational
`role="status"`: a neutral surface, the `CLICKHOUSE_URL` guidance in monospace,
the control plane's own message under it, and no retry, because no retry can
help. A screen passes its own copy as `i18nKey`, with a `title` and a `body`
under it, and the body says what the screen will show once the store is there.
The panel is chosen on the error's class, before `LoadError` is reached:

```tsx
{error instanceof AnalyticsUnavailableError ? (
  <AnalyticsUnavailable error={error} i18nKey="pages.logs.noAnalytics" />
) : (
  <LoadError error={error} resource={t("errors.resources.requestLogs")} onRetry={retry} />
)}
```

| screen                   | where the panel goes                                                                                                  |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------- |
| LLM Logs (#1984)         | in place of the table and its filters                                                                                 |
| Dashboard (#1976)        | in place of every card, once any read answers it while holding nothing; the setup checklist above it stays            |
| MCP Logs (#2016)         | in place of the stats and the table; also inside a call's detail drawer if the store goes away under an open list     |
| Cost Attribution (#2016) | in place of the spend strip; the business units or customers below it keep working, since they are read from Postgres |
| Account (#2016)          | above the key cards; each card still says its usage is unavailable, and the keys can still be created and rotated     |

A screen that hands an `AnalyticsUnavailableError` to `LoadError` anyway gets
the `server` kind, not `unreachable`: the control plane did answer. That is a
fallback for a screen that forgot the panel, not a way to show the state.

On the Dashboard the panel is one answer for the screen rather than one per
card. It replaces every card as soon as any of the four reads answers
`AnalyticsUnavailableError` while holding no data, since each card would
otherwise be a skeleton or an error about a store that was never there. The
setup checklist above it stays, because it reads rows rather than traffic.

## A polled query that fails

A screen that polls with `refetchInterval` can fail in a way a one-shot read
cannot. react-query sends a query that has never held data back to `pending` on
every refetch, and clears its error when it does. So a poll that keeps firing
after a failed first load unmounts the `LoadError`, shows the skeleton while
the retries run, then mounts the alert again. A screen reader hears the alert
anew on every cycle. LLM Logs did this every five seconds (#1984).

Stop polling while the query is in error with no data, using a function
`refetchInterval`:

```tsx
refetchInterval: (q) =>
  streaming && !(q.state.status === "error" && q.state.data === undefined) ? pollMs : false,
```

The `LoadError`'s retry is then what asks again, and a retry that lands starts
the poll again. A failure with rows already on screen keeps its error through
a refetch, so that case may go on polling, and should say it is retrying.

Anything that claims the feed is live (a pulse, "Streaming") reads the query,
never the pause toggle alone. It says live only after a fetch that succeeded.
On a failure it says so, gives the time from `errorUpdatedAt` through
`useFormat()`, and says "retrying" only when a poll will actually retry. The
`Failed` story in `Logs.stories.tsx` holds a reference to the alert node across
three polling intervals and asserts it is still connected: an alert that
flickered would have been unmounted and replaced, so the held node would be
detached even if a new alert had since appeared.

The Dashboard polls four queries under the same rule (#1975), and each card
owns the state of the read behind it (#1976). The four figures, the spend chart,
the traffic donut, the by-model bars and the recent rows each read one query
through `CardRead`, which shows the card's own skeleton while the read is
awaited, the card's own `LoadError` with a retry for that read alone when it
failed holding nothing, and its content once it holds data. The content is a
render prop handed the data, so a card cannot say "No traffic yet." or "Nothing
logged yet." about a read that has not answered or that failed. One failing
endpoint takes down the cards that read it and leaves the rest of the screen up.
The donut and the bars read the same endpoint, so one failure of it is one alert:
the traffic share card holds the `LoadError` and its retry, and the by-model card
holds a plain sentence (`pages.dashboard.sharedRead`, not a live region, no
button) saying it reads the same data and where to retry. Either card comes back
with that read.

When every read fails at once the screen says so once (#2342). All four queries in
error holding nothing, and none of them the no-analytics answer, replace the cards
with one screen-level `LoadError` that names the analytics
(`errors.resources.analytics`), quotes the error of the figures read, and offers one
Try again that refetches all four reads. The setup checklist stays above it, as it
does above the no-analytics panel. Before, a total outage put five `role="alert"`
blocks on screen, so a screen reader announced five alerts on mount, two of them (the
traffic share and the by-model bars) stating one failure twice.

A partial failure is unchanged: a card whose read failed holds its own alert while the
ones that answered stay up, and those keep polling. So does a failure next to a read
that is still out, since a read that has not answered is not a failure: the screen
becomes one alert only when the last read fails. The per-card error signals of
`useErrorState` are withheld while the screen-level alert is the placeholder on
screen, so one outage is one signal, `dashboard-analytics`.

A poll that fails over data a card already shows keeps that data, and every card
says so. `RefreshFailed` writes `Refresh failed at {time}, retrying` in the
danger text colour under the figures and under each chart, and the Recent
requests card, the one that says "Live", swaps the word for the same sentence in
its label, or for `Load failed at {time}` when its first read failed and polling
stopped. The line is plain text rather than a live region: every failed poll
rewrites it, and five cards announcing once a minute would be noise. Each card
keeps the `pollEvery` rule for its own read, so a query that never held data
stops polling and holds its alert.

The Recent requests label follows the same rule as every feed that claims to be
live: it says `Live` only once a read has succeeded. While the first read is out it
says `Loading`, and on a failure it gives the time. It used to say `Live` from the
first paint because it tested `!isError`, which is also true of a read nobody has
answered yet (#2341).

The `AFailedPollKeepsWhatLoaded` and `AFirstLoadThatFailsStopsPolling` stories in
`Dashboard.stories.tsx` pin the two rules; the second one is the total outage, and
asserts exactly one alert, held across three intervals with nothing asked, whose
retry asks for each of the four reads once. `OneFailedCardLeavesTheRestUp`,
`AFailedRecentReadIsNotAnEmptyOne`, `TheTwoCardsOnOneReadShareOneAlert`,
`SeveralFailedCardsEachHoldTheirOwnAlert`, `AReadStillOutIsNotYetAnOutage` and
`AFailedCardHoldsItsAlertWhileTheOthersKeepPolling` pin the partial failure: the
per-card alert, its retry, where the outage starts, and the polling that goes on around
it. The `…IsStillLoading` stories pin each card's skeleton, and
`TheRecentLabelSaysLiveOnlyAfterTheFirstRead` the label.

A polled screen that reads a time window keeps the window's name in the query key
and works the bounds out inside the query function, with `windowBounds()` from
`ui/src/lib/time-window.ts`. Bounds fixed when the screen mounted keep widening
on a tab left open, so "the last 24 hours" would mean every hour since the tab
was opened, and a key that carried a fresh timestamp would fetch on every render.
The Dashboard and LLM Logs (#2315) do this, and the `TheWindowRollsForward…`
stories record two reads and assert the second `since` is later than the first.

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

**A failure is not an empty result either.** The reverse mistake is the
common one. A failed read holds no rows, so a screen that derives its empty
state or its count from the rows renders "No providers yet" and "0 providers"
directly under the `LoadError` that says the list could not be read, which
turns an outage into "nothing configured" (#2211). The empty state waits for a
read that succeeded and a count waits for the data it counts; the
`ListEmptyRow`, `Table` and `ListSummary` primitives that enforce this are in
[loading and empty states](loading-and-empty-states.md#a-read-that-has-not-answered-holds-no-rows-either).
A screen's error stories assert it with `expectNoFalseEmpty` after
`expectLoadError`.

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

Routing Rules made the same mistake with its target reads (#2133):
`targetQueries[i]?.data ?? []` drew a route whose targets were still loading, or
had failed, as "No targets yet." and "0 targets". It resolves the failure
inside the row, not in a group. The route's name, strategy and labels are
known there and only its targets are not, so the card stays in the grid and
holds a skeleton, or its own `LoadError` with a retry for that one read, where
the targets would be. Its target count is left out until the read answers. The
rule that matters is the same on both screens: the empty copy and the zero
count wait for an answer. When route editing lands (#2134), a card whose
targets failed must not open an editor seeded from nothing.

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

A sheet reports its failure in two parts. `EditorSheet`'s `errorMessage` is the
screen's own translated lead ("Could not create the key") and `errorDetail` is
what the control plane said, read with `errorDetail()` from `ui/src/lib/toast.ts`.
The server answers in English whatever the locale and the dashboard has no table
to translate it with, so the lead keeps the sheet readable in every language and
the server's words sit under it in mono, the way a failed toast and `LoadError`
carry them. A sheet that passes only `errorMessage` renders one line as before;
Account's mint sheet is the first to use both, and the other sheets follow as
#2216 reaches them.

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
