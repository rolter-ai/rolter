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
| GET | `/v1/models` | lists configured public model names |
| GET | `/healthz` | liveness |
| GET | `/metrics` | Prometheus exposition |

## Routing

The `model` field in the body selects a **route**. The route's strategy picks a target; rolter rewrites `model` to the target's upstream model id and forwards with the provider's credentials. Session affinity uses `x-session-id` when present.

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

## Roadmap

`/v1/audio/transcriptions` and `/v1/audio/translations` (multipart upload), plus OpenAI<->Anthropic request/response translation (call Anthropic models through the OpenAI schema and vice versa), and an OpenAPI document served by the gateway.
