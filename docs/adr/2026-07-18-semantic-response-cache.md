# Bounded semantic response caching in Redis

## Metadata

| Field | Value |
| --- | --- |
| Product | rolter |
| Date | 18 Jul 2026 |
| Status | ACCEPTED |
| Issues | [#261](https://github.com/rolter-ai/rolter/issues/261) |

## Context

The exact response cache only reuses byte-equivalent normalized requests. Similar
prompts still reach an upstream model even when an existing response would be
acceptable. Semantic reuse adds an embedding request and a nearest-neighbour
search, but it must not make gateway availability depend on the embedding provider
or Redis and must keep request-path work bounded.

## Options considered

1. Keep exact caching only.
2. Introduce a dedicated vector database and approximate-nearest-neighbour index.
3. Store embeddings alongside Redis cache entries and scan a bounded recent window.

## Decision

Adopt option 3 as an opt-in route capability. Exact lookup always runs first. On
an exact miss, rolter embeds the normalized prompt using an explicitly configured
provider/model and compares cosine similarity against at most `max_candidates`
recent entries in the route/key-isolated Redis index. A response is reused only
when its score reaches `threshold`. Semantic metadata and the exact response share
the route cache TTL.

Embedding, Redis, decoding, dimension, missing-entry, and stale-entry failures all
fail open to normal upstream routing. Streaming responses enter both indexes only
after the stream completes successfully.

## Consequences

- Similar prompts can avoid model cost and latency without a new datastore.
- Exact-cache behavior and its faster lookup remain unchanged.
- Search cost is predictable but linear in the configured candidate bound; this is
  not intended as a large corpus vector-search engine.
- The embedding provider adds cost and latency on semantic misses and becomes part
  of cache-result quality, though never gateway availability.
- Operators own the similarity threshold and must account for unsafe reuse in
  domains where small wording changes alter the correct answer.
