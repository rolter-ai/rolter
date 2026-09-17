# OpenRouter

rolter's `openrouter` provider targets OpenRouter's OpenAI-compatible API while
keeping OpenRouter model identifiers and routing controls intact.

## Configuration

Create an API key in OpenRouter and expose it only through the environment:

```bash
export OPENROUTER_API_KEY='...'
```

```toml
[[providers]]
name = "openrouter"
kind = "openrouter"
api_base = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"

[[routes]]
model = "router-chat"
strategy = "round_robin"
[[routes.targets]]
provider = "openrouter"
model = "anthropic/claude-sonnet-4"
```

## Pointing at something other than openrouter.ai

An `openrouter` provider's `api_base` is pinned to `https://openrouter.ai/api/v1`.
The kind names one operator and the key belongs to that operator, so the pin is
what stops an edited base URL from sending a live OpenRouter key to another
host.

When the endpoint genuinely is somewhere else — a corporate egress proxy or
internal gateway in front of OpenRouter, an endpoint that re-implements the
API, or a local fake used to exercise the dialect — opt out explicitly:

```toml
[[providers]]
name = "openrouter-via-proxy"
kind = "openrouter"
api_base = "https://llm-egress.corp.example/openrouter/v1"
api_key_env = "OPENROUTER_API_KEY"
allow_custom_api_base = true
```

The opt-out relaxes the host pin and nothing else: the key must still come from
`api_key_env`. `rolter check` reports every provider that sets it, and
`rolter check --strict` fails on it, so the redirection stays visible in a
review rather than becoming a copied default. See
[ADR-0029](../adr/2026-09-02-hosted-provider-host-pin-opt-out.md).

Providers stored in the control-plane database cannot opt out yet; the pin
applies to them unconditionally.

The public rolter model (`router-chat`) is rewritten only to the target model
override. OpenRouter identifiers such as `anthropic/claude-sonnet-4`, including
their provider prefix and optional variants, are otherwise forwarded verbatim.
rolter fallback chooses another configured target after a retryable failure;
OpenRouter's own `provider` request object then controls routing among upstreams
inside the selected OpenRouter target.

For example, this body preserves OpenRouter's provider ordering and fallback
policy:

```json
{
  "model": "router-chat",
  "messages": [{"role": "user", "content": "hello"}],
  "provider": {
    "order": ["Anthropic", "Google"],
    "allow_fallbacks": true,
    "data_collection": "deny"
  }
}
```

Chat completions, SSE chunks, usage/cost fields, response metadata, and
OpenRouter error JSON pass through without normalization. rolter still applies
its normal authentication, route policy, retries, cooldowns, health checks,
request logging, and routing headers. `/v1/models` lists configured rolter route
aliases rather than exposing every model in OpenRouter's catalog.

## Attribution headers

OpenRouter recommends `HTTP-Referer` and `X-Title` for application attribution;
they are not required for authentication. rolter omits them by default so it
does not disclose deployment identity. Set either explicitly when desired:

```bash
export OPENROUTER_HTTP_REFERER='https://example.com'
export OPENROUTER_X_TITLE='Example gateway'
```

These values are forwarded only by `openrouter` providers. Never put API keys,
user identifiers, or private internal hostnames in attribution headers.

## Live smoke

The ignored live test makes a billable request and therefore requires both a
credential and an explicitly selected model:

```bash
OPENROUTER_API_KEY=... ROLTER_OPENROUTER_LIVE_MODEL=openai/gpt-4.1-mini \
  cargo test -p rolter-gateway --test openrouter live_openrouter_smoke -- --ignored
```
