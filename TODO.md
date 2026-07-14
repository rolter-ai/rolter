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
- [x] [`cargo build`/`test`/`clippy` green in CI](https://linear.app/rolter/issue/ROL-19/cargo-buildtestclippy-green-in-ci-confirm-on-first-push)

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
- [x] Per-provider bounded queues with configurable backpressure (ROL-113)
- [x] Upstream health checks; skip unhealthy targets
- [x] In-flight load counters feeding `loads` to balancers
- [x] Weighted selection honoring `Target.weight`
- [x] Request timeouts + graceful shutdown/drain
- [ ] [vLLM/SGLang compatibility contracts and direct-vs-gateway baselines](https://linear.app/rolter/issue/ROL-238/testgateway-add-local-and-ci-vllmsglang-compatibility-tests-and)

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
- [x] OpenAI⇄Anthropic request/response translation (+ streaming)
- [x] `/v1/embeddings` (OpenAI-compatible passthrough; built-in `fake-llm` serves deterministic vectors)
- [x] `/v1/images/generations` (OpenAI-compatible passthrough; built-in `fake-llm` returns a deterministic 1x1 png)
- [x] `/v1/audio/speech` (OpenAI-compatible TTS passthrough; built-in `fake-llm` returns a silent wav)
- [x] `/v1/audio/transcriptions`, `/v1/audio/translations` (multipart passthrough; routes on the `model` form field, forwards the upload verbatim)
- [x] `/v1/rerank` (Cohere/Jina-compatible passthrough; built-in `fake-llm` ranks deterministically)
- [ ] Pluggable custom AI APIs (generic passthrough + balancing)
- [x] Served OpenAPI document (`GET /openapi.json`, hand-authored 3.1) + interactive Scalar reference (`GET /docs`, bundle embedded in the binary — air-gapped safe)

## Phase 9 — Packaging & release
- [x] [Unified `rolter` CLI with `gateway`/`control` subcommands (one wheel ships both)](https://linear.app/rolter/issue/ROL-73/unified-rolter-cli-with-gatewaycontrol-subcommands-one-wheel-ships)
- [ ] cibuildwheel/maturin-action wheels → PyPI (`uv tool install rolter`)
- [x] [Publish crates to crates.io](https://linear.app/rolter/issue/ROL-75/publish-crates-to-cratesio)
- [ ] Multi-arch images → GHCR
- [ ] Helm chart / K8s manifests
- [ ] Release automation (release-please/semantic-release) from Conventional Commits
- [x] [`cargo deny` + dependency/advisory scanning in CI](https://linear.app/rolter/issue/ROL-79/cargo-deny-dependencyadvisory-scanning-in-ci)

## Phase 10 — Control panel
Full-featured hostable web control panel, not a read-only dashboard.
- [x] [Zero-cred startup + runtime provider/model CRUD with encrypted keys](https://linear.app/rolter/issue/ROL-250/zero-cred-startup-run-with-fake-llm-only-add-providersmodels-at) (provider `api_key` via API sealed with `ROLTER_KEK`, `PUT /providers/{id}`, `ROLTER_ADMIN_TOKEN` guard on CRUD + snapshot, gateway `/admin/*` proxy)
- [ ] Auth screens (login, SSO)
- [x] CRUD: providers, routes (+ targets/strategy), virtual keys, budgets, pricing (members CRUD blocked on Phase 3 accounts)
- [x] Model management UI: add/edit/enable-disable/delete models + provider/route binding
- [ ] User & team management UI: create/invite/edit/deactivate users, assign roles/teams (blocked on Phase 3 accounts/RBAC)
- [ ] End-user self-service panel: personal API keys + usage/spend view
- [x] In-UI config editing with reload-free apply + validation feedback (route admin params/policy)
- [x] Cost/usage dashboards (ClickHouse), latency percentiles, error rates (per-request logs explorer still needed — no drill-down endpoint exists yet)
- [x] Org/team/project switcher; role-aware UI (role-aware UI blocked on Phase 3 RBAC)
- [x] [`bun run lint`/build wired into CI](https://linear.app/rolter/issue/ROL-85/bun-run-lintbuild-wired-into-ci)

## Cross-cutting / tech debt
- [ ] [Full-stack Docker Compose smoke test in CI](https://linear.app/rolter/issue/ROL-245/ci-add-full-stack-docker-compose-smoke-test)
- [ ] [Publish Rust coverage and establish a ratcheting threshold](https://linear.app/rolter/issue/ROL-246/ci-publish-rust-coverage-and-establish-a-ratcheting-threshold)
- [ ] [Document and enforce the `ci-ok` branch-protection policy](https://linear.app/rolter/issue/ROL-244/ci-document-and-enforce-the-ci-ok-branch-protection-policy)
- [ ] Publish `rolter` to PyPI (no wheel there yet). crates.io is done (all 8 crates @0.0.2). PyPI trusted publisher (OIDC) is configured as a *pending* publisher (repo `ormeilu/rolter`, workflow `release.yml`, env `pypi`); `PYPI_PUBLISH_ENABLED=true` is set. Blocker: the `v0.0.2` tag predates `crates/rolter`, so its wheel would ship the gateway-only binary. Fix: cut a fresh tag off master (v0.0.3 via release-plz) so `release.yml` builds the unified-launcher wheel and the pending publisher activates on first upload.
- [x] Integration tests for the gateway (mock upstream) + streaming assertions
- [x] [`criterion` benches for `pick`/trie](https://linear.app/rolter/issue/ROL-232/adopt-criterionrs-for-benchmarks)
- [ ] [`oha`/`k6` load-test harness](https://linear.app/rolter/issue/ROL-87/load-test-harness-ohak6-for-gateway-added-latency-max-rps)
- [ ] Structured error type surfaced as OpenAI-style JSON everywhere
- [ ] Config schema validation + helpful startup errors
- [ ] Secret backends (Vault/cloud KMS) behind the encryption trait
- [ ] Guardrails (PII/content/prompt-injection) hooks
- [ ] A/B traffic mirroring

## Stretch
Beyond the core phased roadmap.
- [ ] Rust SDK: client library for rolter gateway + control API
- [ ] Python SDK: client library for rolter gateway + control API
- [ ] MCP gateway: proxy MCP tool servers through rolter with per-key auth
- [ ] A2A gateway: agent-to-agent protocol bridge through rolter
- [ ] Multi-region deployment: cross-region routing + config sync
