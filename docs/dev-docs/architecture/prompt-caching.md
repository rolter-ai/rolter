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

## Whether a provider reports cache metrics in the dashboard

The verdicts of `provider_cache_probes` (see [the data model](data-model.md)) are shown on Providers, Models and Provider Groups by `CacheReportingBadge`, in three states that must stay apart: reports (success tone), no cache metrics (warning tone) and not probed yet (outline tone). A provider with no stored verdict is "not probed yet", never "no". Each verdict carries the time it was established, and its title the model and field spelling, so a stale or narrow verdict is visible as such.

A route or group has no verdict of its own; `summarizeCacheReporting` (`ui/src/lib/cache-reporting.ts`) derives it over the providers behind its targets or members:

- **no cache metrics** as soon as any one of them is known not to report, since that provider's traffic carries no cache usage;
- **reports** only when every one of them has a verdict and all of them report;
- **not probed yet** otherwise.

The time shown is that of the oldest verdict the claim rests on. When the verdicts cannot be read (still loading, or the read failed) no badge is drawn at all, because "not probed yet" would be a claim the screen has no evidence for. The probe itself is a real one-token completion, so the dashboard runs it only on request, from the Providers row, gated as `provider:update`, and not for the kinds the endpoint refuses (`canProbeCache` in `ui/src/lib/api/providers.ts`).
