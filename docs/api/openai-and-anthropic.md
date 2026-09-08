# API surface

rolter speaks the OpenAI and Anthropic HTTP APIs so existing SDKs work unchanged — point them at the gateway base URL and use a rolter virtual key.

## Authentication

- OpenAI-style: `Authorization: Bearer <virtual-key>`
- Anthropic-style: `x-api-key: <virtual-key>`

When no virtual keys are configured the gateway runs open (useful for local dev).

## Endpoints (v1)

| Method | Path | Notes |
| --- | --- | --- |
| POST | `/v1/chat/completions` | OpenAI chat; streaming via `"stream": true` (SSE) |
| POST | `/v1/completions` | OpenAI legacy completions |
| POST | `/v1/responses` | OpenAI Responses; provider-native passthrough, streaming supported |
| GET, DELETE | `/v1/responses/{id}` | retrieve or delete a tenant-scoped native Responses resource |
| POST | `/v1/responses/{id}/cancel` | cancel a tenant-scoped native Responses resource |
| GET | `/v1/responses/{id}/input_items` | list input items for a tenant-scoped native Responses resource |
| POST | `/v1/messages` | Anthropic Messages; streaming supported |
| POST | `/v1/embeddings` | OpenAI embeddings; non-streaming |
| POST | `/v1/rerank` | Cohere/Jina rerank; non-streaming |
| POST | `/v1/images/generations` | OpenAI image generation; non-streaming |
| POST | `/v1/audio/speech` | OpenAI text-to-speech; binary audio response |
| POST | `/v1/audio/transcriptions` | OpenAI speech-to-text; `multipart/form-data` upload |
| POST | `/v1/audio/translations` | OpenAI audio translation; `multipart/form-data` upload |
| GET | `/v1/realtime?model=…` | OpenAI-compatible Realtime API; WebSocket relay. **Experimental** — see below |
| GET, POST, DELETE | `/mcp/{server}/{path…}` | authenticated Streamable HTTP/SSE MCP proxy |
| GET | `/v1/models` | lists configured public model names |
| GET | `/openapi.json` | OpenAPI 3.1 description of this request surface (self-contained, no external assets) |
| GET | `/docs` | interactive Scalar API reference (assets embedded in the binary — works air-gapped) |
| GET | `/` | service-info landing (version + links to docs/openapi/health) |
| GET | `/healthz` | liveness — process is up, no dependency checks |
| GET | `/readyz` | readiness — `503` while draining (see [Health & readiness](../architecture/health-and-readiness.md)) |
| GET | `/metrics` | Prometheus exposition |

## Request header forwarding

By default the gateway forwards trace context and nothing else. An operator can
name additional client headers to forward with the `forwarded_headers`
allowlist in the client policy.

**`anthropic-*` headers are the exception: they forward by prefix, not by
allowlist entry** (#1013). Any inbound header whose name begins with
`anthropic-` is passed unchanged to an Anthropic-dialect upstream, whether or
not anyone has heard of it.

This is not a convenience. Anthropic extends its protocol *through headers*: a
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
*translated* requests — an OpenAI-dialect call rewritten to Anthropic, where no
client version exists — and that is still exactly what it does. Overriding a
version a native client did state would silently change the protocol it asked
for. Exactly one value is sent either way.

## Realtime WebSocket

> **Experimental.** This surface carries the `realtime` [stability marker](../development/stability-markers.md): it may change shape or be withdrawn in a minor release. A session is admitted against the process-local caps in `[realtime]` and nothing else — budgets, rate limits, guardrails, usage recording and cost attribution all sit on the HTTP request path and do not see it, so spend through a realtime session is neither capped nor recorded. Treat it as a preview rather than something to meter a deployment on.

Connect with the usual gateway bearer key and the public route model as a query parameter:

```text
wss://gateway.example.com/v1/realtime?model=gpt-realtime
```

rolter authenticates and selects an upstream before accepting the client upgrade, then pins that upstream and its selected provider key for the session. Text, binary audio and WebSocket control frames are relayed in both directions without application-level buffering. If the upstream drops, the client must reconnect; rolter does not fail a live session over to another target because replaying audio or tool events is unsafe.

The WebSocket-first implementation supports the OpenAI Realtime event stream, including `session.update`, `input_audio_buffer.*`, `response.*`, and function-call events. WebRTC/browser ephemeral-token handoff is not exposed by the gateway yet.

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

Responses translated through Chat Completions or Anthropic Messages retain an
ownership record but expose no lifecycle capabilities, because those upstream
contracts do not retain an OpenAI Responses resource. Their lifecycle calls
return `501 response_lifecycle_unsupported`. Compaction and input-token counting
remain unsupported for all providers.

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

> Multipart audio (`/v1/audio/transcriptions`, `/v1/audio/translations`) forwards the upload verbatim and routes on the `model` form field; the route target's upstream model name is not rewritten into the multipart body, and variant routing / per-model param defaults (JSON-only) do not apply.
