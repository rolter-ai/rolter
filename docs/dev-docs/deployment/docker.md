# Docker deployment

## Compose (full stack)

`docker/docker-compose.yml` brings up Postgres, Redis, ClickHouse, and the rolter `gateway` + `control` services. It is the **local stack**, open on purpose: no admin token (`ROLTER_ALLOW_OPEN_MODE=1` acknowledges the `0.0.0.0` bind), the example `rolter`/`rolter` Postgres login, no Redis password, a passwordless ClickHouse user, every port published on every interface.

```bash
docker compose -f docker/docker-compose.yml up -d
docker compose -f docker/docker-compose.yml logs -f gateway
```

- Gateway: http://localhost:4000
- Control + UI: http://localhost:4001
- Postgres `5432`, Redis `6379`, ClickHouse `8123/9000`

The gateway follows the control plane (`ROLTER_SNAPSHOT_URL=http://control:4001/internal/snapshot`, plus `ROLTER_REDIS_URL` for immediate wake-ups), so a provider, route or virtual key created in the dashboard reaches it within a poll (5 s). It starts whether or not the control plane answers yet: the watcher logs a failed poll and retries, and `depends_on: control` only orders the start, since the distroless image has no shell to run a container healthcheck with. The control plane reads `ROLTER_GATEWAY_URL=http://gateway:4000` for the Playground's `/gw/*` proxy, whose default, `localhost:4000`, is the control container itself. `ROLTER_KEK` is passed through when exported; without it the dashboard refuses to store a provider key.

All three rolter services share one `build:` block that sets `CARGO_FEATURES=postgres`. That is also the Dockerfile default, so the published image gets it too (#2405). A control plane built with `CARGO_FEATURES=` has no `--database-url`, no CRUD API and no sign-in, so the Postgres next to it would sit unused; the image smoke in `quality.yml` fails if the image's `rolter-control --help` does not list `--database-url`. On a PR that smoke runs against the `runtime-prebuilt` target, whose binaries `rust build` compiles with the same `--features postgres`; `release.yml`'s `smoke image` job runs the same check against the `runtime` image of every published architecture. The feature is enabled on the launcher as well, so `rolter config export`, `mfa reset` and `kek verify` work from the image.

### Team shape

`docker/docker-compose.team.yml` is an override for one shared host:

```bash
docker compose -f docker/docker-compose.yml -f docker/docker-compose.team.yml \
               --env-file .env up -d --wait
```

It takes the values `rolter init` writes plus `ROLTER_PG_PASSWORD`, `ROLTER_REDIS_PASSWORD` and `ROLTER_CLICKHOUSE_PASSWORD` (`ROLTER_PG_USER` and `ROLTER_CLICKHOUSE_USER` default to `rolter`), and every required one is a `${VAR:?}`, so `up` stops on the first missing value and names it (`required variable ROLTER_KEK is missing a value`). The message after `:?` is left empty on purpose: GitGuardian flagged a hint there, beside a variable called `*_PASSWORD` or `*_TOKEN`, as a hardcoded password.

What it changes:

- **Project name** `rolter-team`. A volume keeps the credentials it was first created with, so the team stack must not reuse the local stack's `pgdata` and `chdata`.
- **Control plane:** `ROLTER_KEK`, `ROLTER_ADMIN_TOKEN`, `ROLTER_INTERNAL_TOKEN`, both peppers, datastore URLs with the passwords in them, `ROLTER_ALLOW_OPEN_MODE=0`. It still binds `0.0.0.0` inside the container, because the gateway and the published port both reach it over the container network. The published side decides who can connect: `${ROLTER_CONTROL_HOST:-127.0.0.1}:4001`, which is the variable `rolter init` already writes.
- **Internal port:** `ROLTER_INTERNAL_ADDR=0.0.0.0:4002` moves `/internal/*`, which carries decrypted provider keys, off the dashboard port. It is not published, so a reverse proxy in front of 4001 cannot forward it, and the admin token does not open it. The gateway polls `http://control:4002/internal/snapshot` with `ROLTER_INTERNAL_TOKEN`.
- **Gateway:** only `ROLTER_INTERNAL_TOKEN`, `ROLTER_KEY_PEPPER` (the snapshot carries no pepper, so a different value rejects every key with 401), and the Redis and ClickHouse URLs. It never holds the admin token, the KEK, the session pepper or the Postgres credentials, and it gets decrypted provider keys from the snapshot.
- **Datastores:** a password each (ClickHouse's `CLICKHOUSE_USER` other than `default` makes its image drop the passwordless user). Postgres is published on `127.0.0.1` so `rolter-seed` can reach it from the host; Redis and ClickHouse are not published. The override uses the `!override` and `!reset` tags, which need Compose 2.24. Healthchecks make `up --wait` mean something, and keep the control plane from starting before Postgres, Redis and ClickHouse answer.
- **Config file:** `docker/rolter.team.toml` is mounted over `/app/rolter.toml` in both planes and in preflight. It has `[server]` and nothing else. The baked-in example has the public `sk-rolter-dev` virtual key, and the control plane serves its bootstrap file's keys in the snapshot, so the gateway would accept it (#2408). The control plane now also prunes `sk-rolter-dev` from the snapshot whenever an admin token is set or `server.require_auth = true`, with a line on `/api/v1/config/problems`. Open mode (no token) still serves it, since that is what `easy-up` is for. The Helm chart mounts its `config.file` into the control plane and the preflight container rather than letting them read the baked example, `rolter init --profile production` writes a config with no `[[virtual_keys]]`, and `rolter check` fails on a file that declares the key.

The preflight service is handed the control plane's environment and runs `rolter check --strict`:

```bash
docker compose -f docker/docker-compose.yml -f docker/docker-compose.team.yml \
               --env-file .env --profile preflight run --rm preflight
```

`--env-file` is a flag of `docker compose`, not of `run`. `check_exposure` reads `ROLTER_CONTROL_HOST` and warns on `0.0.0.0`, and inside the container that is always `0.0.0.0`, so preflight gets the published address instead, which makes `ROLTER_CONTROL_HOST=0.0.0.0` in the env file fail the strict check, as it should.

Known gap: `rolter init` does not write the three datastore passwords (#2413). The startup log no longer carries them: every datastore URL the control plane and gateway print goes through `rolter_core::redact` (#2406, see [Security](../architecture/security.md)).

ClickHouse runs with `nofile` at `${CLICKHOUSE_NOFILE:-262144}`. Where the
container runtime cannot grant that much (rootless Docker or Podman, sandboxed
runners), creating the container fails with
`error setting rlimit type 7: operation not permitted`. Set `CLICKHOUSE_NOFILE`
lower, e.g. `20000`, in `docker/.env` and rerun (#1819). Compose reads that file
because the project directory is the first `-f` file's directory; a
repository-root `.env` is not read for interpolation unless you pass
`--env-file .env`. The development setup page has the full error text.

ClickHouse's own diagnostics are off. The image's default server config writes
`system.text_log`, `asynchronous_metric_log`, `metric_log`, `trace_log` and the
rest of the `system.*_log` family about the server itself, each with an insert
every few seconds. rolter reads none of them, and on a small host with a slow
disk the merges they cause never catch up: two idle servers on a Raspberry Pi 5
sat at 190% and 320% CPU with every `MergeMutate` thread busy on those tables
(#2795, found by the #1789 livetest). `docker/clickhouse/system-logs.xml` is
mounted into the `clickhouse` service as
`/etc/clickhouse-server/config.d/system-logs.xml` and removes them with
`remove="1"`, so the team stack and the dogfood stack, which layer over this
file, inherit it. The same file is mounted into the SigNoz overlay's ClickHouse
and the e2e stack's (#2815): SigNoz's query service and schema migrator read
`system.tables`, `columns`, `disks`, `clusters`, `databases`, `mutations` and
`distributed_ddl_queue`, never a `*_log` table, so nothing in it needs keeping
there either. `system.query_log` stays (the UX-event ingest test and
[UX telemetry](../development/ux-telemetry.md) read it), and so does
`crash_log`, which only ever gets a row when the server crashes. Mount a single
file, never the `config.d` directory: the image keeps its listen-address config
there, and a directory mount would hide it. A `remove="1"` on a table the
image's ClickHouse version does not have is a no-op (the file names
`session_log`, which 24.10's default config leaves commented out, and
`latency_log` and `s3queue_log`, which only 25.x has), so the list can name
tables a newer image adds.

The file only stops the tables being created. A volume that ran with the default
config keeps the ones it already has, and ClickHouse does not drop them. Drop
them once; the statement list is generated so that the `_0`, `_1` copies
ClickHouse leaves behind when an upgrade changes a table's schema go too. In the
team stack add `--user "$ROLTER_CLICKHOUSE_USER" --password
"$ROLTER_CLICKHOUSE_PASSWORD"` to both `clickhouse-client` calls and the team
`-f`/`--env-file` flags to `docker compose`:

```bash
ch() { docker compose -f docker/docker-compose.yml exec -T clickhouse clickhouse-client "$@"; }
ch -q "select 'drop table if exists system.' || name || ' sync;' from system.tables
       where database = 'system' and match(name, '^(aggregated_zookeeper|asynchronous_insert|asynchronous_metric|backup|background_schedule_pool|blob_storage|error|latency|metric|opentelemetry_span|part|processors_profile|query_metric|query_thread|query_views|s3queue|session|text|trace|zookeeper_connection)_log(_[0-9]+)?\$')" | ch -n
```

Nothing recreates them on the next start. The SigNoz overlay's volume is the
same story: its ClickHouse is `signoz-clickhouse`, and the drop above needs
`-f docker/docker-compose.signoz.yml` and that service name. The e2e stack's
ClickHouse has no volume, so it never has them to drop.

On a small host also cap ClickHouse's caches, which the default config sizes
for a large server (`uncompressed_cache_size` 8 GiB, `mark_cache_size` 5 GiB).
The right figure depends on the host, so rolter does not set it. Put it in a
file and layer a second compose file that mounts it, the same way the
system-log file is mounted:

```xml
<!-- docker/clickhouse/caches.xml -->
<clickhouse>
  <mark_cache_size>268435456</mark_cache_size>
  <uncompressed_cache_size>268435456</uncompressed_cache_size>
</clickhouse>
```

```yaml
# docker/docker-compose.caches.yml
services:
  clickhouse:
    volumes:
      - ./clickhouse/caches.xml:/etc/clickhouse-server/config.d/caches.xml:ro
```

```bash
docker compose -f docker/docker-compose.yml -f docker/docker-compose.caches.yml up -d
```

`select name, value from system.server_settings where name like '%cache_size'`
shows what the running server picked up. The relative path resolves against
`docker/` whichever file names it, because that is the project directory.

DB schemas auto-apply on first start, by two different routes. The Postgres
schema is owned by `sqlx::migrate!`, which `rolter-control` and `rolter-seed`
both run on startup; Compose deliberately does _not_ mount `migrations/` into
the container's `docker-entrypoint-initdb.d`, because letting Postgres apply
those files itself would bypass sqlx's `_sqlx_migrations` bookkeeping and make
the next startup replay every migration against a populated database (#499).
ClickHouse has no such runner, so `clickhouse/` _is_ mounted into its initdb
directory and is that schema's only provisioning path.

## Image

The multi-stage `docker/Dockerfile` produces a slim Debian runtime with both
binaries and the built UI at `/app/ui/dist`. Its default command is `rolter
easy-up`, so one image serves the gateway and dashboard with the built-in
`fake-llm` model — no compose file, provider key, or config mount required.

```bash
docker build -f docker/Dockerfile -t rolter:dev .
docker run --rm -p 127.0.0.1:4000:4000 -p 127.0.0.1:4001:4001 \
  -e ROLTER_ALLOW_OPEN_MODE=1 rolter:dev
```

The default command passes `--host 0.0.0.0` to `easy-up`. A published port
forwards to the container's own interface and never to its loopback, so
easy-up's loopback default would answer nothing from the host (#1891). The bind
is a flag on that one command rather than an image-wide `ROLTER_HOST`:
`ROLTER_HOST` outranks `[server] host`, so as an image default it would silently
rebind every `rolter-gateway` run from the image, whatever its config says.

That bind does not reopen #970. With no `ROLTER_ADMIN_TOKEN`, `easy-up` refuses
a non-loopback host before it starts anything, and the control plane refuses it
again on its own, unless `ROLTER_ALLOW_OPEN_MODE=1` acknowledges it. The
`127.0.0.1:` prefix keeps an acknowledged open container on the developer's
machine. Anything other hosts reach needs an admin token, and a virtual key of
its own in place of the bundled `sk-rolter-dev`, which is public and allows
every model. `docker/smoke/image-smoke.sh` checks all three states through
published ports: the blocking `rust build` job in `quality.yml` runs it on
every PR, and the release workflow's `smoke image` job runs it against each
built architecture.

On a PR the image under test is the Dockerfile's `runtime-prebuilt` target, not
`runtime` (#2037). Both build on one `runtime-base` stage that carries
everything above: the distroless `nonroot` base, `/app` as the working
directory, the bundled `/app/rolter.toml`, `ROLTER_UI_DIR=/app/ui/dist`, the two
exposed ports and the `easy-up --host 0.0.0.0` command. `runtime` compiles the
binaries and the dashboard in its builder stages; `runtime-prebuilt` copies
them from the named build contexts `rolter-bin` and `rolter-ui`, so CI smokes
the published layout without a cold release build on every call. CI builds it
on distroless debian13 (`--build-arg RUNTIME_DISTRO=debian13`), because binaries
compiled on its Ubuntu 24.04 runner need a newer glibc than debian 12 has; the
default, and every image that is built for use, stays on debian12. It is never
published. See
[testing](../development/testing.md#published-port-image-smoke) for the local
commands.

Then open http://localhost:4001 and verify the data plane with:

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H 'Authorization: Bearer sk-rolter-dev' \
  -H 'Content-Type: application/json' \
  -d '{"model":"fake-llm","messages":[{"role":"user","content":"hello"}]}'
```

Override the command to run just the gateway or control plane. The gateway
binds the `[server] host` of its config, `0.0.0.0` in the bundled example. The
control plane defaults to loopback, so inside a container it needs
`ROLTER_CONTROL_HOST=0.0.0.0`, and with that an admin token:

```bash
docker run --rm -p 4000:4000 rolter:dev rolter-gateway --config /app/rolter.toml

export ROLTER_ADMIN_TOKEN=$(openssl rand -hex 32)
echo "$ROLTER_ADMIN_TOKEN"   # the management API and the dashboard ask for it
docker run --rm -p 4001:4001 \
  -e ROLTER_CONTROL_HOST=0.0.0.0 -e ROLTER_ADMIN_TOKEN \
  rolter:dev rolter-control
```

## Published images

Release tags publish an image to **GHCR** (and, when configured, **Docker Hub**) under the same repo name and tags. Each release is tagged with its version and `latest`:

```bash
docker pull ghcr.io/<owner>/rolter:latest
docker pull ghcr.io/<owner>/rolter:0.0.4
```

Publishing is fail-closed and opt-in, mirroring the PyPI flow. The `publish-docker` job in `.github/workflows/release.yml` runs only when:

- repo variable `DOCKER_PUBLISH_ENABLED` = `true`, and
- the verify + external-check gates pass for the tagged commit.

GHCR always publishes via the built-in `GITHUB_TOKEN`. To also push to Docker Hub, set repo variable `DOCKERHUB_IMAGE` (e.g. `docker.io/acme/rolter`) and secrets `DOCKERHUB_USERNAME` / `DOCKERHUB_TOKEN`; the same tag set is applied to both registries. (Multi-arch images are a separate roadmap item — releases currently ship `linux/amd64`.)

## Production notes

- Put the gateway behind TLS (ingress/load balancer); keep the control plane private.
- Set a strong `ROLTER_KEK`; provide DB/Redis/ClickHouse URLs via env or a secrets manager.
- Scale `gateway` horizontally; all replicas hot-reload config from Redis. ClickHouse and Postgres are shared.
- Kubernetes deployments are supported through the [rolter Helm chart](kubernetes.md).

## Stopping

Both planes drain in-flight requests on `SIGTERM` and Ctrl-C (`SIGINT`), standalone and under `rolter easy-up`, which is the image's default command. Under `easy-up` each plane installs its own handler, so one signal starts both drains and the process exits `0` once both have finished; `docker stop` therefore returns promptly instead of waiting out the grace period for `SIGKILL`. If either plane fails, the command ends with that error. `crates/rolter/tests/easy_up_signals.rs` spawns the real binary and bounds the exit time.

## Air-gapped

Running fully offline behind an internal mirror (Nexus/Artifactory/Harbor)? See
[Air-gapped install & operation](air-gapped.md).
