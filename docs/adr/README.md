# Architecture Decision Records

Lightweight decision log. Each entry: **Status** · **Context** · **Decision** · **Consequences**. Supersede rather than rewrite.

## ADR-0001 — Rust + Axum/Hyper/Tower for the data plane
Accepted. Need maximum proxy throughput with rich API semantics and SSE streaming. Chose Axum/Hyper/Tower on Tokio over Pingora/Actix for ecosystem fit and ergonomics. Consequence: idiomatic async stack; revisit Pingora only if profiling demands it.

## ADR-0002 — Two-binary topology over shared crates
Accepted. Keep the hot proxy path lean and independently scalable from management. `rolter-gateway` (data plane) and `rolter-control` (management + UI host) share library crates. Consequence: clear seam; some duplicated wiring.

## ADR-0003 — Vite + React + shadcn/ui SPA, Bun toolchain, served by Rust
Accepted. shadcn/ui (Radix + Tailwind) for the dashboard, built with Vite and managed with **Bun**; output is static assets served by `rolter-control` (no Node runtime in prod). Consequence: simple prod footprint; Bun used for install/dev/build.

## ADR-0004 — Postgres + Redis + ClickHouse
Accepted. Postgres = source of truth (config/RBAC/keys/pricing); Redis = cache + rate-limit counters + config pub/sub; ClickHouse = high-volume request/cost logs. No SQLite. Consequence: three datastores to operate; each fits its job.

## ADR-0005 — Org → Team → Project → Virtual Key tenancy
Accepted. Budgets and rate limits attach at any scope, most-restrictive-wins. Consequence: flexible multi-tenancy; enforcement must resolve a scope chain.

## ADR-0006 — Local accounts + virtual keys + roles; SSO/LDAP later
Accepted. v1 ships local accounts (argon2id) and roles admin/member/viewer; OAuth2/OIDC and LDAP arrive as pluggable identity providers. Consequence: usable day one without an IdP.

## ADR-0007 — Approximate cache-aware balancing behind a pluggable trait
Accepted. v1 uses an approximate per-target prefix trie (no engine coupling) behind `LoadBalancer`. Precise (KV-events) and lmcache-aware land later without API changes. Consequence: immediate wins; precise mode is additive.

## ADR-0008 — Reload-free config: Postgres truth + Redis pub/sub + ArcSwap
Accepted. Control plane writes Postgres, bumps a version, publishes on Redis; gateways fetch and atomically swap an in-memory snapshot, reconciling by version. Consequence: instant fan-out, self-healing, lock-free reads.

## ADR-0009 — Envelope encryption for provider secrets
Accepted. Upstream keys are AES-256-GCM envelope-encrypted with a master key from env/file; Vault/KMS backends later. Consequence: no plaintext secrets at rest; master key is the critical secret.

## ADR-0010 — Packaging: maturin (uv) + cargo + Docker
Accepted. Ship a maturin-built PyPI wheel bundling the unified `rolter` launcher (`uv tool install rolter`), `cargo install rolter`, and a multi-stage Docker image. The `rolter` binary dispatches to `gateway`/`control` subcommands so one wheel/crate ships the whole system. Consequence: three distribution paths from a single named artifact.

## ADR-0011 — API surface v1
Accepted. OpenAI `/v1/chat/completions`, `/v1/completions`, `/v1/models` and Anthropic `/v1/messages`. Embeddings, images, audio and other modalities follow. Consequence: drop-in for the two dominant client SDKs first.

## ADR-0012 — Conventional Commits + CI PR-title lint
Accepted. Commit messages and PR titles follow Conventional Commits; enforced by commitlint, `conventional-pre-commit`, and a CI PR-title check. Consequence: consistent history, automatable changelogs/releases.

## ADR-0013 — OpenTelemetry-based observability with engine propagation
Accepted. Export traces/metrics via OTLP to any compatible backend (SigNoz, Datadog, Grafana, Langfuse, …); propagate W3C trace context to vLLM/SGLang so engine spans join the same trace; federate upstream engine metrics. Consequence: vendor-neutral observability; an OTel Collector is the recommended hub.
