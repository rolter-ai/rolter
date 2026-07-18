# Load balancing

Each route maps a public model name to one or more upstream targets and a strategy. Strategies implement `rolter_balancer::LoadBalancer`:

```rust
pub trait LoadBalancer: Send + Sync {
    fn name(&self) -> &'static str;
    fn pick(&self, ctx: &RouteContext, loads: &[u64]) -> Option<usize>;
    fn observe(&self, target: usize, ctx: &RouteContext) {}
}
```

`pick` returns an index into the route's targets; `observe` lets learning strategies (cache-aware) record what a target served. `RouteContext` carries an optional `session_key` (from `x-session-id`) and the request `prompt` used for affinity scoring.

## Strategies (v1)

- **round_robin** — sequential rotation; predictable, zero state.
- **random** — uniform random; good for simple homogeneous pools.
- **power_of_two** — pick the less loaded of two random targets; needs a load snapshot.
- **consistent_hash** — hash-ring keyed by `session_key` (falls back to prompt hash); pins a session/user to a target for KV reuse, survives target changes with minimal reshuffle (160 vnodes).
- **cache_aware** — approximate prefix affinity; see [caching.md](caching.md).
- **weighted** — smooth weighted round-robin honouring each target's `weight`.
- **pipeline** — composable **filter → weighted-score → argmax** selection: eligibility filtering drops ineligible targets, then a stack of `Scorer`s (session affinity + static weight + in-flight load + prefix-cache affinity) is combined as a weighted sum and the argmax wins (ties broken randomly). Session affinity pins repeat requests from the same `x-session-id` to their last-served target (TTL-bounded) for warm-cache reuse. The extension point every future cost/latency/KV-cache scorer plugs into.
- **precise_cache_aware** — consumes each target's vLLM ZMQ KV-event stream and scores the exact leading fraction of caller-supplied token blocks resident on that target. Missing token ids and stale, malformed, disconnected, or sequence-gapped streams stay neutral; least-load routing remains the fallback.
- **lmcache_aware** — polls each target's configured LMCache controller signal and prefers available caches with free capacity (`1 - occupancy`). Empty, saturated, failed, and stale controllers stay neutral and fall back to least load.

## Choosing a strategy

| Use case | Strategy |
| --- | --- |
| Homogeneous pool, stateless | `round_robin` / `random` |
| Variable request durations | `power_of_two` |
| Multi-turn chat, sticky session | `consistent_hash` |
| Shared system prompts / few-shot / RAG | `cache_aware` |
| Blend cache + load + weight signals | `pipeline` |
| vLLM fleet with KV event publishing | `precise_cache_aware` |
| LMCache fleet with occupancy controller | `lmcache_aware` |
| Mixed-price providers, minimize spend | `cheapest` |
| Heterogeneous pool, minimize latency | `fastest` |

Both external strategies perform network I/O only in background tasks. The request hot path reads bounded in-process state and atomics.
