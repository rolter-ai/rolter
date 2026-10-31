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
session's row id, an API token's row id, a digest of the admin token keyed with
the ticket's own secret, or "open mode". Keying the admin digest with the secret
means a dump of the table, which holds only the secret's hash, gives nothing to
test guesses at the admin token against, while the secret presented at
redemption still recomputes it and so tells a token rotated since.

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
  deleted since, an API token rotated after the ticket was minted (rotation
  keeps the row, so its `rotated_at` is compared with the ticket's
  `created_at`), an admin token rotated since, and an open-mode ticket on a
  control plane that has since been given a token are all refused (`401`). A
  deleted session or token takes its tickets with it (`on delete cascade`), so
  a signed-out session's link answers `410`.
- Only `GET` redeems. axum answers `HEAD` with the `GET` handler, so the route
  refuses any other method with `405` before it looks the ticket up: a link
  checker or proxy that probes the URL spends nothing.
- The export then runs through the bearer route's own handler, given the
  re-resolved principal and the `AnalyticsAccess` that the same reduction a
  bearer request goes through produces (`analytics_access::analytics_reach`).
  The query checks, the row scope and the export gate are therefore the ones a
  bearer request gets, and there is no second, weaker path to drift.

**Storage.** A Postgres table (`0229_export_tickets.sql`) when the control
plane has a store, so any replica can redeem a ticket another minted. An
in-memory map behind `parking_lot` in the store-less mode and in a build without
postgres. Both sit behind one small trait. A store-less control plane can still
be scaled out, and then a ticket only redeems on the replica that minted it:
the docs say to run one replica in that mode or to keep each client on one
replica (sticky sessions) at the ingress. The memory map lets a ticket go five
minutes after it can change no more (expired unused, or its download ended),
so its size follows the exports in flight rather than a day of mints.
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
- `truncated`, with `rows` and a `reason`: `row_limit`, `time_limit` (the
  deadline timer that closes a stalled connection records it too),
  `read_error`, `client_closed` when the browser stopped reading, or
  `interrupted` when a `streaming` ticket has gone unwritten for longer than a
  live export can (the export time limit plus two progress intervals), which is
  how a replica that died mid-export reads;
- `refused`, with the HTTP `status` the download was answered with and, for a
  `429`, `retry_after`;
- `expired`: never redeemed and past its lifetime.

The replica serving the download writes these: once when the export starts, at
most every two seconds while rows flow, and once at the end, never once per
row. Progress writes are spawned so a slow store never stalls the body, and the
end is written after the last chunk has been handed on, and written again from
the body's `Drop` if that write did not complete. Writes only move forward
(`pending` → `streaming` with rising rows → one end, enforced in the `update`'s
`where`), so a late progress write cannot overwrite the end. Minting pre-checks the gate, but the status read stays the authoritative
answer for a refusal at download time, since the gate can fill between the two.

**Dashboard.** Where `showSaveFilePicker` exists, the stream-to-file path from
#3434 stays. Elsewhere the Export button mints a ticket, follows it with a
hidden same-origin `<a download>`, and polls the status about once a second
until a terminal state, showing the rows written and reusing the translated
`429` and truncated-export messages. It tolerates a few failed reads in a row,
gives up with its own message after five minutes without progress, and offers a
Cancel that only stops watching (the browser's download is unaffected). A
`client_closed` end is most likely the person cancelling in the browser, so it
gets a neutral "download stopped" line rather than a failure. The `Response.blob()` fallback is removed,
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

## Addendum: the audit log export (#3545)

The Audit Log's CSV and JSON exports, deployment-wide and per org, read the
whole file into the tab the way the analytics exports did before this decision,
and now take the same two paths through the same `ExportCsvButton`.

- `ExportKind` gains `DeploymentAudit` (`/api/v1/audit-log/export`) and
  `OrgAudit(org_id)` (`/api/v1/orgs/{org_id}/audit-log/export`, the org id in
  its canonical hyphenated form only). Both exist only on a control plane with a
  database, where the audit log does; a store-less one refuses them as unknown
  paths. The `export` column stores `audit_log` or `org_audit_log:<org id>`, so
  the table needed no migration, and the status reports `audit_log` or
  `org_audit_log`.
- The audit log is governed by its own capability, not the request log's reach.
  The mint runs `audit_log:read` on the org, or `deployment_audit_log:read`, in
  place of the analytics extractors; the redemption skips the request log's
  reach and hands the ticket to the audit handler, which checks the capability
  again for the re-resolved principal before it writes its `audit_log.export`
  row. A security auditor, who reads the deployment's audit log and not the
  request log, can therefore mint, download and read the status, which now asks
  only for the minter's own credential.
- The audit export is paged from Postgres a page at a time, so it is not
  behind the `ExportGate`, and its walk reports rows to the status as it goes.
- The dashboard's fallback for a control plane that predates the export (a
  `404` there or at the mint) still reads the log a page at a time, capped as
  before.
