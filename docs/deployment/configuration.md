# Configuration reference

The gateway boots from a TOML file (`--config`, default `rolter.toml`); see [`rolter.example.toml`](../../rolter.example.toml). At runtime, the control plane is the source of truth and applies changes without a restart ([config-and-hot-reload.md](../architecture/config-and-hot-reload.md)).

## Schema

### `[server]`
- `host` (string, default `0.0.0.0`)
- `port` (u16, default `4000`)
- `metrics_path` (string, default `/metrics`) — path the Prometheus metrics endpoint is served on; change it to avoid colliding with an upstream app or sidecar that already owns `/metrics`. Must be rooted (`/…`) and must not collide with a built-in route (`/healthz`, `/v1/*`).

### `[[providers]]`
- `name` (string, unique) — referenced by route targets
- `kind` (`openai` | `anthropic` | `openai_compatible`)
- `api_base` (string) — base URL, no trailing slash
- `api_key` (string, optional) — prefer `api_key_env`
- `api_key_env` (string, optional) — env var to read the key from
- `egress_proxy` (string, optional) — HTTP/HTTPS/SOCKS5 outbound proxy

### `[[routes]]`
- `model` (string) — public model name clients request
- `strategy` (`round_robin` | `random` | `power_of_two` | `consistent_hash` | `cache_aware` | `weighted` | `pipeline`, default `round_robin`)
- `[[routes.targets]]`
  - `provider` (string) — a provider `name`
  - `model` (string, optional) — upstream model id; defaults to the requested model
  - `weight` (u32, default `1`)
- `[routes.params]` (table, optional) — admin default inference params injected into the request body (e.g. `temperature`, `max_tokens`, `stop`). Provider-agnostic: keys are whatever the upstream accepts. An unset param passes through untouched.
- `[routes.param_policy]` — whether callers may override the `params` defaults
  - `mode` (`allow` | `deny`, default `allow`) — baseline override policy
  - `allow` (string[], default `[]`) — params callers may override when `mode = "deny"`
  - `deny` (string[], default `[]`) — params callers may not override when `mode = "allow"`
  - when an override is denied and the caller sends the param anyway, the admin default silently wins
- `[[routes.variants]]` (optional) — weighted variants for A/B, canary, and key-split traffic. When present, the route ignores the top-level `targets` pool: a request samples one variant by weight (the primary) and, on failure, falls over to the remaining variants in declared order. Within a variant the route's `strategy` picks which target leads; the remaining targets follow in declared order as the deterministic fallback tail.
  - `name` (string) — variant identifier, attributed in request logs (the `variant` column)
  - `weight` (u32, default `1`) — relative traffic share for the primary draw
  - `[[routes.variants.targets]]` — same shape as `[[routes.targets]]`
  - `[routes.variants.params]` (table, optional) — variant-scoped param defaults, layered over `[routes.params]` (the variant wins) under the route's `param_policy`

### `[[virtual_keys]]`
- `key` (string) — the bearer token clients present
- `name` (string, optional)
- `models` (string[], default `[]`) — allow-list; empty = all

### `[logging]`
- `clickhouse_url` (string, optional)

### `[health]`
- `enabled` (bool, default `false`) — master switch for active upstream probing
- `interval_secs` (u64, default `10`) — seconds between probe sweeps
- `timeout_secs` (u64, default `2`) — per-probe timeout
- `path` (string, default `/`) — probe path; the default resolves to each provider kind's free liveness endpoint (`/v1/models`)
- `probe_concurrency` (usize, default `2`) — max probes in flight at once during a sweep, so probing never stampedes upstreams
- `consecutive_failure_threshold` (u32, default `3`) — consecutive probe failures before a provider is marked unhealthy
- `recovery_success_threshold` (u32, default `2`) — consecutive successes before an unhealthy provider recovers
- probes are jittered across the first quarter of the interval (per-provider stable offset), and a `429` on the probe itself pauses that provider's probing with exponential backoff (1, 2, 4, 8 sweeps) without marking it unhealthy

## Environment variables

- `ROLTER_CONFIG`, `ROLTER_HOST`, `ROLTER_PORT` — gateway
- `ROLTER_CONTROL_HOST`, `ROLTER_CONTROL_PORT`, `ROLTER_UI_DIR` — control plane
- `ROLTER_MASTER_KEY` — AES-256-GCM KEK for provider-secret encryption
- `DATABASE_URL`, `REDIS_URL`, `CLICKHOUSE_URL` — datastores
- `RUST_LOG` — tracing filter (e.g. `info`, `rolter_gateway=debug`)
- provider key vars referenced by `api_key_env` (e.g. `OPENAI_API_KEY`)

CLI flags override env, which override file values.
