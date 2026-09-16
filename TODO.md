# rolter TODO

Granular, incremental checklist. See [`ROADMAP.md`](ROADMAP.md) for phase intent and [`docs/`](docs/README.md) for design.

**An item is ticked in the pull request that ships it, never in a later sweep.**
A box left unchecked after the work landed is worse than no checklist at all: it
is read literally, most often by an agent, and it manufactures duplicate work —
#1054 was filed asserting there was no load-test harness months after the
harness shipped. Tick the box, and name the file, job or issue that proves it.

## Phase 0 — Scaffold & gateway MVP ✅
- [x] Cargo workspace + shared dependency/profile config
- [x] `rolter-core`: config model, errors, telemetry
- [x] `rolter-balancer`: `LoadBalancer` trait + round_robin/random/power_of_two/consistent_hash/cache_aware + trie + tests
- [x] `rolter-proxy`: pooled `Forwarder`, header injection, model rewrite, per-egress-proxy clients, streaming
- [x] `rolter-store`: `ConfigStore` trait + in-memory impl
- [x] `rolter-auth`: roles, constant-time key verify, model allow-list
- [x] `rolter-gateway`: `/healthz`, `/metrics`, `/v1/models`, chat/completions, completions, messages; virtual-key auth; arc-swap snapshot
- [x] `rolter-control`: health, `/api/v1/ping|roles|config`, static UI host
- [x] Bootstrap `rolter.example.toml`
- [x] Dockerfile (multi-stage, Bun UI) + docker-compose (pg/redis/clickhouse)
- [x] Postgres schema + ClickHouse logs schema
- [x] UI scaffold (Vite + React + shadcn/ui + Bun): Models/Keys/Logs
- [x] README, AGENTS, docs tree, ROADMAP, TODO
- [x] Conventional Commits: commitlint, pre-commit, PR/issue templates, CI
- [x] `cargo build`/`test`/`clippy` green in CI (#223)

## Phase 1 — Persistence & control plane ✅
- [x] `rolter-store` Postgres backend (`sqlx`) behind a `postgres` feature
- [x] Migration runner (`sqlx migrate` / refinery) replacing initdb-only
- [x] Repositories: orgs, teams, projects, providers, provider_keys, routes, route_targets, virtual_keys, budgets, rate_limits, model_prices
- [x] Control CRUD API for all of the above (Axum + validation)
- [x] Compose a runtime snapshot (`GatewayConfig`-shaped) from the DB
- [x] `GET /internal/snapshot?version=N` for gateways
- [x] Seed/bootstrap command (create org/admin, import `rolter.toml`)
- [x] Config vs DB model split (LiteLLM-style): bootstrap toml merged read-only over DB models, `GET/DELETE /api/v1/models`, 409 on config-owned mutations

## Phase 2 — Reload-free config ✅
- [x] Redis client + `PUBLISH`/`SUBSCRIBE` on `rolter.config` (control publishes on bump, gateway subscriber triggers instant refetch; polling stays as fallback)
- [x] Bump/read `config_version` transactionally on writes (migration 0003 DB triggers on providers/routes/targets/virtual-keys; control publishes the post-commit version to Redis)
- [x] Gateway watcher task: poll `/internal/snapshot?version=N` on an interval, `ArcSwap::store` on change (`--snapshot-url`)
- [x] Snapshot validation (`GatewayConfig::validate`): control refuses to serve an invalid snapshot, gateway refuses to apply one (keeps last good config)
- [x] Metrics for reload (`rolter_config_version`, `rolter_config_reloads_total`, `rolter_config_reload_failures_total`)

## Phase 3 — Auth & RBAC ✅
- [x] Local accounts: argon2id hashing, login, sessions (postgres-backed opaque bearer tokens)
- [x] RBAC middleware resolving most-specific membership per resource
- [x] Enforce roles on every control mutation
- [x] Pluggable `IdentityProvider` trait (`crates/rolter-auth/src/identity.rs`)
- [x] OAuth2/OIDC SSO (group→role mapping) (#240 — per-org providers, PKCE, `source`-tagged memberships, org login policy with superadmin break-glass)
- [x] Invitation onboarding: one-time links, invitee-chosen password (#712)
- [x] LDAP bind + group mapping (#241 — `crates/rolter-control/src/ldap.rs`; SCIM user and group provisioning shipped alongside it in `scim.rs` / `scim_groups.rs`)
- [x] Audit log writes + UI surface (`audit_log` table and repo methods, an RBAC capability, `ui/src/pages/AuditLog.tsx`)
- [x] Virtual-key hardening (pepper, constant-time lookup, expiry/rotation, scopes)

## Phase 4 — Cost, limits & pricing ✅
- [x] ClickHouse client + async batched writer off the hot path
- [x] Capture token usage (parse non-stream usage; accumulate for streams)
- [x] Pricing catalog CRUD + per-request `cost_usd`
- [x] Budgets enforcement (scope chain, most-restrictive-wins) with Redis spend counters
- [x] RPM/TPM rate limits via Redis (sliding window) with `429` + `retry-after`
- [x] Usage/cost aggregation queries for the dashboard

## Phase 5 — Reliability ✅
- [x] Retries (backoff + jitter) on 408/429/5xx, configurable
- [x] Circuit breaker per target (closed/open/half-open)
- [x] Cooldowns on rate-limited targets
- [x] Per-provider bounded queues with configurable backpressure (ROL-113)
- [x] Upstream health checks; skip unhealthy targets
- [x] In-flight load counters feeding `loads` to balancers
- [x] Weighted selection honoring `Target.weight`
- [x] Request timeouts + graceful shutdown/drain
- [x] vLLM/SGLang compatibility contracts and direct-vs-gateway baselines (#442 — `integration/engines/`)

## Phase 6 — Caching v2 ✅
- [x] Composable filter → weighted-score → argmax `Scorer` pipeline (foundation)
- [x] Cache-aware trie eviction (LRU / max-nodes; per-trie eviction counter)
- [x] Precise KV-event scorer (vLLM ZMQ, block hashing, resident-prefix fraction) (`precise_kv_cache` in `crates/rolter-balancer/src/scorer.rs`)
- [x] lmcache-aware strategy (controller occupancy) (`LmCacheScorer` in the same file)
- [x] Response cache: exact (Redis) with TTL + opt-in per route/key (`crates/rolter-gateway/src/cache.rs`)
- [x] Response cache: semantic (embeddings + cosine threshold) (`semantic_get` / `semantic_index_key`, same file)
- [x] `x-rolter-cache` + decision headers

## Phase 7 — Observability ✅
- [x] OpenTelemetry OTLP export for traces (`OTEL_*` env); metrics remain on the Prometheus `/metrics` scrape path
- [x] Inbound W3C `traceparent`/`b3` continuation; `request_id` end-to-end (`crates/rolter-gateway/src/trace.rs`)
- [x] Outbound trace-context propagation to vLLM/SGLang/TGI (`crates/rolter-proxy` forwards the caller's trace context)
- [x] Prometheus exporter: per-model latency histograms (TTFT/total), counters, config-version gauge
- [x] Federate/scrape upstream engine `/metrics` (queue depth → balancer load view)
- [x] Backend recipes: SigNoz, Datadog, Grafana, Langfuse (LLM traces + cost) (`docs/user-docs/observability/tracing.mdx`)
- [x] OTel Collector example config in `infra/` (`infra/otel/collector.yaml`, `collector.compose.yaml`)

## Phase 8 — Providers & modalities
- [x] Providers: Azure OpenAI, Bedrock, Vertex, Gemini, Mistral, Groq, OpenRouter (and ~40 more `ProviderKind` variants; see `docs/user-docs/configuration/`)
- [x] OpenAI⇄Anthropic request/response translation (+ streaming)
- [x] `/v1/embeddings` (OpenAI-compatible passthrough; built-in `fake-llm` serves deterministic vectors)
- [x] `/v1/images/generations` (OpenAI-compatible passthrough; built-in `fake-llm` returns a deterministic 1x1 png)
- [x] `/v1/audio/speech` (OpenAI-compatible TTS passthrough; built-in `fake-llm` returns a silent wav)
- [x] `/v1/audio/transcriptions`, `/v1/audio/translations` (multipart passthrough; routes on the `model` form field, forwards the upload verbatim)
- [x] `/v1/rerank` (Cohere/Jina-compatible passthrough; built-in `fake-llm` ranks deterministically)
- [ ] Pluggable custom AI APIs (generic passthrough + balancing) (#275)
- [x] Served OpenAPI document (`GET /openapi.json`, hand-authored 3.1) + interactive Scalar reference (`GET /docs`, bundle embedded in the binary — air-gapped safe)

## Phase 9 — Packaging & release ✅
- [x] Unified `rolter` CLI with `gateway`/`control` subcommands (one wheel ships both) (#277)
- [x] cibuildwheel/maturin-action wheels → PyPI (`uv tool install rolter`) (`build-wheels` + `publish-pypi` in `release.yml`, PEP 740 attestations; `rolter` 0.0.11 is on PyPI)
- [x] Publish crates to crates.io (#279)
- [x] Multi-arch images → GHCR (`build-image` in `release.yml`, amd64 + arm64)
- [x] Helm chart / K8s manifests (`charts/rolter/`, gated by the `helm chart` job in `quality.yml`)
- [x] Release automation from Conventional Commits (release-plz, per-crate changelogs — see [Changelogs](AGENTS.md#changelogs))
- [x] `cargo deny` + dependency/advisory scanning in CI (#283)

## Phase 10 — Control panel ✅
Full-featured hostable web control panel, not a read-only dashboard.
- [x] Zero-cred startup + runtime provider/model CRUD with encrypted keys (#454) (provider `api_key` via API sealed with `ROLTER_KEK`, `PUT /providers/{id}`, `ROLTER_ADMIN_TOKEN` guard on CRUD + snapshot, gateway `/admin/*` proxy)
- [x] Auth screens (login, SSO): login page renders password form and/or provider buttons from `GET /api/v1/auth/methods`; `/invite/{token}` accept screen
- [x] CRUD: providers, routes (+ targets/strategy), virtual keys, budgets, pricing (members CRUD blocked on Phase 3 accounts)
- [x] Model management UI: add/edit/enable-disable/delete models + provider/route binding
- [x] User & team management UI: create/invite/edit/deactivate users, assign roles/teams (`ui/src/pages/Users.tsx`, `Teams.tsx`, `UserProvisioning.tsx`)
- [x] End-user self-service panel: personal API keys + usage/spend view (`ui/src/pages/Account.tsx`)
- [x] In-UI config editing with reload-free apply + validation feedback (route admin params/policy)
- [x] Cost/usage dashboards (ClickHouse), latency percentiles, error rates (per-request logs explorer still needed — no drill-down endpoint exists yet)
- [x] Org/team/project switcher; role-aware UI (role-aware UI blocked on Phase 3 RBAC)
- [x] `bun run lint`/build wired into CI (#289)

## Cross-cutting / tech debt
- [x] Full-stack Docker Compose smoke test in CI (#449 — `compose-smoke` in `quality.yml`)
- [x] Publish Rust coverage and establish a ratcheting threshold (#450 — `coverage` in `quality.yml`, `.github/scripts/coverage-ratchet.sh`)
- [x] Document and enforce the `ci-ok` branch-protection policy (#448 — `docs/dev-docs/development/testing.md`)
- [x] Publish `rolter` to PyPI (`rolter` 0.0.11 is live; `uv tool install rolter`). All crates publish to crates.io at the same version through release-plz.
- [x] Integration tests for the gateway (mock upstream) + streaming assertions
- [x] `criterion` benches for `pick`/trie (#436)
- [x] `oha`/`k6` load-test harness (#291 — `just load-sim` / `bench-*`, `integration/engines/run.sh --load`)
- [x] Structured error type surfaced as OpenAI-style JSON everywhere (`crates/rolter-gateway/src/error.rs`)
- [x] Config schema validation + helpful startup errors (`GatewayConfig::validate`, plus `rolter check` for pre-boot validation)
- [ ] Secret backends (Vault/cloud KMS) behind the encryption trait (#294)
- [x] Guardrails (PII/content/prompt-injection) hooks (`crates/rolter-core/src/guardrails.rs`, `rolter-gateway/src/guardrails.rs`, guardrail screens in the dashboard)
- [ ] A/B traffic mirroring (#296)

## MCP gateway
Shipped, no longer a stretch item: `mcp_proxy.rs`, `mcp_oauth.rs`,
`mcp_oauth_flow.rs` and `mcp_logs.rs` in the control plane, with catalog,
library, logs and settings screens in the dashboard.
- [x] Proxy MCP tool servers through rolter with per-key auth (Streamable HTTP/SSE with user-bound OAuth)
- [ ] stdio and WebSocket MCP transports (#952 covers credential and transport configuration)
- [ ] Connection status, tool discovery and a try-it surface in the dashboard (#951)

## Stretch
Beyond the core phased roadmap.
- [ ] Rust SDK: client library for rolter gateway + control API (#421)
- [ ] Python SDK: client library for rolter gateway + control API (#422)
- [ ] JS/TS SDK: client library for rolter gateway + control API (#851)
- [ ] A2A gateway: agent-to-agent protocol bridge through rolter (#424)
- [ ] Multi-region deployment: cross-region routing + config sync
