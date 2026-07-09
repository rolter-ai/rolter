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
- **pipeline** — composable **filter → weighted-score → argmax** selection: eligibility filtering drops ineligible targets, then a stack of `Scorer`s (static weight + in-flight load + prefix-cache affinity) is combined as a weighted sum and the argmax wins (ties broken randomly). The extension point every future cost/latency/KV-cache scorer plugs into.

## Choosing a strategy

| Use case | Strategy |
| --- | --- |
| Homogeneous pool, stateless | `round_robin` / `random` |
| Variable request durations | `power_of_two` |
| Multi-turn chat, sticky session | `consistent_hash` |
| Shared system prompts / few-shot / RAG | `cache_aware` |
| Blend cache + load + weight signals | `pipeline` |

## Roadmap

The trait is the extension point. Planned strategies:

- **precise cache-aware** — subscribe to vLLM KV-cache events (ZMQ), index block hashes, score targets by resident-prefix fraction blended with load. Requires vLLM ≥ 0.10 with matching `--block-size` / hash seed.
- **lmcache-aware** — query an LMCache controller for real cache occupancy.
- **latency-based** and **cost-based** — route by observed p50/p95 latency or per-token price.
- **weighted** selection honoring `Target.weight`.
- **health/circuit breaking + cooldowns** — skip unhealthy targets; exponential backoff on 429/5xx.

Live per-target load (`loads`) will be fed from in-flight counters and upstream health so `power_of_two` and `cache_aware` balance against real pressure.
