# External cache telemetry for routing

## Metadata

| Field | Value |
| --- | --- |
| Product | rolter |
| Date | 18 Jul 2026 |
| Status | ACCEPTED |
| Issues | [#258](https://github.com/rolter-ai/rolter/issues/258), [#259](https://github.com/rolter-ai/rolter/issues/259) |
| Supersedes | ADR-0007 for exact vLLM and LMCache-aware modes |

## Context

ADR-0007 deliberately introduced approximate prompt-prefix affinity without
coupling the gateway to an inference engine. vLLM can now publish exact KV block
residency events, while LMCache deployments can expose availability and occupancy.
Using those signals can improve reuse, but blocking network calls or trusting stale
telemetry on the request path would weaken gateway latency and availability.

## Options considered

1. Keep only the engine-independent approximate prefix trie.
2. Query cache state synchronously during every routing decision.
3. Consume telemetry in background tasks and expose bounded in-memory scorers.

## Decision

Adopt option 3 as two opt-in strategies behind the existing balancer interface:

- `precise_cache_aware` subscribes to the supported vLLM V1 three-frame ZMQ
  msgpack KV-event protocol, derives stable token-block identities, and scores the
  resident leading fraction of tokenizer-aligned request blocks.
- `lmcache_aware` polls a rolter-defined HTTP JSON signal containing `occupancy`
  and `cache_available`, then scores available targets as `1 - occupancy`.

Both sources update bounded process-local state outside the request path. Missing
request token IDs, malformed input, disconnects, stale data, and vLLM sequence gaps
produce a neutral score and preserve least-load fallback. A sequence gap clears the
precise index until an explicit all-blocks-cleared boundary re-establishes trust.

## Consequences

- Exact vLLM residency and LMCache capacity can influence routing without request-
  path network I/O.
- Approximate `cache_aware` remains available and engine-independent.
- vLLM exact scoring requires token IDs from the same tokenizer and a compatible
  event protocol; version assumptions are part of the operator documentation.
- LMCache integration depends only on the small documented controller contract,
  not an unstable internal LMCache API.
- Telemetry state is local to each gateway replica and intentionally disposable;
  restart, staleness, or desynchronization temporarily reduces routing quality but
  does not prevent requests.
