# Development setup

## Prerequisites

- **Rust** (stable) via [rustup](https://rustup.rs) — the workspace pins the toolchain in `rust-toolchain.toml`.
- **Bun** for the UI — `curl -fsSL https://bun.sh/install | bash`.
- **prek** for repository Git hooks — install with `uv tool install prek` or `brew install prek`.
- **Docker** + Compose for Postgres/Redis/ClickHouse.
- **uv** (optional) for the PyPI-wheel install path and tooling.

## Clone & build

```bash
git clone https://github.com/rolter-ai/rolter.git
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

ClickHouse asks for 262144 open files, its production recommendation. A
container runtime cannot grant more than its own hard limit, and rootless
Docker or Podman, sandboxed CI runners and locked-down VMs often have less. There
the ClickHouse container is never created:

```text
OCI runtime create failed: runc create failed: unable to start container process:
error during container init: error setting rlimits for ready process:
error setting rlimit type 7: operation not permitted
```

Lower the limit with `CLICKHOUSE_NOFILE`. `20000` is plenty for a dev stack
(#1819). Put it in `docker/.env` (gitignored), so every later compose call
renders the same limit:

```bash
echo CLICKHOUSE_NOFILE=20000 >> docker/.env
docker compose -f docker/docker-compose.yml up -d
```

It has to be `docker/.env`, not a `.env` at the repository root.
`docker compose -f docker/docker-compose.yml` takes its project directory from
the first `-f` file, so it interpolates from the shell environment and from
`docker/.env` only; a root `.env` is read only when you pass `--env-file .env`.
The `just` recipes run compose the same way, so `docker/.env` covers them too.

A one-off prefix such as `CLICKHOUSE_NOFILE=20000 just dogfood` also works, but
only for that command. The next compose call without it (`just up`, or
`just signoz-reset` after a failed SigNoz provisioning) renders 262144 again,
and compose recreates the ClickHouse container at that limit, where the runtime
refuses it.

The same variable covers SigNoz's ClickHouse in `docker/docker-compose.signoz.yml`
and the e2e stack's. The e2e compose file lives in `integration/e2e/`, so its
persistent spot is `integration/e2e/.env`.

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
just docs         # build to docs/dev-docs/book/ (gitignored)
just docs-serve   # live-reloading preview on http://localhost:3001
```

The mermaid runtime is vendored at `docs/dev-docs/mermaid.min.js` and
`docs/dev-docs/mermaid-init.js` so the book renders air-gapped. `mdbook-mermaid
install docs/dev-docs` regenerates both, but `mermaid-init.js` carries a local
fix — upstream still binds theme buttons by their pre-0.5 ids (`ayu`, `navy`,
…), which now throws and leaves diagrams on the light palette after a theme
switch — so re-apply it after any refresh. Write labels in
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

The clippy line above only builds the _default_ feature set, so it cannot see a
lint that fires under one feature combination alone — `dead_code` on a helper
whose only caller is `#[cfg(feature = "otlp")]`, for instance. CI closes that
gap in the `rust lint` job, whose `cargo hack (each feature)` step runs `cargo hack` with
`RUSTFLAGS=-D warnings`. Reproduce a failure from it with:

```bash
cargo install cargo-hack
RUSTFLAGS="-D warnings" cargo hack check --each-feature --workspace --all-targets
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
`ROLTER_TEST_DATABASE_URL`; to run them locally, `eval "$(just test-pg)"` starts
the machine's shared test Postgres and exports the variable (see
[testing.md](testing.md#the-postgres-test-database)).

### When the hooks skip Rust

The `rust-tests`, `cargo-deny` and `cargo-clippy-postgres` push hooks, and the
`cargo-fmt` and `cargo-clippy` commit hooks, run through
`scripts/prek-rust-gate.sh`, which skips them when the change touches no
Rust input — so a dashboard- or docs-only push does not wait on a cold
`--all-features` build of the whole workspace (#1486). A push counts as touching
Rust when any changed, added, deleted or renamed path is one of:

- anything under `crates/`, any `*.rs`, any `Cargo.toml` or `Cargo.lock`
- `rust-toolchain(.toml)`, `.cargo/`, `.config/deny.toml`, `.config/nextest.toml`
- `prek.toml` and the two `scripts/prek-rust-*.sh` scripts
- the files a Rust test reads from outside its crate: `rolter.example.toml`,
  `docs/dev-docs/development/stability-markers.md` and `ui/src/lib/nav.tsx`

A push hook inspects the range prek hands it
(`PRE_COMMIT_FROM_REF...PRE_COMMIT_TO_REF`). A commit hook gets no range, so it
passes `--staged` and the gate reads the index (`git diff --cached`) instead.
Whenever there is nothing to inspect — `prek run --all-files`, an empty index, a
push with no remote ancestor, a ref that does not resolve — the hooks run in
full, and `ROLTER_PREK_RUST_ALWAYS=1` forces them. The gate is a script rather
than a `files =` or `types =` filter because prek drops deleted paths before
matching, so a change that only removes a `.rs` file would otherwise skip the
fmt, clippy and test runs it can break (#1526).

When a Rust test starts reading a new file from outside `crates/`, add the path
to the list in `scripts/prek-rust-gate.sh` and here. The hook only saves local
time: hosted `ci-ok` runs the full suite on every pull request regardless.
