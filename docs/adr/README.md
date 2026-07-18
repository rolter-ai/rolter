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

## ADR-0014 — Extensible API protocol translation
Accepted. Resolve translation by client/upstream protocol pair in `rolter-proxy`, including incremental SSE, while the gateway retains transport, caching and accounting ownership. Consequence: new provider dialects extend one translation boundary; non-equivalent modalities remain explicit and are never silently dropped.

## ADR-0015 — [Трансляция OpenAI Responses API](2026-07-13-responses-api-protocol-translation.md)
Development. Add OpenAI Responses as a protocol pair for native OpenAI, Chat Completions and Anthropic Messages, while model-less lifecycle operations remain uniformly unsupported until tenant-scoped storage exists.

## ADR-0016 — [Маршрутизация ресурсов OpenAI Responses по tenant-scoped registry](2026-07-13-responses-lifecycle-routing-registry.md)
Development. Pin Responses lifecycle operations to a bounded tenant-scoped process-local record, preserving the original provider credential while making unknown and cross-tenant IDs indistinguishable.

## ADR-0017 — [Provider/model addressing to disambiguate identical model names](2026-07-14-provider-model-addressing.md)
Accepted. First-class `provider-slug/model` addressing coexists with named routes: a stable, URL-safe provider `slug` resolves `provider-slug/model` to a pinned `(provider, upstream_model)` target (avoiding LiteLLM's base_url ambiguity). Pinning bypasses cross-provider fan-out but still balances within the provider. Consequence: new immutable `slug` column + proxy parsing + `/v1/models` + UI work (see follow-up issues).

## ADR-0018 — Config mutated via granular CRUD, not whole-config replace
Accepted. The normalized Postgres store (ADR-0004, source of truth) is mutated only through the scoped CRUD API — providers, routes, targets, virtual keys — each write bumping the config version and hot-reloading gateways via the snapshot poll (ADR-0008). `PostgresConfigStore::save` is deliberately read-only: there is no whole-config "apply" endpoint, because a full replace would fight the normalized model and clobber concurrent edits. The dashboard Config page is a **read-only** effective-config viewer; the `/gw` reverse-proxy that fronts the gateway for the Playground is unrelated to config writes. Consequence: one live-edit path (CRUD) with instant fan-out; a raw whole-config editor would need a deliberate transactional diff/apply (guarding config-owned entries, preserving key hashes) and is out of scope (closed #494).

## ADR-0019 — [Per-provider egress proxy pools](2026-07-18-provider-egress-proxy-pools.md)
Accepted. Rotate across a provider-local proxy pool, fail over only connection/tunnel failures, and quarantine repeatedly failing members. Authenticated URLs are resolved exclusively from whole-value environment references. Consequence: resilient egress without leaking credentials; health state remains process-local.
