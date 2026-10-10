# Air-gapped installation & operation

rolter is designed to run in fully air-gapped environments, with no public internet
at build or run time. One outbound call is on by default, the release check, and
`ROLTER_UPDATE_CHECK=false` turns it off. Every other call goes to an endpoint the
operator configures, and this page lists each one with the setting that controls
it and the code that makes it. It also covers how to install rolter behind an
internal mirror and how to verify the deployment offline.

The user-facing copy of this page is
[`docs/user-docs/deployment/air-gapped.mdx`](../../user-docs/deployment/air-gapped.mdx).
A new outbound path adds its row to both tables.

## Runtime egress

### The release check is on by default

Two callers ask `https://api.github.com/repos/rolter-ai/rolter/releases/latest`
whether a newer stable release exists:

- The control plane asks once at boot and every six hours. It feeds
  `GET /api/v1/version` and the dashboard footer's _v0.2.0 available_ hint (#902).
  Implemented in `crates/rolter-control/src/update_check.rs` (the checker, the
  semver order and the endpoint), spawned from `crates/rolter-control/src/lib.rs`.
- The `rolter` launcher starts the same fetch beside **every** subcommand
  (`update_notice::spawn()` in `crates/rolter/src/main.rs`, before the match),
  `gateway`, `easy-up`, `check` and `init` included, and prints a one-line stderr
  notice (#901). A successful answer is cached in `~/.cache/rolter/update-check.json`
  for 24 hours. Only a success is cached (`checked_at` is the last successful
  fetch), so a host with no route to GitHub retries on every invocation. The cache
  and the notice live in `crates/rolter/src/update_notice.rs`.

The `rolter-control` binary runs the control plane's checker. The standalone
`rolter-gateway` binary has none. The image's default command is `rolter easy-up`,
which runs both.

One request, a 5-second timeout, a `User-Agent: rolter/<version>` header and
nothing else: no installation id, config or credentials. Failures never log above
`debug` and never delay a command. `ROLTER_UPDATE_CHECK` set to `false`, `0`, `no`
or `off` (trimmed, case-insensitive) turns off both callers; unset leaves them on.
The endpoint then reports `enabled: false` and the footer shows the running
version alone.

The Helm chart writes `control.updateCheck` (default `true`) into both
`rolter.controlEnv` and `rolter.gatewayEnv`, so the preflight init container of each
Deployment (`rolter check`, the launcher) is covered by the one setting. The
`scripts/check-chart-update-check.py` step in `quality.yml` fails when a container
that runs the launcher lacks the variable.

### Every outbound path

Each row names what the path reaches, the setting that turns it on or off, its
default and the code that makes the call. Every row except the first is idle until
its setting is set.

| Path                        | What it reaches                                                                                                                                                            | Setting                                                                                                                                   | Default                          | Code                                                                                                                                                      |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Release check               | `api.github.com`, above                                                                                                                                                    | `ROLTER_UPDATE_CHECK=false` (Helm `control.updateCheck`)                                                                                  | **On**                           | `rolter-control/src/update_check.rs`, `rolter/src/update_notice.rs`                                                                                       |
| Upstream providers          | Each provider's `api_base`, the provider's `egress_proxy` / `egress_proxies`; a semantic cache's embeddings provider; the control plane's **Test** probe of a provider     | Providers and routes                                                                                                                      | None (`fake-llm` is built in)    | `rolter-proxy/src/lib.rs` (`Forwarder`), `rolter-proxy/src/egress_resolver.rs`, `rolter-gateway/src/realtime.rs`, `rolter-control/src/crud.rs` (the test) |
| Postgres                    | `ROLTER_DATABASE_URL`, from the control plane (`rolter check --connect` also opens a TCP connection)                                                                       | `ROLTER_DATABASE_URL`                                                                                                                     | Unset: in-memory store           | `rolter-store/src/postgres/mod.rs`, `rolter/src/preflight.rs`                                                                                             |
| Redis                       | `ROLTER_REDIS_URL` from both planes: budgets, rate limits, response cache, config pub/sub, the sign-in throttle                                                            | `ROLTER_REDIS_URL` / `--redis-url`                                                                                                        | Unset                            | `rolter-gateway/src/redis_conn.rs`, `watcher.rs`, `rolter-control/src/crud.rs` (`publish_config_change`), `login_throttle.rs`                             |
| ClickHouse                  | Request logs, payload capture, provider health events, analytics and alert queries                                                                                         | `CLICKHOUSE_URL` or `[logging].clickhouse_url`; a gateway with neither inherits the control plane's at boot                               | Unset: nothing is logged         | `rolter-gateway/src/logging.rs`, `health_events.rs`, `lib.rs`, `rolter-control/src/analytics.rs`                                                          |
| Gateway to control plane    | Snapshot poll, the log destination at boot, adaptive-routing telemetry (all to the snapshot URL's origin); the `/admin/*` proxy                                            | `ROLTER_SNAPSHOT_URL`, `ROLTER_SNAPSHOT_POLL_SECS` (5), `ROLTER_ADMIN_URL`                                                                | Unset: static bootstrap config   | `rolter-gateway/src/watcher.rs`, `adaptive_telemetry.rs`, `admin_proxy.rs`                                                                                |
| Control plane to gateway    | The Playground's `/gw/*` HTTP and WebSocket calls                                                                                                                          | `ROLTER_GATEWAY_URL`                                                                                                                      | `http://localhost:4000`          | `rolter-control/src/proxy.rs`                                                                                                                             |
| Alert channels              | Each enabled channel's `endpoint`, a JSON `POST` per state change; redirects off, egress policy checked                                                                    | **Alerting → Channels**, `/api/v1/alert-channels`                                                                                         | No channels                      | `rolter-control/src/alerting.rs` (`deliver`)                                                                                                              |
| OTLP export                 | The OTLP receiver, for traces, metrics and logs                                                                                                                            | `OTEL_EXPORTER_OTLP_ENDPOINT` or the `_TRACES_` / `_METRICS_` / `_LOGS_` variants; `ROLTER_TELEMETRY_ENABLED=false` blocks every exporter | Unset: no exporter is built      | `rolter-core/src/telemetry.rs`                                                                                                                            |
| Connector test              | The connector's `endpoint`: one empty OTLP `resourceLogs` request per **Test**, enabled or not. A collector you run does the export to a connector                         | **Observability → Connectors**                                                                                                            | No connectors                    | `rolter-control/src/connectors.rs` (`deliver_probe`)                                                                                                      |
| Dashboard browser tracing   | The OTLP/HTTP endpoint injected into the served page, called from each user's browser                                                                                      | `ROLTER_UI_OTEL_ENDPOINT`; `ROLTER_TELEMETRY_ENABLED=false` drops it                                                                      | Unset: no tracing code is loaded | `rolter-control/src/lib.rs` (`ui_runtime`), `ui/src/lib/telemetry.ts`                                                                                     |
| Active health probes        | Each provider's `api_base` plus `[health].path`; `also_track_via_llm_call` sends a one-token completion                                                                    | `[health] enabled`                                                                                                                        | `false`                          | `rolter-gateway/src/health.rs`                                                                                                                            |
| Engine metrics scrape       | Each provider's `api_base` plus `[metrics_scrape].path` (`/metrics`)                                                                                                       | `[metrics_scrape] enabled`                                                                                                                | `false`                          | `rolter-gateway/src/upstream_metrics.rs`                                                                                                                  |
| Provider status pages       | A provider's `status_page_url`, every `[health].status_page_interval_secs` (60), independent of `[health] enabled`                                                         | `status_page_url` on a provider                                                                                                           | Unset                            | `rolter-gateway/src/status_page.rs`                                                                                                                       |
| Cache-aware routing signals | A provider's `kv_events.endpoint` (a ZeroMQ `SUB` socket) and `lmcache.endpoint` (an HTTP poll every `refresh_secs`)                                                       | `kv_events`, `lmcache` on a provider                                                                                                      | Unset                            | `rolter-gateway/src/cache_telemetry.rs`                                                                                                                   |
| MCP servers                 | The `url` of each registered server, per call through `/mcp/{server}`                                                                                                      | The registered MCP servers                                                                                                                | None                             | `rolter-gateway/src/mcp_proxy.rs`                                                                                                                         |
| MCP OAuth                   | The server's protected-resource metadata and its authorization server's metadata at consent start, the token endpoint at consent end, and a refresh sweep every 60 seconds | MCP servers set to OAuth                                                                                                                  | None                             | `rolter-control/src/mcp_oauth_discovery.rs`, `mcp_oauth_flow.rs`                                                                                          |
| Single sign-on              | `<issuer>/.well-known/openid-configuration`, the `jwks_uri` and the `token_endpoint`, at each sign-in                                                                      | **Governance → Single Sign-On**                                                                                                           | No providers                     | `rolter-control/src/sso.rs`                                                                                                                               |
| Guardrail webhook           | The webhook URL, one `POST` per request before it goes upstream                                                                                                            | `[guardrail_webhook] enabled`, or an active provider in the registry                                                                      | Disabled                         | `rolter-gateway/src/guardrail_webhook.rs`, `rolter-core/src/guardrail_webhook.rs`                                                                         |
| PII sanitizer               | `[pii_sanitizer].url` and, with restoration on, `restore_url`                                                                                                              | `[pii_sanitizer] enabled`                                                                                                                 | Disabled                         | `rolter-gateway/src/pii_sanitizer.rs`                                                                                                                     |
| Webhook plugins             | The `endpoint` of each enabled plugin instance, per matching request at its stage                                                                                          | **Plugins**; an instance is live while enabled                                                                                            | No instances                     | `rolter-gateway/src/plugin_dispatch.rs`                                                                                                                   |

The egress policy (`[egress]`, see [security](../architecture/security.md#egress-policy-ssrf))
covers provider `api_base`, provider egress proxies, MCP server URLs, alert
channels, connectors and MCP OAuth. It does not yet cover SSO issuers, the
guardrail webhook, the PII sanitizer, plugin endpoints, status pages, KV-event
and LMCache endpoints (#2383).

These make no outbound call:

- **SCIM** is inbound only. The IdP calls `/scim/v2/*` (`crates/rolter-control/src/scim.rs`);
  nothing calls the IdP.
- **Invitations** send no email. rolter returns the link and the operator passes
  it on ([invitations](../architecture/invitations.md)).
- **Payload capture** writes to ClickHouse only, and only when
  `[logging.payload_capture]` is enabled.
- **Prometheus metrics** are scraped from `/metrics`; nothing is pushed.
- **Built-in guardrails** and the `fake-llm` model run in the gateway process.

### The dashboard and the API reference

- The interactive API reference at `/docs` embeds the Scalar JS bundle in the
  binary and sets `withDefaultFonts: false`, so it never reaches a CDN or
  `fonts.scalar.com`. This is asserted by the `docs_page_is_self_contained` and
  `scalar_bundle_is_embedded` tests in `crates/rolter-gateway/src/openapi.rs`.
- The dashboard SPA is served as static assets by the control plane; it loads no
  third-party scripts, fonts, or styles. The two typefaces are vendored through
  fontsource (`ui/src/lib/fonts.ts`) and the icons and link-preview image live in
  `ui/public/`. The footer's repository and bug-report buttons are plain `href`s
  to `github.com` that load nothing until clicked.

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
docker pull ghcr.io/rolter-ai/rolter:latest
docker save ghcr.io/rolter-ai/rolter:latest -o rolter.tar

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

**Set before you deploy:**

- `ROLTER_UPDATE_CHECK=false` on the control plane and wherever the `rolter`
  launcher runs. In Helm, `control.updateCheck: false` covers both Deployments.

**Must be reachable inside the enclave:**

- Every configured provider `api_base` (your internal model servers or a proxied
  provider endpoint).
- Postgres, Redis, and ClickHouse hosts — only for the features you enable.
- The control plane, if the gateway runs with `--snapshot-url`.
- Every endpoint you chose to configure from the table above: an alert webhook,
  an identity provider, an MCP server, a collector.

**Must NOT be required:**

- Public package registries at run time (only at install/build time, through the
  mirror).
- CDNs (`cdn.jsdelivr.net`, `fonts.scalar.com`, npm/unpkg) — rolter references none.
- `api.github.com` — with `ROLTER_UPDATE_CHECK=false` the release check is not
  attempted.
- Telemetry endpoints — unless you deliberately set `OTEL_*` to an internal
  collector.
- Provider status pages — leave `status_page_url` unset.

## Offline smoke test

Verify a running gateway with zero external providers using the built-in
`fake-llm` model (deterministic, no upstream or secrets needed):

```bash
ROLTER_UPDATE_CHECK=false rolter gateway --port 4000 &
curl -s http://localhost:4000/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"fake-llm","messages":[{"role":"user","content":"hello"}]}'
```

A `200` with a lorem-ipsum completion confirms the gateway serves traffic with no
outbound calls. Open `http://localhost:4000/docs` and confirm the API reference
renders with no network requests leaving the host.
