# DevOps / SRE

**Maintenance only.** Keeps the deployment healthy, upgraded, backed up and
observable; drains a node before a change and restores a database after one. Has
no reason to read prompts, mint keys or change who may do what — but today the
screens they need are all superadmin-only, so they hold superadmin anyway.

**Goal:** "I know it is healthy, I can change it without an outage, and I can put
it back if the change goes wrong."

**Dogfood account:** `dev@rolter.local` (superadmin). See
[dogfood data](../user-journeys.md#dogfood-data).

**The observer watches:** the gateway's `/metrics` and **Cluster Config** while
D2–D4 run, and SigNoz `rolter · overview` for anything that got slower.

## D0 — the role

| #    | step                                      | where | expect                                                                                            | status      |
| ---- | ----------------------------------------- | ----- | ------------------------------------------------------------------------------------------------- | ----------- |
| D0.1 | operate the deployment without superadmin | —     | an operator role: cluster, health, runtime policy, alerting, connectors; no tenant data or grants | gap — #1834 |

## D1 — stand it up and upgrade it

| #    | step                               | where                                                               | expect                                                                       | status                                           |
| ---- | ---------------------------------- | ------------------------------------------------------------------- | ---------------------------------------------------------------------------- | ------------------------------------------------ |
| D1.1 | install                            | [platform-admin A0](platform-admin.md#a0--choose-the-install-shape) | both planes, Postgres, Redis, ClickHouse healthy                             | verified (compose)                               |
| D1.2 | reproduce an issue on a local copy | `just dogfood`                                                      | the whole stack on one host                                                  | partial — #1819 on hosts with a low `nofile` cap |
| D1.3 | preview an upgrade                 | [upgrading](../../../user-docs/deployment/upgrading.mdx)            | what the new version will change in the stored configuration, before it runs | works                                            |
| D1.4 | upgrade                            | new image; migrations run on control-plane boot                     | `/readyz` not ready until migrations finish; `/healthz` stays up             | works                                            |

## D2 — know it is healthy

| #    | step                                                  | where                                                                  | expect                                                                            | status                                        |
| ---- | ----------------------------------------------------- | ---------------------------------------------------------------------- | --------------------------------------------------------------------------------- | --------------------------------------------- |
| D2.1 | liveness and readiness                                | `/healthz`, `/readyz` on both planes                                   | liveness never depends on the database; readiness does                            | verified                                      |
| D2.2 | every node on the current config                      | **Cluster Config**                                                     | each gateway live and converged; a lagging one is distinguishable from a dead one | verified                                      |
| D2.3 | the fleet's own picture                               | **Circuit Breaker**, **Adaptive Routing → Dashboard**, provider health | breaker states, per-target latency, adaptive engagement                           | works; dogfood adaptive never engages — #1817 |
| D2.4 | metrics and traces in the tools the team already uses | `/metrics`; **Connectors** for OTLP export                             | request rate, latency, errors, queue depth; traces in the team's backend          | verified (SigNoz)                             |

## D3 — tune capacity

| #    | step                                           | where                             | expect                                                                                      | status                                              |
| ---- | ---------------------------------------------- | --------------------------------- | ------------------------------------------------------------------------------------------- | --------------------------------------------------- |
| D3.1 | size the per-provider queue                    | **Settings → Performance Tuning** | `workers` requests in flight per provider; `capacity` waiting; `error`/`block` backpressure | bug — #1815 (one in flight whatever `workers` says) |
| D3.2 | retries and timeouts                           | the same screen                   | a retry budget that absorbs a flaky target without multiplying load                         | works                                               |
| D3.3 | switch subsystems on and off without a restart | **Settings → Feature Flags**      | the change reaches every replica on the next config poll                                    | works                                               |

## D4 — handle an incident

| #    | step                               | where                                                           | expect                                                                                   | status      |
| ---- | ---------------------------------- | --------------------------------------------------------------- | ---------------------------------------------------------------------------------------- | ----------- |
| D4.1 | a provider goes down               | stop a fleet target                                             | its breaker opens, its routes fail over, health shows the outage and the recovery (MTTR) | works       |
| D4.2 | clients time out                   | **LLM Logs**, status 499                                        | which target the clients were waiting on                                                 | bug — #1816 |
| D4.3 | be paged                           | **Alerting → Channels / Rules**: `error_rate`, `p95_latency_ms` | a webhook when the error rate or p95 crosses the line                                    | works       |
| D4.4 | take a node out before touching it | **Cluster Config → Drain**                                      | the node reports not-ready, finishes in-flight work, receives nothing new                | works       |

## D5 — back up and restore

| #    | step                                        | where                                                        | expect                                                       | status |
| ---- | ------------------------------------------- | ------------------------------------------------------------ | ------------------------------------------------------------ | ------ |
| D5.1 | back up the control-plane database          | [backup and restore](../../deployment/backup-and-restore.md) | a dump plus the KEK, stored apart                            | works  |
| D5.2 | restore it and prove the secrets still open | `rolter kek verify`                                          | every sealed column opens                                    | works  |
| D5.3 | keep the configuration as a file too        | `rolter config export`                                       | providers, groups, routes, prices, templates; no credentials | works  |

## D6 — automate

| #    | step                                                          | where                  | expect                                    | status                                      |
| ---- | ------------------------------------------------------------- | ---------------------- | ----------------------------------------- | ------------------------------------------- |
| D6.1 | apply configuration from CI with a least-privilege credential | —                      | a scoped control-plane token              | gap — #1836 (the admin token is superadmin) |
| D6.2 | apply it from a file                                          | `rolter-seed --import` | desired state for the sections it imports | partial — #1818 (silent about the rest)     |
