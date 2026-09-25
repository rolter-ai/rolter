# Dogfooding fleet

A local rolter with a fleet that looks like a real one, for operator dogfooding
(#924). The short version, and the gotchas worth reading before a session, are
in [`docs/dev-docs/development/dogfooding-fleet.md`](../../docs/dev-docs/development/dogfooding-fleet.md);
this file is the detail.

The built-in `fake-llm` model answers "does the gateway work at all". It does
not answer "what is it like to run this", because one route with one target
exercises none of the screens an operator lives in — provider groups, routing
strategies, per-target health, key scoping. Every strategy looks identical when
every target answers in the same time.

So this stands up fifteen fake upstreams with deliberately uneven latencies, one
route per strategy worth looking at, and the whole session traced into SigNoz.

## What is here

| File            | What it is                                                                           |
| --------------- | ------------------------------------------------------------------------------------ |
| `fleet.ts`      | fifteen fake OpenAI-compatible upstreams on `127.0.0.1:18001-18015`                  |
| `dogfood.toml`  | the matching rolter config — fifteen providers, three provider groups, eleven routes |
| `keys.env`      | the API keys the fleet expects (fake, loopback-only, checked in on purpose)          |
| `ux-capture.sh` | applies `clickhouse/*.sql` and proves the dashboard UX capture end to end (#1728)    |

## The fleet

Three shapes, named the way each of them names things:

- **`:18001`** — OpenAI's model names (`gpt-4o`, `o3-mini`), added as an `openai` provider
- **`:18002`** — OpenRouter's `vendor/model` names, declared `kind = "openrouter"` with `allow_custom_api_base = true` so the dialect itself is exercised locally (#925)
- **`:18003-18015`** — a self-hosted vLLM/TEI fleet, one model per instance, roughly half behind a key

Three of them exist to make failure legible: `vllm-a100-03` is ~4x slower than
its pair, `vllm-spot-01` returns a 503 for a quarter of requests, and
`vllm-spot-02` takes ~1.4s to first token. A fleet with no bad targets leaves
the health, breaker and latency screens permanently green and unreadable.

## Running it

Bring up Postgres, Redis, ClickHouse and SigNoz:

```bash
docker compose -f docker/docker-compose.yml -f docker/docker-compose.signoz.yml \
  up -d postgres redis clickhouse signoz-zookeeper signoz-clickhouse \
        signoz-schema-migrator signoz-otel-collector signoz signoz-mcp
```

Start the fleet, then seed the database from the same config:

```bash
bun integration/dogfood/fleet.ts &
set -a; . integration/dogfood/keys.env; set +a
export ROLTER_DATABASE_URL=postgres://rolter:rolter@127.0.0.1:5432/rolter
cargo run -p rolter-control --features postgres --bin rolter-seed -- \
  --import integration/dogfood/dogfood.toml
```

> `--import` is desired state: re-importing an edited file updates the rows it
> already created, so the database ends up matching the file.

The three `[[provider_groups]]` in `dogfood.toml` are what makes `group-slug/model`
addressing exercisable — `vllm-a100` is the whole rack, `vllm-a100-fast` the
subset without the slow card (two providers are in both), and `openai-pool` is a
pool of one with a model name that carries no vendor prefix. Seeding them is
currently the _only_ way to get a group in front of the gateway: a group created
through the dashboard never bumps `config_version`, so it does not propagate
until the gateway restarts (#1643).

The gateway also needs `ROLTER_NODE_ID`. Without it, and without a `HOSTNAME`
(a shell-launched process has none; a container gets one from the runtime), its
cluster heartbeat and adaptive-routing telemetry are posted with no node header,
dropped by the control plane with a `204`, and logged nowhere — leaving the
Cluster and Adaptive Routing screens empty forever (#1644). `just dogfood` sets
it.

Run the control plane and the gateway, both exporting to the collector:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4317
export ROLTER_REDIS_URL=redis://127.0.0.1:6379 CLICKHOUSE_URL=http://127.0.0.1:8123
OTEL_SERVICE_NAME=rolter-control ROLTER_UI_DIR=ui/dist \
  ROLTER_UI_OTEL_ENDPOINT=https://otel.localhost/v1/traces \
  cargo run -p rolter-control --features postgres --bin rolter-control &
OTEL_SERVICE_NAME=rolter-gateway \
  ROLTER_SNAPSHOT_URL=http://127.0.0.1:4001/internal/snapshot \
  cargo run -p rolter-gateway -- --config integration/dogfood/dogfood.toml &
```

`[logging].clickhouse_url` in `dogfood.toml` is what makes the dashboard's
analytics screens fill; the control plane's `CLICKHOUSE_URL` only lets it
_read_ the table (#929).

## Proving the UX capture before a week of it

```bash
just dogfood-ux   # or ./integration/dogfood/ux-capture.sh
```

Signs in, posts a probe event through the real `POST /api/v1/ui-events`, reads it
back out of ClickHouse and says which hop broke if any did. Run it before
starting a capture week and again at the end of a session: `ui/src/lib/ux.ts`
swallows every failure by design, so a pipeline that stopped working looks
exactly like one nobody used.

It applies `clickhouse/*.sql` first. That directory is mounted into ClickHouse's
`docker-entrypoint-initdb.d`, which runs **only when the data directory is first
created** — so a `chdata` volume older than a migration has never seen it, and a
stack with no `ui_events` table captures nothing for the entire week while every
screen looks healthy. `just dogfood` applies them on every boot now for the same
reason; running the script by hand is for a stack that is already up.

What a mid-week outage costs, and which failures disable the stream permanently
rather than dropping one batch, is tabulated in
[`docs/dev-docs/development/ux-telemetry.md`](../../docs/dev-docs/development/ux-telemetry.md#failure-modes-and-who-notices).

## Browser tracing

The dashboard's own spans (#805) are posted from the operator's browser, so
the collector has to answer a cross-origin preflight and the page cannot be
`https` while the endpoint is `http`. Both are handled: the OTLP receiver in
`docker/signoz/otel-collector-config.yaml` allows loopback origins, and the
`.localhost` names below terminate TLS.

Without those, the spans are dropped by the browser before the collector sees
them and the only symptom is an empty `rolter-ui` service in SigNoz.

## URLs

With [portless](https://github.com/vercel-labs/portless):

```bash
portless alias rolter 4001 && portless alias api.rolter 4000
portless alias signoz 8080 && portless alias otel 4318
```

| URL                            | What                  |
| ------------------------------ | --------------------- |
| `https://rolter.localhost`     | dashboard             |
| `https://api.rolter.localhost` | gateway (`/v1/*`)     |
| `https://signoz.localhost`     | traces, metrics, logs |

## Credentials

`creds.env` is the only place a local credential is defined (#956). The
justfile, `provision-signoz.sh` and `sheet.sh` all read it, so changing it there
changes it everywhere. The dashboard and SigNoz share one login:

| Service                 | User               | Password                    |
| ----------------------- | ------------------ | --------------------------- |
| rolter dashboard        | `dev@rolter.local` | `rolter-dev-2026`           |
| SigNoz                  | `dev@rolter.local` | `rolter-dev-2026`           |
| postgres                | `rolter`           | `rolter`                    |
| redis, ClickHouse, OTLP | —                  | unauthenticated on loopback |

These are checked in and printed on every run. That is safe only because the
stack binds to loopback, talks to fake providers and holds nothing real — do not
carry them anywhere else, and leave `docker/docker-compose.yml` and the Helm
chart on their own defaults.

### Operator tokens

`creds.env` holds the human logins. The machine credentials — `ROLTER_ADMIN_TOKEN`,
`ROLTER_INTERNAL_TOKEN`, `ROLTER_KEY_PEPPER` and `ROLTER_SESSION_PEPPER` — are
**generated per machine** into `integration/dogfood/.tokens.env` on the first
`just dogfood` and are not checked in (#1649). Two of them are peppers: one is
mixed into every stored virtual-key digest and the other into every session, so
they must outlive a restart exactly like the KEK, and a shared checked-in value
would be the pepper of every developer's stack at once.

With them set, RBAC actually enforces and `/internal/*` moves to its own port
(`4002`) behind the internal token, which is how the e2e stack and a real
deployment run. Without them the operator API is open and the snapshot is served
on the public port, which is the configuration least able to surface an auth bug.
The dashboard login is unaffected — a session is a session, not a token — but a
hand-rolled `curl` against the operator API now needs
`-H "authorization: Bearer $ROLTER_ADMIN_TOKEN"`. `just dogfood-sheet` prints
all four.

`just dev-creds` brings an already-running stack in line without a full restart.
It also runs an `ALTER USER` on Postgres, which is necessary because
`POSTGRES_PASSWORD` is only applied when the data directory is first
initialised — an existing volume keeps whatever it was built with.

### SigNoz

`just dogfood` provisions SigNoz with that login and imports the dashboards in
`signoz/dashboards/`. Re-running is a no-op.

A SigNoz that already has a different account is left alone: rewriting the
credential store of a running service behind its own back is not something this
script does. It reports the mismatch and stops. To adopt the shared credential:

```bash
just signoz-reset   # drops SigNoz's users, dashboards and alerts, then provisions
```

Traces survive that — they live in ClickHouse, not in the database it removes.

The dashboards can always be imported by hand instead: **Dashboards → Import
JSON** in SigNoz, using the files in `signoz/dashboards/`.

| Dashboard                   | What it shows                                                                                              |
| --------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `rolter · overview`         | request rate, p95 and errors across gateway, control plane and dashboard; slowest operations               |
| `rolter · dashboard UX`     | the SPA's own browser tracing: which API calls fail, with which status, on which path                      |
| `rolter · gateway capacity` | provider queue wait p95, depth and in-flight calls; Redis connection state and requests admitted unchecked |

The first two query `signoz_traces`, and the capacity board queries the gateway's OTLP metrics in `signoz_metrics`. All use ClickHouse SQL rather than the query builder, so
they survive SigNoz changing the builder's shape between releases. The UX board
is the one that makes an auth fault obvious: a screen 401ing while every route
beside it returns 200 shows up as a wall of one status code, which is exactly
how #942 was found.
