# Configuration reference

The gateway boots from a TOML file (`--config`, default `rolter.toml`); see [`rolter.example.toml`](../../rolter.example.toml). At runtime, the control plane is the source of truth and applies changes without a restart ([config-and-hot-reload.md](../architecture/config-and-hot-reload.md)).

## Schema

### `[server]`
- `host` (string, default `0.0.0.0`)
- `port` (u16, default `4000`)
- `metrics_path` (string, default `/metrics`) — path the Prometheus metrics endpoint is served on; change it to avoid colliding with an upstream app or sidecar that already owns `/metrics`. Must be rooted (`/…`) and must not collide with a built-in route (`/healthz`, `/v1/*`).

### `[tls]`
- `ca_bundles` (string[], default `[]`) — PEM CA-bundle files added to the normal public-root trust store for outbound upstream TLS. `ROLTER_CA_BUNDLE` replaces this global list with a single deployment-local path. Files are checked for missing, unreadable, empty, and malformed content while config is loaded.

### `[[providers]]`
- `name` (string, unique) — referenced by route targets
- `kind` (`openai` | `anthropic` | `openai_compatible` | `ollama` | `ollama_cloud` | `llama_cpp` | `openrouter` | `tei` | `azure_openai` | `bedrock` | `vertex` | `gemini` | `gemini_native` | `gemini_interactions` | `mistral` | `groq` | `xai` | `meta_llama_api` | `cohere` | `perplexity` | `together` | `fireworks` | `databricks` | `aleph_alpha` | `nebius` | `ovhcloud` | `scaleway` | `deepseek` | `qwen` | `zhipu` | `kimi` | `ernie` | `doubao` | `hunyuan` | `yi` | `minimax` | `baichuan` | `gigachat` | `yandex_gpt` | `cloud_ru` | `mts_ai` | `naver` | `upstage` | `rinna` | `rakuten` | `sarvam` | `krutrim` | `falcon`)
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

Google Gemini, Mistral, Groq, and xAI (Grok) expose hosted OpenAI-compatible
APIs. Their
`api_base` already carries the version segment, so rolter strips the leading
`/v1` from the gateway path before appending it. Keys are bearer tokens sourced
from `api_key_env` (inline keys are rejected); the free health probe lists
`{api_base}/models`.

`gemini_native` targets Gemini's native `generateContent` surface instead of its
OpenAI-compatible shim. rolter translates OpenAI Chat / Anthropic Messages /
OpenAI Responses requests into Gemini's `contents`/`parts` wire format and
converts the response (and SSE stream) back, so clients keep speaking their
usual protocol. The model and method are embedded in the URL
(`{api_base}/models/{model}:generateContent`, or `:streamGenerateContent?alt=sse`
for streaming), the key is sent as `x-goog-api-key`, and `api_base` points at the
version root with no `/openai` suffix.

`gemini_interactions` targets Gemini's stateful Interactions API. Every
chat-shaped request is translated onto the single `{api_base}/interactions`
endpoint: turns become `input` items, system/developer messages become
`system_instruction`, tool calls and tool results become `function_call` /
`function_result` items, and sampling parameters become `generation_config`. The
response `steps[]` (and the `interaction.created` / `step.delta` /
`interaction.completed` SSE events) are converted back into the client's dialect.
The thread is client-driven: the interaction id is returned as the response `id`,
and a client resumes it by sending `previous_response_id` (OpenAI Responses) or
`previous_interaction_id` — rolter keeps no interaction state of its own. Auth is
`x-goog-api-key`, and `api_base` points at the version root. Endpoints with no
interactions equivalent (embeddings, audio) are rejected rather than forwarded.

```toml
[[providers]]
name = "gemini"
kind = "gemini"
api_base = "https://generativelanguage.googleapis.com/v1beta/openai"
api_key_env = "GEMINI_API_KEY"

# native generateContent wire format (translated from OpenAI/Anthropic)
[[providers]]
name = "gemini-native"
kind = "gemini_native"
api_base = "https://generativelanguage.googleapis.com/v1beta"
api_key_env = "GEMINI_API_KEY"

# stateful Interactions API (translated from OpenAI/Anthropic/Responses)
[[providers]]
name = "gemini-interactions"
kind = "gemini_interactions"
api_base = "https://generativelanguage.googleapis.com/v1beta"
api_key_env = "GEMINI_API_KEY"

[[providers]]
name = "mistral"
kind = "mistral"
api_base = "https://api.mistral.ai/v1"
api_key_env = "MISTRAL_API_KEY"

[[providers]]
name = "groq"
kind = "groq"
api_base = "https://api.groq.com/openai/v1"
api_key_env = "GROQ_API_KEY"

[[providers]]
name = "xai"
kind = "xai"
api_base = "https://api.x.ai/v1"
api_key_env = "XAI_API_KEY"
```

- `[[providers.api_keys]]` (optional) — multiple weighted API keys for one provider; when present it takes precedence over the single `api_key`/`api_key_env` pair. Providers cap throughput per key, so rotating across keys multiplies effective RPM/TPM
  - `key` (string, optional) — inline key value; prefer `env`
  - `env` (string, optional) — environment variable to read the key from
  - `weight` (u32, default `1`) — relative selection weight
- `api_key_env` (string, optional) — env var to read the key from
- `egress_proxy` (string, optional) — legacy single HTTP/HTTPS/SOCKS5 outbound proxy; treated as a one-element pool
- `egress_proxies` (string[], optional) — round-robin HTTP, HTTPS, SOCKS5, or SOCKS5H proxy pool. A connect/tunnel failure retries the next member; three consecutive failures quarantine a member for 30 seconds. Authenticated proxy URLs must be supplied as whole-value environment references such as `"${PROVIDER_PROXY_EU}"`, keeping credentials out of config snapshots and database/API output
- `ca_bundles` (string[], optional) — provider-specific replacement for global `[tls].ca_bundles`; `[]` explicitly selects public roots only
- `[providers.kv_events]` (optional) — vLLM V1 ZMQ KV-event source for `precise_cache_aware`: `endpoint` (`tcp://…`), `topic` (default `kv-events`), `max_blocks` (default 1,000,000), and `stale_secs` (default 30)
- `[providers.lmcache]` (optional) — LMCache controller signal for `lmcache_aware`: `endpoint` (HTTP JSON occupancy signal), `refresh_secs` (default 2), and `stale_secs` (default 10)
- `also_track_via_llm_call` (bool, default `false`) — when set, active health checks send a real `max_tokens = 1` completion to this provider instead of the free `/v1/models` liveness probe, so a healthy result proves end-to-end inference. **This burns a few tokens on every sweep** (`interval_secs`); leave it off unless you need inference-level health. Recorded as `source = llm_call` in `provider_health_events`.
- `llm_probe_model` (string, optional) — the upstream model id the `also_track_via_llm_call` completion targets (e.g. `gpt-4o-mini`). **Required** when the flag is on; without it (or an api key) the checker logs a warning and falls back to the free probe.
- `status_page_url` (string, optional) — statuspage.io-style `status.json` URL (e.g. `https://status.anthropic.com/api/v2/status.json`). When set, a slow background poll records the provider's public status as a **secondary** `status_page` health signal — it surfaces in `provider_health_events`, the dashboard and `rolter_status_page_degraded_total`, but never marks the provider unhealthy or affects routing on its own. Parse/transport failures are logged and skipped.

See [Custom CA bundles](custom-ca-bundles.md) for rotation behavior and Docker/Kubernetes mount examples.

### `[[routes]]`
- `model` (string) — public model name clients request
- `strategy` (`round_robin` | `random` | `power_of_two` | `consistent_hash` | `cache_aware` | `weighted` | `pipeline` | `cheapest` | `fastest` | `precise_cache_aware` | `lmcache_aware` | `adaptive` | `lora_aware` | `predicted_latency`, default `round_robin`)
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
- `[routes.advanced.guardrails]` (optional) — per-route selection layered over the global [`[guardrails]`](#guardrails) rule set. Rules are named, so adding one globally still reaches every route that has not opted out of it by name.
  - `disable` (string[], default `[]`) — rules that do not apply on this route
  - `enable` (string[], default `[]`) — rules that apply on this route; wins over `disable` on a conflict
  - a name matching no configured rule fails validation rather than being ignored — a typo in `disable` would otherwise read as "this rule is off here" while the rule kept running

### `[[virtual_keys]]`
- `key` (string) — the bearer token clients present
- `name` (string, optional)
- `models` (string[], default `[]`) — allow-list; empty = all

### `[adaptive_routing]`
Deployment-wide policy for routes using the `adaptive` strategy. See [load balancing](../architecture/load-balancing.md#adaptive-routing).
- `enabled` (bool, default `false`) — kill switch; while off, every `adaptive` route serves the `pipeline` stack
- `latency_weight` (f32, default `1.0`), `cost_weight` (f32, default `0.5`), `load_weight` (f32, default `0.25`) — blend weights; negatives are clamped to `0`, and all-zero disables the blend
- `exploration_ratio` (f32, default `0.05`) — share of picks made at random to keep latency samples fresh; clamped to `[0, 0.5]`
- `min_samples` (u32, default `50`) — requests a route must serve before the blend engages

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

The `/v1/realtime` relay carries the `realtime` [stability marker](../development/stability-markers.md), so these keys may change shape in a minor release. They are also the *only* limits a realtime session meets: budgets, rate limits and usage recording sit on the HTTP request path and do not see it.

- `max_connections` (u64, default `1000`) — concurrent sessions admitted by this gateway instance
- `max_session_secs` (u64, default `3600`) — hard session-duration limit
- `idle_timeout_secs` (u64, default `300`) — closes a session when neither side sends a frame

### `[egress]`

Where the gateway may send upstream traffic. A provider's `api_base` (and any `egress_proxy`) is operator-supplied, so without a destination policy a crafted or compromised provider row turns the gateway into an SSRF primitive pointed at whatever sits inside its network position — cloud instance metadata being the classic target.

- `block_link_local` (bool, default `true`) — deny `169.254.0.0/16` and `fe80::/10`. This is the cloud instance-metadata range; leave it on unless you have a concrete reason
- `block_loopback` (bool, default `false`) — deny `127.0.0.0/8` and `::1`. Off by default because sidecar and single-host deployments legitimately serve models on localhost
- `block_private` (bool, default `false`) — deny `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` and `fc00::/7`. Off by default because on-prem clusters are the common case
- `allow_hosts` (string[], default `[]`) — hosts exempt from every check above, matched verbatim against the URL host (an IP literal or a hostname). The escape hatch for a deployment that must reach one otherwise-denied address

Enforcement happens twice. IP literals are classified during config validation, so a bad `api_base` is rejected at startup or when the control plane writes it. Hostnames are classified **at connect time**, against the address DNS actually returned — validation never resolves DNS itself (that would make config validation depend on a live resolver and break air-gapped deployments), and a connect-time check is what makes the policy DNS-rebinding-safe. A denied address fails the request; it is not silently retried elsewhere.

```toml
[egress]
block_link_local = true
block_private = true
allow_hosts = ["vllm.internal.example.com"]  # the one private upstream we mean to reach
```

### `[guardrails]`

Built-in, zero-dependency regex guardrails and PII redaction, evaluated inside the gateway with no external service and no network hop. Disabled by default; a disabled or empty block adds no hot-path cost. Complements — never replaces — the custom guardrail webhook (ROL-257) and external PII engines (ROL-258).

- `enabled` (bool, default `false`) — master switch
- `max_scan_bytes` (usize, default `262144`) — cap on total request text scanned per request; oversized content is forwarded unscanned so work stays bounded
- `streaming_post_call` (string, default `reject`) — what a streamed request does on a route that has `post_call` rules; see [Output masking and streaming](#output-masking-and-streaming)

Each `[[guardrails.rules]]` entry:

- `name` (string, required) — stable, unique; surfaced in metrics, never carries match text
- `builtin` (string) — one of `email`, `phone`, `api_token`, `payment_card`; **or** `pattern` (string) for a custom regex. Set exactly one.
- `stage` (string, default `pre_call`) — `pre_call` scans request content before proxying; `post_call` masks the response body before it reaches the client
- `action` (string, default `annotate`) — `annotate` (count only, forward unchanged), `block` (reject with an OpenAI-compatible `guardrail_blocked` error), or `redact` (replace each match with `replacement`)
- `replacement` (string) — redaction token; defaults to the built-in entity token (e.g. `[REDACTED:EMAIL]`) or `[REDACTED]`
- `include_system` (bool, default `false`) — also scan operator-authored `system`/`developer` messages; excluded by default

> `default_on` was removed. It documented a per-request client opt-in that was
> never implemented, so it never selected anything: every configured rule
> applied regardless of its value. Existing configs that still set it keep
> loading — the key is ignored — and behaviour is unchanged. To turn a rule off
> somewhere, name it in that route's `[routes.advanced.guardrails]` `disable`
> list (see [`[[routes]]`](#routes)).

Patterns use the linear-time (RE2-style) `regex` engine with no catastrophic backtracking, and are compiled under a bounded program size during config validation — an invalid or unbounded pattern fails at startup/snapshot validation, never on the request path. The request path never logs raw matched values; metrics expose `rolter_guardrail_blocks_total` and `rolter_guardrail_redactions_total` only.

Scanned surfaces: OpenAI `/v1/chat/completions` and `/v1/responses` (`messages` + `input`), `/v1/completions` (`prompt`), and Anthropic `/v1/messages` (`system` + `messages`). String, string-array, and typed `text` parts are all covered.

```toml
[guardrails]
enabled = true

[[guardrails.rules]]
name = "email"
builtin = "email"
action = "redact"

[[guardrails.rules]]
name = "card"
builtin = "payment_card"
action = "block"

[[guardrails.rules]]
name = "leaked-key"
builtin = "api_token"
stage = "post_call"
action = "redact"
```

#### Output masking and streaming

A `post_call` rule runs on the response body after the upstream replies and before the client sees it. `redact` rewrites the matched text in place; `block` withholds the completion and returns a `403` with code `guardrail_blocked` and the rule name — nothing was malformed and no upstream failed, so it is neither a `400` nor a `5xx`, and it stays out of the range clients retry on.

Masked surfaces: `choices[].message.content` and `choices[].text` (OpenAI chat and legacy completions), the top-level `content` parts array (Anthropic `/v1/messages`), and `output[].content[].text` plus `output_text` (`/v1/responses`).

**Tool-call arguments are scanned too.** An address the model passes to a `send_email` tool has left the completion just as surely as one it printed. Arguments that arrive as a JSON document inside a JSON string (`choices[].message.tool_calls[].function.arguments`, the legacy `function_call.arguments`, and `/v1/responses` `output[].arguments`) are parsed, masked and re-encoded, so a replacement token containing a quote or a backslash cannot turn valid arguments into something the client fails to parse; non-string values are untouched. Anthropic `tool_use` parts are masked through their `input` object. Arguments that are not valid JSON are masked as plain text.

Output masking requires the whole completion. A match can straddle any number of token boundaries, so a rule redacting `a@b.com` cannot act on a frame holding only `a@b`, and buffering the full stream would remove the only property streaming has. `streaming_post_call` therefore decides what a streamed request does on a route with `post_call` rules:

- `reject` (default) — refuse with a `400` carrying code `guardrail_streaming_unsupported`, counted in `rolter_guardrail_stream_rejections_total`. The default fails closed: a masking rule that silently stops applying because the client passed `"stream": true` is the failure mode worth ruling out.
- `passthrough` — serve the stream with output rules not applied. `pre_call` rules still run on the request.

Non-streamed responses are buffered by the gateway when (and only when) a `post_call` rule applies to the route, so a route without them keeps its existing forwarding behaviour. Cached responses are stored as the upstream returned them and masked on every delivery, not once at store time — so a rule added after an entry was cached still applies to it, and an entry shared by two routes is masked per the route serving it. Output metrics: `rolter_guardrail_output_redactions_total` and `rolter_guardrail_output_blocks_total`.

### `[guardrail_webhook]`

A vendor-neutral hook to a self-hosted semantic guardrail service (e.g. Guardrails AI, LLM Guard). Before proxying, the gateway POSTs a stable JSON envelope to the configured endpoint; the service replies with an allow/block/transform/annotate decision. Disabled by default; complements the built-in regex guardrails.

- `enabled` (bool, default `false`) — master switch
- `url` (string) — http(s) endpoint the envelope is POSTed to (required when enabled)
- `stage` (string, default `pre_call`) — `pre_call` inspects the request. `post_call` is validated but not yet enforced (output/SSE stage deferred).
- `timeout_ms` (u64, default `2000`) — per-call timeout
- `max_retries` (u32, default `0`) — extra attempts on a transient failure (connect/timeout/non-2xx)
- `failure_mode` (string, default `fail_open`) — `fail_open` forwards unchanged when the service is unreachable; `fail_closed` rejects with an OpenAI-compatible error
- `max_body_bytes` (usize, default `65536`) — cap on the content forwarded; oversized content is sent as a truncated preview with `truncated: true`
- `auth` — optional credential resolved from the environment at call time, never inlined:
  - `{ bearer = { token_env = "GUARD_TOKEN" } }` → `Authorization: Bearer <env>`
  - `{ shared_secret = { secret_env = "GUARD_SECRET" } }` → `X-Rolter-Guardrail-Secret: <env>`

**Contract.** Request envelope: `{ direction, stage, model, route, trace_id, tenant: { org, team, project, key }, truncated, content }`. Only these fields are sent; prompt content is never logged by the gateway. Response: `{ "action": "allow" | "block" | "transform" | "annotate", "content"?, "reason"?, "annotations"? }`. An unrecognized or malformed decision defaults to `allow` (transport failures are governed by `failure_mode`). Metrics: `rolter_guardrail_webhook_blocks_total`, `_transforms_total`, `_errors_total`; the trace id is propagated in the `X-Rolter-Trace-Id` header.

```toml
[guardrail_webhook]
enabled = true
url = "https://guard.internal/check"
failure_mode = "fail_closed"
auth = { bearer = { token_env = "GUARD_TOKEN" } }
```

### `[prompt_templates]`

Centrally-managed, versioned prompt templates and deterministic route decorators (ROL-256). Applications reuse approved system instructions through a named template without being granted arbitrary prompt-authoring privileges. Disabled by default; an empty or disabled block adds no hot-path cost.

- `enabled` (bool, default `false`) — master switch

Each `[[prompt_templates.templates]]` entry is one immutable version:

- `id` (string, required) — stable identifier surfaced in safe metadata, never in content logs
- `version` (u32, required, ≥ 1) — immutable version; the operator lists exactly the versions to activate. `(id, version)` must be unique.
- `routes` (array of string, default all) — public model names this template applies to; empty means every route
- `[[prompt_templates.templates.variables]]` — a named variable a decorator may reference as `{{ name }}`:
  - `name` (string, `[A-Za-z_][A-Za-z0-9_]*`)
  - `required` (bool, default `false`) — the caller must supply it; mutually exclusive with `default`
  - `default` (string) — value used when the caller omits it
- `[[prompt_templates.templates.decorators]]` — a message injected around the caller's own messages:
  - `role` (string, default `system`) — `system`, `assistant`, or `user`
  - `position` (string, default `prepend`) — `prepend` (before the caller's messages) or `append` (after), both in declared order
  - `content` (string) — message text, with optional `{{ variable }}` placeholders

**Variables and escaping.** Callers pass values in a `rolter_template_vars` object on the request body; it is always stripped before forwarding upstream. A caller value overrides the declared default; an unknown variable, a missing required variable, or an oversized value (variable > 4 KiB, rendered message > 16 KiB) is rejected with an `invalid_prompt_template` error. Substitution is **structural**: each rendered message is emitted as a JSON string through the serializer, never string-concatenated into raw JSON, so a variable value can never break out of its string or inject additional messages. Every `{{ placeholder }}` is validated at config-load time to reference a declared variable.

**Surfaces and ordering.** Applied to `/v1/chat/completions`, `/v1/responses`, and Anthropic `/v1/messages`. Prepend decorators wrap before, append after, preserving the caller's own message order and semantics. For Anthropic, `system` decorators fold into the top-level `system` field (joined by blank lines); `assistant`/`user` decorators wrap the `messages` array. Surfaces without a chat message array (e.g. `/v1/completions`) are not decorated. The gateway applies only the configured immutable version from its reload-free snapshot. Applied template id/version and decoration count are recorded in safe metadata; metrics expose `rolter_prompt_template_decorations_total` and `rolter_prompt_template_rejections_total`.

```toml
[prompt_templates]
enabled = true

[[prompt_templates.templates]]
id = "support-preamble"
version = 3
routes = ["gpt-4o"]

[[prompt_templates.templates.variables]]
name = "persona"
default = "a helpful support assistant"

[[prompt_templates.templates.decorators]]
role = "system"
position = "prepend"
content = "You are {{persona}}. Follow the company policy and be concise."
```

A caller then supplies variables per request:

```json
{
  "model": "gpt-4o",
  "messages": [{ "role": "user", "content": "hi" }],
  "rolter_template_vars": { "persona": "a billing specialist" }
}
```

When the control plane uses PostgreSQL, prompt templates can also be authored
through the org-scoped `/api/v1/orgs/{org_id}/prompt-templates` endpoints. Draft
versions are immutable once published, publication and rollback update the
reload-free snapshot, and activation can be limited to an organization, project,
route, or virtual key. Seed import creates missing versions idempotently and
refuses to overwrite an existing immutable version with different content.

Organization skills use the adjacent
`/api/v1/orgs/{org_id}/skills` endpoints. A skill has mutable metadata and access
policy, immutable inline or reference-backed versions, explicit
publish/rollback/retire lifecycle, and a deterministic
`/resolve/{slug}` endpoint. Reference-backed versions accept `https`, `git+https`,
`oci`, and `s3` references without embedded credentials; secret-bearing metadata
keys are rejected.

## Environment variables

- `ROLTER_CONFIG`, `ROLTER_HOST`, `ROLTER_PORT` — gateway
- `ROLTER_CONTROL_HOST`, `ROLTER_CONTROL_PORT`, `ROLTER_UI_DIR` — control plane
- `ROLTER_KEK` — AES-256-GCM KEK for provider-secret encryption
- `ROLTER_PUBLIC_URL` — the control plane's externally reachable base URL (default `http://localhost:4001`). The OIDC redirect URI is derived from it, so single sign-on needs it set correctly behind a proxy; see [Single sign-on](../architecture/sso.md)
- `DATABASE_URL`, `REDIS_URL`, `CLICKHOUSE_URL` — datastores
- `RUST_LOG` — tracing filter (e.g. `info`, `rolter_gateway=debug`)
- provider key vars referenced by `api_key_env` (e.g. `OPENAI_API_KEY`)

CLI flags override env, which override file values.
