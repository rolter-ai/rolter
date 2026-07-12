# Air-gapped installation & operation

rolter is designed to run in fully air-gapped environments — no public internet
at build or run time. The runtime is egress-free by default: it talks only to the
backends you configure (upstream providers, Postgres, Redis, ClickHouse) and to
nothing else. This page covers how to install rolter behind an internal mirror,
what the runtime does and does not reach, and how to verify the deployment offline.

## Runtime egress guarantees

rolter makes outbound network calls **only** to endpoints you configure:

- **Upstream providers** — the `base_url` of each configured provider.
- **Postgres / Redis / ClickHouse** — only when their URLs are set
  (`DATABASE_URL` / `ROLTER_DATABASE_URL`, `REDIS_URL`, `clickhouse_url`).
- **Control-plane snapshot** — only when the gateway is started with
  `--snapshot-url` (and Redis pub/sub only with `--redis-url`).
- **OTLP traces** — only when an `OTEL_EXPORTER_OTLP_ENDPOINT` /
  `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` is set. With no `OTEL_*` env, no exporter
  is built and there is zero tracing egress.

Everything else is self-contained:

- The interactive API reference at `/docs` embeds the Scalar JS bundle in the
  binary and sets `withDefaultFonts: false`, so it never reaches a CDN or
  `fonts.scalar.com`. This is asserted by the `docs_page_is_self_contained` and
  `scalar_bundle_is_embedded` tests in `crates/rolter-gateway/src/openapi.rs`.
- The dashboard SPA is served as static assets by the control plane; it loads no
  third-party scripts, fonts, or styles.

> **Caveat — status-page pollers.** If a provider sets `status_page_url`, the
> gateway periodically fetches that URL as a secondary health signal. Leave
> `status_page_url` unset in air-gapped configs (or point it at an internal
> mirror) so the poller stays inside the enclave.

## Install paths through a mirroring proxy

Air-gapped sites usually proxy public registries through an internal mirror
(Sonatype Nexus, JFrog Artifactory, Harbor, …). Pick the path that matches how
you ship rolter.

### Docker image (recommended)

Pull through a registry that proxies GHCR/Docker Hub:

```bash
docker pull registry.internal.example/rolter/rolter:latest
```

Or transfer a fully offline image with `docker save` / `docker load`:

```bash
# on a connected host
docker pull ghcr.io/ormeilu/rolter:latest
docker save ghcr.io/ormeilu/rolter:latest -o rolter.tar

# copy rolter.tar into the enclave, then
docker load -i rolter.tar
```

### PyPI wheel (`uv tool install` / `pip`)

Install through a Nexus/Artifactory PyPI proxy:

```bash
uv tool install rolter --index-url https://nexus.internal.example/repository/pypi/simple
# or
pip install rolter --index-url https://nexus.internal.example/repository/pypi/simple
```

Or install a downloaded wheel with no index at all:

```bash
uv tool install ./rolter-<version>-py3-none-any.whl
# or
pip install --no-index ./rolter-<version>-py3-none-any.whl
```

### crates.io (`cargo install`)

Point Cargo at a registry mirror or vendored sources via `.cargo/config.toml`:

```toml
# .cargo/config.toml
[source.crates-io]
replace-with = "internal"

[source.internal]
registry = "sparse+https://nexus.internal.example/repository/cargo/"
```

For a fully offline build, vendor the dependency sources on a connected host and
copy them in:

```bash
cargo vendor vendor/                 # connected host, writes a [source] snippet
# copy vendor/ into the enclave, add the printed snippet to .cargo/config.toml
cargo build --workspace --offline
```

### Building from source (cargo + bun)

The Rust build follows the crates.io section above. The UI needs an internal npm
mirror for `bun install`:

```toml
# ui/bunfig.toml
[install]
registry = "https://nexus.internal.example/repository/npm/"
```

```ini
# alternatively ui/.npmrc
registry=https://nexus.internal.example/repository/npm/
```

```bash
cd ui && bun install && bun run build
cargo build --workspace --release --offline
```

## Operator checklist

**Must be reachable inside the enclave:**

- Every configured provider `base_url` (your internal model servers or a proxied
  provider endpoint).
- Postgres, Redis, and ClickHouse hosts — only for the features you enable.
- The control plane, if the gateway runs with `--snapshot-url`.

**Must NOT be required:**

- Public package registries at run time (only at install/build time, through the
  mirror).
- CDNs (`cdn.jsdelivr.net`, `fonts.scalar.com`, npm/unpkg) — rolter references none.
- Telemetry endpoints — unless you deliberately set `OTEL_*` to an internal
  collector.
- Provider status pages — leave `status_page_url` unset.

## Offline smoke test

Verify a running gateway with zero external providers using the built-in
`fake-llm` model (deterministic, no upstream or secrets needed):

```bash
rolter gateway --port 4000 &
curl -s http://localhost:4000/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"fake-llm","messages":[{"role":"user","content":"hello"}]}'
```

A `200` with a lorem-ipsum completion confirms the gateway serves traffic with no
outbound calls. Open `http://localhost:4000/docs` and confirm the API reference
renders with no network requests leaving the host.
