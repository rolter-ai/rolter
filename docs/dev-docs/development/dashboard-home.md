# Dashboard home

The shaped brief for the Dashboard home (`ui/src/pages/Dashboard.tsx`), written for #1978. It is the
output of the `/impeccable shape dashboard` step the issue asks for: what the screen answers, for
whom, and the constraints the build keeps. `PRODUCT.md` holds the users and `DESIGN.md` the tokens;
this page is the decision for one screen and the reasons behind it.

## What the home answers in one look

> Is the fleet serving, how fast is it at the slow end, are the caches earning their keep, and what
> is it costing, over a window and a scope the screen states?

The Dashboard used to answer a different question, "what is the traffic?", with four tiles, a spend
line, a donut and a bar chart of the same by-model read, and an 8-row table that linked nowhere. It
showed a mean latency, no cache signal, and nothing about whether the gateways were up or on the
current config. The figures covered the caller's whole access scope without saying so.

## For whom

- **Platform and infra engineers, mid-incident.** They arrive with one question, "is it up, and is it
  slow?". Fleet health and p95 come first; the error rate is next to them. This is the reader the
  order of the tiles is written for.
- **Org admins and FinOps.** Spend and the by-model table, with the unpriced and withheld notices
  that qualify a total (#3302).
- **App developers.** They hold a virtual key and see only the rows their roles reach. The scope line
  tells them so, instead of letting a small number read as the deployment's.

RBAC decides the rows, not the screen: the same layout serves all three, and a member's figures are
the requests their roles can read.

## What it shows

| Block                        | Source                                                                                | Why                                                                                                                                                                                        |
| ---------------------------- | ------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Gateways tile                | `GET /api/v1/cluster/summary`, falling back to `/readyz`                              | The fleet, and whether the latest config reached it. The same hook as the header's pill.                                                                                                   |
| Providers tile               | `GET /api/v1/health/uptime` (the Health screen's cached read)                         | Of the providers that reported, how many meet the SLA, and the worst by name. Mid-incident the upstream is as likely to be down as the gateways.                                           |
| Prompt-cache read share tile | `cache_read_tokens / cache_reported_prompt_tokens` on `GET /api/v1/analytics/summary` | The provider's prompt cache, the signal `cache_aware` balancing exists to raise. Weighted by `sample_weight`; requests whose provider does not report it are left out and counted (#3410). |
| p95 latency tile             | `p95_latency_ms` on `GET /api/v1/analytics/summary`                                   | A mean hides the tail. The by-model table already returned p95; the overall one is new.                                                                                                    |
| Error rate tile              | `errors / requests`                                                                   | Kept.                                                                                                                                                                                      |
| Response-cache hit rate tile | `cache_hit_requests / requests`                                                       | How much traffic rolter's own response cache absorbs. Not prompt caching: see below.                                                                                                       |
| Requests, Spend              | `requests`, `cost_usd`                                                                | Kept, last in the strip: they follow traffic, they are not an alarm.                                                                                                                       |
| Spend over time              | `GET /api/v1/analytics/timeseries`                                                    | Kept.                                                                                                                                                                                      |
| By model                     | `GET /api/v1/analytics/by-model`                                                      | One view for what the donut and the bars showed twice: share, p95 and spend per model.                                                                                                     |
| Recent requests              | `GET /api/v1/analytics/invocations`                                                   | Each row opens its request; a link under it opens the whole log with the same filters.                                                                                                     |

**The cache tile is the response cache, not prompt caching.** `cache_hit` on a log row is a rolter
response-cache hit, which the gateway records apart from `cache_read_tokens` (the provider's prompt
cache; see `logging.rs`). Cache-aware balancing shows up in `cache_read_tokens`, so this tile does
not measure it. A deployment with no response cache reads 0.0% with a line saying no response came
from the response cache, rather than a bare zero that looks like a failure. The prompt-cache read share (#3356) is its own tile
beside it. Since #3410 the gateway records `cache_usage_reported` on each row (`Usage::cache_reported`:
the body or a response header stated a cache read, zero included), and the summary sends
`cache_reported_prompt_tokens` and `cache_unmeasured_requests` beside `prompt_tokens` when
`request_logs` has the column. The share is then over the prompt tokens of the requests that
reported it, so a fleet that mixes a reporting vLLM with a provider that says nothing reads the
reporting provider's share rather than one diluted by prompt tokens that could never have counted,
a reporting fleet that never hits reads a measured 0.0%, and the line under the tile says how many
requests it could not measure. Without the column (an un-migrated ClickHouse, or an older control
plane) the share stays over every prompt token, and a window where no provider reported a cached
token reads "—" with a line saying so, since `cache_read_tokens = 0` cannot tell a provider that
does not report from one that never hits.

Both new summary fields are weighted by `sample_weight`, the hit count as a sum and the percentile as
a weighted quantile (`P95_LATENCY`, #3355), so a sampled log neither under-reports hits nor
skews the percentile. A control plane older than the fields sends neither, and the tiles read "—"
rather than showing the mean as a p95 or a rate of zero.

## Decisions

- **One by-model view.** The donut and the bar chart drew one read twice. A single ranked table
  replaces both: the nth model is the nth colour of the categorical sequence in its swatch and bar,
  and the table lists the eight busiest, saying how many it leaves out.
- **p95, not the mean.** The latency tile is the 95th percentile of the window. A mean is not shown
  anywhere on the home.
- **The scope is stated.** The analytics reads follow the caller's roles, not the org picked in the
  scope switcher, so the line under the controls says "every request on this deployment" for a
  superadmin and "the requests your roles can read" for anyone else, then names the window and the
  number of filters. It waits for the session before it claims either.
- **The fleet is not analytics.** The Gateways tile also shows on the "analytics unavailable" and
  outage screens, and it is drawn outside the summary's `CardRead`: when only the summary (the
  heaviest read) is slow or fails, the tile stays as the first of the strip and the other tiles'
  skeleton or alert fill the rest of the grid.
- **Degraded is amber.** The tile uses the header pill's severity: a lagging or stale gateway is
  `warning` (a new `StatTone`), and only "no gateway live" is `bad`. After a rollout a stale gateway
  holds both there for 15 minutes.
- **Honest absence.** A tile with nothing to measure reads "—" with the reason; a failed read is the
  card's own error, never a zero. The loading, empty, error and no-ClickHouse states are the shared
  primitives (see [error states](error-states.md) and
  [loading and empty states](loading-and-empty-states.md)).

## Not in this home

Per-target health and the failure timeline (`fetchHealthTimeline`) stay on the Health screen; the
home carries the fleet roll-up and one provider roll-up (#3344: `rollUpProviders` in
`ui/src/lib/provider-health.ts`, the same headline figures the Health cards use), not a per-provider
grid. The provider tile needs no read of its own beyond `GET /api/v1/health/uptime`, which already
answers every provider the caller's roles reach in one rollup, so there is no fan-out to replace;
it counts a provider as up unless it breaches the SLA over the window (the "at risk" state needs the
timeline and stays on Health), leaves out a provider that reported nothing, and states a 503 as the
no-ClickHouse panel of its own. Withheld spend and unknown-usage
counts (#3302) are a notice under the strip, the way unpriced spend already is, each linking to
LLM Logs narrowed to those rows by `withheld=true` or `usage_unknown=true`.
