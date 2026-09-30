# Docker deployment

## Compose (full stack)

`docker/docker-compose.yml` brings up Postgres, Redis, ClickHouse, and the rolter `gateway` + `control` services.

```bash
cp .env.example .env            # set OPENAI_API_KEY etc.
docker compose -f docker/docker-compose.yml up -d
docker compose -f docker/docker-compose.yml logs -f gateway
```

- Gateway: http://localhost:4000
- Control + UI: http://localhost:4001
- Postgres `5432`, Redis `6379`, ClickHouse `8123/9000`

ClickHouse runs with `nofile` at `${CLICKHOUSE_NOFILE:-262144}`. Where the
container runtime cannot grant that much (rootless Docker or Podman, sandboxed
runners), creating the container fails with
`error setting rlimit type 7: operation not permitted`. Set `CLICKHOUSE_NOFILE`
lower, e.g. `20000`, in `docker/.env` and rerun (#1819). Compose reads that file
because the project directory is the first `-f` file's directory; a
repository-root `.env` is not read for interpolation unless you pass
`--env-file .env`. The development setup page has the full error text.

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
published ports: the blocking `image-smoke` job in `quality.yml` runs it on
every PR, and the release workflow's `smoke image` job runs it against each
built architecture.

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
