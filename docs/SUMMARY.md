# Summary

[Introduction](README.md)

# Architecture

- [Overview](architecture/overview.md)
- [Load balancing](architecture/load-balancing.md)
- [Caching](architecture/caching.md)
- [Config & hot reload](architecture/config-and-hot-reload.md)
- [Data model](architecture/data-model.md)
- [RBAC & auth](architecture/rbac-and-auth.md)
- [Invitations](architecture/invitations.md)
- [Single sign-on (OIDC)](architecture/sso.md)
- [MCP OAuth](architecture/mcp-oauth.md)
- [SCIM provisioning](architecture/scim-provisioning.md)
- [Security](architecture/security.md)
- [LDAP authentication](architecture/ldap.md)
- [Health & readiness](architecture/health-and-readiness.md)
- [Observability](architecture/observability.md)
- [Client disconnects](architecture/client-disconnects.md)
- [Performance](architecture/performance.md)

# Decisions

- [ADRs / decision log](adr/README.md)
  - [ADR-0015 — OpenAI Responses API translation](adr/2026-07-13-responses-api-protocol-translation.md)
  - [ADR-0016 — Responses lifecycle routing registry](adr/2026-07-13-responses-lifecycle-routing-registry.md)
  - [ADR-0017 — Provider/model addressing](adr/2026-07-14-provider-model-addressing.md)
  - [ADR-0019 — Per-provider egress proxy pools](adr/2026-07-18-provider-egress-proxy-pools.md)
  - [ADR-0020 — Bounded semantic response caching](adr/2026-07-18-semantic-response-cache.md)
  - [ADR-0021 — External cache telemetry for routing](adr/2026-07-18-external-cache-telemetry-routing.md)
  - [ADR-0022 — Config-vs-DB entity tiering](adr/2026-07-20-config-db-entity-tiering.md)
  - [ADR-0023 — Access policy propagation](adr/2026-08-04-access-policy-propagation.md)
  - [ADR-0024 — Dashboard UX telemetry](adr/2026-08-06-dashboard-ux-telemetry.md)
  - [ADR-0025 — Events as logs](adr/2026-08-06-events-as-logs.md)
  - [ADR-0026 — Tenant telemetry destinations](adr/2026-08-06-tenant-telemetry-destinations.md)
  - [ADR-0027 — End-to-end test harness](adr/2026-07-21-e2e-test-harness.md)
  - [ADR-0028 — Disaggregated prefill/decode routing](adr/2026-08-11-disaggregated-prefill-decode-routing.md)
  - [ADR-0029 — Hosted-provider host-pin opt-out](adr/2026-09-02-hosted-provider-host-pin-opt-out.md)
  - [ADR-0030 — Control-plane OpenAPI document](adr/2026-09-08-control-plane-openapi-document.md)

# API

- [OpenAI & Anthropic surface](api/openai-and-anthropic.md)

# Development

- [Setup](development/setup.md)
- [Testing](development/testing.md)
- [Contributing](development/contributing.md)
- [Parallel development with Worktrunk](development/worktrees.md)
- [Dashboard localization (i18n)](development/i18n.md)
- [Dashboard error states](development/error-states.md)
- [Dashboard loading and empty states](development/loading-and-empty-states.md)
- [Dashboard destructive actions](development/destructive-actions.md)
- [Dashboard capability gating](development/rbac-gating.md)
- [Dashboard navigation rail](development/dashboard-navigation.md)
- [Dashboard theme](development/dashboard-theme.md)
- [Dashboard code highlighting](development/highlighting.md)
- [Commit conventions](development/commit-conventions.md)
- [Issue tracking](development/issue-tracking.md)
- [Merge protection on master](development/merge-protection.md)
- [API stability and the semver gate](development/api-stability.md)
- [Packaging (uv / cargo / docker)](development/packaging.md)

# Deployment

- [Zero-config quickstart](deployment/zero-config-quickstart.md)
- [Configuration reference](deployment/configuration.md)
- [Secure configuration (`rolter init` / `check`)](deployment/preflight-validation.md)
- [Custom CA bundles](deployment/custom-ca-bundles.md)
- [Self-hosted Ollama](deployment/ollama.md)
- [OpenRouter](deployment/openrouter.md)
- [Docker](deployment/docker.md)
- [Kubernetes and Helm](deployment/kubernetes.md)
- [llama.cpp](deployment/llama-cpp.md)
- [Hugging Face TEI](deployment/tei.md)
- [Air-gapped install & operation](deployment/air-gapped.md)
- [Backup, restore and KEK rotation](deployment/backup-and-restore.md)
