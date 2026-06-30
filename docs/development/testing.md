# Testing

## Run

```bash
cargo test --workspace        # unit tests
cd ui && bun run lint         # ui typecheck
```

## Layout

- **Unit tests** live next to the code in `#[cfg(test)] mod tests`. Current coverage: balancer strategies (round-robin cycling, consistent-hash stability, cache-aware affinity, empty targets), the prefix trie, config parsing, model rewrite, auth checks, and the in-memory store.
- Keep the pure crates (`rolter-core`, `rolter-balancer`, `rolter-auth`) fully unit-testable without I/O.

## Strategy as the project grows

- **Integration tests** for the gateway: spin up the Axum app with a mock upstream (`wiremock`/`httpmock`) and assert routing, auth, model rewrite, error mapping and streaming passthrough.
- **Property tests** (`proptest`) for the balancer: distribution fairness, affinity invariants.
- **DB tests** for `rolter-store` Postgres backend behind a feature, using a disposable container.
- **Load tests** (`oha`/`k6`) against a mock upstream to track added latency and max RPS (see [performance.md](../architecture/performance.md)).
- **Benches** (`criterion`) for `pick`/trie hot paths.

## CI

`.github/workflows/ci.yml` runs `cargo fmt --check`, `cargo clippy -D warnings`, and `cargo test --workspace` on every push/PR, plus a Conventional Commit PR-title check.
