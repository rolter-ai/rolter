# Configuration reference

The gateway boots from a TOML file (`--config`, default `rolter.toml`); see [`rolter.example.toml`](../../rolter.example.toml). At runtime, the control plane is the source of truth and applies changes without a restart ([config-and-hot-reload.md](../architecture/config-and-hot-reload.md)).

## Schema

### `[server]`
- `host` (string, default `0.0.0.0`)
- `port` (u16, default `4000`)

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

### `[[virtual_keys]]`
- `key` (string) — the bearer token clients present
- `name` (string, optional)
- `models` (string[], default `[]`) — allow-list; empty = all

### `[logging]`
- `clickhouse_url` (string, optional)

## Environment variables

- `ROLTER_CONFIG`, `ROLTER_HOST`, `ROLTER_PORT` — gateway
- `ROLTER_CONTROL_HOST`, `ROLTER_CONTROL_PORT`, `ROLTER_UI_DIR` — control plane
- `ROLTER_MASTER_KEY` — AES-256-GCM KEK for provider-secret encryption
- `DATABASE_URL`, `REDIS_URL`, `CLICKHOUSE_URL` — datastores
- `RUST_LOG` — tracing filter (e.g. `info`, `rolter_gateway=debug`)
- provider key vars referenced by `api_key_env` (e.g. `OPENAI_API_KEY`)

CLI flags override env, which override file values.
