# Caching

rolter deals with two distinct kinds of caching.

## 1. KV-cache affinity (load balancing)

The big win for self-hosted fleets: vLLM/SGLang reuse the attention KV cache for shared prompt prefixes (system prompts, few-shot examples, conversation history). But that only helps if the next matching request lands on the **same** replica. Naive round-robin scatters related requests and destroys cache locality.

The `cache_aware` strategy keeps, per target, a byte **trie** of prompts it has served. For an incoming prompt it computes the fraction of leading bytes already present on each target and:

- if the best match ≥ `threshold` (default `0.5`), pins the request to that target (cache hit)
- otherwise spreads to the least-warmed target (or least loaded once load is wired)

This is **approximate** (no coupling to the engine). It is intentionally simple in v1; the trie grows unbounded and eviction is a roadmap item.

```mermaid
flowchart TD
  R[incoming prompt] --> S{best prefix match >= threshold?}
  S -- yes --> P[pin to best target<br/>cache hit]
  S -- no --> L[least-warmed / least-loaded target]
  P --> O[observe: insert prompt into target trie]
  L --> O
```

### Precise mode (roadmap)

Subscribe to vLLM KV-cache events over ZMQ, hash blocks the same way vLLM does (`--block-size`, hash seed), and maintain a global block→target index. Score targets by exact resident-prefix fraction blended with live load. This mirrors llm-d's precise prefix-cache-aware scheduling and gives the largest, most reliable TTFT/throughput wins on prefix-heavy workloads.

## 2. Response cache (roadmap)

Optional caching of full responses to cut cost/latency for repeated requests:

- **exact**: hash of the normalized request → cached response (Redis), short TTL, opt-in per route/key.
- **semantic**: embed the prompt, match by cosine similarity above a threshold; requires an embeddings provider.

Streaming responses are cached on completion and replayed as a synthetic stream. Cache status is surfaced via response headers (e.g. `x-rolter-cache: hit|miss`).
