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
- **adaptive** — a weighted blend of observed latency, catalog cost and in-flight load, governed by the deployment-wide `[adaptive_routing]` policy. See below.
- **predicted_latency** — rank targets by what *this* request is modelled to cost on each of them, from the queue it would join and its own prompt size, rather than by a per-target average. See below.
- **lora_aware** — LoRA-adapter affinity for a fleet serving many adapters over shared base weights: prefer a target that already holds the requested adapter resident, with prefix affinity and in-flight load behind it. See below.

## Adaptive routing

`adaptive` is the only strategy whose behavior is owned by a global policy rather than the route:

```toml
[adaptive_routing]
enabled = false           # kill switch; the whole feature is off by default
latency_weight = 1.0
cost_weight = 0.5
load_weight = 0.25
exploration_ratio = 0.05  # clamped to [0, 0.5]
min_samples = 50
```

Three conditions must all hold before the blend routes a single request: the kill switch is on, at least one weight is non-zero, and the route has both served `min_samples` requests and gathered latency samples for at least two targets. Until then — and immediately again if the policy is switched off — every pick goes to the same `pipeline` stack the route would have used otherwise, so moving a route to `adaptive` shifts no traffic on its own. The fallback stack keeps learning while the blend is engaged, so disengaging lands on a warm session/prefix cache.

The policy is also owned by the control plane: `GET`/`PUT /api/v1/adaptive-routing-policy` (superadmin only) persists it, audits the change as `adaptive_routing_policy.update`, and returns the `affected_routes` the change reaches. It travels to the data plane in the normal snapshot, so a change applies without a restart. The API refuses an all-zero blend outright — stopping adaptive routing is what `enabled = false` is for.

Once engaged, an `exploration_ratio` share of picks is made uniformly at random so a target the blend has learned to avoid keeps producing fresh latency samples instead of going dark. Operator input is clamped on the way in: negative weights become zero and exploration never exceeds half the traffic.

### Telemetry

The scores the blend ranks on live in the balancer, which lives in the gateway process, so the control plane cannot read them directly. Each gateway therefore **pushes** a sample every 15s to `POST /internal/adaptive-telemetry` on the control plane — the same internal token and the same `x-rolter-node-id` identity as the snapshot poll, so one node is one row here and in the cluster inventory (#543). Nothing is written on the request path: the sample is taken by a background task, and a pick costs two relaxed atomics for per-target attribution.

Per route and target the sample carries the blended score, its latency/cost/load components, the raw signals behind them (smoothed latency in ms, catalog price, in-flight count), how many picks that target has served and how long ago the last one was — plus the decision split, and the *sanitized policy that node actually runs*, which can lag the stored policy until the node converges.

The control plane keeps the newest sample per `(node, model)` in `adaptive_routing_telemetry` and serves `GET /api/v1/adaptive-routing-telemetry` (superadmin only), grouped by route with one entry per reporting node. Samples older than 60s are excluded as no longer current, and rows are pruned after an hour so a scaled-down node leaves the scoreboard.

This is deliberately **current state, not history**: one row per node and route, overwritten on every report, so the table is the size of the fleet rather than of the traffic. A time series belongs in the request log, and would be a separate endpoint rather than a change to this one. Prometheus deployments get the same per-target scores without the dashboard from `rolter_adaptive_routing_target_score{model,target}`.

## LoRA-aware routing

For a fleet serving many LoRA adapters off shared base weights, routing a
request to a node that already holds the adapter is the same class of win as
prefix-cache affinity (#853, borrowed from llm-d). `lora_aware` composes with
the existing scorers rather than replacing them:

| Scorer | Weight |
| --- | --- |
| adapter residency | 1.0 |
| prefix affinity | 0.5 |
| in-flight load | 0.25 |

Adapter residency outranks prefix affinity because the costs are not
comparable: missing a warm prefix recomputes some tokens, while missing a
resident adapter can force the engine to load adapter weights before it decodes
anything at all. Load stays in the stack so a fleet where every target holds the
adapter still balances instead of pinning.

**Residency is learned from traffic, not declared.** rolter cannot see which
adapters an engine currently holds, and a static declaration would go stale the
moment the engine evicted one. Each target keeps a bounded LRU set of the
adapters it has recently served, sized to mirror vLLM's `--max-loras`, so it
tracks what the engine plausibly still holds rather than everything it has ever
served. An unbounded "has ever served" set would be worse than no scoring at
all, because it steers confidently to a target that went cold long ago.

**Adapter identity is the requested model**, which is how vLLM addresses
adapters over shared base weights. The gateway only sets it when the request
addresses something other than the route's own model — that is, a passthrough
provider-group route (ADR-0017). On a single-model route the two are equal, no
adapter is set, and the scorer is inert. This matters: adapter affinity
deliberately *pins* rather than spreads, so treating a route's one model as an
adapter would pin the entire route to whichever target happened to serve first.

When no candidate holds the adapter — a cold adapter, or a request with none —
every candidate scores `0.0`. That is neutral rather than a penalty: an equal
contribution cannot move the argmax, so the rest of the pipeline decides and
this scorer stays silent instead of guessing.

## Predicted-latency scheduling

`fastest` ranks on a per-target latency EWMA. That is the right signal when
every request is the same size, and the wrong one otherwise: a target whose
average is high because it happened to serve the long prompts looks slow even
when it is the emptiest box in the fleet, and a target with a deep queue looks
fast right up until the queue is what you join.

`predicted_latency` models the cost instead of averaging it (#853, borrowed
from llm-d). Per target:

```text
latency_ms ≈ w0 + w1 · queue_depth + w2 · prompt_ktokens
```

`w0` is fixed overhead, `w1` is what one queued request ahead of you costs, and
`w2` is prefill cost per thousand prompt tokens. The three coefficients are
learned online from completed requests with normalized least mean squares — one
multiply-add per feature per sample, no matrix, no allocation, no periodic
refit.

| Scorer | Weight |
| --- | --- |
| predicted latency | 1.0 |
| in-flight load | 0.25 |

Load stays in the stack for two reasons: it carries the route while the models
are cold, and once they are warm it is the tiebreaker between targets the model
rates equally — the common case on a homogeneous fleet, where the honest answer
is "either, take the emptier".

**Three features, not more.** A gateway sees queue depth and prompt size. It
does not see batch composition, KV-cache pressure, or where the engine is in
its scheduling loop, and a model with parameters it cannot observe fits noise.
The interesting error is between "the queue is deep" and "the prompt is long",
and a linear model separates those. That is the honest ceiling for this vantage
point.

**A cold target predicts nothing.** Below 8 completed requests a target returns
no prediction, and the scorer reads that as *unknown*, not as *slow*. A route
that switches to `predicted_latency` therefore behaves exactly like the
least-load pipeline until the models have evidence, so the switch itself moves
no traffic.

**Only successful requests teach.** The same gate the latency EWMA uses: a fast
failure cannot train a target into looking cheap. Coefficients are clamped, so a
client that held a stream open for a week produces one bad sample rather than a
permanently poisoned model.

**Queue depth is read before the increment**, so the model learns the queue a
request *joined*, not the one it created.

**Models live in the load tracker, not the routing snapshot**, for the same
reason the latency EWMA does: a config reload must not throw away what they
learned, or every reload would send the route back to cold behaviour. A route
that gains a target keeps its existing models and the new target reads as
unpredicted until the next process start — the conservative direction, since a
target with no evidence should not be ranked.

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
| Mixed price *and* latency, let the gateway tune | `adaptive` |
| Many LoRA adapters over shared base weights | `lora_aware` |
| Heterogeneous pool, variable prompt sizes, deep queues | `predicted_latency` |

Both external strategies perform network I/O only in background tasks. The request hot path reads bounded in-process state and atomics.

## Selecting a strategy from the dashboard

The route and provider-group editors offer every strategy in this table except `adaptive`, which is governed by the deployment-wide `[adaptive_routing]` policy and has its own screen — a per-route dropdown would misrepresent how it is controlled. `precise_cache_aware` and `lmcache_aware` are offered with a hint that they need a telemetry source on the target providers, since without one they fall back to least-load silently rather than failing.

A picker always renders the value the route or group already holds, even one it would not otherwise offer — including a strategy set from `rolter.toml` or the API, or one added to the backend allowlist ahead of the dashboard. A native `<select>` whose value matches no option displays the *first* option instead, so before #897 a group balanced by `adaptive` read as `round_robin`. Whatever the menu chooses to offer, editing must never rewrite a strategy the operator did not touch.
