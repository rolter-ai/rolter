# Architecture Decision Records

Lightweight decision log. Each entry: **Status** · **Context** · **Decision** · **Consequences**. Supersede rather than rewrite.

A standalone record opens with its title and a single metadata line — no table:

```markdown
# Title of the decision

**Status:** Accepted · **Date:** 6 Aug 2026 · **Issues:** [#812](https://github.com/rolter-ai/rolter/issues/812)
**Supersedes:** ADR-0007 for exact vLLM modes
**Relates:** ADR-0022 (config-vs-DB tiering)

## Context
```

`Status`, `Date` and `Issues` are the line; `Supersedes` and `Relates` get their
own line and are omitted when there is nothing to say. Nothing else belongs
there — authorship and dates are what git is for, and a field whose value is
either constant across every record or a `— (unassigned)` placeholder is noise
that makes the actual decision harder to find. Records are English only.

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

## ADR-0015 — [OpenAI Responses API translation](2026-07-13-responses-api-protocol-translation.md)
Development. Add OpenAI Responses as a protocol pair for native OpenAI, Chat Completions and Anthropic Messages, while model-less lifecycle operations remain uniformly unsupported until tenant-scoped storage exists.

## ADR-0016 — [Routing OpenAI Responses resources through a tenant-scoped registry](2026-07-13-responses-lifecycle-routing-registry.md)
Development. Pin Responses lifecycle operations to a bounded tenant-scoped process-local record, preserving the original provider credential while making unknown and cross-tenant IDs indistinguishable.

## ADR-0017 — [Provider/model addressing to disambiguate identical model names](2026-07-14-provider-model-addressing.md)
Accepted. First-class `provider-slug/model` addressing coexists with named routes: a stable, URL-safe provider `slug` resolves `provider-slug/model` to a pinned `(provider, upstream_model)` target (avoiding LiteLLM's base_url ambiguity). Pinning bypasses cross-provider fan-out but still balances within the provider. Slugs are org-scoped and collision-safe: deterministic reported migration backfill with `-N` de-dup, 409+suggestion on runtime create-conflict (never silent suffixing), soft-delete-preferred reclaim. Addendum: **provider groups** unify fleets (e.g. ten vLLM instances) under one group slug in the same namespace — `vllm-cluster/qwen3` fans out across members with a chosen balancing strategy and passthrough model names, so no route-per-model. Consequence: new immutable `slug` column + proxy parsing + `/v1/models` + UI work + provider-groups entity (see follow-up issues).

## ADR-0018 — Config mutated via granular CRUD, not whole-config replace
Accepted. The normalized Postgres store (ADR-0004, source of truth) is mutated only through the scoped CRUD API — providers, routes, targets, virtual keys — each write bumping the config version and hot-reloading gateways via the snapshot poll (ADR-0008). `PostgresConfigStore::save` is deliberately read-only: there is no whole-config "apply" endpoint, because a full replace would fight the normalized model and clobber concurrent edits. The dashboard Config page is a **read-only** effective-config viewer; the `/gw` reverse-proxy that fronts the gateway for the Playground is unrelated to config writes. Consequence: one live-edit path (CRUD) with instant fan-out; a raw whole-config editor would need a deliberate transactional diff/apply (guarding config-owned entries, preserving key hashes) and is out of scope (closed #494).

## ADR-0019 — [Per-provider egress proxy pools](2026-07-18-provider-egress-proxy-pools.md)
Accepted. Rotate across a provider-local proxy pool, fail over only connection/tunnel failures, and quarantine repeatedly failing members. Authenticated URLs are resolved exclusively from whole-value environment references. Consequence: resilient egress without leaking credentials; health state remains process-local.

## ADR-0020 — [Bounded semantic response caching in Redis](2026-07-18-semantic-response-cache.md)
Accepted. Run semantic lookup only after an exact miss, embed through an explicitly configured provider, and scan a bounded recent Redis window. Consequence: similarity reuse without another datastore; embedding and cache failures fail open and candidate search remains deliberately bounded.

## ADR-0021 — [External cache telemetry for routing](2026-07-18-external-cache-telemetry-routing.md)
Accepted. Extend ADR-0007 with opt-in vLLM KV-event and LMCache occupancy scorers whose network I/O runs in background tasks. Consequence: exact/capacity-aware routing stays allocation-light and fail-open; stale or untrusted telemetry becomes neutral and least-load selection remains available.

## ADR-0022 — [Uniform config-vs-DB tiering for models, providers, and provider groups](2026-07-20-config-db-entity-tiering.md)
Accepted. Give models, providers, and provider groups the same two-tier config wrapper over the DB (CRUD) tier: `readonly` entries are immutable and config-owned; `default` entries are seeded into the default project once at startup and then owned/editable via API/UI; pure DB rows come from CRUD. readonly wins resolution; a `default` colliding with a readonly key is a load-time error. Top-level `[[routes]]`/`[[providers]]`/`[[provider_groups]]` stay as deprecated readonly aliases for back-compat. Consequence: providers gain a seed-then-edit story and provider groups gain a full DB lifecycle (new tables/repos/seed/CRUD, tracked as follow-up PRs); the model tiering pattern is reused rather than reinvented per entity.

## ADR-0023 — [Propagating access-profile model policy to the data plane](2026-08-04-access-policy-propagation.md)
Accepted. Resolve each virtual key owner's merged access-profile policy when the snapshot is built, carry it on the key record, and enforce it on the gateway's model and route selection; the key's own allow-list and the owner's policy must both permit a model. Rejects key-mint-time resolution because a later policy edit would never reach issued keys, so a revocation would silently fail to revoke. Adds `bump_config_version()` triggers to exactly the four tables that feed the resolution, changing migration 0058's premise rather than its rule. Consequence: the policy `/rbac/effective` reports is the policy enforced, and a deployment with no access profiles is unaffected.

## ADR-0024 — [Dashboard UX telemetry as a structural-only event stream](2026-08-06-dashboard-ux-telemetry.md)
Accepted. Collect dashboard usability events into a `ui_events` ClickHouse table beside `request_logs` rather than adding a product-analytics vendor, and make the schema itself the privacy guarantee: every column is a key, an enum, a duration or an id, so a form value or prompt has nowhere to land. Attribution is server-side; the ingest endpoint is guarded by `CurrentUser` with no capability row, because a `create` capability granted to any authenticated caller would break the RBAC invariant that a viewer writes nothing. Consequence: usability data with no new vendor, egress or retention policy, joinable to gateway traffic on `trace_id`; the cost is that any question needing a value rather than a key is unanswerable by design.

## ADR-0025 — [Events as logs, not span events](2026-08-06-events-as-logs.md)
Accepted. OpenTelemetry is deprecating the Span Events API, so log-based events become rolter's event model: no new explicit `add_event`/`record_exception`, and #809 (OTLP log export) sequences before #808 (GenAI conventions) because those conventions are specified as log-based events and cannot be emitted until logs are exported at all. A grep finds zero uses of the deprecated API, but `tracing-opentelemetry` turns every `tracing` event fired inside a span into a span event, so rolter is in practice an exclusive implicit user of it — which makes the migration a dependency upgrade in one bridge rather than a sweep of 57 call sites. Consequence: `tracing` stays; #809's `trace_id`/`span_id` correlation becomes a blocking acceptance criterion rather than a nicety; new event types have no correct home until it lands, and should wait rather than add a span event that must later be removed.

## ADR-0026 — [Client control over telemetry: a kill switch now, collector-routed tenant destinations later](2026-08-06-tenant-telemetry-destinations.md)
Accepted. Splits #812 in two. `ROLTER_TELEMETRY_ENABLED` ships now as one explicit off for every signal — traces, metrics, the dashboard's browser tracing, and logs once #809 lands — that can only subtract, defaults to enabled so no existing deployment changes, and leaves export on for an unrecognized value so a typo cannot silently blind a deployment. It is environment-only despite the issue asking for a config key: `telemetry::init` installs the subscriber before any config file is read, so a config-file switch could not gate trace export at all. Per-tenant destinations are deferred and, when built, belong in the OpenTelemetry Collector's routing processor keyed on an org resource attribute rather than in-process exporters — that keeps one egress path in the gateway and keeps tenant-supplied URLs, an SSRF surface, out of the data plane entirely. Consequence: operators get a reviewable "no telemetry leaves here" today; per-tenant routing later requires running a collector, which is already the recommended topology.

## ADR-0027 — [End-to-end test harness: Python/uv project driving a black-box stack](2026-07-21-e2e-test-harness.md)
Accepted. The e2e suite is a Python driver on the latest CPython managed by `uv`, exercising the real HTTP APIs of a docker-compose stack (postgres, redis, clickhouse, control, gateway, and N `llm-d-inference-sim` fake-vLLM engines) with no in-process shortcuts, so the tests see exactly what an operator or tenant sees. Numbered after ADR-0026 despite its earlier date because ADR numbers are append-only and never reused. Consequence: the suite needs a container runtime rather than `cargo test` alone; in exchange it covers the wiring between the two binaries and the three datastores that unit tests cannot reach.

## ADR-0028 — [Disaggregated prefill/decode routing belongs to the engine, not the gateway](2026-08-11-disaggregated-prefill-decode-routing.md)
Accepted. rolter will not implement P/D disaggregation; a disaggregated fleet is one upstream and its own coordinator owns phase selection and the KV handoff. Engines move KV tensors through connector-selected transports such as NIXL/UCX. Some can coordinate that transfer through engine-specific HTTP fields, but rolter would still have to own compatible-worker topology, version-specific metadata, two upstream calls and per-request handoff state — exactly the lifecycle coupling ADR-0014 prevents. The distinguishing test is whether rolter can consume an input through a stable, engine-independent contract without joining the engine's request lifecycle. Consequence: disaggregated fleets work with rolter today for any engine, with no code and no mid-request-handoff failure modes; revisit if a stable phase-placement contract emerges on the OpenAI or Anthropic surface.

## ADR-0029 — [An explicit per-provider opt-out from a hosted kind's host pin](2026-09-02-hosted-provider-host-pin-opt-out.md)
Accepted. `ProviderKind::Openrouter` pinned `api_base` to the literal `https://openrouter.ai/api/v1`, which closes a real exfiltration path — a hosted kind names one operator, so an edited base URL would send that operator's key elsewhere — but also ruled out an egress proxy, a vendor-compatible endpoint, and testing the dialect at all, so the #924 dogfooding fleet had to declare the endpoint `openai_compatible` and left the adapter unexercised. Adds `allow_custom_api_base = true` per provider: the safe default is unchanged, the opt-out is one reviewable line, only the host pin is relaxed (the `api_key_env` rule still holds), and `rolter check` warns on every provider that sets it while `--strict` fails. Rejects a loopback/private-range carve-out, which would not help a corporate proxy and would key the rule on where rolter runs rather than on intent. Consequence: the OpenRouter dialect gets local coverage; providers stored in Postgres keep the safe default until the column, CRUD surface and dashboard control land as their own change.

## ADR-0030 — [A served OpenAPI document for the control-plane API](2026-09-08-control-plane-openapi-document.md)
Accepted. The gateway already serves a hand-built OpenAPI 3.1 document at `/openapi.json` with an embedded Scalar reference at `/docs`; the control plane serves the same for its ~160-path management API, built from one operations table in `crates/rolter-control/src/openapi.rs` rather than from a second OpenAPI toolchain. Rejects hand-written SDK surfaces (#421/#422/#851 would each transcribe 240 operations, three times, forever) and `utoipa` derivation (a proc-macro graph plus a schema describing rolter's Rust types rather than its wire contract, which diverge wherever a handler flattens or hides a field). Completeness is a merge gate, not a hope: a test reads every route registration under `src/` and fails naming any `(path, method)` missing from the document, with the reverse check for stale rows. Consequence: adding an endpoint costs one row and forgetting it fails the build; body schemas are typed for the stable CRUD surfaces and open objects for the dynamic ones, which the gate makes a visible, enumerable debt rather than a silent gap.

## ADR-0031 — [One model for turning a subsystem off](2026-09-08-feature-enablement-model.md)
Proposed. A survey for #1073 found **eight** different ways a rolter subsystem can be turned off — a deployment-wide flag, a per-subsystem `enabled`, an `Option` left unset, an empty collection, a numeric sentinel, a per-object row, a cargo feature, and absent infrastructure — with no written boundary between them, and ten subsystems (MCP gateway, PII sanitizer, guardrail webhooks, semantic cache, budgets, rate limits, plugins, payload capture, realtime, analytics) having no deployment-wide switch at all. Collapses these to two layers: **capability** (compile-time or infrastructure, read-only, extending the existing `unavailable_flags()`) and **enablement** (one DB-backed boolean per subsystem, hot-reloaded through `/internal/snapshot`). Defines "off" per class rather than per subsystem — filters and optimisations bypass, gateway surfaces answer `501` naming the flag rather than a 404 indistinguishable from a typo, enforcement bypasses audibly, recorders drop at ingest, UI-only surfaces hide — under one rule: a disabled subsystem never changes an answer the client already had a right to. Per-object `enabled` rows stay and are explicitly outside the model. Rejects a bare per-subsystem `enabled` (cannot express capability), a generic `HashMap<String, bool>` (loses the compile-time flag-name guarantee and the default-on test), and a second SPA bootstrap payload. Consequence: default-on is guaranteed by construction and by a test over `FeatureFlagsConfig::default()`, so an upgrade cannot change behaviour; the cost is ten subsystems each needing a flag, a `bump_config_version()` migration, snapshot plumbing and a dashboard switch, filed as sized follow-ups.
## ADR-0032 — [What 1.0.0 guarantees, surface by surface](2026-09-09-one-point-oh-compatibility-guarantees.md)
Accepted. `1.0.0` is a compatibility claim semver assigns a meaning to whether or not one is written, so #922 writes one per surface. **The Rust crates are internal**: no crate offers a stable Rust API at 1.0 or during 1.x, said in each published crate's `description`, its `//!` docs and the README, because a shared version number cannot carry the caveat — and both mechanisms that would otherwise enforce the unmade promise are made to agree, with `semver_check = false` for release-plz (a Rust API diff must never major-bump the product) and the `semver-checks` job permanently advisory rather than promoted at 1.0 as `api-stability.md` had planned. **`/v1/*` guarantees the dialect, not a schema**: the `v1` is OpenAI's and there is no `/v2/` escape hatch, so rolter promises fidelity and tracking — following an upstream dialect change is not a rolter breaking change — while the rolter-owned parts (virtual-key auth, `provider-slug/model` addressing, the error envelope, `/v1/models`) are promised like the control API. **`/api/v1/*` is additive within `v1`**, and removing anything published in the OpenAPI document takes **two minor releases and 90 days, whichever ends later**, announced in the release notes, `deprecated: true` in the document and served with `Deprecation`/`Sunset` headers; both bounds because a release count is worthless at an unfixed cadence and a time period can pass with no release. A 1.0 `rolter.toml` is read by every 1.x on the same terms; the schema moves forward only, with restore-from-backup as the sole rollback. After 1.0 `BREAKING CHANGE:` means exactly one thing — a change to a promised surface shipping *without* a completed deprecation window — never a Rust API change, an upstream dialect change, or a subsystem carrying the `experimental` marker, which stays the single documented exemption route. Consequence: internal refactors such as #1041/#1042 stay unblocked, the deprecation machinery (`Deprecation`/`Sunset`, `deprecated: true`, a release-notes section) becomes prerequisite work for the first deprecation rather than for 1.0, and removing a stability marker becomes the expensive act.
