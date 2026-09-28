# Finance / FinOps

**Viewer at the org** for reading, and — where the organisation lets finance set
limits — **admin at the org** for budgets. Reads spend, never prompts: a viewer
sees every request-log row of the org with the captured bodies withheld (#1820),
which is exactly the separation a finance function wants.

**Goal:** "I know what we spent, on which models, for which business unit and
customer; spend stops at the limits we agreed; and month-end is an export, not a
project."

**Dogfood account:** `finops@rolter.local`, viewer at org `default`. Steps that
change a budget use `orgadmin@rolter.local`. See
[dogfood data](../user-journeys.md#dogfood-data).

**The observer watches:** which screen FinOps opens first, and how long it takes
to answer "what did business unit X spend last month" — the answer should be one
screen and one filter.

## F1 — see the spend

| #    | step                                      | where                                                                                   | expect                                                                      | status   |
| ---- | ----------------------------------------- | --------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- | -------- |
| F1.1 | sign in                                   | login screen                                                                            | the org's Dashboard; every mutating control disabled with the role it needs | verified |
| F1.2 | spend over time, by model                 | **Observability → Dashboard**                                                           | totals, a time series, per-model cost and latency — the org's traffic only  | verified |
| F1.3 | read a request without reading its prompt | **LLM Logs** → a row                                                                    | tokens, cost, status; the bodies say they are hidden for the viewer role    | verified |
| F1.4 | spend by business unit and customer       | **Governance → Business Units**, **Customers** · `GET /api/v1/analytics/by-attribution` | one row per unit or customer                                                | verified |

## F2 — make the numbers trustworthy

| #    | step                                                | where                                                    | expect                                                                         | status                                      |
| ---- | --------------------------------------------------- | -------------------------------------------------------- | ------------------------------------------------------------------------------ | ------------------------------------------- |
| F2.1 | find traffic that counted as free                   | **Dashboard** unpriced share, **LLM Logs** unpriced flag | every model with no price row named                                            | verified                                    |
| F2.2 | price it                                            | **Models → Pricing Overrides**                           | refused for an org admin: the price catalog is deployment-wide, a superadmin's | verified (by design); ask the platform team |
| F2.3 | decide what unpriced traffic does to a budget       | the budget's unpriced policy                             | count it as zero, or refuse it                                                 | works                                       |
| F2.4 | keep a standing view of "spend by unit, this month" | —                                                        | a saved filter preset                                                          | gap — #1825                                 |

## F3 — set limits

| #    | step                                             | where                                                                         | expect                                                                         | status      |
| ---- | ------------------------------------------------ | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------ | ----------- |
| F3.1 | a monthly cap per business unit and per customer | **Budgets & Limits → Add budget** (admin at the org) · `POST /api/v1/budgets` | HTTP 402 for the unit's or customer's keys once the cap is reached             | verified    |
| F3.2 | a cap per person                                 | —                                                                             | an allowance per engineer across their keys                                    | gap — #1830 |
| F3.3 | change a cap                                     | the budget row                                                                | delete and recreate: spend so far is kept, because counters are keyed by scope | partial     |

## F4 — be told

| #    | step                               | where                                 | expect                                         | status      |
| ---- | ---------------------------------- | ------------------------------------- | ---------------------------------------------- | ----------- |
| F4.1 | a warning at 80% of a budget       | —                                     | a notification before the 402                  | gap — #337  |
| F4.2 | an alert when spend velocity jumps | **Alerting → Rules** `spend_velocity` | refused below superadmin; deployment-wide only | gap — #1829 |

## F5 — month-end

| #    | step                                            | where                                                                  | expect                                                        | status      |
| ---- | ----------------------------------------------- | ---------------------------------------------------------------------- | ------------------------------------------------------------- | ----------- |
| F5.1 | export spend by unit and customer for the month | —                                                                      | a CSV with the window, the currency and unpriced rows flagged | gap — #1838 |
| F5.2 | the workaround                                  | page `GET /api/v1/analytics/by-attribution` with a session and convert | the same numbers, by script                                   | partial     |
