# Infinity (embeddings and reranking)

The `infinity` provider kind fronts a self-hosted
[michaelfeil/infinity](https://github.com/michaelfeil/infinity) server (#463).
The operator-facing page, with the Docker and air-gapped setup, is
`docs/user-docs/configuration/infinity.mdx`; this page records how the kind is
built.

## Wire facts

Read from Infinity's own source (`libs/infinity_emb/infinity_emb/`):

- `infinity_server.py` mounts `{url_prefix}/embeddings`, `{url_prefix}/rerank`
  and `{url_prefix}/models`; `/health` is outside the prefix. `env.py` defaults
  `url_prefix` to `""` and the port to `7997`.
- The key, when `--api-key` / `INFINITY_API_KEY` is set, is checked through
  FastAPI's `HTTPBearer`, so it is `Authorization: Bearer`, the default of
  `ProviderKind::auth_header`. A refusal is `401 {"detail": "Unauthorized"}`.
- The engine is picked by the body's `model` (`_resolve_engine`); an unknown
  one is `400` in OpenAI's error shape (`errors.OpenAIException`), an
  overloaded one `429`.
- `fastapi_schemas/pymodels.py`: `OpenAIEmbeddingResult` is OpenAI's shape;
  `RerankInput.documents` is `conlist(str)` and `_ReRankObject.document` is
  `Optional[str]`. `BatchHandler.rerank` sorts by descending score before
  cutting to `top_n`.

## In rolter

- `ProviderKind::Infinity` is in `strips_gateway_v1_prefix`: with the default
  empty prefix the base is the server address, and `/v1/embeddings` resolves to
  `{api_base}/embeddings`. A non-empty `--url-prefix` goes in the base.
- `primary_request` is `Embeddings` and `primary_upstream_path` is
  `EMBEDDINGS_PATH`, so the provider sheet's preview, the connection test and
  the `also_track_via_llm_call` check all aim at an embedding.
- The probe is `GET {api_base}/models`, a catalogue (`ProbeExpectation::Catalogue`).
- Embeddings are passthrough. A rerank resolves to
  `Protocol::InfinityRerank` in `TranslationPlan::resolve`, a translation pair
  whose functions live in `crates/rolter-proxy/src/infinity.rs`: `{"text": ...}`
  documents are flattened to strings on the way up (a document with no `text`
  fails closed as `unsupported_content_part`), and a returned bare string
  document is wrapped as `{"text": ...}` on the way back. Because the plan is a
  translation, the gateway buffers the (never streamed) rerank body and runs it
  through `translate_response`; error statuses stay passthrough.

## Tests

`crates/rolter-gateway/tests/infinity.rs` drives the gateway against a fake
Infinity that reproduces the behaviour above: both endpoints, three models on
one server, a non-empty `--url-prefix`, the bearer key and its `401`, the
`400`/`422` refusals, a malformed body and a route timeout. There is no
real-server suite yet (#3246); the TEI one under `integration/tei` is the model for one.
