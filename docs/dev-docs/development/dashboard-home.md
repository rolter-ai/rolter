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

| Block               | Source                                                   | Why                                                                                      |
| ------------------- | -------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| Gateways tile       | `GET /api/v1/cluster/summary`, falling back to `/readyz` | The fleet, and whether the latest config reached it. The same hook as the header's pill. |
| p95 latency tile    | `p95_latency_ms` on `GET /api/v1/analytics/summary`      | A mean hides the tail. The by-model table already returned p95; the overall one is new.  |
| Error rate tile     | `errors / requests`                                      | Kept.                                                                                    |
| Cache hit rate tile | `cache_hit_requests / requests`                          | Cache-aware balancing is the product's differentiator; the signal belongs on the home.   |
| Requests, Spend     | `requests`, `cost_usd`                                   | Kept, last in the strip: they follow traffic, they are not an alarm.                     |
| Spend over time     | `GET /api/v1/analytics/timeseries`                       | Kept.                                                                                    |
| By model            | `GET /api/v1/analytics/by-model`                         | One view for what the donut and the bars showed twice: share, p95 and spend per model.   |
| Recent requests     | `GET /api/v1/analytics/invocations`                      | Each row opens its request; a link under it opens the whole log with the same filters.   |

Both new summary fields are weighted by `sample_weight` like the other counts (the hit count), or read
from the sampled rows as they are (the percentile), so a sampled log neither under-reports hits nor
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
  outage screens, since an operator without ClickHouse, or with it down, still needs to know whether
  the gateways are up.
- **Honest absence.** A tile with nothing to measure reads "—" with the reason; a failed read is the
  card's own error, never a zero. The loading, empty, error and no-ClickHouse states are the shared
  primitives (see [error states](error-states.md) and
  [loading and empty states](loading-and-empty-states.md)).

## Not in this home

Provider and target health per upstream (`fetchUptime`, `fetchHealthTimeline`) stay on the Health
screen; the home carries the fleet roll-up, not a per-provider grid. Withheld spend and unknown-usage
counts (#3302) are a notice under the strip, the way unpriced spend already is, each linking to
LLM Logs narrowed to those rows by `withheld=true` or `usage_unknown=true`.
