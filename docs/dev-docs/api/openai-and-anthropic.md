# API surface

rolter speaks the OpenAI and Anthropic HTTP APIs so existing SDKs work unchanged — point them at the gateway base URL and use a rolter virtual key.

What this surface guarantees across releases is the _dialect_, not a frozen schema: the `v1` in the path is OpenAI's, so there is no `/v2/` to move to, and following an upstream dialect change is not treated as a rolter breaking change. The rolter-owned parts — virtual-key auth, model addressing, the error envelope, `GET /v1/models` — are held stable on the same terms as the control API. See [ADR-0032](../adr/2026-09-09-one-point-oh-compatibility-guarantees.md).

## Authentication

- OpenAI-style: `Authorization: Bearer <virtual-key>`
- Anthropic-style: `x-api-key: <virtual-key>`

When no virtual keys are configured the gateway runs open (useful for local dev).

## Playground decision discovery

`GET /v1/models` adds `rolter_capabilities: { decisions, chat }` to each resolved
address, including provider pins and groups. The values reuse the System One
endpoint-refusal policy rather than guessing from names. The built-in `fake-llm`
supports both unless a configured route shadows it. Playground filters Decisions
on explicit support and excludes decision-only addresses from Chat. The Decisions
panel preserves JSON state and question ids, sends OpenJev pixels in `image_data`,
and tracks/cancels requests through the shared gateway client.

## Endpoints (v1)

The served OpenAPI document includes `GET /v1/realtime` with its required
`model` query parameter, WebSocket upgrade response and pre-upgrade admission
failures.

| Method            | Path                             | Notes                                                                                                |
| ----------------- | -------------------------------- | ---------------------------------------------------------------------------------------------------- |
| POST              | `/v1/chat/completions`           | OpenAI chat; streaming via `"stream": true` (SSE)                                                    |
| POST              | `/v1/completions`                | OpenAI legacy completions                                                                            |
| POST              | `/v1/responses`                  | OpenAI Responses; provider-native passthrough, streaming supported                                   |
| GET, DELETE       | `/v1/responses/{id}`             | retrieve or delete a tenant-scoped native Responses resource                                         |
| POST              | `/v1/responses/{id}/cancel`      | cancel a tenant-scoped native Responses resource                                                     |
| GET               | `/v1/responses/{id}/input_items` | list input items for a tenant-scoped native Responses resource                                       |
| POST              | `/v1/messages`                   | Anthropic Messages; streaming supported                                                              |
| POST              | `/v1/embeddings`                 | OpenAI embeddings; non-streaming                                                                     |
| POST              | `/v1/rerank`                     | Cohere/Jina rerank; non-streaming; documents reshaped for an `infinity` target                       |
| POST              | `/v1/systemone`                  | System One typed decisions, forwarded unchanged to a decision server; non-streaming (ADR-0035)       |
| POST              | `/v1/images/generations`         | OpenAI image generation; non-streaming                                                               |
| POST              | `/v1/audio/speech`               | OpenAI text-to-speech; binary audio response                                                         |
| POST              | `/v1/audio/transcriptions`       | OpenAI speech-to-text; `multipart/form-data` upload                                                  |
| POST              | `/v1/audio/translations`         | OpenAI audio translation; `multipart/form-data` upload                                               |
| GET               | `/v1/realtime?model=…`           | OpenAI-compatible Realtime API; WebSocket relay, metered per response turn — see below               |
| GET, POST, DELETE | `/mcp/{server}/{path…}`          | authenticated Streamable HTTP/SSE MCP proxy                                                          |
| GET               | `/v1/models`                     | lists route names, provider-pinned and group addresses (see [Model listing](#model-listing))         |
| GET               | `/openapi.json`                  | OpenAPI 3.1 description of this request surface (self-contained, no external assets)                 |
| GET               | `/docs`                          | interactive Scalar API reference (assets embedded in the binary — works air-gapped)                  |
| GET               | `/`                              | service-info landing (version + links to docs/openapi/health)                                        |
| GET               | `/healthz`                       | liveness — process is up, no dependency checks                                                       |
| GET               | `/readyz`                        | readiness — `503` while draining (see [Health & readiness](../architecture/health-and-readiness.md)) |
| GET               | `/metrics`                       | Prometheus exposition                                                                                |

The document's request and 200-response bodies name `components/schemas` entries (#3335), built in `crates/rolter-gateway/src/openapi_schemas.rs`. Each lists the fields rolter reads or guarantees and leaves `additionalProperties` open, because bodies are forwarded; a field it does not name still reaches the upstream. The streaming schemas (`ChatCompletionChunk`, `MessageStreamEvent`, `ResponseStreamEvent`) describe only the event families rolter forwards or produces by translation. A schema's provenance comes from the record that lists it under `gateway_schemas` in `crates/rolter-core/specifications.toml`; a new schema needs a record, or the `model_endpoint_bodies_reference_open_schemas_with_provenance` test fails.

## Request header forwarding

By default the gateway forwards trace context and nothing else. An operator can
name additional client headers to forward with the `forwarded_headers`
allowlist in the client policy.

**`anthropic-*` headers are the exception: they forward by prefix, not by
allowlist entry** (#1013). Any inbound header whose name begins with
`anthropic-` is passed unchanged to an Anthropic-dialect upstream, whether or
not anyone has heard of it.

This is not a convenience. Anthropic extends its protocol _through headers_: a
capability pairs an `anthropic-beta` value with body fields, and the pair
travels together. An allowlist is a closed list against a vendor that ships new
capabilities as new header values, so every new beta would silently break until
an operator added it by name — and the failure is not a graceful downgrade.
Stripping the header while the body passes produces a hard `400`
(`Extra inputs are not permitted`), while capabilities like extended context go
silently unavailable because the upstream never sees the request for them.

Two rules bound it:

- The match requires the separator (`anthropic-`), so a header merely starting
  with those letters is not in the namespace.
- These headers only reach a provider that speaks the dialect. They are
  collected before the balancer picks a target, so the proxy drops them again
  for an upstream that has no use for them.

### `anthropic-version`

The client's value wins when it sends one; the configured
`compatibility.anthropic_version` is the fallback. That pin exists for
_translated_ requests — an OpenAI-dialect call rewritten to Anthropic, where no
client version exists — and that is still exactly what it does. Overriding a
version a native client did state would silently change the protocol it asked
for. Exactly one value is sent either way.

## Realtime WebSocket

Connect with the usual gateway bearer key and the public route model as a query parameter:

```text
wss://gateway.example.com/v1/realtime?model=gpt-realtime
```

rolter authenticates and selects an upstream before accepting the client upgrade, then pins that upstream and its selected provider key for the session. Text, binary audio and WebSocket control frames are relayed in both directions without application-level buffering. If the upstream drops, the client must reconnect; rolter does not fail a live session over to another target because replaying audio or tool events is unsafe.

The WebSocket-first implementation supports the OpenAI Realtime event stream, including `session.update`, `input_audio_buffer.*`, `response.*`, and function-call events. WebRTC/browser ephemeral-token handoff is not exposed by the gateway yet.

A session is admitted through the same budget, `unpriced_policy` and rate-limit checks as an HTTP request, on the same key, project, team and org scope, and a refusal is the HTTP status and error body a chat request would get (`402 insufficient_quota`, `402 model_unpriced`, `429 rate_limit_exceeded`). Opening a session takes one `rpm` slot.

Once open, the session is metered per response turn: each `response.done` event's `usage` becomes one `request_logs` row, priced and attributed like an HTTP row, and its cost and tokens are added to the scope's budgets and `tpm` windows every `[realtime] usage_flush_secs`. When a budget runs out mid-session the client receives an `error` event with code `insufficient_quota`, then a `1008` close. A gateway shutting down closes each live session with `1001` (going away) after metering it, and refuses new upgrades with `503`. [Realtime metering](../architecture/realtime-metering.md) has the full model, including how a response cut short is recorded. Guardrails and plugins do not run on realtime events yet (#1880).

## MCP gateway

`/mcp/{server}` and any path beneath it proxy an organization-scoped MCP
server registered in the control plane. The caller supplies a normal rolter
virtual key. Unlike LLM routes, MCP calls require a database-backed key minted
by a user: that owner must have a live, unrevoked and unexpired OAuth session
for the named server, and its scopes must cover the server's
`required_scopes`. The gateway fails closed before connecting when any owner,
server or scope check fails.

The caller's virtual key is never forwarded. Rolter replaces it with the
session's OAuth bearer token and preserves end-to-end MCP headers, body,
query, status and response stream. Streamable HTTP and legacy SSE registrations
use this path; stdio and WebSocket registrations currently return
`mcp_transport_unsupported` rather than bypassing authorization.

A server that cannot be reached, or does not answer within its budget, is a
`502` with code `mcp_upstream_error` and the fixed message `MCP upstream request
failed` (#2919). The registered URL often holds the server's token, so the
cause, with its URLs masked, is written to the gateway log with the server slug
and the request id instead of being echoed. The tool-call log row records the
category only (`timeout` or `transport_error`).

## Routing

The `model` field in the body selects a **route**. The route's strategy picks a target; rolter rewrites `model` to the target's upstream model id and forwards with the provider's credentials. Session affinity uses `x-session-id` when present.

When the selected upstream speaks the other chat protocol, rolter translates
OpenAI Chat Completions and Anthropic Messages in both directions. Translation
includes system/developer instructions, sampling and stop parameters, function
tools and tool results, token usage, finish reasons, and live SSE events. Image
and document inputs retain URL, base64 media type/data, and file references.
Blocks with no equivalent in the target protocol (for example OpenAI input
audio sent to an Anthropic Messages upstream) are preserved as opaque content
blocks; the target may reject them rather than rolter silently dropping data.

### Parameters translation did not forward as sent

A translator that cannot carry a caller parameter to the upstream dialect says
so instead of discarding it silently (#1586). Every adapter in
`crates/rolter-proxy/src/translation.rs` records into one
`rolter_proxy::ParamAdjustments` (`adjustments.rs`), an entry per parameter with
a `Disposition`: `dropped` (no equivalent, not sent), `clamped` (sent within
the upstream's range: Chat → Anthropic clamps `temperature`, see below) or `translated` (sent as a lossy
approximation, such as a forced-function `tool_choice` sent to Gemini as mode
`ANY`, an Anthropic `tool_choice` carrying `disable_parallel_tool_use` sent to
Gemini or Interactions, which have no such switch, or a tool list with
non-function tools). A rename that keeps the meaning
(`max_completion_tokens` → `max_tokens`, `stop` → `stop_sequences`) is not an
adjustment. What each adapter reports:

| Pair                         | Reported                                                                                                                                                                                |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Chat → Anthropic             | `n`, `presence_penalty`, `frequency_penalty`, `logprobs` dropped; `max_completion_tokens` dropped when `max_tokens` is also set; `temperature` clamped when above 1                     |
| Anthropic → Chat             | nothing: `tool_choice.disable_parallel_tool_use: true` is carried as top-level `parallel_tool_calls: false` (#3325), and `false`, or `true` on a request with no tools, leaves it unset |
| Responses → Chat / Anthropic | `background`, `store`, `previous_response_id`, `reasoning`, `text` dropped, plus the Chat → Anthropic row for Anthropic                                                                 |
| any → Gemini generate        | every known parameter the translator does not map (`top_k`, `seed`, `response_format`, …); `tool_choice` and `tools` as above                                                           |
| any → Gemini Interactions    | as Gemini, except that `store` and `previous_response_id` are carried rather than dropped                                                                                               |

`temperature` is the one clamp (#3324, the maintainer's decision). OpenAI's Chat
Completions and Responses accept 0 to 2 and Anthropic Messages 0 to 1, so a
caller routed to Anthropic with `temperature: 1.3` used to get Anthropic's 400
for a request valid in the dialect it speaks. `clamp_temperature` sends 1
instead and records `temperature=clamped`; a value at or below 1, or one that is
not a number, is forwarded as sent for the upstream to judge. Scaling (`t / 2`)
was weighed and rejected, since it would change every request's temperature,
including the ones Anthropic accepts as they are. Responses → Anthropic goes
through the same Chat → Anthropic translator and clamps the same way.

Gemini and Interactions build their body afresh, so the parameters left over in
the source object are the dropped ones. Only names from `KNOWN_PARAMS` are
recorded, so a key the caller invented never reaches a header or a log row, and
`model`, `stream` and `stream_options` are left out because rolter carries or
answers them itself.

The record is made at request-translation time:
`TranslationPlan::translate_request_reporting` returns it beside the body, and
`Forwarder::forward_json_with` inserts it into the upstream `reqwest::Response`'s
extensions when it is not empty. The gateway takes it off the response before
reading a byte, puts it on `RequestLog::param_adjustments`, and
`DecisionHeaders` renders it as `x-rolter-param-adjustments:
top_k=dropped, tool_choice=translated` (an RFC 8941 dictionary of tokens) on the
streamed, buffered and cached-store paths alike. The log row writes
`<disposition>:<param>` entries to `request_logs.param_adjustments`
(`clickhouse/022_param_adjustments.sql`, `Array(String)`, serialized only when
non-empty, so a ClickHouse without the column keeps accepting every other row).
A response served from the response cache did not go through translation, so
the store keeps the record instead (#3326): `CachedResponse::param_adjustments`
holds the miss's `log_entries()` (`#[serde(default)]` and skipped when empty, so
an entry an older gateway stored reads as nothing adjusted and an unadjusted one
is byte for byte what it was), and `cached_response` rebuilds it with
`ParamAdjustments::from_log_entries`, which keeps only `<disposition>:<known
parameter>` entries, onto the hit's log row and its header. The miss path does
no extra work beyond rendering the entries it already writes to the log row. A
semantic hit replays the adjustments of the request that produced the stored
answer, which is what that answer was generated under.

The common path pays nothing: a plan that does not translate returns an empty
record, an empty `Vec` does not allocate, nothing is inserted into the response
extensions, and `DecisionHeaders` builds no header string. Under ADR-0032 the
header is a rolter-specific response header whose name and grammar are
promised; the parameters it names and the dispositions it uses are an open
vocabulary, so a new disposition is additive, not breaking. Tests: the
`reported` cases in `translation.rs`, `adjustments.rs`, and the
`param_adjustments_*` cases in `crates/rolter-gateway/tests/integration.rs`.

### Token usage and the prompt cache across dialects

A translated answer carries the upstream's prompt-cache split in the usage
object the client's own dialect defines (#2863). The dialects disagree on where
the cached share sits, so the same figures read differently on each side:

| Client dialect                            | Prompt total                       | Cached share                                                            |
| ----------------------------------------- | ---------------------------------- | ----------------------------------------------------------------------- |
| Chat Completions (`/v1/chat/completions`) | `prompt_tokens`, cache included    | `prompt_tokens_details.cached_tokens`                                   |
| Responses (`/v1/responses`)               | `input_tokens`, cache included     | `input_tokens_details.cached_tokens`                                    |
| Messages (`/v1/messages`)                 | `input_tokens`, **cache left out** | `cache_read_input_tokens`, and `cache_creation_input_tokens` for writes |

So an Anthropic upstream that reports `input_tokens: 400`,
`cache_read_input_tokens: 1500` and `cache_creation_input_tokens: 100` reaches a
chat client as `prompt_tokens: 2000` with `cached_tokens: 1500`, and a chat
upstream that reports `prompt_tokens: 2000` with `cached_tokens: 1500` reaches
a Messages client as `input_tokens: 500` with `cache_read_input_tokens: 1500`.
Gemini's `cachedContentTokenCount` (Interactions: `total_cached_tokens`) sits
inside its prompt count, like Chat Completions.

Chat Completions and Responses have no field for tokens written to the cache, so
a write count travels as `cache_write_tokens` beside `cached_tokens` in the same
details block (OpenRouter's spelling). A chat upstream that names its writes
differently is read under its own name and re-emitted as `cache_write_tokens`:
Qwen's `prompt_tokens_details.cache_creation_input_tokens` and vLLM's
`created_cache_tokens` (#2879). The cache fields appear only when the
upstream reported them: a provider without prompt caching gets none added, and
no zero is stated that the provider never said. Anthropic splits its writes by cache lifetime in `usage.cache_creation` (`ephemeral_5m_input_tokens`, `ephemeral_1h_input_tokens`), and the gateway prices them at different rates (#2891). A chat or Responses client served by an Anthropic upstream is shown the 1 hour share as `cache_write_1h_tokens` beside `cache_write_tokens` (a rolter spelling, within the write count, present only when the upstream split its writes), and a Messages client served by an upstream that stated one is shown the `cache_creation` object again, its two shares adding up to `cache_creation_input_tokens`. On a streamed Anthropic answer
the closing chunk carries the input side from `message_start` together with the
output side from `message_delta`.

The thinking share of the completion follows the same rule (#2881). The
OpenAI dialects state how much of the completion was reasoning, inside the
completion count, so a chat or Responses client served by a thinking model
(Gemini, or a chat upstream that reports the split) is shown it, and the
Messages dialect, which has no field for it, is not:

| Client dialect                            | Completion total                  | Reasoning share                              |
| ----------------------------------------- | --------------------------------- | -------------------------------------------- |
| Chat Completions (`/v1/chat/completions`) | `completion_tokens`, reasoning in | `completion_tokens_details.reasoning_tokens` |
| Responses (`/v1/responses`)               | `output_tokens`, reasoning in     | `output_tokens_details.reasoning_tokens`     |
| Messages (`/v1/messages`)                 | `output_tokens`, reasoning in     | not stated                                   |

A Gemini answer with 5 candidate tokens and 20 thinking tokens reaches a chat
client as `completion_tokens: 25` with `reasoning_tokens: 20`: the 20 are counted
into the 25 once, never again. A body whose provider reported no thinking gets no
details block. xAI, which states its reasoning tokens beside `completion_tokens`
and only in its `total_tokens`, is folded into the completion first (#2888).

A chat client served by Gemini's OpenAI-compatible endpoint (`kind = "gemini"`), which states its thinking in `total_tokens` alone, is shown the same completed `usage` although its body is otherwise passed through, buffered or streamed (#2926). Only a body or SSE event that contains `total_tokens` is parsed, and only one whose total is beyond its prompt and completion is rewritten; every other chat provider's bytes reach the client untouched.

The request log and the budgets read the same body the client receives, so a
priced model is billed the cached share at `cached_input_per_mtok` and the
written share at `cache_write_per_mtok` whichever dialect the client spoke; see
[Observability](../architecture/observability.md).

The Gemini dialects — `generateContent` and Interactions — are the exception:
their wire formats are typed part unions with no opaque carrier, so an
unrecognized part cannot be passed through. Rather than drop it, rolter fails
the request closed with `400 unsupported_content_part`, naming the part type
and the upstream dialect:

```json
{
  "error": {
    "message": "the interactions upstream has no equivalent for content part type 'input_audio'; remove it or route this model to a provider that accepts it",
    "type": "invalid_request_error",
    "code": "unsupported_content_part",
    "param": "messages"
  }
}
```

This applies to every client dialect (Chat Completions, Messages, Responses) and
covers new part types a client SDK starts sending. Only `text`/`input_text` and
`image_url`/`input_image` have Gemini equivalents today; route models that need
other modalities to a provider whose dialect carries them.

## System One decisions

`POST /v1/systemone` carries typed decisions: a `model`, a `state` (string,
object or array) and a map of `questions` keyed by caller-chosen ids, each with
a `type` (`choice`, `score`, `noul`, or whatever else the model supports),
`instructions` and usually `criteria`. The answer has one entry per id under
`answers`, plus `usage.input_tokens` and `usage.output_tokens`.

The request runs the same pipeline as `/v1/rerank` and is never translated.
The target's upstream model replaces `model`, the body is otherwise forwarded
as sent, and the response comes back as the upstream wrote it. Usage is read
from the response, so cost follows the model's price row; TypeSafe's Jev bills
input only, which is a row with `output_per_mtok = 0`.

Which routes answer which endpoint follows the provider kinds behind them
(ADR-0035, `crates/rolter-gateway/src/system_one.rs`):

- a decision for a route none of whose targets serves decisions
  (`system_one`, `openai_compatible`, `ollama`) is refused with
  `400 unsupported_endpoint`;
- any other endpoint for a route whose targets are all `system_one` is refused
  the same way, naming `/v1/systemone`;
- any endpoint but `/v1/embeddings` for a route whose targets are all
  text-embeddings servers (`tei`, `infinity`; `ProviderKind::embeddings_only`)
  is refused the same way, naming `/v1/embeddings`, without calling the
  upstream (#3247), and so is `/v1/rerank` unless one of them is `infinity`,
  the only one with a rerank mapping; this also turns the route's
  `rolter_capabilities.chat` in `GET /v1/models` to `false`;
- a route that mixes decision and chat targets balances as usual.

Pre-call guardrails scan every string in `state` and in each question's
`instructions` and `criteria`; ids, option keys and `type` are never rewritten.
The built-in `fake-llm` model answers decisions deterministically.

## Authentication errors

A missing, unknown, disabled or expired virtual key all answer `401` with `type: authentication_error` and `code: invalid_api_key`, as OpenAI does (OpenAI uses the same code for a missing key, with a different message; so does rolter). Unknown, disabled and expired read alike so a caller cannot probe which happened. The realtime close sends the same code in its `error` event (#1881). Every dialect, including `/v1/messages`, gets this one envelope: rolter does not emit Anthropic's `{"type":"error"}` shape, so there is no separate Anthropic field to set.

## Upstream failures

When a request ends because the upstreams failed, the answer says which kind of failure it was and names the last upstream status (#2807). The envelope is the same for every dialect, `/v1/messages` included.

| Situation                                                                         | Status      | `type`             | `code`                                         |
| --------------------------------------------------------------------------------- | ----------- | ------------------ | ---------------------------------------------- |
| every target tried answered `429`, none left to fail over to                      | `429`       | `rate_limit_error` | `upstream_rate_limited`                        |
| every target tried failed with another retryable status (`408`, `5xx`), none left | `503`       | `overloaded_error` | `upstream_unavailable`                         |
| the last attempt never got a response (refused, reset, timed out)                 | `502`       | `overloaded_error` | none                                           |
| the provider's queue is full, timed out or dropped the request                    | `429`/`503` | by status          | `queue_full`, `queue_timeout`, `queue_dropped` |

The message of the `502` is the transport error with every URL in it masked (`rolter_core::redact::redact_urls_in_text`), so a credential-named query value or userinfo in a provider's `api_base` never reaches the caller (#2919); the same masked text is the request-log row's `error`, and a body that stops arriving after a `200` (`upstream response body could not be read`) gets the same treatment in both places. See [What an upstream failure says](../architecture/security.md#what-an-upstream-failure-says-2919) for what masking does not catch.

A `429` carries the upstream's `Retry-After` when it sent one. The message names the status and the model (`every upstream target for model 'x' is rate limited (last upstream status 429)`) and never the upstream's own words, which are in the request-log row (`error`, with `upstream_status` and `attempts` beside it; see [Observability](../architecture/observability.md#request--cost-logs)). When the retry budget runs out while an untried target remains, the upstream's own response is handed to the caller as it was. The loop does not back off before an attempt that cannot happen (#2835): when the failed attempt used the last untried target it ends the request there, so a single-target route whose upstream answers `429` with `Retry-After: 30` returns that `429` at once instead of after 30 seconds. The wait is kept wherever a next attempt exists, a sibling target and the multi-key path included. An earlier gateway answered all of these with `503 no target selected`, which points at routing config when the cause was an upstream that said no.

## Model listing

`GET /v1/models` answers with three kinds of id, filtered to what the caller's
virtual key may reach. A key minted in the store sees only its own org's
routes, providers and groups, the same boundary the route authorization
contract enforces on a call (#1844). Names are per org (#1857), so two orgs may
each list a `gpt-4o`; a key lists the one its name resolves to, its own org's
before a global one. A key from the gateway's config file carries no org: it
sees global rows and every org row whose name only one org uses, while a name
two orgs share (with no global row of it) is unknown to it, on a call and in
the listing:

| Id                    | `owned_by`          | Where it comes from                                                                                                  |
| --------------------- | ------------------- | -------------------------------------------------------------------------------------------------------------------- |
| a route name (`chat`) | `rolter`            | every configured route, plus the built-in `fake-llm` unless a route the key can use shadows it                       |
| `provider-slug/model` | the provider's name | the upstream models the provider's routes name, **plus** the catalogue the provider reported to its model-list probe |
| `group-slug/model`    | the group's name    | the union of its member providers' models (a member with an explicit model rewrite contributes that one)             |

A route's entry also carries `capabilities`, `model_type` and `context_length`
when the route sets them (`handlers.rs`, `add_model_metadata`; #2939). They are
advertised, never enforced. `description` is never served: it is free text
written for the team and may hold internal routing notes. Provider-pinned and
group addresses have no route, so they carry none of these.

The provider-pinned half used to be derived from route targets alone, which
under-reported a fleet: `provider-slug/model` resolves for **any** model the
provider serves (ADR-0017), so a provider with five models behind one route
answered `200` for all five but listed two. A client that builds a model picker
from `/v1/models` could not offer the rest (#1647).

The catalogue comes from the provider's free model-list probe
(`rolter_core::probe::probe_request`). For every provider kind whose liveness
endpoint is a model list — everything except TEI — the response is parsed
instead of discarded, and the model ids are cached per provider. Two writers
fill the cache: a dedicated refresher (`model_catalog::spawn_refresher`) that
runs whether or not active health checks are on (#1729), and the health sweep,
which parses the body it already fetched while `[health] enabled = true`. The
refresher always probes the kind's default path, so an overridden `health.path`
does not stop it.
The probe carries the provider's key, presented the way the provider kind
expects it (`x-api-key` for Anthropic, a bearer token for OpenAI, and so on; see
[How the provider key is presented](../deployment/configuration.md#how-the-provider-key-is-presented)).
An `openai` or `anthropic` provider answers a keyless model list with `401`,
which still counts as healthy but has no catalogue to read, so these two kinds
listed no extra models until the sweep sent their key (#2806).

An id is listed **whole**. Infinity and vLLM report Hugging Face ids such as
`BAAI/bge-small-en-v1.5` and refuse the bare `bge-small-en-v1.5`, so the listing
is `provider-slug/BAAI/bge-small-en-v1.5` and `resolve_pinned` (which splits at
the first slash) sends `model = "BAAI/bge-small-en-v1.5"` upstream (#3245). Only a
google resource name loses its `models/` prefix. `rolter_core::probe::catalogue_names`
still cuts to the last segment; the control plane's model check uses it to match
names, never to build an address.

Two consequences follow:

- **The wider listing does not depend on `[health]`.** The refresher runs every
  `[health] catalogue_interval_secs` (default `900`; `0` turns it off, leaving
  only the sweep). It works for a config-file-only deployment. A provider with
  no catalogue yet is fetched within 30 seconds, and a failed fetch keeps the
  previous catalogue rather than emptying the listing. Addressing is unaffected
  either way: an unlisted `provider-slug/model` still routes.
- **It is one free, non-inference GET per provider per interval.** The cache
  survives config reloads and drops a provider a reload removed. Fetches are
  bounded by `probe_concurrency` and a 10 second timeout.

Two bounds keep the listing from growing without limit, since it is the product
of the fleet and each provider's catalogue:

- at most **200 models per provider** are kept from a catalogue, which is where
  an aggregator's several-hundred-entry `/v1/models` is cut. The cap is per
  provider so no single one crowds out the others, and it never removes a model
  a route target names — that set is listed regardless
- at most **1 MiB** of a probe response is read before the body is abandoned
  unparsed, so a liveness probe can never pull an unbounded response into the
  gateway

Both dialects read the same endpoint: rolter serves one `/v1/models` for the
OpenAI and Anthropic surfaces alike.

## OpenAI Responses

`POST /v1/responses` is routed by its required `model` field. Native OpenAI
providers receive the request and SSE events unchanged. For Chat Completions or
Anthropic Messages upstreams, rolter translates the common text, multimodal,
function-tool, tool-result, sampling, and usage fields in both directions and
emits Responses-shaped events to the caller. Responses-only features without a
wire equivalent (for example `background`, `store`, `previous_response_id`,
and provider-specific reasoning controls) are not forwarded to those older
surfaces; use a native Responses provider when those features are required.

For native OpenAI providers, rolter records the selected provider, target,
upstream model, provider credential fingerprint, and native response ID after a
successful creation. `GET`/`DELETE /v1/responses/{id}`, cancellation, and
input-item retrieval are then pinned to that record. Records are isolated by
virtual key, retained for 24 hours by default, bounded to 100,000 entries per
gateway process, and removed after a successful delete. Configure these limits
with `[responses] registry_ttl_secs` and `registry_max_entries`; setting either
to `0` disables registration.

The registry is process-local. Multi-replica deployments must keep lifecycle
requests sticky to the gateway replica that accepted creation; records do not
survive a gateway restart. Route changes do not retarget an existing response.
If its provider is removed, its provider kind changes, or its credential is
rotated away, the record becomes unavailable. Unknown, expired, deleted,
cross-key, and unavailable records all return the same `404 response_not_found`
error so route ownership is not leaked.

The record names its provider by its stable identity, the owning org and the
display name, not by the runtime id that served the request (#3461). A
provider's runtime id changes from `openai` to `openai@<org>` once a second org
creates a provider of the same name (ADR-0040), and a response created before
that still reaches the provider that holds it: each lifecycle call finds the
provider's runtime id in the current snapshot again (`Snapshot::provider_id`).
A record that carries a runtime id instead (`provider_org` unset) is looked up
by that id, as records always were.

Each lifecycle call is also re-authorized against the current configuration,
exactly as a new request would be: the model the caller named, the route that
served the response (visibility, route policy, provider list) and the provider
that holds it. Access revoked since creation is refused with the usual `403`
codes before anything is forwarded, and a response whose route has since been
removed is refused with `403 route_not_allowed` (fail closed). See the
[route authorization contract](../architecture/rbac-and-auth.md#the-route-authorization-contract-1485).

Responses translated through Chat Completions or Anthropic Messages retain an
ownership record but expose no lifecycle capabilities, because those upstream
contracts do not retain an OpenAI Responses resource. Their lifecycle calls
return `501 response_lifecycle_unsupported`. Compaction and input-token counting
remain unsupported for all providers.

Every lifecycle call from an identified caller leaves one request-log row, the
refused ones included (#2836): the model and the provider and target come from
the stored record, the operation is in `lifecycle_operation`, and there are no
tokens and no cost. The calls are not billable, so they never move a budget, a
token window or a request admission. See
[Observability](../architecture/observability.md#request--cost-logs).

## Examples

```bash
# openai chat (streaming)
curl -N http://localhost:4000/v1/chat/completions \
  -H "Authorization: Bearer sk-rolter-dev" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o","stream":true,"messages":[{"role":"user","content":"hi"}]}'

# anthropic messages
curl http://localhost:4000/v1/messages \
  -H "x-api-key: sk-rolter-dev" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude","max_tokens":256,"messages":[{"role":"user","content":"hi"}]}'

# openai embeddings
curl http://localhost:4000/v1/embeddings \
  -H "Authorization: Bearer sk-rolter-dev" \
  -H "Content-Type: application/json" \
  -d '{"model":"text-embedding-3-small","input":["hello","world"]}'

# self-hosted vllm pool via a public model name
curl http://localhost:4000/v1/chat/completions \
  -H "Authorization: Bearer sk-rolter-dev" \
  -H "x-session-id: user-123" \
  -H "Content-Type: application/json" \
  -d '{"model":"llama","messages":[{"role":"user","content":"hi"}]}'
```

> Multipart audio (`/v1/audio/transcriptions`, `/v1/audio/translations`) forwards the upload verbatim and routes on the `model` form field; the route target's upstream model name is not rewritten into the multipart body, and per-model and per-variant param defaults (JSON-only) do not apply. Variant routing does (#3494): a route with variants samples one exactly as a chat request does (`forward_variants`, given the upload as a `VariantPayload::Multipart` it forwards verbatim), dials only the variants' targets, falls over along the same chain and logs the variant it served from. Authorization does apply in full: the upload passes the same model, visibility, route-policy and provider gates as a JSON request ([route authorization contract](../architecture/rbac-and-auth.md#the-route-authorization-contract-1485)). The response side of the guardrails applies to the transcript (#3331): a built-in `post_call` rule that blocks, a semantic policy's `post_call` provider that refuses, or a `post_response` plugin that blocks answers `403` with `guardrail_blocked` (`plugin_blocked` for a plugin) in place of the transcript, which is still billed by its audio minutes; see [semantic guards](../architecture/semantic-guards.md#audio-transcriptions-3331). A `stream=true` form field is read too (#3432): on a route with a response-side check it is refused or buffered as a chat request's `"stream": true` is, with `400 guardrail_streaming_unsupported` (or `plugin_streaming_unsupported` / `pii_streaming_unsupported`) before the upstream call.
