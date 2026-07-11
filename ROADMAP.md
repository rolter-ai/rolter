# rolter roadmap

Incremental path from the current scaffold to a full LiteLLM-proxy alternative. Each phase is shippable; see [`TODO.md`](TODO.md) for the granular checklist.

## Phase 0 — Scaffold & gateway MVP ✅
Cargo workspace + crates, runnable `rolter-gateway` (OpenAI/Anthropic passthrough, round-robin + approximate cache-aware, virtual-key auth, `/healthz`, `/metrics`, streaming), balancer strategies with tests, control-plane stub + UI scaffold, Docker/compose, DB schemas, CI, Conventional Commits.

## Phase 1 — Persistence & control plane
Wire Postgres (`sqlx`) behind `rolter-store`, a migration runner, and the control-plane CRUD API for orgs/teams/projects/providers/routes/keys. Compose a runtime snapshot from the DB.

## Phase 2 — Reload-free config
Redis pub/sub + `config_version` reconciliation; gateway watcher that atomically swaps the `ArcSwap` snapshot. Import a bootstrap TOML into the DB.

## Phase 3 — Auth & RBAC
Local accounts (argon2id) + sessions, RBAC enforcement across the API, then pluggable OAuth2/OIDC SSO and LDAP identity providers; audit log.

## Phase 4 — Cost, limits & pricing
Async ClickHouse logging pipeline; pricing catalog + per-request cost; budgets and RPM/TPM rate limits enforced via Redis counters across instances; usage/cost dashboards.

## Phase 5 — Reliability
Retries with backoff/jitter, circuit breakers, cooldowns on 429/5xx, upstream health checks, weighted selection, and `power_of_two`/`cache_aware` balancing against live load.

## Phase 6 — Caching v2
Precise KV-event cache-aware routing (vLLM ZMQ), lmcache-aware routing, and an optional response cache (exact + semantic) with cache-status headers.

## Phase 7 — Observability
OpenTelemetry OTLP export (traces + metrics) to SigNoz/Datadog/Grafana/Langfuse; W3C trace-context propagation to vLLM/SGLang so engine spans join the trace; latency histograms; federate upstream engine metrics.

## Phase 8 — Providers & modalities
More providers (Azure, Bedrock, Vertex, Gemini, Mistral, …); OpenAI⇄Anthropic translation; embeddings, then images/audio/video; pluggable custom AI APIs.

## Phase 9 — Packaging & release
Unified `rolter` CLI (gateway/control subcommands); cibuildwheel/maturin wheels to PyPI; crates.io; multi-arch GHCR images; Helm chart / K8s manifests; release automation from Conventional Commits.

## Phase 10 — Control panel
Full-featured hostable web control panel, not a read-only dashboard. Admins get complete CRUD over models, providers, routes, virtual keys, users/teams/roles, budgets, and pricing, plus reload-free in-UI config editing. End users get a scoped self-service panel for their own keys and usage. Live logs, cost/latency analytics, auth/SSO screens, role-aware UI throughout.

## Stretch
Beyond the core phased roadmap: Rust/Python SDKs, MCP gateway (proxy MCP tool servers through rolter), A2A gateway, multi-region deployment. (Guardrails and A/B traffic mirroring moved into Phase 3 / Phase 5 respectively — see TODO.md.)
