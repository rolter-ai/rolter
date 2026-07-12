# llama.cpp (`llama-server`)

Rolter's `llama_cpp` provider preset targets the OpenAI-compatible API exposed
by `llama-server`. It needs no API key by default and works with local CPU or
GPU GGUF deployments.

## Start llama-server

Choose a GGUF whose license permits your intended use and whose quantization
fits available RAM/VRAM. `Q4_K_M` is a practical starting point for local use;
smaller quantizations use less memory at the cost of quality.

With a native llama.cpp build:

```bash
llama-server -m /models/model.gguf --host 0.0.0.0 --port 8080
```

Or with the upstream Docker image:

```bash
docker run --rm -p 8080:8080 -v "$PWD/models:/models" \
  ghcr.io/ggml-org/llama.cpp:server \
  -m /models/model.gguf --host 0.0.0.0 --port 8080
```

## Configure Rolter

`api_base` is the server origin, without `/v1`. `model` on the target is the
model identifier reported by llama-server; the public route can be a stable
alias.

```toml
[[providers]]
name = "local-llama"
kind = "llama_cpp"
api_base = "http://127.0.0.1:8080"

[[routes]]
model = "local-chat"
strategy = "round_robin"

[[routes.targets]]
provider = "local-llama"
model = "model.gguf"
```

Rolter forwards `/v1/chat/completions` and `/v1/completions`, including SSE,
sampling fields, `grammar`, and OpenAI `response_format`. `/v1/models` lists
Rolter's public route aliases. Routing headers, retries, cooldowns, and active
health checks behave like other providers; the default health probe calls the
upstream `/v1/models` endpoint.

llama.cpp-native routes such as `/completion`, `/tokenize`, `/detokenize`, and
slot/metrics administration are intentionally **not** exposed by Rolter's
generic OpenAI API. Call llama-server directly for those endpoints.

## Smoke test

With llama-server running and the model id from its `/v1/models` response:

```bash
integration/llama-cpp-smoke.sh http://127.0.0.1:8080 model.gguf
```

The script starts a temporary Rolter gateway, verifies model listing,
non-streaming completion, SSE, and routing headers, then cleans up.

