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
