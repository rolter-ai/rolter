<p align="center">
  <img src="assets/logo.svg" alt="rolter" width="140" height="140">
</p>

<h1 align="center">rolter</h1>

<p align="center">
  A high-performance, open-source <b>LiteLLM-proxy alternative</b> in Rust —<br>
  an OpenAI/Anthropic-compatible <b>AI gateway</b> and load balancer.
</p>

<p align="center">
  <a href="https://github.com/rolter-ai/rolter/actions/workflows/ci.yml"><img src="https://github.com/rolter-ai/rolter/actions/workflows/ci.yml/badge.svg?branch=master" alt="CI"></a>
  <a href="https://github.com/rolter-ai/rolter/actions/workflows/release.yml"><img src="https://github.com/rolter-ai/rolter/actions/workflows/release.yml/badge.svg" alt="Release"></a>
  <a href="https://github.com/rolter-ai/rolter/actions/workflows/docs.yml"><img src="https://github.com/rolter-ai/rolter/actions/workflows/docs.yml/badge.svg?branch=master" alt="Documentation"></a>
  <a href="docs/development/testing.md#coverage"><img src="https://img.shields.io/badge/coverage%20baseline-64%25-yellowgreen" alt="Coverage baseline: 64%"></a>
</p>

<p align="center">
  <a href="https://github.com/rolter-ai/rolter/releases/latest"><img src="https://img.shields.io/github/v/release/rolter-ai/rolter" alt="Latest release"></a>
  <a href="https://crates.io/crates/rolter"><img src="https://img.shields.io/crates/v/rolter" alt="crates.io"></a>
  <a href="https://pypi.org/project/rolter/"><img src="https://img.shields.io/pypi/v/rolter" alt="PyPI"></a>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/MSRV-1.82-blue" alt="MSRV: Rust 1.82"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/rolter-ai/rolter" alt="License"></a>
</p>

---

rolter proxies commercial providers and load-balances self-hosted OpenAI-compatible fleets (e.g. 20–30 vLLM instances) with **cache-aware routing**, full RBAC, reload-free configuration, and cost/usage tracking.

> **Status:** active development. The gateway, Postgres-backed control plane, reload-free configuration, cost controls, reliability primitives, and core provider surfaces are implemented; remaining work is tracked in [`ROADMAP.md`](ROADMAP.md), [`TODO.md`](TODO.md), and the Linear project.

## Why rolter

- **Fast** — a Rust data plane (Axum/Hyper/Tower on Tokio) with lock-free config reads and minimal-copy streaming.
- **Cache-aware load balancing** — route prefix-heavy traffic to the vLLM replica most likely to have the KV cache warm.
- **Drop-in** — speak the OpenAI and Anthropic APIs your clients already use.
- **Operable** — virtual keys, budgets, rate limits, cost tracking, RBAC, and reload-free config changes from the UI.

## Quick start

One command brings up the gateway, control plane, and UI. No provider keys are needed — the built-in `fake-llm` model answers locally:

```bash
just dev          # from a source checkout (hot reload via cargo/bun)
rolter easy-up    # from an installed binary or the docker image
```

Then query the built-in model:

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H "Authorization: Bearer sk-rolter-dev" \
  -H "Content-Type: application/json" \
  -d '{"model":"fake-llm","messages":[{"role":"user","content":"hello"}]}'
```

Install methods, the `rolter` CLI reference, and configuration are in the [documentation](#documentation).

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

## Documentation

- [Quickstart](user-docs/quickstart.mdx) and [Installation](user-docs/installation.mdx) — install methods and the unified `rolter` CLI (`gateway` / `control` / `easy-up`)
- [Configuration](user-docs/configuration), [Deployment](user-docs/deployment), and [Observability](user-docs/observability) guides
- [Air-gapped install & operation](user-docs/deployment/air-gapped.mdx) — running fully offline behind an internal mirror
- [Architecture overview](docs/architecture/overview.md) — the full design and ADRs

## Repository layout

- `crates/rolter-core` — config model, domain types, errors, telemetry
- `crates/rolter-balancer` — load-balancing strategies (incl. approximate cache-aware)
- `crates/rolter-proxy` — upstream forwarding, header injection, streaming
- `crates/rolter-store` — repository traits + in-memory store (Postgres/Redis/ClickHouse next)
- `crates/rolter-auth` — virtual keys, roles, access checks
- `crates/rolter-gateway` — data-plane binary
- `crates/rolter-control` — control-plane binary + static UI host
- `crates/rolter` — unified `rolter` launcher (`gateway` / `control` / `easy-up`)
- `ui/` — Vite + React + shadcn/ui dashboard
- `docs/`, `user-docs/` — architecture/ADRs and the user documentation site
- `migrations/`, `clickhouse/` — database schemas

## Development

```bash
cargo build --workspace
cargo nextest run --workspace   # tests via nextest (as CI does); + `cargo test --doc --workspace`
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

Commits and PR titles follow [Conventional Commits](docs/development/commit-conventions.md). See [`AGENTS.md`](AGENTS.md) and [`docs/development/contributing.md`](docs/development/contributing.md).

## License

Apache-2.0 — see [`LICENSE`](LICENSE).
