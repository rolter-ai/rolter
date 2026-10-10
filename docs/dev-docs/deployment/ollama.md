# Self-hosted Ollama

rolter supports a local or privately hosted Ollama daemon through Ollama's
OpenAI-compatible API. This provider does not require an API key.

## Native setup

Install Ollama, start the daemon, and pull a small smoke-test model:

```bash
ollama serve
ollama pull qwen2.5:0.5b
ollama pull all-minilm:22m
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

# embeddings need a model with embedding support, see "Compatibility and known gaps"
[[routes]]
model = "local-embed"
strategy = "round_robin"
[[routes.targets]]
provider = "ollama-local"
model = "all-minilm:22m"
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
  -d '{"model":"local-embed","input":"hello"}'
```

`/v1/models` lists rolter's configured public route names, so the example
returns `local-qwen` and `local-embed`; it does not expose unrelated models installed in Ollama.

## Docker setup

Containers must address Ollama by its Compose service name:

```yaml
services:
  ollama:
    image: ollama/ollama:0.40.2
    volumes:
      - ollama-data:/root/.ollama
```

Use `api_base = "http://ollama:11434"` in the gateway container's config. The
opt-in smoke suite under `integration/ollama/` provides a complete reproducible
Compose setup and pulls `qwen2.5:0.5b` and `all-minilm:22m` automatically.

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
for production, route it to a dedicated embedding model. Ollama 0.40 answers
`/v1/embeddings` for a generative model such as `qwen2.5:0.5b` with a 501
("This server does not support embeddings"), which rolter counts as an upstream
failure and cools the target down for, so never point an embeddings route at a
chat model. Ollama's native
`/api/*` endpoints and Ollama Cloud authentication are outside this provider's
scope.
