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

DB schemas auto-apply on first start (`migrations/` → Postgres initdb, `clickhouse/` → ClickHouse initdb).

## Image

The multi-stage `docker/Dockerfile` produces a slim Debian runtime with both
binaries and the built UI at `/app/ui/dist`. Its default command is `rolter
easy-up`, so one image serves the gateway and dashboard with the built-in
`fake-llm` model — no compose file, provider key, or config mount required.

```bash
docker build -f docker/Dockerfile -t rolter:dev .
docker run --rm -p 4000:4000 -p 4001:4001 rolter:dev
```

Then open http://localhost:4001 and verify the data plane with:

```bash
curl -s http://localhost:4000/v1/chat/completions \
  -H 'Authorization: Bearer sk-rolter-dev' \
  -H 'Content-Type: application/json' \
  -d '{"model":"fake-llm","messages":[{"role":"user","content":"hello"}]}'
```

Override the command to run just the gateway or control plane:

```bash
docker run --rm -p 4000:4000 rolter:dev rolter-gateway --config /app/rolter.toml
docker run --rm -p 4001:4001 rolter:dev rolter-control
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
- Set a strong `ROLTER_MASTER_KEY`; provide DB/Redis/ClickHouse URLs via env or a secrets manager.
- Scale `gateway` horizontally; all replicas hot-reload config from Redis. ClickHouse and Postgres are shared.
- Kubernetes deployments are supported through the [rolter Helm chart](kubernetes.md).

## Air-gapped

Running fully offline behind an internal mirror (Nexus/Artifactory/Harbor)? See
[Air-gapped install & operation](air-gapped.md).
