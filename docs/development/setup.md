# Development setup

## Prerequisites

- **Rust** (stable) via [rustup](https://rustup.rs) — the workspace pins the toolchain in `rust-toolchain.toml`.
- **Bun** for the UI — `curl -fsSL https://bun.sh/install | bash`.
- **prek** for repository Git hooks — install with `uv tool install prek` or `brew install prek`.
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

## Build these docs

This book is mdBook. Diagrams are ```mermaid fences rendered by the
`mdbook-mermaid` preprocessor, so both tools have to be on `PATH` — with
`mdbook` alone the build still succeeds and every diagram silently ships as a
plain code block.

```bash
cargo install mdbook mdbook-mermaid --locked
just docs         # build to docs/book/ (gitignored)
just docs-serve   # live-reloading preview on http://localhost:3001
```

The mermaid runtime is vendored at `docs/mermaid.min.js` and
`docs/mermaid-init.js` so the book renders air-gapped. `mdbook-mermaid install
docs` regenerates both, but `mermaid-init.js` carries a local fix — upstream
still binds theme buttons by their pre-0.5 ids (`ayu`, `navy`, …), which now
throws and leaves diagrams on the light palette after a theme switch — so
re-apply it after any refresh. Write labels in
mermaid's own dialect rather than GitHub's — quote any label containing `/`
(`R["/v1/responses"]`, since `[/…]` is parallelogram syntax) and break lines
with `<br/>`, never `\n`.

## Before committing

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace && cargo test --doc --workspace   # or `just test`
prek install --prepare-hooks
prek run --all-files
prek run --all-files --hook-stage pre-push
```

The hooks add staged-file hygiene and secret scanning, Conventional Commit
validation, Rust/workflow/TOML/spelling checks, workspace tests, dependency
policy checks, and UI lint/build checks. Install the system tools used by the
project-specific hooks:

```bash
brew install actionlint taplo typos-cli
cargo install cargo-nextest cargo-deny
```

`cargo-nextest` is recommended but optional for the push hook; it falls back to
`cargo test`. CI remains authoritative for database-backed tests that need
`ROLTER_TEST_DATABASE_URL`.
