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
| 4     | the gateway is shutting down   | `503`, see [Shutdown](#shutdown)                           |
| 5     | `[realtime] max_connections`   | `429`, the process-local resource guard                    |
| 6     | rate limits                    | `429 rate_limit_exceeded` with `Retry-After`               |
| 7     | upstream connect with failover | `502` when every candidate target fails                    |

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

`response.created` opens a turn and starts its clock, and `response.done` closes
it. Time to first token is stamped by the first `response.*.delta` that names
the turn in its `response_id`, or by the first one naming none. Other deltas do
not count: `conversation.item.input_audio_transcription.delta` streams the
user's own words, often well before the model answers. A `done` whose `created`
was never seen is still billed, with zero latency.

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

`usage_flush_secs = 0` flushes after every turn instead of on a timer. The meter
still ticks once a second in that mode, but only to re-read the budgets while
the session is quiet (see the next section), so `0` is the tightest setting on
both counts. There is no setting that turns metering off.

The snapshot is read per flush, so a price, budget or limit edited mid-session
applies from the next flush on.

## A budget that runs out mid-session

The meter re-reads the budgets on every tick, not only after this session
spent something: a budget is shared by its whole scope chain, so another
session or plain HTTP traffic may be what used it up. A tick is one
`usage_flush_secs`, or one second when that is `0`. The re-read costs nothing
when no budget applies to the session or Redis is not configured.

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

## A key revoked mid-session

Authentication happens once, at the upgrade, so a session opened a moment
before its key was disabled, expired or deleted would otherwise run until
`max_session_secs` (#1881). The session keeps the peppered digest of the key it
was opened with, and every tick of the meter, in the snapshot it loads for that
tick, calls `handlers::recheck_session_access`. That is the upgrade's own
sequence: the `snap.keys` lookup and `is_active` test `authenticate` does,
`authorize_model`, `named_route_for` and `authorize_route`, so narrowing `models`
or removing route access reaches a live session as well. The refusal wording
comes from `AccessDenial::message_and_code`, shared with the HTTP responses.

A failure closes the session like a spent budget does: an `error` event, then a
`1008` close and the upstream leg closed. The event code is `invalid_api_key`
for a disabled, expired or deleted key (the HTTP 401 carries no code, so this is
the OpenAI one), otherwise the denial's `model_not_allowed` /
`route_not_allowed`, or `model_not_found` when the route was removed. The check
runs before the budget read and shares its channel, so a revoked key is reported
instead of a spent budget. The bound is one flush interval, or the one-second
tick when `usage_flush_secs = 0`, plus snapshot propagation. A session opened
without a key (auth disabled) has nothing to re-check. The key's scope is fixed
at the upgrade: moving a key to another org does not re-scope a live session.

## Responses cut short

A response still in flight when the session ends never reports usage, but the
upstream generated part of it and will bill it. The last flush writes a row
for it with `usage_unknown = 1`, zero tokens, and the reason the session ended
as its status:

| Session ended because           | `status`          | `error`                                |
| ------------------------------- | ----------------- | -------------------------------------- |
| the client closed or went away  | `499`             | `client disconnected`                  |
| the upstream closed the session | `502`             | `upstream closed the realtime session` |
| the upstream connection failed  | `502`             | `upstream realtime connection failed`  |
| `max_session_secs` or idle time | `408`             | `realtime session closed by <limit>`   |
| a budget ran out                | `402`             | the budget refusal message             |
| its key or access was revoked   | `401`/`403`/`404` | the refusal the upgrade would give     |
| the gateway shut down           | `503`             | `gateway shutting down`                |

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

## Shutdown

A session outlives the HTTP request that opened it. axum's graceful shutdown
stops tracking a connection once it upgrades to a WebSocket, so on its own it
would let the process exit under live sessions. The runtime would then cancel
each relay and meter wherever it stood, and every turn still waiting for the
next flush would lose its row, its budget charge and its `tpm` tokens. With the
default `usage_flush_secs` that is up to 15 seconds of every session, on every
rollout.

So the gateway drains realtime sessions itself (`Sessions` in `realtime.rs`,
`AppState::drain_realtime_sessions`):

1. Every relay and meter task is tracked from admission on, including the
   moment between the upgrade response and the completed handshake.
2. When `SIGTERM` or Ctrl-C arrives, every session is told to close at once,
   in parallel with axum's drain of plain HTTP requests. Each client receives a
   `1001` (going away) close, the code a WebSocket server sends when it stops,
   so an SDK reconnects rather than treating the session as refused. The
   upstream leg is closed too.
3. New upgrades are refused with `503` from that moment.
4. Each meter runs its last flush: rows for finished turns, `usage_unknown` rows
   with status `503` for responses in flight, budget and `tpm` writes.
5. Once the HTTP drain is done, the process waits up to 10 seconds for the
   tracked tasks to finish, then exits. The sessions were closed when the signal
   arrived, so the wait only covers a meter still waiting on Redis, and it fits
   well inside the 30 seconds an orchestrator usually allows before `SIGKILL`.
   A session still open after it is logged as a warning.

The budget and `tpm` writes are awaited by the meter, so they land before the
process exits. The request-log rows are handed to the shared ClickHouse writer
and the budget and `tpm` records to the usage-recording workers; both are
flushed by the sink drain that runs after the realtime drain (below).

### Sink drain

The request-log writer, the health-event writer, the MCP tool-call writer and the usage-recording workers
(`SinkTasks` in `sink_drain.rs`, `AppState::drain_sinks`) each hold work that
only leaves the process once they flush: up to `[logging] flush_ms` of rows in a
batch, whatever is queued on their channel, and any budget or `tpm` record not
yet written to Redis. They cannot rely on "every sender dropped" to finish,
because `AppState` clones live on in the prober, scraper and watcher tasks, so
at shutdown they used to be cancelled mid-batch (#1924).

After the HTTP and realtime drains, `run()` cancels each sink's token. The task
closes its receiver, which still yields everything already queued and then
`None`, so the normal "senders gone" path flushes the remainder and exits. The
three sinks drain concurrently and the process waits at most 5 seconds for all
of them. That bound is deliberately short: a healthy ClickHouse or Redis takes
milliseconds, so it only ever expires when one is unreachable, and it must not
push the HTTP drain, the 10 second realtime grace and this wait past the 30
seconds an orchestrator usually allows before `SIGKILL`. When it expires, a
warning is logged and whatever the sinks still held is lost. A `try_send` that
races the close is counted as dropped, like any other full or stopped queue.

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
- **The process shuts down.** Covered by [Shutdown](#shutdown). A `SIGKILL`, or
  a drain that outlives its 10 seconds (realtime) or 5 seconds (sinks), still
  loses whatever the meter or the writers had not written.

## Content policy on a bidirectional stream (#1880)

A realtime session used to relay frames without consulting the guardrails, the
guardrail webhook or the plugins, so a key that a rule stopped on
`/v1/chat/completions` could send the same text over a socket. The chat
pipelines scan a whole body at one moment; a session has events in both
directions and no such moment. This section records how the same rules apply to
it. The code is `crates/rolter-gateway/src/realtime_guard.rs`.

The policy is resolved once, when the session opens, from the snapshot and the
route's rule selection, and pinned for the session like the target: a reload
does not change what a live session is held to, and the tenant's rules cannot
be swapped under a compiled rule index. When the route has no applicable
guardrail rule, no webhook and no `pre_upstream` plugin, there is no policy and
the relay forwards frames exactly as before, with no parsing and no copy. Audio
frames are never parsed on either leg.

### What is in scope

| Direction | Event                                                                                                                            | Text checked                                               | Stage             |
| --------- | -------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- | ----------------- |
| client    | `conversation.item.create`                                                                                                       | `item.content[].text` and `.transcript`, `item.output`     | `pre_call` rules  |
| client    | `session.update`                                                                                                                 | `session.instructions` (treated as system text)            | `pre_call` rules  |
| client    | `response.create`                                                                                                                | `response.instructions` (system), `response.input[]` items | `pre_call` rules  |
| server    | `response.output_text.delta`, `response.text.delta`, `response.audio_transcript.delta`, `response.output_audio_transcript.delta` | `delta`                                                    | `post_call` rules |
| server    | the matching `.done` events, `response.content_part.done`, `response.output_item.done`, `response.done`                          | `text` and `transcript` of the completed content           | `post_call` rules |

Instructions are operator-authored in the same sense as a chat system message,
so a rule scans them only when it sets `include_system`, exactly as on the HTTP
path. The route's `advanced.guardrails` `enable`/`disable` selection applies.
Audio is out of scope except where the provider supplies a transcript, which is
text and is checked like any other. Tool-call arguments
(`response.function_call_arguments.*`) and input-audio transcription events are
not scanned (see the limits below).

### What `block` does mid-session

A WebSocket has no status code to refuse with and ending the session for one
matched phrase would punish the whole conversation, so a block is scoped to the
event or response that matched and the session stays open:

- **A blocked client event** is not forwarded. The client receives an
  OpenAI-shaped `error` event (`code: guardrail_blocked`, or `plugin_blocked`
  for a plugin) carrying the offending client `event_id` in `error.event_id`
  when it sent one. The message names the rule, never the matched text.
  Upstream never sees the event, so its conversation state is unchanged.
- **A blocked server delta** is dropped and the client receives the `error`
  event in its place. Rolter then sends `response.cancel` upstream so the model
  stops generating text nobody will read, and drops the remaining text events
  of that response. The `.done` events and `response.done` are still delivered,
  with their text blanked, so the client's turn ends and the turn is metered
  and billed as usual.
- **A blocked completed text** (a `.done` event or `response.done` whose whole
  text matches) is delivered with the matching text blanked and an `error`
  event ahead of it.

Closing the socket was rejected as the default: it throws away the session's
audio state for a policy hit, and a client can reconnect and send the same
text. A close would be safer only if a blocked event could be followed by
others that depend on it, and the relay forwards nothing from the blocked
event, so nothing does. The budget and revoked-key closures stay closures
because they end the right to be on the socket, not one message.

`redact` rules rewrite the text in place on both legs and count in the same
`guardrail_redactions_total` / `guardrail_output_redactions_total` counters;
`annotate` rules only count. Blocks count in `guardrail_blocks_total` and
`guardrail_output_blocks_total`.

### Output: deltas against the completed text

A guardrail on one delta can miss a pattern the model splits across two. There
are two ways to close that, and the trade-off decides it:

- **Buffer until the item is complete**, then check once. Exact, but a text
  response then arrives all at once at the end, which defeats the reason to use
  realtime, and on a model that speaks, text deltas are the transcript of audio
  the client is already playing.
- **Check as it streams.** Latency is untouched, at the price that a match can
  be partly out before it is recognised.

Rolter streams. Each delta is checked alone, and also together with the last 512
bytes of what was already delivered for the same item, so a pattern split across
deltas is caught by the delta that completes it and that delta is withheld. The
completed text is checked again at the `.done` events as a backstop. The leak
this accepts is the first part of a pattern before the delta that completes it,
and a pattern longer than the window split across more deltas than that. An
operator who cannot accept that should not enable `post_call` rules on realtime
routes, or should serve the route's output only through the chat pipelines,
which buffer.

`redact` on a delta rewrites that delta only: a match split across deltas is
not redacted, since the first part has gone. A `redact` rule that must hold
should be paired with a `block` rule for the same entity.

### Webhook and plugins

- **Guardrail webhook** (`pre_call` stage): consulted for each in-scope client
  event with the event as its content. `block` refuses the event as above;
  `transform` replaces it, and only with an event of the same `type`, since a
  transform that changed the kind of event could carry text past the checks
  that ran on the original. Failures follow the webhook's `failure_mode`. The
  consult is awaited inline, so a slow webhook delays that session's relay for
  its timeout; the other sessions are unaffected.
- **`pre_upstream` plugins**: run on the same client events, after the
  guardrails and the webhook, with the same block/transform contract.
- **`pre_route` plugins** do not apply: the route is chosen from the URL when
  the socket is opened and there is no body to consult them on.
- **`post_response` plugins** do not apply: they act on a buffered chat-shaped
  response body, and a session has none. They are the one plugin stage that
  remains a gap.
- **The PII sanitizer** does not apply. It replaces entities with placeholders
  and restores them in the reply, which needs the response to be buffered and
  the mapping to live for one request; a session has neither. A `redact`
  guardrail rule is the way to remove an entity from client text on a realtime
  route.

While a policy applies, a binary client frame is refused with an `error` event:
the protocol is JSON text, and an upstream that read JSON out of a binary frame
would skip every check above.

### Other limits

- Function-call arguments the model emits and tool outputs it receives are
  checked only on the client side (`item.output`); streamed
  `response.function_call_arguments.*` events are not scanned.
- `conversation.item.input_audio_transcription.completed` (what the user said)
  is not scanned: the audio has already reached the model, so a block there
  could only notify.
- Each event has its own scan-byte budget (`max_scan_bytes`); an event larger
  than it is passed unscanned, as a body is on the HTTP path.

## What is not metered

These are tracked rather than silently missing:

- The PII sanitizer and `post_response`/`pre_route` plugins do not run on
  realtime events; built-in guardrails, the guardrail webhook and
  `pre_upstream` plugins do (see [Content policy](#content-policy-on-a-bidirectional-stream-1880)).
  `realtime` still carries its [stability marker](../development/stability-markers.md):
  its note has not been rewritten to match, and the dashboard's translated
  notes are the part that holds graduation back.
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
logged as unknown, turns waiting on the timer are flushed when the session
ends, an unpriced model is refused under `block`, a spent budget refuses a new
session, a session is closed when its budget runs out, a budget spent by other
traffic closes an idle session on the flush timer and, with
`usage_flush_secs = 0`, on the one-second budget tick, usage is charged while
the session continues, a session is closed when its key is disabled or its `models` are narrowed (and left alone when nothing changed), `rpm` applies per key rather than per process, and turn
tokens fill the key's `tpm` window. The Redis-backed ones read
`ROLTER_TEST_REDIS_URL` and skip, or skip their Redis assertions, without it.

Shutdown has three: an in-process drain flushes every live session before it
returns and refuses new ones with `503`, a response in flight at shutdown is
logged as unknown, and a real `rolter-gateway` child sent `SIGTERM` closes its
session with `1001` and exits `0` only after the meter has flushed. With Redis,
the first and the third route the gateway through a relay that holds every
reply back by 200ms once shutdown starts. A flush charges a budget with `INCRBYFLOAT` and
sends its `EXPIRE` only after that reply, so a budget key that carries its
expiry proves the meter ran to the end of its flush instead of merely starting
it. `Sessions` itself is unit-tested for holding the drain while a meter or an
upgrade is still running.

`crates/rolter-gateway/tests/realtime_guardrails.rs` drives real sessions
against a mock upstream that records what it is sent: a blocked client event is
refused with an `error` event and the session carries on, a blocked
`session.update` instruction never reaches the upstream, a redacted client event
is rewritten before it is forwarded, a blocked server delta is withheld and the
response cancelled, a match split across deltas is caught, a clean response is
unchanged, and the guardrail webhook refuses a client event.

The tracker's frame handling, including which deltas count as a first token, is
unit-tested in `realtime_metering.rs` itself.
