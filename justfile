# rolter dev tasks - run `just <task>` (https://github.com/casey/just)

# list tasks
default:
    @just --list

# build the whole workspace
build:
    cargo build --workspace

# run all unit tests
test:
    cargo test --workspace

# format rust sources
fmt:
    cargo fmt --all

# lint with warnings as errors
lint:
    cargo clippy --workspace --all-targets -- -D warnings

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
    docker compose up -d

# tear down the docker stack
down:
    docker compose down

# run fmt, lint and tests like ci does
ci: fmt lint test
