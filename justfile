# rolter dev tasks - run `just <task>` (https://github.com/casey/just)

# list tasks
default:
    @just --list

# build the whole workspace
build:
    cargo build --workspace

# run all tests the way CI does: nextest for unit/integration tests plus a
# separate doc-test pass (nextest does not run doc tests). needs cargo-nextest
# (`cargo install cargo-nextest` or see https://nexte.st/docs/installation/).
test:
    cargo nextest run --workspace
    cargo test --doc --workspace

# format rust sources
fmt:
    cargo fmt --all

# lint with warnings as errors
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# create rolter.toml from the example if it does not exist yet
_config:
    #!/usr/bin/env bash
    if [ ! -f rolter.toml ]; then
        cp rolter.example.toml rolter.toml
        echo "[dev] created rolter.toml from rolter.example.toml"
    fi

# one-command dev stack: gateway (:4000) + control (:4001) + UI (:3000)
# creates rolter.toml on first run; no provider API keys needed to boot (the
# built-in fake-llm model works with the bundled `sk-rolter-dev` virtual key).
# uses bun when available (incl. ~/.bun/bin), else npm. ctrl-c stops all three.
dev: _config
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$HOME/.bun/bin:$PATH"
    if command -v bun >/dev/null 2>&1; then ui=bun; else ui=npm; fi
    if [ ! -d ui/node_modules ]; then ( cd ui && "$ui" install ); fi
    echo "[dev] UI http://localhost:3000  ·  gateway http://localhost:4000  ·  control http://localhost:4001"
    # kill the whole process group (all three children) on exit / ctrl-c
    trap 'kill 0' EXIT
    ( cargo run -p rolter-gateway -- --config rolter.toml 2>&1 | sed 's/^/[gateway] /' ) &
    ( cargo run -p rolter-control -- --config rolter.toml 2>&1 | sed 's/^/[control] /' ) &
    ( cd ui && "$ui" run dev 2>&1 | sed 's/^/[ui]      /' ) &
    wait

# run the data-plane gateway against rolter.toml
gateway config="rolter.toml":
    cargo run -p rolter-gateway -- --config {{config}}

# run the control plane + ui host
control:
    cargo run -p rolter-control

# install ui dependencies with bun
ui-install:
    cd ui && bun install

# ui dev server
ui-dev:
    cd ui && bun run dev

# build the ui to ui/dist
ui-build:
    cd ui && bun run build

# bring up postgres, redis, clickhouse and rolter
up:
    docker compose -f docker/docker-compose.yml up -d

# tear down the docker stack
down:
    docker compose -f docker/docker-compose.yml down

# supply-chain audit (advisories, bans, licenses, sources)
deny:
    cargo deny check --config .config/deny.toml

# run fmt, lint and tests like ci does
ci: fmt lint test
