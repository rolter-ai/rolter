# rolter TODO

Granular, incremental checklist. See [`ROADMAP.md`](ROADMAP.md) for phase intent and [`docs/`](docs/README.md) for design.

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
- [ ] `cargo build`/`test`/`clippy` green in CI (verified locally; confirm on first push)

## Phase 1 — Persistence & control plane
- [x] `rolter-store` Postgres backend (`sqlx`) behind a `postgres` feature
- [x] Migration runner (`sqlx migrate` / refinery) replacing initdb-only
- [x] Repositories: orgs, teams, projects, providers, provider_keys, routes, route_targets, virtual_keys, budgets, rate_limits, model_prices
- [x] Control CRUD API for all of the above (Axum + validation)
- [x] Compose a runtime snapshot (`GatewayConfig`-shaped) from the DB
- [x] `GET /internal/snapshot?version=N` for gateways
- [x] Seed/bootstrap command (create org/admin, import `rolter.toml`)
- [x] Config vs DB model split (LiteLLM-style): bootstrap toml merged read-only over DB models, `GET/DELETE /api/v1/models`, 409 on config-owned mutations

## Phase 2 — Reload-free config
- [x] Redis client + `PUBLISH`/`SUBSCRIBE` on `rolter.config` (control publishes on bump, gateway subscriber triggers instant refetch; polling stays as fallback)
- [x] Bump/read `config_version` transactionally on writes (migration 0003 DB triggers on providers/routes/targets/virtual-keys; control publishes the post-commit version to Redis)
- [x] Gateway watcher task: poll `/internal/snapshot?version=N` on an interval, `ArcSwap::store` on change (`--snapshot-url`)
- [x] Snapshot validation (`GatewayConfig::validate`): control refuses to serve an invalid snapshot, gateway refuses to apply one (keeps last good config)
- [x] Metrics for reload (`rolter_config_version`, `rolter_config_reloads_total`, `rolter_config_reload_failures_total`)

## Phase 3 — Auth & RBAC
- [ ] Local accounts: argon2id hashing, login, sessions/JWT
- [ ] RBAC middleware resolving most-specific membership per resource
- [ ] Enforce roles on every control mutation
- [ ] Pluggable `IdentityProvider` trait
- [ ] OAuth2/OIDC SSO (group→role mapping)
- [ ] LDAP bind + group mapping
- [ ] Audit log writes + UI surface
- [x] Virtual-key hardening (pepper, constant-time lookup, expiry/rotation, scopes)

## Phase 4 — Cost, limits & pricing
- [x] ClickHouse client + async batched writer off the hot path
- [x] Capture token usage (parse non-stream usage; accumulate for streams)
- [x] Pricing catalog CRUD + per-request `cost_usd`
- [x] Budgets enforcement (scope chain, most-restrictive-wins) with Redis spend counters
- [x] RPM/TPM rate limits via Redis (sliding window) with `429` + `retry-after`
- [x] Usage/cost aggregation queries for the dashboard

## Phase 5 — Reliability
- [x] Retries (backoff + jitter) on 408/429/5xx, configurable
- [x] Circuit breaker per target (closed/open/half-open)
- [x] Cooldowns on rate-limited targets
- [x] Upstream health checks; skip unhealthy targets
- [x] In-flight load counters feeding `loads` to balancers
- [x] Weighted selection honoring `Target.weight`
- [x] Request timeouts + graceful shutdown/drain

## Phase 6 — Caching v2
- [x] Composable filter → weighted-score → argmax `Scorer` pipeline (foundation)
- [x] Cache-aware trie eviction (LRU / max-nodes; per-trie eviction counter)
- [ ] Precise KV-event scorer (vLLM ZMQ, block hashing, resident-prefix fraction)
- [ ] lmcache-aware strategy (controller occupancy)
- [ ] Response cache: exact (Redis) with TTL + opt-in per route/key
- [ ] Response cache: semantic (embeddings + cosine threshold)
- [ ] `x-rolter-cache` + decision headers

## Phase 7 — Observability
- [x] OpenTelemetry OTLP export for traces (`OTEL_*` env); metrics remain on the Prometheus `/metrics` scrape path
- [ ] Inbound W3C `traceparent`/`b3` continuation; `request_id` end-to-end
- [ ] Outbound trace-context propagation to vLLM/SGLang/TGI
- [x] Prometheus exporter: per-model latency histograms (TTFT/total), counters, config-version gauge
- [x] Federate/scrape upstream engine `/metrics` (queue depth → balancer load view)
- [ ] Backend recipes: SigNoz, Datadog, Grafana, Langfuse (LLM traces + cost)
- [ ] OTel Collector example config in `infra/`

## Phase 8 — Providers & modalities
- [ ] Providers: Azure OpenAI, Bedrock, Vertex, Gemini, Mistral, Groq, OpenRouter
- [ ] OpenAI⇄Anthropic request/response translation (+ streaming)
- [ ] `/v1/embeddings`
- [ ] `/v1/images/generations`, `/v1/audio/*` (transcriptions/speech)
- [ ] `/v1/rerank`
- [ ] Pluggable custom AI APIs (generic passthrough + balancing)
- [ ] Served OpenAPI document

## Phase 9 — Packaging & release
- [ ] Unified `rolter` CLI with `gateway`/`control` subcommands (one wheel ships both)
- [ ] cibuildwheel/maturin-action wheels → PyPI (`uv tool install rolter`)
- [ ] Publish crates to crates.io
- [ ] Multi-arch images → GHCR
- [ ] Helm chart / K8s manifests
- [ ] Release automation (release-please/semantic-release) from Conventional Commits
- [ ] `cargo deny` + dependency/advisory scanning in CI

## Phase 10 — Dashboard build-out
- [ ] Auth screens (login, SSO)
- [ ] CRUD: providers, routes (+ targets/strategy), virtual keys, members, budgets, pricing
- [ ] In-UI config editing with reload-free apply + validation feedback
- [ ] Logs explorer + cost/usage dashboards (ClickHouse), latency percentiles, error rates
- [ ] Org/team/project switcher; role-aware UI
- [ ] `bun run lint`/build wired into CI

## Cross-cutting / tech debt
- [ ] Publish `rolter` to PyPI (no wheel there yet). crates.io is done (all 8 crates @0.0.2). PyPI trusted publisher (OIDC) is configured as a *pending* publisher (repo `ormeilu/rolter`, workflow `release.yml`, env `pypi`); `PYPI_PUBLISH_ENABLED=true` is set. Blocker: the `v0.0.2` tag predates `crates/rolter`, so its wheel would ship the gateway-only binary. Fix: cut a fresh tag off master (v0.0.3 via release-plz) so `release.yml` builds the unified-launcher wheel and the pending publisher activates on first upload.
- [x] Integration tests for the gateway (mock upstream) + streaming assertions
- [ ] `criterion` benches for `pick`/trie; `oha`/`k6` load tests
- [ ] Structured error type surfaced as OpenAI-style JSON everywhere
- [ ] Config schema validation + helpful startup errors
- [ ] Secret backends (Vault/cloud KMS) behind the encryption trait
- [ ] Guardrails (PII/content/prompt-injection) hooks
- [ ] A/B traffic mirroring
