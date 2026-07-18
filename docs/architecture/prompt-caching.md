# Provider prompt caching

Rolter's response cache and provider prompt caching solve different problems. The response cache replays a complete prior answer from Redis. Provider prompt caching lets an upstream reuse stable input context while it still generates a fresh answer.

Use the portable `cache_control` object only on requests routed to a direct Anthropic provider:

```json
{
  "model": "claude-route",
  "messages": [{"role": "user", "content": "Summarize this document."}],
  "cache_control": {
    "enabled": true,
    "ttl": "5m",
    "breakpoints": ["system", "tools", "messages"]
  }
}
```

Rolter converts each selected breakpoint to Anthropic's native ephemeral `cache_control` marker. `ttl` is limited to `5m` or `1h`; omitted `breakpoints` defaults to `system`. Existing nested provider-native controls are preserved.

`bedrock` and `vertex` providers currently use their OpenAI-compatible wire mode in Rolter. They reject this portable control with `prompt_cache_unsupported` instead of silently forwarding it or claiming cache use. Configure a direct Anthropic provider for the portable contract until native Bedrock/Vertex Claude transports are added.

Prompt-cache reads and writes are provider billing/usage concepts. They are distinct from the invocation log's `cache_hit`, which only denotes a Rolter response-cache hit.
