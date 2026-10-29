# Single-use download tickets for the analytics CSV exports

**Status:** Accepted (29 Oct 2026) · **Date:** 29 Oct 2026 · **Issues:** [#3504](https://github.com/rolter-ai/rolter/issues/3504), [#3434](https://github.com/rolter-ai/rolter/issues/3434)
**Relates:** ADR-0032 (deprecation registry, for the OpenAPI rows)

## Context

The request log CSV export can carry a million rows (`ROLTER_EXPORT_MAX_ROWS`).
#3434 streams it to a file the person picks through the File System Access API
(`showSaveFilePicker`). Firefox, Safari and any dashboard served over plain
http lack that API. There the dashboard read the body into a `Response.blob()`,
which keeps it off the JavaScript heap but still holds the whole file in the
tab, and a small machine's tab can die of it.

Handing the body to the browser's download manager instead needs a URL the
browser can follow on its own. Every dashboard call authenticates with a bearer
header (`ui/src/lib/api/request.ts`), and a followed link cannot carry one.

Two ways round that were weighed:

- **A service worker** that injects the header into the download request. The
  dashboard has none. Adding one brings registration, scope, update and
  cache-invalidation behaviour that every page would inherit; it only exists
  after it has installed and claimed the page, so the first export after a
  deploy would still need another path; and it does not answer how the page
  learns of a `429` or a cut-off body either.
- **A same-origin link carrying a short-lived credential** for one export,
  which the control plane mints for the bearer that asks.

## Decision

A same-origin download link with a server-minted, single-use download ticket.
No service worker.

**Mint.** `POST /api/v1/analytics/exports/tickets` with the caller's normal
bearer. The body names one of the three export routes
(`/api/v1/analytics/by-attribution/export`, `.../timeseries/export`,
`.../invocations/export`) by its exact path, plus the query string the button
would send. Any other path is a `400`. The handler takes the same extractors the
export takes, so a caller the export would refuse is refused here, and parses
the query as the export parses it, so a window the export would refuse is a
`400` now. For the request log it asks the export gate, without taking a place,
whether it would let the export in, and answers the gate's coded `429` with
`Retry-After` at once when it would not.

**The secret.** 32 bytes from the OS-seeded CSPRNG, base64url, behind a
`rolter_dl_` prefix so a leaked one can be recognised. It is returned once, in
`download_url`, and stored only as its SHA-256.

**Bound to.** The minting principal, the export and its query. It is accepted
on its own download route only and is never a credential anywhere else. What is
stored of the principal is what can be checked again later, never the bearer: a
session's row id, an API token's row id, a domain-separated digest of the admin
token, or "open mode".

**Lifetime.** 60 seconds. The browser follows the link within a second of the
mint, so the window only has to cover a slow click.

**Single use.** `GET /api/v1/analytics/exports/download/{ticket}` consumes the
ticket with one statement
(`update … set used_at = now() where secret_hash = $1 and used_at is null and expires_at > now() returning …`),
so of two replicas racing on one secret, one gets it. It is spent by the first
request even if that export then fails or is refused. Unknown, used and expired
tickets all get one uniform `410 Gone`.

**Redemption.**

- The principal is re-resolved from what was stored. A session that is revoked
  or expired, an account deactivated since, an API token revoked, expired or
  deleted since, an admin token rotated since, and an open-mode ticket on a
  control plane that has since been given a token are all refused (`401`). A
  deleted session or token takes its tickets with it (`on delete cascade`).
- The export then runs through the bearer route's own handler, given the
  re-resolved principal and the `AnalyticsAccess` that the same reduction a
  bearer request goes through produces (`analytics_access::analytics_reach`).
  The query checks, the row scope and the export gate are therefore the ones a
  bearer request gets, and there is no second, weaker path to drift.

**Storage.** A Postgres table (`0229_export_tickets.sql`) when the control
plane has a store, so any replica can redeem a ticket another minted. An
in-memory map behind `parking_lot` in the store-less mode, which runs as one
replica, and in a build without postgres. Both sit behind one small trait.
Rows are purged opportunistically, a day after expiry, by the next mint; there
is no background task for it. The table is read by the control plane only, so
it has no `bump_config_version()` trigger.

**Never logged.** The secret is a path segment of a route whose request span
and latency metrics carry the route template, never the path. The secret is
read off the URI rather than through a `Path` extractor, whose rejection could
quote it. The API's `404` fallback, which answers `no such endpoint: <path>`,
redacts a download path. No error body on these routes names the secret, and no
audit entry is written for a download. The download response carries
`Cache-Control: no-store` and `Referrer-Policy: no-referrer`. A reverse proxy in
front of the control plane may still log the URL; the short lifetime and single
use are what bound that exposure, and the user docs say so.

**429 and a cut-off body.** The page cannot read a response the download
manager receives, so each ticket also has a non-secret id. The page polls
`GET /api/v1/analytics/exports/tickets/{id}` with its bearer; only the minting
principal can read it, and anyone else gets the same `404` as an id that does
not exist. The status is one of:

- `pending`: not yet redeemed;
- `streaming`, with `rows` written so far, which drives the progress display;
- `complete`, with `rows`;
- `truncated`, with `rows` and a `reason`: `row_limit`, `time_limit`,
  `read_error`, or `client_closed` when the browser stopped reading;
- `refused`, with the HTTP `status` the download was answered with and, for a
  `429`, `retry_after`;
- `expired`: never redeemed and past its lifetime.

The replica serving the download writes these: once when the export starts, at
most every two seconds while rows flow, and once at the end, never once per
row. Minting pre-checks the gate, but the status read stays the authoritative
answer for a refusal at download time, since the gate can fill between the two.

**Dashboard.** Where `showSaveFilePicker` exists, the stream-to-file path from
#3434 stays. Elsewhere the Export button mints a ticket, follows it with a
hidden same-origin `<a download>`, and polls the status about once a second
until a terminal state, showing the rows written and reusing the translated
`429` and truncated-export messages. The `Response.blob()` fallback is removed,
so no copy of the body is held in the tab.

## Consequences

- No browser holds a large export in a tab any more; the download manager
  writes it.
- Each export without the File System Access API costs one row in Postgres and
  a few status writes, plus a status read a second from the page while it runs.
- A download URL is a credential for 60 seconds. It is single use and bound to
  one export, and the principal behind it is checked again, so a leaked one
  reveals at most the file its minter could already read, and only before the
  minter's own browser uses it.
- The export handlers grew an optional `ExportProgress` extension so the
  ticket's status can report rows and the reason a body was cut; a bearer
  request carries none and behaves as before.
- The progress reported for the two rollups is coarse: they are built in one
  query, so they go from `streaming` to `complete`.
