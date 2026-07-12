# Testing

## Run

Tests run under [nextest](https://nexte.st/) (the same runner CI uses), plus a
separate doc-test pass since nextest does not run doc tests:

```bash
cargo nextest run --workspace   # unit + integration tests
cargo test --doc --workspace    # doc tests
cd ui && bun run lint           # ui typecheck
```

Install the runner once with `cargo install cargo-nextest` (or see the
[nextest install docs](https://nexte.st/docs/installation/)). `just test` runs
both Rust passes for you. Plain `cargo test --workspace` still works if you
haven't installed nextest, but CI runs nextest so prefer it locally.

The Ollama Cloud live smoke sends a billed request and is ignored by default:

```bash
OLLAMA_API_KEY=... ROLTER_OLLAMA_LIVE_MODEL=gpt-oss:20b \
  cargo test -p rolter-gateway --test ollama_cloud live_smoke -- --ignored
```

Test grouping is configured in [`.config/nextest.toml`](../../.config/nextest.toml):
the Postgres-backed `rolter-store`/`rolter-control` suites share one database and
reset the schema per test, so they run in a single-threaded group to avoid
clobbering each other.

## Layout

- **Unit tests** live next to the code in `#[cfg(test)] mod tests`. Current coverage: balancer strategies (round-robin cycling, consistent-hash stability, cache-aware affinity, empty targets), the prefix trie, config parsing, model rewrite, auth checks, and the in-memory store.
- Keep the pure crates (`rolter-core`, `rolter-balancer`, `rolter-auth`) fully unit-testable without I/O.

## Strategy as the project grows

- **Integration tests** for the gateway: spin up the Axum app with a mock upstream (`wiremock`/`httpmock`) and assert routing, auth, model rewrite, error mapping and streaming passthrough.
- **Property tests** (`proptest`) for the balancer: distribution fairness, affinity invariants.
- **DB tests** for `rolter-store` Postgres backend behind a feature, using a disposable container.
- **Load tests** (`oha`/`k6`) against a mock upstream to track added latency and max RPS (see [performance.md](../architecture/performance.md)).

## Benchmarks

Hot-path micro-benchmarks run under [criterion](https://github.com/criterion-rs/criterion.rs). They live in `crates/<crate>/benches/` with a `[[bench]] harness = false` entry per file, and cover the per-request cost that shows up as pure gateway overhead:

```bash
just bench                       # cargo bench --workspace
cargo bench -p rolter-balancer   # just the balancer benches
cargo bench -p rolter-balancer --bench pick   # one bench target
```

Current coverage (`rolter-balancer`):

- `pick` — `LoadBalancer::pick` for every built-in strategy over a ~24-target pool with a populated `RouteContext`.
- `trie` — prefix-trie `insert` (bounded/unbounded, so LRU eviction is measured) and `longest_prefix` on a warm trie.

criterion writes HTML reports to `target/criterion/`. Benches are **not** run in CI (timings are noisy on shared runners), but `cargo clippy --workspace --all-targets -- -D warnings` compiles them on every PR, so they cannot silently bit-rot. Use `just bench-check` (`cargo bench --workspace --no-run`) to compile them locally without running.

## CI

`.github/workflows/ci.yml` delegates to the shared `quality.yml` gate, which runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo nextest run --workspace --all-features` plus a `cargo test --doc` pass, the feature matrix, `cargo doc` (warnings as errors), cargo-deny, gitleaks, the UI lint/build, and a Conventional Commit PR-title check on every push/PR.
