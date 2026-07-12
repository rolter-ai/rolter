# Development setup

## Prerequisites

- **Rust** (stable) via [rustup](https://rustup.rs) — the workspace pins the toolchain in `rust-toolchain.toml`.
- **Bun** for the UI — `curl -fsSL https://bun.sh/install | bash`.
- **Docker** + Compose for Postgres/Redis/ClickHouse.
- **uv** (optional) for the PyPI-wheel install path and tooling.

## Clone & build

```bash
git clone https://github.com/ormeilu/rolter.git
cd rolter
cargo build --workspace
cargo nextest run --workspace   # or `cargo test --workspace`; install: cargo install cargo-nextest
```

## Run the gateway (no external services needed)

```bash
cp rolter.example.toml rolter.toml
export OPENAI_API_KEY=sk-...        # referenced by api_key_env in the config
cargo run -p rolter-gateway -- --config rolter.toml
# -> http://localhost:4000  (/healthz, /metrics, /v1/*)
```

## Run the control plane + UI

```bash
cargo run -p rolter-control          # http://localhost:4001
cd ui && bun install && bun run dev  # http://localhost:3000 (proxies /api -> :4001)
```

## Run the full stack

```bash
docker compose -f docker/docker-compose.yml up -d                 # postgres, redis, clickhouse, gateway, control
```

## Handy tasks

`just` wraps the common commands:

```bash
just build | just test | just fmt | just lint
just gateway | just control | just ui-dev | just up
```

## Before committing

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace && cargo test --doc --workspace   # or `just test`
# optional hooks: prek install   (conventional commit msg + fmt/clippy)
```
