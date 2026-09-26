# Realtime metering

A `/v1/realtime` session is a WebSocket that can stay open for an hour and run
hundreds of model responses. Until #1396 it was admitted against the
process-local `[realtime]` caps and nothing else: no budget, no rate limit, no
request-log row. This page states how a session is admitted, how it is metered
and what happens when its budget runs out.

The code is in `crates/rolter-gateway/src/realtime.rs` (admission and the relay
loop) and `crates/rolter-gateway/src/realtime_metering.rs` (the meter).

## Admission

The upgrade request goes through the same checks as an HTTP request, keyed on
the same scope chain (key, project, team, org, plus the business unit and
customer the key is attributed to). They run before any upstream is dialled:

| Order | Check                          | Refusal                                                    |
| ----- | ------------------------------ | ---------------------------------------------------------- |
| 1     | authentication, model, route   | `401` / `403` / `404`, unchanged                           |
| 2     | budgets                        | `402 insufficient_quota`, the HTTP path's `budget_refusal` |
| 3     | `unpriced_policy`              | `402 model_unpriced` when the policy resolves to `block`   |
| 4     | `[realtime] max_connections`   | `429`, the process-local resource guard                    |
| 5     | rate limits                    | `429 rate_limit_exceeded` with `Retry-After`               |
| 6     | upstream connect with failover | `502` when every candidate target fails                    |

Opening a session is one request against `rpm`. The process-local cap is checked
first so that a session this gateway could not hold anyway never costs the key a
slot. The tokens a session's turns use reach `tpm` as they are metered, so a
key that spent its token window over a socket is refused its next session, or
its next HTTP request, like any other usage.

A full `tpm` window does not close a live session. Rate limits refuse new work,
and the HTTP path does not cut off a response in flight either.

## The metering unit: the response turn

A session is metered in the unit the upstream bills and reports. Every
`response.done` server event carries the `usage` of the response it ends, with
`input_tokens`, `output_tokens`, `total_tokens` and
`input_token_details.cached_tokens`, and that is what gets priced against the
route model's `[[model_prices]]` row.

The alternatives were weighed and rejected:

- **Metering on close** leaves a session that never closes cleanly unaccounted
  until it is killed, and a session that ends because the network dropped may
  never be accounted at all.
- **Metering wall-clock time** bills something no provider charges for, and
  cannot be reconciled with a provider invoice.
- **Estimating from audio bytes** duplicates the provider's own tokenizer
  badly, while the provider reports the exact count on every turn.

### Reading the event stream cheaply

The relay hands every upstream text frame to a `TurnTracker`. Almost every
frame is a delta carrying base64 audio, often tens of kilobytes, so parsing each
one would put a JSON decode of the whole audio stream on the relay's path. The
tracker reads the event `type` from the leading bytes of the object instead
(the first key, or the second behind `event_id`, which are the two places the
Realtime API writes it) and fully parses only `response.created` and
`response.done`, which are small. A frame whose `type` sits anywhere else falls
back to a substring scan: slower, but still metered correctly.

`response.created` opens a turn and starts its clock, the first `*.delta` after
it stamps time to first token, and `response.done` closes it. A `done` whose
`created` was never seen is still billed, with zero latency.

## The periodic flush

A closed turn goes to the session's meter task over a bounded channel. The relay
never waits on Redis or ClickHouse; it carries live audio.

Every `[realtime] usage_flush_secs` (default `15`), and once more when the
session ends, the meter flushes:

1. one `request_logs` row per turn, attributed like an HTTP row: org, team,
   project, key, business unit, customer, model, provider, target and variant.
   Its `request_id` is the upgrade request's id with the turn's ordinal
   appended (`<id>:1`, `<id>:2`, ...), so every row is unique and one session's
   rows share a prefix and a `trace_id`
2. the window's cost added to every applicable budget counter, in one write
3. the window's tokens added to every applicable `tpm` window
4. a re-read of the session's budgets

Rows are per turn rather than per window so that latency, time to first token
and token counts in analytics describe one model response, as they do for HTTP.
The flush window only batches the counter writes and bounds how late enforcement
can be.

`usage_flush_secs = 0` flushes after every turn instead of on a timer. There is
no setting that turns metering off.

The snapshot is read per flush, so a price, budget or limit edited mid-session
applies from the next flush on.

## A budget that runs out mid-session

The meter re-reads the budgets on every tick, not only after this session
spent something: a budget is shared by its whole scope chain, so another
session or plain HTTP traffic may be what used it up.

When a budget is spent the session is **closed**. The client first receives a
Realtime `error` event whose `error` object is the HTTP refusal's:

```json
{
  "type": "error",
  "event_id": "event_rolter_…",
  "error": {
    "type": "insufficient_quota",
    "code": "insufficient_quota",
    "message": "budget exceeded for Org 'acme' (limit $500.00)",
    "param": null
  }
}
```

It then receives a close frame with code `1008` (policy violation), and the
upstream leg is closed. `rolter_budget_blocks_total` counts the close.

Closing is the conservative choice of the three the issue named:

- **Degrading** has nothing to degrade to. The session is pinned to one
  upstream and model for its life, and switching mid-session is exactly the
  failover the relay refuses to do because replaying audio or tool events is
  unsafe.
- **Allowing the session to completion** turns a budget into a suggestion for
  an hour at a time, which is the bug #1396 was filed about.

The overshoot is bounded: at most the spend of one flush window, plus whatever
turn was in flight when the budget ran out. An operator who needs a tighter
bound lowers `usage_flush_secs`.

## Responses cut short

A response still in flight when the session ends never reports usage, but the
upstream generated part of it and will bill it. The last flush writes a row
for it with `usage_unknown = 1`, zero tokens, and the reason the session ended
as its status:

| Session ended because           | `status` | `error`                                |
| ------------------------------- | -------- | -------------------------------------- |
| the client closed or went away  | `499`    | `client disconnected`                  |
| the upstream closed the session | `502`    | `upstream closed the realtime session` |
| the upstream connection failed  | `502`    | `upstream realtime connection failed`  |
| `max_session_secs` or idle time | `408`    | `realtime session closed by <limit>`   |
| a budget ran out                | `402`    | the budget refusal message             |

This follows [Client disconnects](client-disconnects.md) and
[Billed but withheld](billed-but-withheld.md): spend that cannot be counted is
marked as unknown, never recorded as free. A row cut short by the client also
counts on `rolter_client_disconnects_total`.

These rows do not count against the target's health. A client leaving, a
session limit or a spent budget is not the upstream's failure, and when the
upstream leg did break, the relay has already attributed that to the target
once for the whole session.

A turn whose `response.done` says `status: "failed"` is logged with status
`502`. Its usage is billed if the upstream reported any.

## Failure modes

- **Redis down.** Admission and recording fail open, as on the HTTP path: the
  session is admitted, turns are still logged to ClickHouse, and spend is not
  counted until Redis is back. See [Redis connections](redis-connections.md).
- **The meter falls behind.** A turn that finds the meter's queue full is
  dropped and counted on `rolter_usage_records_dropped_total`, the policy the
  HTTP usage sink already has (#1051). The queue holds 1024 turns and each flush
  is bounded by the Redis timeouts, so reaching it needs a thousand responses
  inside one stalled flush.
- **The relay task dies.** The meter sees its channel close, flushes what it
  was already handed and stops.

## What is not metered

These are tracked rather than silently missing:

- Guardrails and plugins do not run on realtime events (#1880).
- A revoked or expired key does not end a live session (#1881).
- Audio and text tokens are priced at the same rate, because a price row has one
  input and one output rate (#1882).
- Input-audio transcription runs a second model whose usage arrives on
  `conversation.item.input_audio_transcription.completed`, which the meter does
  not read yet (#1883).

## Tests

`crates/rolter-gateway/tests/realtime_metering.rs` drives real sessions against
a mock Realtime upstream: a completed turn is logged with its usage and cost, a
session that stays open is accounted on the flush timer, a response cut short is
logged as unknown, an unpriced model is refused under `block`, a spent budget
refuses a new session, a session is closed when its budget runs out, a budget
spent by other traffic closes an idle session on the flush timer, usage is
charged while the session continues, `rpm` applies per key rather than per
process, and turn tokens fill the key's `tpm` window. The Redis-backed ones read
`ROLTER_TEST_REDIS_URL` and skip without it. The tracker's frame handling is
unit-tested in `realtime_metering.rs` itself.
