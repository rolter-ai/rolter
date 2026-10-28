# PII restoration on streamed responses: the sanitizer restores the stream

**Status:** Accepted (27 Oct 2026) · **Date:** 27 Oct 2026 · **Issues:** [#911](https://github.com/rolter-ai/rolter/issues/911), [#848](https://github.com/rolter-ai/rolter/issues/848)

## Context

The external PII sanitizer (#848, PR #910) hands request content to a
self-hosted de-identification service, forwards the placeholder-bearing body to
the provider, and can reverse the substitution on the way back. Its design rests
on one property, stated in `docs/dev-docs/architecture/security.md` and in
`docs/user-docs/security/pii-sanitizer.mdx`:

> **The placeholder→plaintext mapping never enters the rolter process**, so
> nothing in a log, metric, span or crash dump can leak it.

The gateway holds an opaque restoration token instead (`RestorationTicket` in
`crates/rolter-core/src/pii_sanitizer.rs`): it does not print, and it yields the
token only to `token_for(&scope)` with a matching org, team, project and route.
Restoration is a second call, `POST restore_url` with the token and the whole
response body, made by `pii_response_leg_apply` in
`crates/rolter-gateway/src/handlers.rs` after the cache store.

That second call needs the whole body, so a streamed request whose response leg
is active (`PiiSanitizerConfig::touches_response`: `direction` covers the
response, or the restoration policy applies to this request) is refused up front
with `400 pii_streaming_unsupported`, counted by
`rolter_pii_stream_rejections_total`. `streaming = "passthrough"` serves the
stream with the response leg skipped, so the caller gets placeholders. This
follows the same rule as output guardrails and `post_response` plugins (#1776):
a client's `"stream": true` never switches off a control the operator
configured.

Neither is satisfying for a deployment that wants streaming _and_ restoration,
which is the common shape of a chat product: the substitution exists to keep
data away from the model provider, and the end user expects to see their own
name back, token by token. #911 asks which design, if any, serves that without
giving up the property above.

### What the property does and does not cover

It is worth being exact, because every option below is judged against it.

- **Plaintext already passes through the process.** The request body arrives
  with the personal data in it and is held until the sanitizer replaces it, and
  a restored response body passes through the gateway on its way to the caller.
  The property never claimed otherwise.
- **What it rules out is a decoding table.** Today no structure in rolter pairs
  `<PERSON_1>` with `Jan Kowalski`. A crash dump taken mid-request contains
  fragments of in-flight bodies, but nothing labelled, nothing that outlives the
  request, and nothing that a stray `Debug` derive, a `tracing` field or a heap
  profile could serialize as a ready-made list of who is who.
- **It is a property of the code, not of a configuration.** No setting makes it
  false. That is what lets the docs state it flatly and what a DPIA can cite.

### The problem every streaming option shares: placeholder boundaries

A placeholder can straddle any number of SSE frames. OpenAI-style providers
send roughly one token per frame, and a tokenizer cuts `<EMAIL_ADDRESS_12>` into
five to eight tokens, so the placeholder arrives as, say, `<`, `EMAIL`,
`_ADDRESS`, `_`, `12`, `>` in six frames. The text is also not raw bytes: it
sits inside a JSON string in `choices[i].delta.content`, in Anthropic's
`content_block_delta` (`text_delta`, `thinking_delta`, and `input_json_delta`
whose `partial_json` is a JSON string _inside_ a JSON string), and in the
Responses API's `response.output_text.delta`. A provider may escape `<` as
`\u003c`. Tool-call arguments stream the same way.

So any restoring design needs, per text channel (choice index × content block ×
field), a **holdback**: decode the delta, append it to the channel's pending
text, emit everything except the longest suffix that is a prefix of some
placeholder, and keep that suffix for the next delta of the same channel. On
`finish_reason`, `content_block_stop` or end of stream the remainder is flushed
as is. The frame that carried the held text is still forwarded, with a shorter
text field, so `role`, `usage`, `logprobs` and the frame count are unchanged.

The latency cost depends on what the matcher knows:

| Matcher                                           | Text that is held                                                       | Added delay                                                                                                   |
| ------------------------------------------------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| the exchange's actual placeholders (a small trie) | only a suffix that is a prefix of a placeholder issued for this request | that span only, until its last token arrives: ~5–8 tokens, so ~50–150 ms at 50–100 tok/s; zero for other text |
| a placeholder grammar (`<[A-Z_]+_[0-9]+>`)        | every `<` and what follows until it fails to match                      | the same per placeholder, plus a short stall on every `<` in code, HTML or maths                              |

Holding the exact placeholder set is cheaper and more correct, but the
placeholder set is half of the mapping. Who holds it is part of each option.

Whichever side runs the holdback, restoration on a stream is never _more_ than
the buffered path restores: a placeholder the model mangled (`PERSON_1`,
`<Person_1>`) is not restored in either.

## Options

### 1. A restore round trip per chunk

The gateway forwards each frame's text to `restore_url` with the token and
emits what comes back.

- **Mapping:** stays in the sanitizer, as today.
- **Leak surface:** unchanged from today, except that the token crosses the
  wire once per frame instead of once per exchange.
- **Boundaries:** a per-chunk restore cannot see a placeholder split across
  chunks, so the gateway still needs a holdback — and with only the grammar to
  go on, since it has no placeholder list. The sanitizer contract would have to
  accept fragments, which Presidio-style `deanonymize` does not.
- **Latency:** the honest figure for TTFT alone is one extra round trip on the
  first frame: a few ms in the same cluster, tens of ms across zones, and up to
  `timeout_ms` (default 5 s) × (`max_retries` + 1) in the tail. That would be
  tolerable on its own. What rules the option out is the rest of the stream. If
  calls are serialised to keep order, delivery is capped at one frame per round
  trip `r`. With `r` = 10 ms (an in-cluster HTTP service doing JSON and string
  replacement) a 100 tok/s model is barely kept up with and any jitter
  accumulates; with `r` = 40 ms a 1,000-token answer that the model produced in
  10 s reaches the caller in 40 s. Pipelining the calls restores the rate but
  not the volume: one call per frame is 1,000 restore calls for that answer
  instead of one, and 200 concurrent streams at 100 frames/s are 20,000
  calls/s at the sanitizer.
- **Cancellation and backpressure:** simple. A client disconnect drops the
  stream and with it any in-flight call; a slow sanitizer simply slows the
  stream.
- **Contract change:** `restore_url` must accept a fragment and a holdback
  protocol, or the gateway's grammar matcher has to be good enough.
- **Tests:** the segmentation tests below, plus a latency bench against a stub
  sanitizer with a fixed delay.

**Rejected.** It keeps the property but makes streaming slower than not
streaming for any sanitizer that is not on the same host, multiplies the
sanitizer's load by the token count, and still needs a gateway-side holdback.

### 2. The gateway holds the mapping

The sanitizer returns the placeholder→plaintext mapping with every reversible
sanitize reply. The gateway keeps it (in memory, or in Redis so any replica can
restore) and restores both buffered and streamed responses itself; `restore_url`
goes away.

- **Mapping:** in the gateway, for the life of the exchange at least, and in a
  shared store if restoration must survive a replica hop.
- **Leak surface:** every one the property was written to exclude. A crash or
  core dump and a heap profile hold labelled pairs. A `Debug` derive, a
  `tracing` field or a panic message added later in any of the structs that
  carry it would print it, and nothing in the type system or review stops that
  from happening once. In Redis the mapping inherits Redis's persistence,
  replication and backups, which makes rolter a PII store with retention
  obligations of its own.
- **Which guarantee breaks:** the property itself, unconditionally, and with it
  the sentence in both docs. It cannot be scoped: the buffered path would use
  the same mapping.
- **Boundaries:** the cheap trie matcher, since the gateway knows the exact set.
- **Cancellation and backpressure:** trivial; restoration is local.
- **Contract change:** the reply gains the mapping; `restore_url` is unused.
- **Tests:** segmentation tests; leak tests become a permanent obligation on
  every future change to the request path.

**Rejected.** It is the design #848 was built to avoid.

### 3. A short-lived, scope-bound mapping streamed once per exchange, behind an operator opt-in

Only for a streamed request that needs restoration, and only when the operator
has explicitly opted in, the sanitizer returns the mapping _for this exchange_
alongside the token. The gateway holds it in a dedicated type for the life of
that one stream and drops it at the end. The buffered path keeps `restore_url`
and never sees a mapping.

- **Mapping:** in the gateway, for one stream (seconds to minutes, bounded by
  the response's own duration), in a type that implements neither `Debug`,
  `Display` nor `Serialize`, zeroes its memory on drop, is never cloned, and is
  scoped like `RestorationTicket` so `token_for`-style scope checks guard it.
  It never touches the cache path: a cached SSE replay is restored with the
  replaying request's own mapping.
- **Leak surface:** smaller than option 2 but not zero. A core dump or heap
  profile taken during a stream holds labelled pairs for every stream in flight;
  zeroing on drop does nothing for a dump of a live process. A future edit can
  still format it somewhere, though the missing trait impls make that a
  compile error rather than a review catch. Compared with what is in memory
  anyway — the request plaintext — the difference is **duration** (the
  request plaintext is replaced within the sanitize call; the mapping lives as
  long as the stream), **structure** (a labelled entity table rather than prose)
  and **the claim**: the property becomes "true unless
  `streaming = "restore_in_gateway"` is set", and every statement of it in the
  docs gains that condition.
- **Opt-in:** a new `streaming` value plus an explicit acknowledgement key, so
  the tradeoff is written in the config file rather than inferred, and the
  sanitizer must also agree per request (`stream_mapping: true` in the envelope,
  which a sanitizer may refuse).
- **Boundaries:** the trie matcher over the exchange's own placeholders, in the
  gateway. Cheapest latency of any option that restores.
- **Cancellation and backpressure:** trivial. The mapping is dropped with the
  stream on a client disconnect.
- **Contract change:** a `stream_mapping` flag on the sanitize request and a
  `mapping` object on the reply; nothing else.
- **Tests:** the segmentation tests; a test that runs a restoring stream with
  `tracing` at `TRACE` into a capturing subscriber and with payload capture on,
  and asserts that no mapped plaintext appears in any log line, span field,
  metric or the captured payload; a compile-fail test that the mapping type has
  no `Debug`/`Serialize`; a test that the mapping is dropped when the client
  disconnects mid-stream.

**Not recommended.** It is coherent and cheap, and a maintainer could
reasonably choose it. But it turns a property of rolter into a property of one
deployment's configuration, and option 4 below delivers the same feature
without doing so.

### 4. The sanitizer restores the stream

The gateway pipes the upstream stream's text through the sanitizer, which
restores it and returns it, so the mapping and the placeholder set stay outside
rolter. The gateway keeps the parts it already owns: SSE parsing, dialects,
usage and billing, framing.

- **Shape of the pipe.** Sending raw SSE to the sanitizer would make every
  sanitizer understand three or more streaming dialects and their JSON
  escaping. Instead the gateway decodes each frame's text fields and sends the
  sanitizer a stream of text deltas, one NDJSON line per delta:
  `{"seq": 17, "channel": "0/content", "text": "_ADDRESS"}`, then
  `{"close": "0/content"}` on `finish_reason` / `content_block_stop`, then
  `{"end": true}`. The sanitizer answers each input line, in order, with exactly
  one line, `{"seq": 17, "text": "", "held": 9}` — the restored text to emit for
  that delta and how many input characters it is holding back. A held prefix
  comes out later, restored, in the reply to a later `seq` of the same channel,
  or in the reply to `close`. Because replies are one-to-one with inputs, the
  gateway re-frames trivially: frame 17 goes out with its text field replaced by
  reply 17's text and everything else in the frame untouched.
- **Mapping:** in the sanitizer, as today. The gateway holds the token for one
  more request and sees only restored text, which it delivers anyway.
- **Leak surface:** unchanged from the buffered path: restored text passes
  through the gateway on its way to the caller, exactly as a restored buffered
  body does today. No mapping, no placeholder set, no new log or metric field.
  The text deltas are not logged; the pipe is subject to the same
  `EgressClient` checks as `url` and `restore_url`.
- **Boundaries:** the sanitizer runs the holdback with the trie over the
  exchange's own placeholders, so the latency is the cheap row of the table:
  ~50–150 ms on a placeholder span, none elsewhere. The gateway adds one
  network hop per delta, but pipelined over one long-lived connection rather
  than one request per delta: on an HTTP/2 stream in the same cluster that is
  well under a millisecond of transit plus the sanitizer's own per-line cost.
  TTFT grows by one connection setup (once per exchange, reusable from a pool)
  plus one line's round trip.
- **Transport:** a single full-duplex exchange per stream, a `POST` to
  `restore_stream_url` with a streamed request body and a streamed response
  body. This needs HTTP/2 (or a server that reads and writes an HTTP/1.1 body
  concurrently, which many proxies do not); a WebSocket is the fallback if it
  does not.
- **Backpressure:** a bounded window of unanswered deltas (by count and bytes).
  When the window is full the gateway stops polling the upstream, which pushes
  back to the provider through TCP exactly as a slow client does today. A slow
  sanitizer therefore slows the stream rather than growing a buffer.
- **Cancellation:** a client disconnect drops the response body, which drops
  the sanitizer stream (an HTTP/2 `RST_STREAM`) and the upstream request. The
  sanitizer must discard the exchange's holdback state on reset; its token TTL
  covers a sanitizer that does not. The upstream accounting is unchanged:
  `UsageLoggingStream` taps the **upstream** bytes, not the restored ones, so
  billing follows the provider (#1478) and a disconnect is logged as one
  (#1083).
- **Failure:** a restore failure is never fatal, as on the buffered path, and
  the worst case stays "placeholders instead of plaintext, never the reverse".
  If the pipe cannot be opened, the stream is served unrestored. If it fails
  mid-stream (error, reset, or no reply within `timeout_ms`), the gateway still
  has the original text of every unanswered delta and the `held` count of the
  last answered one, so it emits the held tail and the remaining deltas with
  their placeholders and continues unrestored. `rolter_pii_restore_errors_total`
  counts it. No retry mid-stream: replaying deltas into a new pipe would need
  the sanitizer to resume holdback state it has discarded.
- **Contract change:** a new, optional endpoint (`restore_stream_url`) and the
  NDJSON protocol above. A sanitizer that does not offer it is unaffected:
  `streaming` stays `reject` or `passthrough` for it. Presidio has no such
  endpoint, so the reference integration needs a small shim that wraps
  `deanonymize` with the holdback; rolter would have to document or ship one.
- **Tests:** the segmentation tests; a stub sanitizer speaking the NDJSON
  protocol with configurable latency, stall, mid-stream error and reset; a
  disconnect test asserting the stub sees the reset and the upstream is
  dropped; a backpressure test asserting the gateway stops reading the
  upstream while the stub stalls; a mid-stream failure test asserting the
  caller gets restored text up to the failure and placeholders after, with
  nothing duplicated or lost; a TTFT and inter-token bench against the stub at
  0, 1 and 10 ms per line.

### 5. Keep the refusal and document it

Nothing changes. A streamed request with an active response leg gets
`400 pii_streaming_unsupported`, or placeholders under `passthrough`.

- **Mapping, leak surface, contract:** unchanged.
- **Boundaries, cancellation, backpressure:** not applicable.
- **Cost:** a chat product that needs restoration must turn streaming off for
  those requests, and with `caller_authorized` it can at least do so per
  request. The docs already state the refusal; they would gain the reasoning
  from this ADR.
- **Tests:** the existing streaming-mode integration tests.

Coherent, and the right default whatever else is chosen, but it leaves #911's
use case unserved.

### Tests every restoring option needs

Whichever option is built, its correctness test is the same and should come
first: take a response whose buffered-path restore is known, cut it into every
frame segmentation that matters — one character per frame, a cut inside every
placeholder at every offset, cuts inside `\u003c` escapes and inside multi-byte
UTF-8 — and assert that the text the client reassembles equals the buffered
restore. Run it over OpenAI chat (`n > 1`, tool-call `arguments`), Anthropic
messages (`text_delta`, `thinking_delta`, `input_json_delta`, where restored
plaintext containing `"` must be re-escaped twice), and the Responses API, on
a live stream and on a cached SSE replay.

## Comparison

| Option                     | Mapping lives                       | Property                      | Boundary latency                 | Sanitizer load          | New contract               |
| -------------------------- | ----------------------------------- | ----------------------------- | -------------------------------- | ----------------------- | -------------------------- |
| 1. per-chunk round trip    | sanitizer                           | kept                          | grammar holdback + `r` per frame | one call per frame      | fragment restore           |
| 2. gateway holds it        | gateway (and Redis), every exchange | **broken**, unconditionally   | trie, cheapest                   | none for restore        | mapping on every reply     |
| 3. per-exchange, opt-in    | gateway, one stream, opted-in only  | **conditional** on the opt-in | trie, cheapest                   | none for restore        | mapping on a flagged reply |
| 4. sanitizer restores pipe | sanitizer                           | kept                          | trie, plus one pipelined hop     | one stream per exchange | streaming restore endpoint |
| 5. refuse                  | sanitizer                           | kept                          | n/a                              | none                    | none                       |

## Decision

Accepted on 27 Oct 2026 as proposed below, and built in #911. As built, the
`close` and `end` lines carry a `seq` like the deltas, so every reply is matched
by its `seq`; a delta whose text is JSON source (tool-call arguments,
`partial_json`) carries `"format": "json"`, under which a restored value is
written JSON-escaped. The contract is specified in
`docs/user-docs/security/pii-sanitizer.mdx` and the gateway side is described in
`docs/dev-docs/architecture/security.md`. The reference shim for Presidio is not
part of it. It ships as `examples/presidio-restore-shim/` (#3339, decided
below), and the TTFT and inter-token bench is tracked in #3340.

1. **Keep option 5 as the default.** `streaming = "reject"` stays the default
   and `passthrough` stays available. Nothing changes for an existing
   deployment.
2. **Build option 4 as the opt-in.** Add `streaming = "restore"` and
   `restore_stream_url`; it applies only to restoration. A config whose
   `direction` covers the response still refuses or passes a stream through,
   because sanitizing what a provider generates means entity recognition over
   a stream, which needs context the holdback does not give and is a separate
   question.
3. **Reject options 1 and 2.** Option 1 for its inter-token rate and call
   volume, option 2 for breaking the property outright.
4. **Do not build option 3 now.** Keep it on record as the fallback if option 4
   proves impractical for the reference engine. If it is ever chosen, it needs
   all of: the explicit acknowledgement key, a per-request sanitizer
   agreement, the no-`Debug`/no-`Serialize`/zero-on-drop type, no path to the
   cache, and every statement of the property in the docs amended to name the
   condition.

## Consequences

- The property stays a property of rolter. Both doc pages keep their sentence
  unconditionally.
- The sanitizer becomes part of the streaming critical path when `restore` is
  set: its per-line latency adds to the stream, and its availability decides
  whether the caller sees names or placeholders. The failure mode is the safe
  one, and `rolter_pii_restore_errors_total` is the signal.
- rolter has to specify, document and test a second sanitizer endpoint, and an
  operator on Presidio needs a shim. rolter ships one as a reference
  (see the decision below), not as a supported product.
- The gateway gains an SSE text-field decoder and re-framer for each streaming
  dialect. `rolter-proxy`'s `SseConverter` already parses the upstream
  dialects it translates, so much of this is reuse rather than new parsing.
- Placeholder holdback adds roughly 50–150 ms to the span of a placeholder and
  nothing elsewhere. That is the cost of restoration on a stream under any
  option, and the one users will notice.

## Implementation, if accepted

Not part of this record; listed so the follow-up issues are sized from it.
The segmentation test harness first, against a stub; the NDJSON contract and
its user-docs section; the gateway pipe with window, cancellation and failure
handling; the `streaming = "restore"` config value through `rolter-core`,
`rolter.example.toml`, `config-file.mdx` and the snapshot; the reference shim
decision; and the dev and user docs replacing the "streaming is refused"
sections.

## Reference shim (#3339)

rolter ships the Presidio shim as a reference service rather than leaving it to
a copy-paste block in the docs: `examples/presidio-restore-shim/`, a small
Starlette service with its own tests. It wraps Presidio's `DeanonymizeEngine`
with the per-channel holdback (a trie over the exchange's own placeholders),
escapes restored values on `"format": "json"` channels and discards an
exchange's state when it ends or is reset. It is code to read and adapt, not an
image or a supported release: the mapping store behind it is an interface
(`MappingStore`) because it has to be the store the operator's sanitize service
already writes to, which rolter cannot know. Its test drives it over a socket
with the gateway's own NDJSON exchange, one line out and one reply in, so a
shim that buffered would hang the test. The shim never logs a text delta, a
restored value or a mapping.

It is plain Starlette rather than FastAPI because the endpoint has no request
or response model to validate and has to read the request body while it writes
the reply, which a response class that listens for the client's disconnect
cannot do (it takes body messages from the same receive channel).
