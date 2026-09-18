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

# format the markdown, mdx, json and yaml outside ui/ (ui/ has `bun run format`)
fmt-docs:
    bash scripts/format-docs.sh --write

# check that formatting, the way the prek hook and CI do
fmt-docs-check:
    bash scripts/format-docs.sh --check

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

# run criterion benchmarks (hot-path: balancer pick + prefix trie)
bench:
    cargo bench --workspace

# compile benches without running them (the CI bit-rot guard)
bench-check:
    cargo bench --workspace --no-run

# Engine smoke tests. sim uses the lightweight vLLM API simulator (no
# downloads); the real engines use dummy weights (no model weights or provider
# secrets) but still download/cache the public model config+tokenizer.
integration-sim:
    integration/engines/run.sh sim

integration-vllm:
    integration/engines/run.sh vllm

integration-sglang:
    integration/engines/run.sh sglang

# Manual/CI-dispatch performance samples; artifacts land under artifacts/.
bench-sim:
    integration/engines/run.sh sim --bench

bench-vllm:
    integration/engines/run.sh vllm --bench

bench-sglang:
    integration/engines/run.sh sglang --bench

# Concurrency sweep: max sustainable RPS before the p99 knee, ITL and error rate
# under load (#847). Separate from bench-* because a sweep across five
# concurrency levels takes minutes. Tune with LOAD_LEVELS / LOAD_REQUESTS /
# LOAD_MAX_TOKENS; artifacts land under artifacts/load-*.json.
load-sim:
    integration/engines/run.sh sim --load

load-vllm:
    integration/engines/run.sh vllm --load

load-sglang:
    integration/engines/run.sh sglang --load

# harness unit tests (stdlib only, no engine or network needed)
test-bench:
    python3 -m unittest discover -s integration/engines -t integration/engines -v

# full-stack black-box e2e suite (#613): boots the compose stack + fake-vLLM
# fleet and drives the real HTTP APIs. heavy — not on the per-PR gate. needs
# docker + uv (https://docs.astral.sh/uv/).
e2e:
    cd integration/e2e && uv run pytest

# build the contributor mdbook to docs/dev-docs/book/. needs both binaries
# (`cargo install mdbook mdbook-mermaid --locked`) — without the preprocessor
# the build succeeds but every ```mermaid diagram ships as a code block.
docs:
    mdbook build docs/dev-docs

# live-reloading preview of the contributor mdbook
docs-serve:
    mdbook serve docs/dev-docs --port 3001 --open

# supply-chain audit (advisories, bans, licenses, sources)
deny:
    cargo deny check --config .config/deny.toml

# run fmt, lint and tests like ci does
ci: fmt lint test

# ── dogfooding stack (#924) ──────────────────────────────────────────────────
# a local rolter with a fleet that looks like a real one, for poking at the
# dashboard as an operator rather than as the person who wrote the screen.
#
# deliberately seeds **no** providers or routes: adding them is the thing being
# tested. `just dogfood-sheet` prints every endpoint, model and credential to
# copy-paste. see integration/dogfood/README.md.

# bring up the whole fuck-around stack and print the sheet
dogfood:
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$HOME/.bun/bin:$PATH"
    d=integration/dogfood

    # the KEK is per-machine and must outlive a restart: regenerating it would
    # orphan every provider key already stored under the old one
    [ -f "$d/.kek" ] || openssl rand -hex 32 > "$d/.kek"
    kek="$(cat "$d/.kek")"

    # RBAC only enforces once the control plane has an admin token, so a stack
    # started without one runs with the operator API wide open and /internal/*
    # on the public port — the one shape of bug dogfooding exists to catch
    # (#942 was invisible that way). the four values live in a generated file
    # rather than creds.env because two of them are peppers: a pepper that
    # changes invalidates every stored key digest and every live session, so
    # like the KEK they must outlive a restart of this stack (#1649)
    if [ ! -f "$d/.tokens.env" ]; then
      umask 077
      {
        echo "ROLTER_ADMIN_TOKEN=$(openssl rand -hex 32)"
        echo "ROLTER_INTERNAL_TOKEN=$(openssl rand -hex 32)"
        echo "ROLTER_KEY_PEPPER=$(openssl rand -hex 32)"
        echo "ROLTER_SESSION_PEPPER=$(openssl rand -hex 32)"
      } > "$d/.tokens.env"
    fi
    set -a; . "$d/.tokens.env"; set +a
    # /internal/* off the public port, as the e2e stack runs it (#636)
    export ROLTER_INTERNAL_ADDR=127.0.0.1:4002

    echo "[dogfood] docker: postgres, redis, clickhouse, signoz"
    docker compose -f docker/docker-compose.yml -f docker/docker-compose.signoz.yml \
      up -d postgres redis clickhouse signoz-zookeeper signoz-clickhouse \
            signoz-schema-migrator signoz-otel-collector signoz signoz-mcp

    echo "[dogfood] waiting for postgres"
    for _ in $(seq 1 60); do docker exec rolter-postgres-1 pg_isready -U rolter >/dev/null 2>&1 && break; sleep 1; done

    # one credential for every local service that has a human login (#956)
    set -a; . "$d/creds.env"; set +a

    export ROLTER_DATABASE_URL="postgres://$DEV_PG_USER:$DEV_PG_PASSWORD@127.0.0.1:5432/rolter"
    export ROLTER_KEK="$kek"
    echo "[dogfood] seeding org + admin user (no providers: add those yourself)"
    cargo run -q -p rolter-control --features postgres --bin rolter-seed -- \
      --admin-email "$DEV_EMAIL" --admin-password "$DEV_PASSWORD" >/dev/null

    [ -d ui/node_modules ] || ( cd ui && bun install )
    [ -d ui/dist ] || ( cd ui && bun run build )

    # portless gives stable .localhost names. an https page cannot post traces
    # to an http collector, so the browser tracing endpoint needs one too
    if command -v portless >/dev/null 2>&1; then
      portless alias rolter 4001 >/dev/null 2>&1 || true
      portless alias api.rolter 4000 >/dev/null 2>&1 || true
      portless alias signoz 8080 >/dev/null 2>&1 || true
      portless alias otel 4318 >/dev/null 2>&1 || true
    fi

    trap 'kill 0' EXIT
    set -a; . "$d/keys.env"; set +a
    export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4317
    export ROLTER_REDIS_URL=redis://127.0.0.1:6379 CLICKHOUSE_URL=http://127.0.0.1:8123

    ( bun "$d/fleet.ts" 2>&1 | sed 's/^/[fleet]   /' ) &
    ( OTEL_SERVICE_NAME=rolter-control ROLTER_UI_DIR=ui/dist \
        ROLTER_UI_OTEL_ENDPOINT=https://otel.localhost/v1/traces \
        ROLTER_UI_OTEL_SERVICE_NAME=rolter-ui \
        cargo run -q -p rolter-control --features postgres --bin rolter-control 2>&1 \
        | sed 's/^/[control] /' ) &
    sleep 6
    # ROLTER_NODE_ID is not optional in practice: without it (and without a
    # HOSTNAME, which a shell-launched process does not have) the gateway posts
    # its cluster heartbeat and its adaptive-routing telemetry with no node
    # header, the control plane drops both with a 204, and the Cluster and
    # Adaptive Routing screens are empty forever with nothing logged (#1644)
    # the snapshot now lives on the internal port, behind the internal token.
    # ROLTER_KEY_PEPPER must match control's: the snapshot carries no pepper, so
    # the gateway hashes a presented virtual key with its own and a mismatch
    # rejects every key with 401 "invalid api key"
    ( OTEL_SERVICE_NAME=rolter-gateway ROLTER_SNAPSHOT_URL=http://127.0.0.1:4002/internal/snapshot \
        ROLTER_NODE_ID=dogfood-gw-1 \
        cargo run -q -p rolter-gateway -- --config "$d/gateway.toml" 2>&1 \
        | sed 's/^/[gateway] /' ) &
    sleep 6
    just dogfood-key >/dev/null 2>&1 || true
    # non-fatal: a SigNoz that already has a different account is a thing to
    # report, not a reason to tear down a working stack
    ./"$d"/provision-signoz.sh || true
    ./"$d"/sheet.sh
    wait

# print every url, login and fleet endpoint for the dogfooding stack
dogfood-sheet:
    ./integration/dogfood/sheet.sh

# mint a gateway virtual key and remember it for the sheet
dogfood-key:
    #!/usr/bin/env bash
    set -euo pipefail
    c=http://127.0.0.1:4001/api/v1
    # sourced so the recipe works when run on its own, not only from `just
    # dogfood` where the environment already carries it
    if [ -f integration/dogfood/.tokens.env ]; then
      set -a; . integration/dogfood/.tokens.env; set +a
    fi
    # RBAC only enforces when the control plane has an admin token, and the
    # stack can be started either way. sending the header when one is set keeps
    # this recipe working on both instead of 401ing on the authenticated one.
    auth=()
    if [ -n "${ROLTER_ADMIN_TOKEN:-}" ]; then
      auth=(-H "authorization: Bearer $ROLTER_ADMIN_TOKEN")
    fi
    org=$(curl -fsS ${auth[@]+"${auth[@]}"} $c/orgs | python3 -c 'import json,sys;print(json.load(sys.stdin)[0]["id"])')
    team=$(curl -fsS ${auth[@]+"${auth[@]}"} $c/orgs/$org/teams | python3 -c 'import json,sys;print(json.load(sys.stdin)[0]["id"])')
    proj=$(curl -fsS ${auth[@]+"${auth[@]}"} $c/teams/$team/projects | python3 -c 'import json,sys;print(json.load(sys.stdin)[0]["id"])')
    curl -fsS ${auth[@]+"${auth[@]}"} -X POST $c/projects/$proj/virtual-keys -H 'content-type: application/json' \
      -d '{"name":"dogfood"}' \
      | python3 -c 'import json,sys;print(json.load(sys.stdin)["key"])' \
      > integration/dogfood/.virtual-key
    cat integration/dogfood/.virtual-key

# seed the full 15-provider fleet instead of adding it by hand
dogfood-seed:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a; . integration/dogfood/keys.env; set +a
    export ROLTER_DATABASE_URL=postgres://rolter:rolter@127.0.0.1:5432/rolter
    export ROLTER_KEK="$(cat integration/dogfood/.kek)"
    cargo run -q -p rolter-control --features postgres --bin rolter-seed -- \
      --import integration/dogfood/dogfood.toml

# bring an already-running stack in line with creds.env, and print it
dev-creds:
    #!/usr/bin/env bash
    set -euo pipefail
    d=integration/dogfood
    set -a; . "$d/creds.env"; set +a

    # postgres only honours POSTGRES_PASSWORD when it initialises an empty data
    # directory, so an existing volume keeps whatever it was built with. bring
    # it in line explicitly rather than pretending the env var did it.
    if docker exec rolter-postgres-1 pg_isready -U "$DEV_PG_USER" >/dev/null 2>&1; then
      docker exec -e PGPASSWORD="$DEV_PG_PASSWORD" rolter-postgres-1 \
        psql -U "$DEV_PG_USER" -d rolter -q \
        -c "alter user $DEV_PG_USER password '$DEV_PG_PASSWORD'" >/dev/null 2>&1 \
        && echo "[dev-creds] postgres password set" \
        || echo "[dev-creds] postgres unchanged (already correct, or not reachable)"
    fi

    export ROLTER_DATABASE_URL="postgres://$DEV_PG_USER:$DEV_PG_PASSWORD@127.0.0.1:5432/rolter"
    export ROLTER_KEK="$(cat "$d/.kek" 2>/dev/null || true)"
    echo "[dev-creds] rolter admin -> $DEV_EMAIL"
    cargo run -q -p rolter-control --features postgres --bin rolter-seed -- \
      --admin-email "$DEV_EMAIL" --admin-password "$DEV_PASSWORD" >/dev/null

    ./"$d"/provision-signoz.sh || true
    ./"$d"/sheet.sh

# provision SigNoz with the shared dev login and the checked-in dashboards
signoz-provision:
    ./integration/dogfood/provision-signoz.sh

# discard SigNoz's own database (users, dashboards, alerts) and provision it
# fresh. traces are in ClickHouse and are NOT touched by this.
signoz-reset:
    #!/usr/bin/env bash
    set -euo pipefail
    read -r -p "remove SigNoz users/dashboards/alerts and re-provision? [y/N] " a
    [ "$a" = "y" ] || [ "$a" = "Y" ] || { echo "cancelled"; exit 1; }
    c=docker/docker-compose.yml s=docker/docker-compose.signoz.yml
    docker compose -f $c -f $s stop signoz
    docker compose -f $c -f $s rm -f signoz
    docker volume ls -q --filter name=signoz | grep -E 'sqlite|signoz-db|signoz_db' \
      | xargs -r docker volume rm
    docker compose -f $c -f $s up -d signoz
    ./integration/dogfood/provision-signoz.sh

# tear the dogfooding stack down (keeps volumes, so the KEK stays valid)
dogfood-down:
    docker compose -f docker/docker-compose.yml -f docker/docker-compose.signoz.yml down
