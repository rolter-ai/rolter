# Self-hosted Ollama

rolter supports a local or privately hosted Ollama daemon through Ollama's
OpenAI-compatible API. This provider does not require an API key.

## Native setup

Install Ollama, start the daemon, and pull a small smoke-test model:

```bash
ollama serve
ollama pull qwen2.5:0.5b
```

Configure the daemon origin, without `/v1` (rolter appends endpoint paths):

```toml
[[providers]]
name = "ollama-local"
kind = "ollama"
api_base = "http://localhost:11434"

[[routes]]
model = "local-qwen"
strategy = "round_robin"
[[routes.targets]]
provider = "ollama-local"
model = "qwen2.5:0.5b"
```

Start rolter and exercise model discovery, chat, legacy completions, embeddings,
and streaming:

```bash
curl http://localhost:4000/v1/models
curl http://localhost:4000/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"local-qwen","messages":[{"role":"user","content":"hello"}]}'
curl http://localhost:4000/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"local-qwen","stream":true,"messages":[{"role":"user","content":"hello"}]}'
curl http://localhost:4000/v1/completions \
  -H 'content-type: application/json' \
  -d '{"model":"local-qwen","prompt":"hello"}'
curl http://localhost:4000/v1/embeddings \
  -H 'content-type: application/json' \
  -d '{"model":"local-qwen","input":"hello"}'
```

`/v1/models` lists rolter's configured public route names, so the example
returns `local-qwen`; it does not expose unrelated models installed in Ollama.

## Docker setup

Containers must address Ollama by its Compose service name:

```yaml
services:
  ollama:
    image: ollama/ollama:0.9.6
    volumes:
      - ollama-data:/root/.ollama
```

Use `api_base = "http://ollama:11434"` in the gateway container's config. The
opt-in smoke suite under `integration/ollama/` provides a complete reproducible
Compose setup and pulls `qwen2.5:0.5b` automatically.

## Compatibility and known gaps

rolter passes OpenAI request JSON and response bodies through unchanged (apart
from the configured model-name rewrite), preserving retry, cooldown, health,
logging, error mapping, routing, and SSE semantics. Ollama currently documents
chat and legacy completions, streaming, JSON mode (`response_format`), tools,
vision message content, `seed`, and usage fields. The gateway also passes
`stream_options` through, though Ollama may ignore unsupported options.

Support depends on the installed Ollama release and model: tool calling and
vision require capable models, JSON schemas are not guaranteed to be obeyed by
every model, and some OpenAI fields are accepted but ignored. Ollama's
OpenAI-compatible embeddings endpoint accepts models with embedding support;
for production, route it to a dedicated embedding model. Ollama's native
`/api/*` endpoints and Ollama Cloud authentication are outside this provider's
scope.

