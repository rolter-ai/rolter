# Load balancing

Each route maps a public model name to one or more upstream targets and a strategy. Strategies implement `rolter_balancer::LoadBalancer`:

```rust
pub trait LoadBalancer: Send + Sync {
    fn name(&self) -> &'static str;
    fn pick(&self, ctx: &RouteContext, loads: &[u64]) -> Option<usize>;
    fn pick_eligible(
        &self,
        ctx: &RouteContext,
        loads: &[u64],
        eligible: &dyn Fn(usize) -> bool,
    ) -> Option<usize> {
        self.pick(ctx, loads)
    }
    fn reads_prompt(&self) -> bool {
        true
    }
    fn observe(&self, target: usize, ctx: &RouteContext) {}
}
```

`pick` returns an index into the route's targets; `observe` lets learning strategies (cache-aware) record what a target served. The gateway calls `pick_eligible`, passing which targets the attempt can use (not tried, not parked, healthy, breaker not open, allowed to the key). The default ignores that and calls `pick`, since the gateway skips an ineligible pick itself. A strategy that weighs targets against each other overrides it, so a target that takes no traffic cannot shape the comparison: `cache_aware`, `power_of_two`, `Pipeline` (and so `pipeline`, `lora_aware`, `predicted_latency`, `cheapest`, `fastest`) and `adaptive` do. `round_robin`, `weighted`, `random` and `consistent_hash` keep the default. Whatever the strategy, with nothing eligible an override behaves as `pick`. `RouteContext` carries an optional `session_key` (from `x-session-id`) and the request `prompt` used for affinity scoring. The gateway bounds `prompt` to the conversation's first 32 KiB and passes the whole prompt's size and identity alongside as `prompt_len` and `prompt_digest` ([caching.md](caching.md#1-kv-cache-affinity-load-balancing)).

`reads_prompt` says whether the strategy looks at `prompt`, `prompt_len` or `prompt_digest`. It defaults to `true`; `round_robin`, `weighted`, `random`, `power_of_two` and any pipeline whose scorers rank only on load, price, latency, weight, session, adapter or token ids answer `false`, and a pipeline answers from its scorers (`Scorer::reads_prompt`, also defaulting to `true`). The gateway builds the prompt text, length and digest (`prompt_affinity::affinity`: a walk over the conversation, a copy of up to 32 KiB and an xxh3 of all of it) only when the route's balancer or one of its variants' balancers reads it (`RouteEntry::reads_prompt`), and also skips the raw-body fallback then (#1929). `consistent_hash` reads it only for a request with no session, but that is per request, so it answers `true`. A new strategy that never reads the prompt should override `reads_prompt` to `false`.

## Strategies (v1)

- **round_robin** — sequential rotation; predictable, zero state.
- **random** — uniform random; good for simple homogeneous pools.
- **consistent_hash** — hash-ring keyed by `session_key` (falls back to a digest of the whole conversation: its text plus the identity of its images, tool calls and tool results, or to the raw body when the gateway could not read one); pins a session/user to a target for KV reuse, survives target changes with minimal reshuffle (100 vnodes per target). The ring is built over the route's target list by index, so the snapshot loads targets in a fixed order (`created_at`, then `id`) and a weight edit keeps each target's place (#2962).
- **power_of_two** — pick the less loaded of two random targets; needs a load snapshot. The two are sampled among the eligible targets: sampling over every target would let a dead replica, drained to load 0, win each draw it was in (#1942).
- **cache_aware** — approximate prefix affinity on the conversation text, with a load guard: a warm replica keeps its affinity until it runs more than 2 requests and 1.5× past the least-loaded usable one, then the request spreads (#1851). Unhealthy, parked and breaker-open targets are left out of the comparison through `LoadBalancer::pick_eligible`; see [caching.md](caching.md#1-kv-cache-affinity-load-balancing).
- **weighted** — smooth weighted round-robin honouring each target's `weight`.
- **pipeline** — composable **filter → weighted-score → argmax** selection: eligibility filtering drops ineligible targets, then a stack of `Scorer`s (session affinity + static weight + in-flight load + prefix-cache affinity) is combined as a weighted sum and the argmax wins (ties broken randomly). Session affinity pins repeat requests from the same `x-session-id` to their last-served target (TTL-bounded) for warm-cache reuse. The extension point every future cost/latency/KV-cache scorer plugs into.
- **precise_cache_aware** — consumes each target's vLLM ZMQ KV-event stream and scores the exact leading fraction of caller-supplied token blocks resident on that target. Missing token ids and stale, malformed, disconnected, or sequence-gapped streams stay neutral; least-load routing remains the fallback.
- **lmcache_aware** — polls each target's configured LMCache controller signal and prefers available caches with free capacity (`1 - occupancy`). Empty, saturated, failed, and stale controllers stay neutral and fall back to least load.
- **adaptive** — a weighted blend of observed latency, catalog cost and in-flight load, governed by the deployment-wide `[adaptive_routing]` policy. See below.
- **predicted_latency** — rank targets by what _this_ request is modelled to cost on each of them, from the queue it would join and its own prompt size, rather than by a per-target average. See below.
- **lora_aware** — LoRA-adapter affinity for a fleet serving many adapters over shared base weights: prefer a target that already holds the requested adapter resident, with prefix affinity and in-flight load behind it. See below.

### Which strategies read a target's weight

`build_with_stats` hands the weight slice to three strategies: `weighted`,
`pipeline` (its static-weight scorer) and `adaptive` (its fallback stack).
Every other strategy is built from the target count alone, so a weight on one
of their targets is stored and never consulted. The dashboard mirrors that
list as `usesWeights()` in `ui/src/lib/strategies.ts`: the Model Catalog, the
model sheet and the Routing Rules cards state a traffic share only for those
three, and say the weights are ignored everywhere else (#1979, #2133). A new
strategy that reads weights
has to be added there too, or the dashboard will tell operators its weights do
nothing.

### Eligibility cost

`pick_untried` hands the balancer an `eligible(i)` closure. Answering it per target would take the cooldown, health and breaker locks for each target the strategy weighs, about three per target. Instead the three registries each answer for the whole route in one lock acquisition (`Cooldowns::parked_mask`, `Health::unhealthy_mask`, `Breaker::open_mask`, as `TargetMask`s), read the first time the closure is called, so an attempt takes three acquisitions however many targets the route has, plus the checks on the one target it picks. A strategy that never calls the closure (`round_robin`) takes only the latter. The masks are read-only: `open_mask` lists an open breaker inside its window and a half-open one whose probe still holds the slot, and leaves a breaker whose window (or probe lease) has elapsed to `Breaker::allows`, which the chosen target is still asked, so exactly one probe is admitted ([health-and-readiness.md](health-and-readiness.md#the-circuit-breakers-half-open-probe-3248)).

Measured with `just bench-hot-path` (a 4 vCPU cloud VM, so read the ratios, not the nanoseconds), eligible pick with one target parked and one unhealthy, per-target answers against the snapshot:

| strategy       | 4 targets    | 24             | 64             | 128           |
| -------------- | ------------ | -------------- | -------------- | ------------- |
| `round_robin`  | 102 / 93 ns  | 112 / 112 ns   | 143 / 114 ns   | 105 / 116 ns  |
| `power_of_two` | 472 / 394 ns | 2.93 / 0.80 µs | 6.98 / 1.79 µs | 14.1 / 3.6 µs |
| `cache_aware`  | 8.1 / 7.5 µs | 84.6 / 85.6 µs | 282 / 275 µs   | 620 / 609 µs  |
| `pipeline`     | 8.4 / 7.9 µs | 84.4 / 88.3 µs | 295 / 288 µs   | 597 / 587 µs  |

Only `power_of_two` is dominated by the eligibility answers, and the snapshot makes its cost grow slowly with the pool. `cache_aware` and the pipeline spend their time matching prefixes and scoring (the bench gives every target the same prompt), so the per-target locks were within noise there; the snapshot is for the constant-locks bound, not a measured win for them. Each pick's allocations are unchanged: none for `round_robin`, `power_of_two` and `cache_aware` up to 64 targets. The pipeline stacks (`pipeline`, `lora_aware`, `cheapest`, `fastest`, `predicted_latency`) allocated 6 to 12 vectors per pick (the candidate list, the running totals and one score vector per scorer) until #3249: a `Scorer` now writes into caller-provided scratch (`Scorer::score_into`), and `Pipeline::select` holds that scratch on the stack for a route of up to `STACK_TARGETS` (64) targets, so the pick itself allocates nothing there, and a wider route allocates its three buffers once per pick. A scorer backed by a live source (`fastest`, `predicted_latency`, the KV-cache and LMCache scorers) still pays for the one snapshot its source returns. `tests/hot_path_allocations.rs` asserts zero for `pipeline`, `lora_aware` and `cheapest` at 4 and 64 targets.

The same rule holds on the two paths `pick_untried` does not cover (#3249). The weighted-variant forward loop builds each variant's registry key once per request, and reads each variant's `Blocked` masks at most once per attempt (lazily, only for a variant a candidate is asked about), then asks `Breaker::allows` only of the candidate it is about to use; it used to build the key and take the three locks for every candidate it walked past. A multi-key provider's key pick reads its pool's parks with one `Cooldowns::parked_mask` (`handlers::parked_keys`) instead of an `is_parked` per key, on the chat, raw (audio), variant and realtime paths.

## Balancer lifetime, and provider groups

**A balancer belongs to the pool, not to the request.** Most strategies carry
selection state — `round_robin`'s counter, `weighted`'s running weights,
`cache_aware`'s trie, `adaptive`'s samples — and that state is exactly what
makes them balance. A balancer built per request is not a balancer: a fresh
`RoundRobin` always returns index `0`, and a fresh `WeightedRoundRobin` always
returns the heaviest target, so every request lands on the same place.

Configured routes get this for free: `Snapshot::build` constructs one balancer
per route and hands out references to it for the snapshot's whole life, and a
config reload builds a new snapshot (and so a new balancer, deliberately — a new
snapshot may have different targets, and a carried-over index would name a
different provider).

A **provider group** address (`group-slug/model`, ADR-0017 addendum) is the case
that is not free. A group has no configured route: `Snapshot::resolve_pinned`
builds a _synthetic_ one per request, because the model each member forwards
depends on what the caller asked for. Only the target list is per request,
though — the strategy and the member weights are fixed for the life of the
snapshot, so the group's balancer is built once in `Snapshot::build`, stored
with the group in `Snapshot::groups_by_slug` (keyed by `(org, slug)`, #1857),
and shared by every synthetic entry built for that group. Before #1655 it was built inside `resolve_pinned`, and a
whole fleet's traffic went to whichever member was listed first, whatever the
group's strategy and weights said.

Costs for a group's balancer come from each member's declared `upstream_model`,
since the requested model is not known at build time and a `group-slug/model`
address is never itself a priced model. A group whose members are all
passthrough is therefore cost-neutral, which is the right answer for the
homogeneous fleet a group exists to describe.

A provider _slug_ address pins a single provider, so its pool has one target and
no selection state to keep; that balancer is still built per request.

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

#### Warm-up

The latency condition cannot be satisfied by waiting (#1645). The fallback stack is free to send every pick to one target — a configured weight, session affinity or prefix affinity all do this legitimately — and the peer it skips then never earns the latency sample the evidence check is counting. Before the fix a route on `adaptive` over such a pair sat at `engaged: false` with `fallback: 50` of 50 picks and `samples: 0` on the unsampled target indefinitely, which made `adaptive` permanently equal to the strategy it replaced.

So while the evidence is thin, a disengaged route steers picks to the targets that have no latency sample yet, at most eight per target. Those picks are counted as `exploration` in the decision split, since they buy evidence rather than the best target. The budget is what bounds the cost: a target that never answers absorbs at most eight picks in total and is then left to the fallback stack, rather than swallowing a permanent share of the route. The budget is eight rather than one because a latency sample lands only when a request finishes, so a smaller one would be spent on picks still in flight and the warm-up would give up on a healthy target under concurrency. The kill switch still outranks all of this: a policy with `enabled = false` probes nothing.

The policy is also owned by the control plane: `GET`/`PUT /api/v1/adaptive-routing-policy` (superadmin only) persists it, audits the change as `adaptive_routing_policy.update`, and returns the `affected_routes` the change reaches. It travels to the data plane in the normal snapshot, so a change applies without a restart. The API refuses an all-zero blend outright — stopping adaptive routing is what `enabled = false` is for.

Once engaged, an `exploration_ratio` share of picks is made uniformly at random so a target the blend has learned to avoid keeps producing fresh latency samples instead of going dark. Operator input is clamped on the way in: negative weights become zero and exploration never exceeds half the traffic.

### Telemetry

The scores the blend ranks on live in the balancer, which lives in the gateway process, so the control plane cannot read them directly. Each gateway therefore **pushes** a sample every 15s to `POST /internal/adaptive-telemetry` on the control plane — the same internal token and the same `x-rolter-node-id` identity as the snapshot poll, so one node is one row here and in the cluster inventory (#543). Nothing is written on the request path: the sample is taken by a background task, and a pick costs two relaxed atomics for per-target attribution.

Per route and target the sample carries the blended score, its latency/cost/load components, the raw signals behind them (smoothed latency in ms, catalog price, in-flight count), how many picks that target has served and how long ago the last one was — plus the decision split, and the _sanitized policy that node actually runs_, which can lag the stored policy until the node converges.

The control plane keeps the newest sample per `(node, model)` in `adaptive_routing_telemetry` and serves `GET /api/v1/adaptive-routing-telemetry` (superadmin only), grouped by route with one entry per reporting node. Samples older than 60s are excluded as no longer current, and rows are pruned after an hour so a scaled-down node leaves the scoreboard.

This is deliberately **current state, not history**: one row per node and route, overwritten on every report, so the table is the size of the fleet rather than of the traffic. A time series belongs in the request log, and would be a separate endpoint rather than a change to this one. Prometheus deployments get the same per-target scores without the dashboard from `rolter_adaptive_routing_target_score{model,target}`.

## LoRA-aware routing

For a fleet serving many LoRA adapters off shared base weights, routing a
request to a node that already holds the adapter is the same class of win as
prefix-cache affinity (#853, borrowed from llm-d). `lora_aware` composes with
the existing scorers rather than replacing them:

| Scorer            | Weight |
| ----------------- | ------ |
| adapter residency | 1.0    |
| prefix affinity   | 0.5    |
| in-flight load    | 0.25   |

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
deliberately _pins_ rather than spreads, so treating a route's one model as an
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

| Scorer            | Weight |
| ----------------- | ------ |
| predicted latency | 1.0    |
| in-flight load    | 0.25   |

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
no prediction, and the scorer reads that as _unknown_, not as _slow_. A route
that switches to `predicted_latency` therefore behaves exactly like the
least-load pipeline until the models have evidence, so the switch itself moves
no traffic.

**Only successful requests teach.** The same gate the latency EWMA uses: a fast
failure cannot train a target into looking cheap. Coefficients are clamped, so a
client that held a stream open for a week produces one bad sample rather than a
permanently poisoned model.

**Queue depth is read before the increment**, so the model learns the queue a
request _joined_, not the one it created.

**Models live in the load tracker, not the routing snapshot**, for the same
reason the latency EWMA does: a config reload must not throw away what they
learned, or every reload would send the route back to cold behaviour. A route
that gains a target keeps its existing models and the new target reads as
unpredicted until the next process start — the conservative direction, since a
target with no evidence should not be ranked.

## Choosing a strategy

| Use case                                               | Strategy                 |
| ------------------------------------------------------ | ------------------------ |
| Homogeneous pool, stateless                            | `round_robin` / `random` |
| Variable request durations                             | `power_of_two`           |
| Multi-turn chat, sticky session                        | `consistent_hash`        |
| Shared system prompts / few-shot / RAG                 | `cache_aware`            |
| Blend cache + load + weight signals                    | `pipeline`               |
| vLLM fleet with KV event publishing                    | `precise_cache_aware`    |
| LMCache fleet with occupancy controller                | `lmcache_aware`          |
| Mixed-price providers, minimize spend                  | `cheapest`               |
| Heterogeneous pool, minimize latency                   | `fastest`                |
| Mixed price _and_ latency, let the gateway tune        | `adaptive`               |
| Many LoRA adapters over shared base weights            | `lora_aware`             |
| Heterogeneous pool, variable prompt sizes, deep queues | `predicted_latency`      |

Both external strategies perform network I/O only in background tasks. The request hot path reads bounded in-process state and atomics.

## Changing a route's strategy and targets

A route's strategy and its targets and weights change after creation through
`PUT /api/v1/routes/{id}` (#2235); the user-facing contract is the
[Route updates](../../user-docs/api/routes.mdx) API page and the store side is
in [Data model](data-model.md). Every field is optional and an omitted one keeps
its value, so the old `{"enabled": false}` body still only toggles the route.
A target list is the route's whole list, matched to the stored targets by
provider and upstream model: a weight edit keeps the target's id, and a changed
provider or upstream model is a new target.

No strategy is refused for the targets it is given. `build_with_stats` cannot
fail, and each strategy whose signal is missing degrades rather than erroring
(`precise_cache_aware` and `lmcache_aware` to least-load without a telemetry
source, `fastest` and `predicted_latency` to least-load while cold), so the
update checks the strategy against the same `STRATEGIES` allowlist creation
uses and the targets against the single-target checks, and nothing more. A
reload rebuilds every route's balancer anyway, so a strategy change carries no
selection state across.

## Fallback targets that take over failure classes

A route target may carry `takes_over` (#1584), a set of `rolter_core::failover::FailureClass`es: `throttled`, `timeout`, `transport`, `server_error`, `refusal` and `refusal:<category>`. A target with a set is **restricted**: it is never dialled first, never the balancer's pick for ordinary traffic, and tried only as the successor of an attempt whose failure one of its classes covers. A target with no set is an ordinary member of the pool, so a route that sets none behaves exactly as before. The user-facing contract is [Fallback targets](../../user-docs/reliability/fallback-targets.mdx).

**Vocabulary.** The classes are rolter's own words for the failures the forward loop already tells apart when `[retry]` decides whether to retry: `Failure::from_status` maps `429` to `throttled`, `408`/`504` to `timeout` and any other `5xx` to `server_error`; `Failure::from_transport_error` maps a forward error to `timeout` when the forwarder says it timed out and to `transport` otherwise. `refusal` is new: `Failure::refusal` reads a `451`, or a `400`/`403`/`422` whose `error.code` or `error.type` contains a content-policy marker (`content_policy`, `content_filter`, `content_management`, `responsible_ai`, `safety`, `moderation`) or that carries Azure's `content_filter_result`. Its categories are the marked code and each filter category marked `filtered: true`, lowercase. `FailureClass::matches` compares kinds and, for a narrowed class, requires the category among the failure's.

**Selection.** `rolter_core::failover::may_dial(classes, failure)` is the one rule: with no failure (the first dial) only an unrestricted target; after a failure an unrestricted target unless the failure is a refusal (a refusal is the same request to the same kind of model, so it is never retried onto an ordinary target), and a restricted one only when a class matches. In the classic pool `admits` folds it into the key's provider check, so it reaches every place `pick_untried` consults: the `usable` closure handed to `LoadBalancer::pick_eligible` (#3255), the `skip` check on the balancer's pick, the declared-order fallback and the fail-open `first_untried_allowed`, which therefore never fails open onto a restricted target. `has_untried_target` takes the same failure, so the backoff check and the pick agree. The variant chain does the same through `candidate_admits`, and `variant_candidates` keeps a restricted target out of each variant balancer's lead. Realtime candidates are ordered before any attempt fails, so `realtime_candidates` passes no failure and leaves restricted variant targets out: a realtime session never uses one (#3322).

**The ladder.** Each loop (`proxy`, `proxy_multipart`, `forward_variants`) keeps `last_failure`, set wherever an attempt is superseded: the key-level `429`/`401` and the retryable statuses through `Failure::from_status`, a forward error through `Failure::from_transport_error`. A refusal is not retryable, so a `4xx` used to end the loop with its response. Now, while a retry is left, a `400`/`403`/`422`/`451` on a route with a target that takes refusals over (`ModelRoute::takes_over_refusals`) is read by `failover::peek_refusal`: up to 16 KiB within 1 s, classified, and the response rebuilt (status, version, headers, and the prefix chained ahead of whatever the upstream still sends) so an answer that is not taken over reaches the caller byte for byte. A refusal some untried target takes over supersedes the attempt like a retryable status, except that it neither parks the target nor feeds its breaker (a refusal says nothing about health) and there is no backoff. Routes with no such target never read the body, so the hot path of every existing route is unchanged. The refusal takeover spends the retry budget like any failover, so `limits.retries = 0` turns it off.

**The recorded reason.** At every pick the loop sets `fallback_reason` to `last_failure`'s token (`Failure::token`: the kind, or `refusal:<first category>`), empty on the first dial, and the request row carries it (`RequestLog::fallback_reason`, serialized only when non-empty, `clickhouse/023_fallback_reason.sql`, `LowCardinality(String)`). The control plane's invocation list selects it through `OPTIONAL_COLUMNS`, so a ClickHouse without `023` reads it as `''`.

**Failing closed.** A route whose every target is restricted could never dial anything. `ModelRoute::takes_over_problems` (also a class listed twice on one target) feeds `GatewayConfig::validate`, so the gateway refuses such a file at startup; `rolter check` reports it as an error, `rolter-seed --import` refuses the file before writing a row, and the routes API refuses a list, a single added target or a delete that would leave a route that way, each naming `all_targets_restricted`. The snapshot assembly cannot refuse (one bad route would fail validation of the whole snapshot), so `load_routes` drops a target whose stored class does not parse rather than serving it as an ordinary target, and leaves out, with a warning, a route whose remaining targets are all restricted, as it does a route whose settings do not parse (neither is in `load_problems` yet, #3321).

**Storage.** `route_targets.takes_over` is a `text[]` (`0129_route_target_takes_over.sql`) of canonical class tokens, `'{}'` by default; `route_targets` already carries the `0003` `bump_config_version()` trigger. `RouteRepo::update` matches targets by provider and upstream model as before and rewrites a match whose weight or set changed, reporting the previous set in the audit detail. The seed import applies an edited `takes_over` in place like an edited weight. The routing-rules screen does not edit it yet (#3319).

Tests: `rolter_core::failover` unit tests (vocabulary, classification, `may_dial`, the all-restricted rule), `handlers::tests::a_restricted_target_*` and `a_refusal_moves_only_*`, `failover::tests` in the gateway (the rebuilt body), `crates/rolter-gateway/tests/conditional_failover.rs` (end to end, including the recorded reason), the store snapshot test `takes_over_reaches_the_snapshot_and_fails_closed`, the CRUD test `a_target_takes_over_failure_classes_and_a_route_is_never_left_only_restricted` and the seed test `the_import_applies_takes_over_and_refuses_an_all_restricted_route`.

## Selecting a strategy from the dashboard

The route and provider-group editors offer every strategy in this table except `adaptive`, which is governed by the deployment-wide `[adaptive_routing]` policy and has its own screen — a per-route dropdown would misrepresent how it is controlled. `precise_cache_aware` and `lmcache_aware` are offered with a hint that they need a telemetry source on the target providers, since without one they fall back to least-load silently rather than failing.

The hint also says where that source is set. A provider carries two optional settings, `kv_events` (a vLLM kv-event stream, `precise_cache_aware`) and `lmcache` (an LMCache controller, `lmcache_aware`), in `rolter.toml` as `[providers.kv_events]` and `[providers.lmcache]` and on a database provider as the `kv_events` / `lmcache` jsonb columns (migration `0087`) behind the CRUD API (#2236). Until then the store loaded every database provider with both set to `None`, so a route or group whose targets were all dashboard providers always ran on the least-load fallback; a provider with neither setting still does, silently rather than failing. The hint's "only on a file provider" wording (#2137) is stale once the provider sheet offers the fields, and goes with that change. When the deployment has a documentation host, the hint links the user-facing cache-aware routing page through `DocsLink`; an air-gapped deployment with none gets the hint without a link.

The two strategies stay selectable: a route or group can target providers from `rolter.toml` that do carry a source, and an operator who wired one up must be able to choose them.

A picker always renders the value the route or group already holds, even one it would not otherwise offer — including a strategy set from `rolter.toml` or the API, or one added to the backend allowlist ahead of the dashboard. A native `<select>` whose value matches no option displays the _first_ option instead, so before #897 a group balanced by `adaptive` read as `round_robin`. Whatever the menu chooses to offer, editing must never rewrite a strategy the operator did not touch.
