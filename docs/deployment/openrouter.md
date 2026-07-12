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

