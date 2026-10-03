# Application / service account

**No person at all: a program holding a virtual key.** A backend service, a
batch job, a CI pipeline, an agent CLI running headless. It never opens the
dashboard; the people who own it do (usually a
[team lead](team-lead.md) or an [org admin](platform-admin.md)). Its journey is
what happens to its traffic, its limits and its credential over months.

**Goal:** "the service calls models through one endpoint; failures upstream are
absorbed, limits are predictable, and rotating its key never takes it down."

**Dogfood setup:** a shared key minted by `lead@rolter.local` in project
`default/default` (step P1.1), used from `curl` or an SDK. See
[dogfood data](../user-journeys.md#dogfood-data).

**The observer watches:** `request_logs` for the key — status mix, retries,
target spread, cost — and the gateway's `/metrics` for queue depth and
rejections while P5 runs.

## P1 — get a credential that fits the job

| #    | step                                                   | where                                                                     | expect                                                                                                | status   |
| ---- | ------------------------------------------------------ | ------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- | -------- |
| P1.1 | the owner mints a shared key for the service           | **Governance → Virtual Keys** · `POST /api/v1/projects/{id}/virtual-keys` | shown once; not tied to the person who minted it, so it survives them leaving                         | verified |
| P1.2 | narrow it to the models and providers the service uses | the key's model and provider allow-lists                                  | `/v1/models` with the key lists exactly those; anything else is refused, 403 `model_not_allowed`      | verified |
| P1.3 | attribute its spend                                    | the key's business unit and customer                                      | every row the key serves carries both; **Governance → Business Units** and **Customers** roll them up | verified |
| P1.4 | give it an expiry where the service is temporary       | the key's expiry                                                          | refused after the date, with an authentication error                                                  | verified |

## P2 — call models

| #    | step                                                    | where                                         | expect                                                                                                       | status      |
| ---- | ------------------------------------------------------- | --------------------------------------------- | ------------------------------------------------------------------------------------------------------------ | ----------- |
| P2.1 | the unchanged OpenAI or Anthropic SDK, base URL swapped | `<gateway>/v1`                                | drop-in: no code change beyond the URL and the key                                                           | verified    |
| P2.2 | correlate its own logs with rolter's                    | send `x-request-id`, or read the one returned | the same id on the `request_logs` row                                                                        | verified    |
| P2.3 | join its distributed trace                              | send `traceparent`                            | rolter's spans land in the service's trace; the upstream receives the context                                | verified    |
| P2.4 | keep a conversation on one replica                      | send `x-session-id`                           | session affinity on `consistent_hash` and `pipeline` routes; `cache_aware` follows the prompt prefix instead | bug — #1851 |

## P3 — rotate its key without an outage

| #    | step                                   | where                                                                   | expect                                                                    | status      |
| ---- | -------------------------------------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------- | ----------- |
| P3.1 | rotate in place with an overlap window | —                                                                       | a new secret on the same key, the old one honoured until the rollout ends | gap — #1837 |
| P3.2 | the workaround                         | mint a second key with the same settings, roll it out, delete the first | no outage — as long as every setting was copied by hand                   | partial     |

## P4 — be managed by automation

| #    | step                                               | where | expect                                                               | status                                               |
| ---- | -------------------------------------------------- | ----- | -------------------------------------------------------------------- | ---------------------------------------------------- |
| P4.1 | a pipeline mints keys and routes for new customers | —     | a control-plane token scoped to one project, audited as the pipeline | gap — #1836 (only the superadmin admin token exists) |

## P5 — live with limits and failures

Run P5 against the dogfood fleet: its bad targets are what make the answers
interesting.

| #    | step                                    | where                                                     | expect                                                                                               | status                                   |
| ---- | --------------------------------------- | --------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | ---------------------------------------- |
| P5.1 | an upstream fails transiently           | `deepseek-r1` (`vllm-spot-01` 503s a quarter of the time) | retried on the other target before the service sees it; no 5xx reached the client in the dogfood run | verified                                 |
| P5.2 | the service sends many requests at once | 16 concurrent calls to one route                          | latency stays near the upstream's; the queue admits up to its worker count per provider              | bug — #1815 (one in flight per provider) |
| P5.3 | it outruns its rate limit               | a rate limit on the key                                   | HTTP 429 with `Retry-After`; the service backs off                                                   | verified                                 |
| P5.4 | it exhausts its budget                  | a budget on the key or its project                        | HTTP 402; nothing reaches the upstream                                                               | verified                                 |
| P5.5 | the queue is full                       | provider queue at capacity                                | HTTP 429 `queue_full` / `queue_timeout`, depending on the backpressure policy                        | works                                    |
| P5.6 | the service disconnects mid-request     | client timeout                                            | a 499 row, billed tokens kept, naming the target it waited on                                        | bug — #1816 (no provider on the row)     |
