# Performance

Goal: beat the reference Python proxy (LiteLLM cites ~8ms P95 added latency at 1k RPS) with a much smaller per-request overhead in Rust.

## Hot-path principles

- **Lock-free config reads** — the routing table is an `ArcSwap<Snapshot>`; readers never block, even during a hot reload.
- **Minimal-copy streaming** — upstream responses are piped to the client as a `Body::from_stream` over the `reqwest` byte stream; rolter does not buffer whole responses.
- **Connection reuse** — pooled `reqwest` clients with HTTP/2 keep-alive and `tcp_nodelay`; one client per egress-proxy target, cached.
- **Cheap auth** — virtual-key lookup is an O(1) hash-map hit on the in-memory snapshot.
- **Avoid full deserialization** — only the fields needed for routing (`model`, `stream`) are read; the body is forwarded as raw bytes.
- **Logging off the hot path** — usage/cost rows are batched and written to ClickHouse asynchronously.
- **Release profile** — `lto = "thin"`, `codegen-units = 1`, `strip = true`.

## Things to watch

- The approximate cache-aware trie is per-route in-memory state; bound its size with eviction before it grows large.
- Per-request JSON parse for `model` is small but measurable; consider a fast path / partial parse for very high RPS.
- Prefer `bytes::Bytes` (ref-counted) over `Vec<u8>` copies when rewriting the model field.

## Benchmarking (roadmap)

- Add a `criterion` micro-bench for balancer `pick`.
- Add an end-to-end load test (e.g. `oha`/`k6`) against a mock upstream to measure added latency and max RPS per core.
- Track TTFT and total-latency histograms in Prometheus and watch them in CI perf runs.

## Real inference engines

The opt-in ROL-238 suite checks rolter against real OpenAI-compatible vLLM and
SGLang servers without downloading model weights. Both servers use a small
public model only for its configuration/tokenizer and initialize random weights
with `--load-format dummy`. Output is intentionally meaningless; this validates
the HTTP, OpenAI JSON, and SSE contracts rather than model quality.

The fixture is `trl-internal-testing/tiny-random-LlamaForCausalLM`, a small
public Llama architecture supported by both vLLM and SGLang. It is served to
the suite as `rolter-dummy`; no model checkpoint is loaded.

It runs on CPU in Docker and therefore works on GitHub-hosted runners. Each
engine profile starts two independent dummy upstreams so the gateway exercises
a real target pool. Run one engine locally:

```sh
just integration-vllm
just integration-sglang
```

Each command boots routes for every balancing strategy (round-robin, random,
power-of-two, consistent-hash, cache-aware, weighted, pipeline, cheapest, and
fastest). It verifies `/v1/models`, non-streaming chat, and SSE both directly
and through rolter, and explicitly confirms round-robin reaches both targets.
Logs are kept in `artifacts/engines/<engine>/`.

### Local end-to-end run

`just integration-vllm` and `just integration-sglang` start two CPU
dummy-weight servers, render the gateway configuration, and run the OpenAI JSON
and SSE assertions. The runner cleans up all containers and child processes on
exit and preserves the combined engine log plus the gateway log under
`artifacts/engines/<engine>/`.

For manual inspection, start the selected two-server pool and leave it running:

```sh
docker compose -f docker/docker-compose.engines.yml --profile vllm up -d
# use profile sglang for ports 30000 and 30001
```

Render the gateway configuration in another terminal and start rolter:

```sh
config=$(mktemp)
sed \
  -e 's/__ROLTER_PORT__/4010/g' \
  -e 's/__ENGINE_1_PORT__/8000/g' \
  -e 's/__ENGINE_2_PORT__/8001/g' \
  integration/engines/rolter-dummy.toml.in >"$config"
cargo run -p rolter-gateway -- --config "$config"
```

Verify non-streaming JSON and streaming SSE through the gateway:

```sh
curl -i http://127.0.0.1:4010/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"dummy-round-robin","messages":[{"role":"user","content":"Reply with one token."}],"max_tokens":1,"temperature":0}'

curl -N http://127.0.0.1:4010/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"dummy-round-robin","stream":true,"messages":[{"role":"user","content":"Reply with one token."}],"max_tokens":1,"temperature":0}'
```

Exercise all configured strategies; each request must return a non-empty
OpenAI `choices` array:

```sh
for model in dummy-round-robin dummy-random dummy-power-of-two \
  dummy-consistent-hash dummy-cache-aware dummy-weighted dummy-pipeline \
  dummy-cheapest dummy-fastest; do
  curl -fsS http://127.0.0.1:4010/v1/chat/completions \
    -H 'content-type: application/json' \
    -d "{\"model\":\"$model\",\"messages\":[{\"role\":\"user\",\"content\":\"ping\"}],\"max_tokens\":1}" \
    | jq -e '.choices | length > 0' >/dev/null
done
```

Clean up the pool and temporary gateway configuration:

```sh
docker compose -f docker/docker-compose.engines.yml --profile vllm down --volumes
rm -f "$config"
```

For non-gating direct-versus-gateway samples, run `just bench-vllm` or `just
bench-sglang`. They record non-streaming and streaming p50/p95/p99 latency and
streaming first-byte time in JSON. Results only compare runs on the same host,
CPU image, engine versions, and host configuration; throughput thresholds are
deliberately not merge gates.

The `engine integration` workflow runs the CPU vLLM smoke suite for relevant
pull requests, weekly, or manually. SGLang remains available through the local
`just integration-sglang` command, but its source-built CPU image is currently
too heavy for the shared CI gate. This suite is for compatibility, not a
performance gate.
When [ROL-67](https://linear.app/rolter/issue/ROL-67/openaianthropic-requestresponse-translation-streaming)
lands, add the equivalent `/v1/messages` assertion through the gateway.
