# rolter documentation

Start here. rolter is a high-performance OpenAI/Anthropic-compatible AI gateway and load balancer (Rust data plane + control plane, shadcn/ui dashboard).

## Architecture

- [Overview](architecture/overview.md) — system shape, crates, data/control plane split
- [Load balancing](architecture/load-balancing.md) — strategies and the cache-aware design
- [Caching](architecture/caching.md) — response cache and KV-cache affinity
- [Config & hot reload](architecture/config-and-hot-reload.md) — reload-free updates
- [Data model](architecture/data-model.md) — tenancy, keys, pricing, budgets
- [RBAC & auth](architecture/rbac-and-auth.md) — roles, virtual keys, SSO/LDAP roadmap
- [Security](architecture/security.md) — secret handling, threat model
- [Observability](architecture/observability.md) — metrics, tracing, logs
- [Performance](architecture/performance.md) — hot-path principles and targets

## Decisions

- [ADRs / decision log](adr/README.md)

## API

- [OpenAI & Anthropic surface](api/openai-and-anthropic.md)

## Development

- [Setup](development/setup.md)
- [Testing](development/testing.md)
- [Contributing](development/contributing.md)
- [Commit conventions](development/commit-conventions.md)
- [Packaging (uv / cargo / docker)](development/packaging.md)

## Deployment

- [Configuration reference](deployment/configuration.md)
- [Docker](deployment/docker.md)

## Planning

- [Roadmap](../ROADMAP.md)
- [TODO](../TODO.md)
