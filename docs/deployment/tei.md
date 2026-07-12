# Hugging Face Text Embeddings Inference (TEI)

The `tei` provider targets TEI's OpenAI-compatible `POST /v1/embeddings`
endpoint. Self-hosted TEI is keyless by default; `api_key` or `api_key_env` can
add bearer authentication when TEI sits behind an authenticated proxy.

## Run TEI

For a reproducible CPU deployment with a small embedding model:

```bash
docker run --rm -p 8080:80 -v "$PWD/data:/data" \
  ghcr.io/huggingface/text-embeddings-inference:cpu-1.9 \
  --model-id sentence-transformers/all-MiniLM-L6-v2
```

On Apple Silicon, install and run the native server:

```bash
brew install text-embeddings-inference
text-embeddings-router \
  --model-id sentence-transformers/all-MiniLM-L6-v2 --port 8080
```

## Configure Rolter

Use the server origin as `api_base`, without `/v1`:

```toml
[[providers]]
name = "tei-local"
kind = "tei"
api_base = "http://127.0.0.1:8080"

[[routes]]
model = "embed-local"
strategy = "round_robin"

[[routes.targets]]
provider = "tei-local"
model = "sentence-transformers/all-MiniLM-L6-v2"
```

Rolter preserves OpenAI string, string-array, token-array, and token-array-batch
inputs, plus `encoding_format`, `dimensions`, `user`, embedding vectors, usage,
and upstream error JSON. Normal routing headers, retries, cooldowns, logging,
and health behavior apply. The default active probe uses TEI's `/health` route.

Only `/v1/embeddings` is part of this adapter. TEI-native `/embed`, `/rerank`,
`/embed_sparse`, `/predict`, `/tokenize`, `/health`, and `/metrics` are not
exposed through Rolter's generic OpenAI surface. Call TEI directly for them.

## Smoke test

The opt-in Compose test starts TEI, downloads the small model, starts Rolter,
and verifies batch embeddings, optional fields, usage, and routing headers:

```bash
integration/tei/run.sh
```

