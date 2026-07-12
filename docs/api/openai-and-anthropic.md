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
| POST | `/v1/messages` | Anthropic Messages; streaming supported |
| POST | `/v1/embeddings` | OpenAI embeddings; non-streaming |
| POST | `/v1/rerank` | Cohere/Jina rerank; non-streaming |
| POST | `/v1/images/generations` | OpenAI image generation; non-streaming |
| POST | `/v1/audio/speech` | OpenAI text-to-speech; binary audio response |
| POST | `/v1/audio/transcriptions` | OpenAI speech-to-text; `multipart/form-data` upload |
| POST | `/v1/audio/translations` | OpenAI audio translation; `multipart/form-data` upload |
| GET | `/v1/realtime?model=…` | OpenAI-compatible Realtime API; WebSocket relay |
| GET | `/v1/models` | lists configured public model names |
| GET | `/openapi.json` | OpenAPI 3.1 description of this request surface (self-contained, no external assets) |
| GET | `/docs` | interactive Scalar API reference (assets embedded in the binary — works air-gapped) |
| GET | `/` | service-info landing (version + links to docs/openapi/health) |
| GET | `/healthz` | liveness |
| GET | `/metrics` | Prometheus exposition |

## Realtime WebSocket

Connect with the usual gateway bearer key and the public route model as a query parameter:

```text
wss://gateway.example.com/v1/realtime?model=gpt-realtime
```

rolter authenticates and selects an upstream before accepting the client upgrade, then pins that upstream and its selected provider key for the session. Text, binary audio and WebSocket control frames are relayed in both directions without application-level buffering. If the upstream drops, the client must reconnect; rolter does not fail a live session over to another target because replaying audio or tool events is unsafe.

The WebSocket-first implementation supports the OpenAI Realtime event stream, including `session.update`, `input_audio_buffer.*`, `response.*`, and function-call events. WebRTC/browser ephemeral-token handoff is not exposed by the gateway yet.

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
