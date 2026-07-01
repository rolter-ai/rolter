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
- [ ] Control CRUD API for all of the above (Axum + validation)
- [ ] Compose a runtime snapshot (`GatewayConfig`-shaped) from the DB
- [ ] `GET /internal/snapshot?version=N` for gateways
- [ ] Seed/bootstrap command (create org/admin, import `rolter.toml`)

## Phase 2 — Reload-free config
- [ ] Redis client + `PUBLISH`/`SUBSCRIBE` on `rolter.config`
- [ ] Bump/read `config_version` transactionally on writes
- [ ] Gateway watcher task: on event/interval, fetch snapshot, `ArcSwap::store`
- [ ] Snapshot validation before version bump (targets reference known providers)
- [ ] Metrics for reload (version, last-applied, failures)

## Phase 3 — Auth & RBAC
- [ ] Local accounts: argon2id hashing, login, sessions/JWT
- [ ] RBAC middleware resolving most-specific membership per resource
- [ ] Enforce roles on every control mutation
- [ ] Pluggable `IdentityProvider` trait
- [ ] OAuth2/OIDC SSO (group→role mapping)
- [ ] LDAP bind + group mapping
- [ ] Audit log writes + UI surface
- [ ] Virtual-key hardening (pepper, constant-time lookup, expiry/rotation, scopes)

## Phase 4 — Cost, limits & pricing
- [ ] ClickHouse client + async batched writer off the hot path
- [ ] Capture token usage (parse non-stream usage; accumulate for streams)
- [ ] Pricing catalog CRUD + per-request `cost_usd`
- [ ] Budgets enforcement (scope chain, most-restrictive-wins) with Redis spend counters
- [ ] RPM/TPM rate limits via Redis (sliding window) with `429` + `retry-after`
- [ ] Usage/cost aggregation queries for the dashboard

## Phase 5 — Reliability
- [ ] Retries (backoff + jitter) on 408/429/5xx, configurable
- [ ] Circuit breaker per target (closed/open/half-open)
- [ ] Cooldowns on rate-limited targets
- [ ] Upstream health checks; skip unhealthy targets
- [ ] In-flight load counters feeding `loads` to balancers
- [ ] Weighted selection honoring `Target.weight`
- [ ] Request timeouts + graceful shutdown/drain

## Phase 6 — Caching v2
- [ ] Cache-aware trie eviction (LRU / max-nodes) + metrics
- [ ] Precise KV-event scorer (vLLM ZMQ, block hashing, resident-prefix fraction)
- [ ] lmcache-aware strategy (controller occupancy)
- [ ] Response cache: exact (Redis) with TTL + opt-in per route/key
- [ ] Response cache: semantic (embeddings + cosine threshold)
- [ ] `x-rolter-cache` + decision headers

## Phase 7 — Observability
- [ ] OpenTelemetry OTLP export for traces + metrics (`OTEL_*` env)
- [ ] Inbound W3C `traceparent`/`b3` continuation; `request_id` end-to-end
- [ ] Outbound trace-context propagation to vLLM/SGLang/TGI
- [ ] `metrics` facade + prometheus exporter: latency histograms (TTFT/total), gauges, labels
- [ ] Federate/scrape upstream engine `/metrics`; correlate per target
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
- [ ] Integration tests for the gateway (mock upstream) + streaming assertions
- [ ] `criterion` benches for `pick`/trie; `oha`/`k6` load tests
- [ ] Structured error type surfaced as OpenAI-style JSON everywhere
- [ ] Config schema validation + helpful startup errors
- [ ] Secret backends (Vault/cloud KMS) behind the encryption trait
- [ ] Guardrails (PII/content/prompt-injection) hooks
- [ ] A/B traffic mirroring
