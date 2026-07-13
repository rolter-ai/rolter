# Configuration reference

The gateway boots from a TOML file (`--config`, default `rolter.toml`); see [`rolter.example.toml`](../../rolter.example.toml). At runtime, the control plane is the source of truth and applies changes without a restart ([config-and-hot-reload.md](../architecture/config-and-hot-reload.md)).

## Schema

### `[server]`
- `host` (string, default `0.0.0.0`)
- `port` (u16, default `4000`)
- `metrics_path` (string, default `/metrics`) — path the Prometheus metrics endpoint is served on; change it to avoid colliding with an upstream app or sidecar that already owns `/metrics`. Must be rooted (`/…`) and must not collide with a built-in route (`/healthz`, `/v1/*`).

### `[[providers]]`
- `name` (string, unique) — referenced by route targets
- `kind` (`openai` | `anthropic` | `openai_compatible` | `ollama` | `ollama_cloud` | `llama_cpp` | `openrouter` | `tei` | `azure_openai` | `bedrock` | `vertex`)
- `api_base` (string) — base URL, no trailing slash
- `api_key` (string, optional) — prefer `api_key_env`
- `api_key_env` (string, optional) — environment variable to read the key from
- `role_profile` (`openai` | `system_only` | `anthropic`, optional) — explicit instruction-role semantics. The default is `openai` for `kind = "openai"`, `anthropic` for `kind = "anthropic"`, and conservative `system_only` for every OpenAI-compatible kind. `system_only` converts leading `developer` messages to `system` in place; it rejects a `system` or `developer` message after a user/assistant/tool turn with `role_capability_unsupported` rather than silently changing it.
- `model_role_profiles` (table, optional) — upstream-model-specific `role_profile` overrides. Use this only for a custom template whose developer-role support is explicitly known; rolter never probes a vLLM template at runtime.

#### Role-capability profiles

`openai_compatible` describes the HTTP surface only. vLLM, in particular,
renders roles using the selected model's chat template, so an endpoint's role
support must not be inferred from its `/v1` API. The default `system_only`
profile is suitable for Qwen-style templates that do not define `developer`.
Set `role_profile = "openai"` or a `model_role_profiles` entry only after
confirming that the deployed template supports distinct `developer` messages.

Anthropic targets collect leading OpenAI `developer` and `system` messages into
ordered top-level `system` blocks. Instruction messages placed after a
conversation turn are rejected for `anthropic` and `system_only` profiles;
rolter returns an OpenAI-style `400` with code
`role_capability_unsupported` instead of dropping or reclassifying them.

#### Ollama: local daemon vs Cloud

Use `ollama` for a local/self-hosted daemon such as `http://localhost:11434` (no authentication). Use `ollama_cloud` for direct programmatic Cloud access. Cloud requires `api_key_env` (normally `OLLAMA_API_KEY`); inline keys and key pools are rejected. Configure `api_base = "https://ollama.com"`; rolter uses the OpenAI-compatible `/v1/chat/completions` and `/v1/models` endpoints with bearer authentication. Ollama's native `/api/*` endpoints are distinct.

```toml
[[providers]]
name = "ollama-cloud"
kind = "ollama_cloud"
api_base = "https://ollama.com"
api_key_env = "OLLAMA_API_KEY"
```

#### Azure OpenAI, Amazon Bedrock, and Vertex AI

These providers use their current OpenAI-compatible APIs. Set `api_base` to the
provider's OpenAI-compatible prefix and use an environment-sourced credential:

```toml
[[providers]]
name = "azure"
kind = "azure_openai"
api_base = "https://RESOURCE.openai.azure.com/openai/v1"
api_key_env = "AZURE_OPENAI_API_KEY"

[[providers]]
name = "bedrock"
kind = "bedrock"
api_base = "https://bedrock-runtime.us-east-1.amazonaws.com/v1"
api_key_env = "AWS_BEARER_TOKEN_BEDROCK"

[[providers]]
name = "vertex"
kind = "vertex"
api_base = "https://aiplatform.googleapis.com/v1/projects/PROJECT/locations/global/endpoints/openapi"
api_key_env = "VERTEX_ACCESS_TOKEN"
```

Azure credentials are sent in the `api-key` header. Bedrock and Vertex
credentials are sent as bearer tokens. The default active-health probes use
Azure's model list, Bedrock `ListFoundationModels`, and Vertex's publisher model
list, respectively; none invokes a model.

- `[[providers.api_keys]]` (optional) — multiple weighted API keys for one provider; when present it takes precedence over the single `api_key`/`api_key_env` pair. Providers cap throughput per key, so rotating across keys multiplies effective RPM/TPM
  - `key` (string, optional) — inline key value; prefer `env`
  - `env` (string, optional) — environment variable to read the key from
  - `weight` (u32, default `1`) — relative selection weight
- `api_key_env` (string, optional) — env var to read the key from
- `egress_proxy` (string, optional) — HTTP/HTTPS/SOCKS5 outbound proxy
- `also_track_via_llm_call` (bool, default `false`) — when set, active health checks send a real `max_tokens = 1` completion to this provider instead of the free `/v1/models` liveness probe, so a healthy result proves end-to-end inference. **This burns a few tokens on every sweep** (`interval_secs`); leave it off unless you need inference-level health. Recorded as `source = llm_call` in `provider_health_events`.
- `llm_probe_model` (string, optional) — the upstream model id the `also_track_via_llm_call` completion targets (e.g. `gpt-4o-mini`). **Required** when the flag is on; without it (or an api key) the checker logs a warning and falls back to the free probe.
- `status_page_url` (string, optional) — statuspage.io-style `status.json` URL (e.g. `https://status.anthropic.com/api/v2/status.json`). When set, a slow background poll records the provider's public status as a **secondary** `status_page` health signal — it surfaces in `provider_health_events`, the dashboard and `rolter_status_page_degraded_total`, but never marks the provider unhealthy or affects routing on its own. Parse/transport failures are logged and skipped.

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
- `path` (string, default `/`) — probe path; the default resolves to each provider kind's free liveness endpoint (normally `/v1/models`, or the provider-native Azure, Bedrock, or Vertex model-list endpoint)
- `probe_concurrency` (usize, default `2`) — max probes in flight at once during a sweep, so probing never stampedes upstreams
- `consecutive_failure_threshold` (u32, default `3`) — consecutive probe failures before a provider is marked unhealthy
- `recovery_success_threshold` (u32, default `2`) — consecutive successes before an unhealthy provider recovers
- `status_page_interval_secs` (u64, default `60`) — seconds between provider status-page polls; only providers with a `status_page_url` are polled, and the poller runs even when `enabled = false`
- probes are jittered across the first quarter of the interval (per-provider stable offset), and a `429` on the probe itself pauses that provider's probing with exponential backoff (1, 2, 4, 8 sweeps) without marking it unhealthy

### `[realtime]`

Guardrails for persistent `/v1/realtime` WebSocket sessions. All limits are per gateway process; set a value to `0` to disable that limit.

- `max_connections` (u64, default `1000`) — concurrent sessions admitted by this gateway instance
- `max_session_secs` (u64, default `3600`) — hard session-duration limit
- `idle_timeout_secs` (u64, default `300`) — closes a session when neither side sends a frame

## Environment variables

- `ROLTER_CONFIG`, `ROLTER_HOST`, `ROLTER_PORT` — gateway
- `ROLTER_CONTROL_HOST`, `ROLTER_CONTROL_PORT`, `ROLTER_UI_DIR` — control plane
- `ROLTER_MASTER_KEY` — AES-256-GCM KEK for provider-secret encryption
- `DATABASE_URL`, `REDIS_URL`, `CLICKHOUSE_URL` — datastores
- `RUST_LOG` — tracing filter (e.g. `info`, `rolter_gateway=debug`)
- provider key vars referenced by `api_key_env` (e.g. `OPENAI_API_KEY`)

CLI flags override env, which override file values.
