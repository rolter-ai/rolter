# rolter

A high-performance, open-source **LiteLLM-proxy alternative** built in Rust with a TypeScript + [shadcn/ui](https://ui.shadcn.com) dashboard.

rolter is an OpenAI/Anthropic-compatible **AI gateway** that proxies commercial providers and load-balances self-hosted OpenAI-compatible fleets (e.g. 20–30 vLLM instances) with **cache-aware routing**, full RBAC, reload-free configuration, and cost/usage tracking.

> Status: early scaffold. The data-plane gateway MVP runs today (OpenAI/Anthropic passthrough + balancing + virtual keys + metrics). The control plane, dashboard, and persistence are being built out — see [`ROADMAP.md`](ROADMAP.md) and [`TODO.md`](TODO.md).

## Why rolter

- **Fast**: a Rust data plane (Axum/Hyper/Tower on Tokio) with lock-free config reads and minimal-copy streaming.
- **Cache-aware load balancing**: route prefix-heavy traffic to the vLLM replica most likely to have the KV cache warm — approximate today, precise (KV-events) on the roadmap.
- **Drop-in**: speak the OpenAI and Anthropic APIs your clients already use.
- **Operable**: virtual keys, budgets, rate limits, cost tracking, RBAC, and reload-free config changes from the UI.

## Architecture

```mermaid
flowchart LR
  Client([OpenAI / Anthropic clients]) -->|/v1/*| GW["rolter-gateway<br/>(data plane)"]
  Admin([Dashboard]) --> CTL["rolter-control<br/>(control plane + UI host)"]
  GW -->|balanced + streamed| UP["Upstreams<br/>OpenAI · Anthropic · vLLM pool"]
  CTL -->|writes config| PG[("PostgreSQL")]
  CTL -->|publishes change events| RDS[("Redis")]
  RDS -->|hot-swap snapshot| GW
  GW -->|async batched logs| CH[("ClickHouse")]
```

See [`docs/architecture/overview.md`](docs/architecture/overview.md) for the full design.

## Quick start (gateway MVP)

```bash
cp rolter.example.toml rolter.toml          # edit providers/routes
export OPENAI_API_KEY=sk-...                 # referenced by api_key_env
cargo run -p rolter-gateway -- --config rolter.toml
```

```bash
curl -s http://localhost:4000/v1/models \
  -H "Authorization: Bearer sk-rolter-dev"

curl -s http://localhost:4000/v1/chat/completions \
  -H "Authorization: Bearer sk-rolter-dev" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o","messages":[{"role":"user","content":"hello"}]}'
```

`GET /healthz` and Prometheus `GET /metrics` are also exposed.

## Install

```bash
# rust
cargo install --path crates/rolter-gateway

# uv (PyPI wheel built with maturin) — see docs/development/packaging.md
uv tool install rolter

# docker
docker compose up -d
```

## Repository layout

- `crates/rolter-core` — config model, domain types, errors, telemetry
- `crates/rolter-balancer` — load-balancing strategies (incl. approximate cache-aware)
- `crates/rolter-proxy` — upstream forwarding, header injection, streaming
- `crates/rolter-store` — repository traits + in-memory store (Postgres/Redis/ClickHouse next)
- `crates/rolter-auth` — virtual keys, roles, access checks
- `crates/rolter-gateway` — data-plane binary
- `crates/rolter-control` — control-plane binary + static UI host
- `ui/` — Vite + React + shadcn/ui dashboard
- `docs/` — architecture, ADRs, API, development and deployment guides
- `migrations/`, `clickhouse/` — database schemas

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

Commits and PR titles follow [Conventional Commits](docs/development/commit-conventions.md). See [`AGENTS.md`](AGENTS.md) and [`docs/development/contributing.md`](docs/development/contributing.md).

## License

Apache-2.0 — see [`LICENSE`](LICENSE).
