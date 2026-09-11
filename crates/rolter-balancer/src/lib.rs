//! Pluggable load-balancing strategies for rolter routes.
//!
//! Every strategy implements [`LoadBalancer`]. The [`build`] factory turns a
//! [`BalancingStrategy`] from the config into a boxed balancer. New strategies
//! (precise KV-cache aware, lmcache aware, latency based, ...) only need to
//! implement the trait and be wired into [`build`].
//!
//! **Internal crate.** It is published only so `cargo install rolter` can
//! resolve, and it offers no stable Rust API: any public item here may change
//! or disappear in any release, including a patch release. Build against
//! rolter's HTTP surfaces instead — see
//! [ADR-0032](https://github.com/rolter-ai/rolter/blob/master/docs/adr/2026-09-09-one-point-oh-compatibility-guarantees.md).

use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

use ahash::RandomState;
use parking_lot::Mutex;
use rand::RngExt;
use rolter_core::BalancingStrategy;

pub mod adaptive;
pub mod complexity;
pub mod predictor;
pub mod scorer;
pub mod trie;
use trie::Trie;

/// Per-request context a balancer may use to make a decision.
#[derive(Debug, Default, Clone)]
pub struct RouteContext<'a> {
    /// stable session/user identifier extracted from headers or body
    pub session_key: Option<&'a str>,
    /// request prompt used for prefix/cache affinity scoring
    pub prompt: Option<&'a str>,
    /// token ids used for exact vLLM block-prefix matching when supplied
    pub token_ids: Option<&'a [u32]>,
    /// LoRA adapter this request needs, when the route serves adapters over a
    /// shared base model (#853). Used to steer the request to a target that
    /// already holds the adapter resident, the same class of win as prefix
    /// affinity; `None` on a route with no adapters, which leaves adapter
    /// scoring inert.
    pub adapter: Option<&'a str>,
}

/// A strategy that selects one target index for a request.
pub trait LoadBalancer: Send + Sync {
    /// stable identifier of the strategy
    fn name(&self) -> &'static str;

    /// Pick a target index given the request context and an optional per-target
    /// load snapshot (`loads[i]` is the in-flight count for target `i`). When no
    /// load is known the slice may be empty.
    fn pick(&self, ctx: &RouteContext, loads: &[u64]) -> Option<usize>;

    /// Record that `target` served the given context. Strategies that learn from
    /// traffic (cache aware) override this; others ignore it.
    fn observe(&self, _target: usize, _ctx: &RouteContext) {}

    /// Decision counters for strategies that choose *how* to pick, not just
    /// what to pick. Only [`adaptive::Adaptive`] reports today; every other
    /// strategy makes one kind of decision and returns `None`.
    fn decisions(&self) -> Option<DecisionCounts> {
        None
    }

    /// Read-only view of the per-target signals the strategy currently ranks
    /// on, for the dashboard and the control plane (#751). `loads[i]` is the
    /// in-flight count for target `i`, exactly as [`LoadBalancer::pick`] takes
    /// it; an empty slice means unknown. Only [`adaptive::Adaptive`] reports
    /// today. Never called on the request path — it recomputes every scorer,
    /// so callers must sample it on a timer instead.
    fn telemetry(&self, _loads: &[u64]) -> Option<AdaptiveTelemetry> {
        None
    }
}

/// How a route's picks were split between the strategy's modes, cumulative
/// since the balancer was built (a config reload rebuilds it, so a Prometheus
/// counter reset lines up with a config-version bump).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DecisionCounts {
    /// picks made by the adaptive blend
    pub blend: u64,
    /// picks spent exploring so starved targets keep producing samples
    pub exploration: u64,
    /// picks served by the deterministic fallback stack
    pub fallback: u64,
    /// whether the blend was engaged at the last pick
    pub engaged: bool,
}

/// What the adaptive blend currently knows about one target, sampled off the
/// live balancer. Scores are the `[0, 1]` signal values the blend ranks on;
/// the raw observations they were derived from travel alongside them so a
/// reader can tell "slowest of three" from "slow".
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TargetTelemetry {
    /// index into the route's targets, which is also the order they are listed
    pub target: usize,
    /// blended score: the weighted sum of the components below
    pub score: f32,
    /// observed-latency component, `0.0` when the signal carries no weight
    pub latency_score: f32,
    /// catalog-cost component
    pub cost_score: f32,
    /// in-flight-load component
    pub load_score: f32,
    /// smoothed observed latency in milliseconds; `0.0` = never sampled
    pub latency_ms: f64,
    /// catalog price in the deployment's base currency; `<= 0` = unknown
    pub cost_per_mtok: f64,
    /// requests in flight against this target at sample time
    pub in_flight: u64,
    /// picks this target has served since the balancer was built
    pub samples: u64,
    /// milliseconds since this target last served a pick; `None` = never
    pub last_sample_age_ms: Option<u64>,
}

/// A route's adaptive-routing state at one instant: the effective policy, how
/// its picks were split, and the per-target signals behind them.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdaptiveTelemetry {
    /// whether the blend is routing right now, rather than the fallback stack
    pub engaged: bool,
    /// picks the route has observed since the balancer was built
    pub observed: u64,
    /// pick split by mode
    pub decisions: DecisionCounts,
    /// the *sanitized* policy this balancer applies, which is what the node
    /// actually runs — it can lag the stored policy until the node converges
    pub policy: rolter_core::AdaptiveRoutingConfig,
    /// per-target signals, route-order aligned
    pub targets: Vec<TargetTelemetry>,
}

/// Build-time, per-target signals for strategies that rank on more than
/// weights. Slices are index-aligned with the route targets; a missing or
/// non-positive entry means "unknown" and the strategy stays neutral for that
/// target.
#[derive(Default, Clone)]
pub struct TargetStats {
    /// catalog price per target in any consistent per-token rate (only the
    /// relative order matters); `<= 0` = no known price
    pub cost_per_mtok: Vec<f64>,
    /// live per-target latency handle for the `fastest` strategy; read at pick
    /// time, so the balancer follows shifting latency without a rebuild
    pub latency: Option<std::sync::Arc<dyn scorer::LatencySource>>,
    /// live exact KV residency source for vLLM event-aware routing
    pub kv_cache: Option<std::sync::Arc<dyn scorer::KvCacheSource>>,
    /// live LMCache occupancy/availability source
    pub lmcache: Option<std::sync::Arc<dyn scorer::LmCacheSource>>,
    /// live per-request latency model for the `predicted_latency` strategy;
    /// read at pick time and taught by the completion path
    pub predictor: Option<std::sync::Arc<dyn scorer::LatencyPredictionSource>>,
    /// deployment-wide adaptive-routing policy, read once at build time by the
    /// `adaptive` strategy and ignored by every other one
    pub adaptive: rolter_core::AdaptiveRoutingConfig,
}

impl std::fmt::Debug for TargetStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TargetStats")
            .field("cost_per_mtok", &self.cost_per_mtok)
            .field("latency", &self.latency.as_ref().map(|_| "<live>"))
            .field("kv_cache", &self.kv_cache.as_ref().map(|_| "<live>"))
            .field("lmcache", &self.lmcache.as_ref().map(|_| "<live>"))
            .field("predictor", &self.predictor.as_ref().map(|_| "<live>"))
            .field("adaptive", &self.adaptive)
            .finish()
    }
}

/// Build a boxed [`LoadBalancer`] from a configured strategy and the route's
/// per-target `weights` (index-aligned with the route targets). Strategies that
/// ignore weights only use `weights.len()` as the target count. Strategies that
/// need no build-time stats get empty [`TargetStats`]; use [`build_with_stats`]
/// for cost-aware routing.
pub fn build(strategy: BalancingStrategy, weights: &[u32]) -> Box<dyn LoadBalancer> {
    build_with_stats(strategy, weights, &TargetStats::default())
}

/// [`build`] with per-target [`TargetStats`] for strategies that rank on
/// build-time signals (`cheapest`). Stats slices shorter than the route are
/// treated as unknown for the missing tail.
pub fn build_with_stats(
    strategy: BalancingStrategy,
    weights: &[u32],
    stats: &TargetStats,
) -> Box<dyn LoadBalancer> {
    let n = weights.len();
    match strategy {
        BalancingStrategy::RoundRobin => Box::new(RoundRobin::new(n)),
        BalancingStrategy::Random => Box::new(Random::new(n)),
        BalancingStrategy::PowerOfTwo => Box::new(PowerOfTwo::new(n)),
        BalancingStrategy::ConsistentHash => Box::new(ConsistentHash::new(n)),
        BalancingStrategy::CacheAware => Box::new(CacheAware::new(n, 0.5)),
        BalancingStrategy::Weighted => Box::new(WeightedRoundRobin::new(weights)),
        BalancingStrategy::Pipeline => Box::new(scorer::Pipeline::default_stack(weights)),
        BalancingStrategy::LoraAware => Box::new(scorer::Pipeline::lora_stack(n)),
        BalancingStrategy::PredictedLatency => match &stats.predictor {
            Some(source) => Box::new(scorer::Pipeline::predicted_latency_stack(n, source.clone())),
            // no predictor wired: degrade to least-load, which is what the
            // stack falls back to while the models are cold anyway
            None => Box::new(
                scorer::Pipeline::new(n)
                    .named("predicted_latency")
                    .with(Box::new(scorer::LeastLoadScorer::new(n)), 1.0),
            ),
        },
        BalancingStrategy::Cheapest => {
            // pad unknown costs so the scorer stays index-aligned with targets
            let mut costs = stats.cost_per_mtok.clone();
            costs.resize(n, 0.0);
            Box::new(scorer::Pipeline::cheapest_stack(&costs))
        }
        BalancingStrategy::Fastest => match &stats.latency {
            Some(source) => Box::new(scorer::Pipeline::fastest_stack(n, source.clone())),
            // no latency handle wired (e.g. plain build()): degrade to a
            // least-load pipeline, the closest latency proxy available
            None => Box::new(
                scorer::Pipeline::new(n)
                    .named("fastest")
                    .with(Box::new(scorer::LeastLoadScorer::new(n)), 1.0),
            ),
        },
        BalancingStrategy::PreciseCacheAware => match &stats.kv_cache {
            Some(source) => Box::new(
                scorer::Pipeline::new(n)
                    .named("precise_cache_aware")
                    .with(Box::new(scorer::PreciseKvScorer::new(source.clone())), 1.0)
                    .with(Box::new(scorer::LeastLoadScorer::new(n)), 0.25),
            ),
            None => Box::new(
                scorer::Pipeline::new(n)
                    .named("precise_cache_aware")
                    .with(Box::new(scorer::LeastLoadScorer::new(n)), 1.0),
            ),
        },
        BalancingStrategy::Adaptive => Box::new(adaptive::Adaptive::new(
            weights,
            &stats.cost_per_mtok,
            stats.latency.clone(),
            &stats.adaptive,
        )),
        BalancingStrategy::LmcacheAware => match &stats.lmcache {
            Some(source) => Box::new(
                scorer::Pipeline::new(n)
                    .named("lmcache_aware")
                    .with(Box::new(scorer::LmCacheScorer::new(source.clone())), 1.0)
                    .with(Box::new(scorer::LeastLoadScorer::new(n)), 0.25),
            ),
            None => Box::new(
                scorer::Pipeline::new(n)
                    .named("lmcache_aware")
                    .with(Box::new(scorer::LeastLoadScorer::new(n)), 1.0),
            ),
        },
    }
}

/// Sequential rotation across targets.
pub struct RoundRobin {
    n: usize,
    next: AtomicUsize,
}

impl RoundRobin {
    pub fn new(n: usize) -> Self {
        Self {
            n,
            next: AtomicUsize::new(0),
        }
    }
}

impl LoadBalancer for RoundRobin {
    fn name(&self) -> &'static str {
        "round_robin"
    }
    fn pick(&self, _ctx: &RouteContext, _loads: &[u64]) -> Option<usize> {
        if self.n == 0 {
            return None;
        }
        Some(self.next.fetch_add(1, Relaxed) % self.n)
    }
}

/// Smooth weighted round-robin (the nginx algorithm). Distributes picks in
/// proportion to each target's weight while keeping the sequence evenly
/// interleaved rather than bursty. Falls back to plain rotation when all weights
/// are equal.
pub struct WeightedRoundRobin {
    /// static configured weight per target (clamped to at least 1)
    weights: Vec<i64>,
    /// mutable running weights advanced on each pick
    current: Mutex<Vec<i64>>,
    total: i64,
}

impl WeightedRoundRobin {
    pub fn new(weights: &[u32]) -> Self {
        let weights: Vec<i64> = weights.iter().map(|&w| (w as i64).max(1)).collect();
        let total = weights.iter().sum();
        let current = vec![0i64; weights.len()];
        Self {
            weights,
            current: Mutex::new(current),
            total,
        }
    }
}

impl LoadBalancer for WeightedRoundRobin {
    fn name(&self) -> &'static str {
        "weighted"
    }
    fn pick(&self, _ctx: &RouteContext, _loads: &[u64]) -> Option<usize> {
        let n = self.weights.len();
        if n == 0 {
            return None;
        }
        let mut current = self.current.lock();
        // advance every target by its weight, pick the current maximum, then
        // pull that target back by the total weight so others catch up
        let mut best = 0usize;
        for i in 0..n {
            current[i] += self.weights[i];
            if current[i] > current[best] {
                best = i;
            }
        }
        current[best] -= self.total;
        Some(best)
    }
}

/// Uniform random selection.
pub struct Random {
    n: usize,
}

impl Random {
    pub fn new(n: usize) -> Self {
        Self { n }
    }
}

impl LoadBalancer for Random {
    fn name(&self) -> &'static str {
        "random"
    }
    fn pick(&self, _ctx: &RouteContext, _loads: &[u64]) -> Option<usize> {
        if self.n == 0 {
            return None;
        }
        Some(rand::rng().random_range(0..self.n))
    }
}

/// Pick the less loaded of two randomly chosen targets.
pub struct PowerOfTwo {
    n: usize,
}

impl PowerOfTwo {
    pub fn new(n: usize) -> Self {
        Self { n }
    }
}

impl LoadBalancer for PowerOfTwo {
    fn name(&self) -> &'static str {
        "power_of_two"
    }
    fn pick(&self, _ctx: &RouteContext, loads: &[u64]) -> Option<usize> {
        if self.n == 0 {
            return None;
        }
        if self.n == 1 {
            return Some(0);
        }
        let a = rand::rng().random_range(0..self.n);
        let mut b = rand::rng().random_range(0..self.n);
        if b == a {
            b = (b + 1) % self.n;
        }
        if loads.len() == self.n {
            return Some(if loads[a] <= loads[b] { a } else { b });
        }
        Some(a)
    }
}

/// Hash-ring routing that pins a session/user to the same target.
pub struct ConsistentHash {
    ring: Vec<(u64, usize)>,
    hasher: RandomState,
    n: usize,
    rr: AtomicUsize,
}

struct VnodeKey {
    bytes: [u8; 48],
    len: usize,
}

impl VnodeKey {
    fn new() -> Self {
        Self {
            bytes: [0; 48],
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).expect("fmt::Write only appends valid UTF-8")
    }
}

impl fmt::Write for VnodeKey {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self.len.checked_add(value.len()).ok_or(fmt::Error)?;
        let target = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        target.copy_from_slice(value.as_bytes());
        self.len = end;
        Ok(())
    }
}

impl ConsistentHash {
    pub fn new(n: usize) -> Self {
        // fixed seeds keep the ring and key hashing deterministic in-process
        let hasher = RandomState::with_seeds(0x1234, 0x5678, 0x9abc, 0xdef0);
        const VIRTUAL_NODES: usize = 100;
        let mut ring = Vec::with_capacity(n.saturating_mul(VIRTUAL_NODES));
        let mut vnode_key = VnodeKey::new();
        for i in 0..n {
            for v in 0..VIRTUAL_NODES {
                vnode_key.clear();
                write!(&mut vnode_key, "{i}#{v}")
                    .expect("vnode key capacity covers two usize values");
                ring.push((hasher.hash_one(vnode_key.as_str()), i));
            }
        }
        ring.sort_by_key(|(h, _)| *h);
        Self {
            ring,
            hasher,
            n,
            rr: AtomicUsize::new(0),
        }
    }

    fn pick_key(&self, key: &str) -> usize {
        let h = self.hasher.hash_one(key);
        match self.ring.binary_search_by_key(&h, |(hh, _)| *hh) {
            Ok(idx) => self.ring[idx].1,
            Err(idx) => {
                let i = if idx == self.ring.len() { 0 } else { idx };
                self.ring[i].1
            }
        }
    }
}

impl LoadBalancer for ConsistentHash {
    fn name(&self) -> &'static str {
        "consistent_hash"
    }
    fn pick(&self, ctx: &RouteContext, _loads: &[u64]) -> Option<usize> {
        if self.n == 0 {
            return None;
        }
        if let Some(key) = ctx.session_key {
            return Some(self.pick_key(key));
        }
        if let Some(prompt) = ctx.prompt {
            return Some(self.pick_key(prompt));
        }
        Some(self.rr.fetch_add(1, Relaxed) % self.n)
    }
}

/// Approximate cache-aware routing.
///
/// Each target keeps a byte trie of prompts it has served. Incoming prompts are
/// scored by the fraction of their leading bytes already present on each target;
/// when the best match clears `threshold` the request is pinned there to reuse
/// the upstream KV cache, otherwise it spreads to the least-warmed target.
pub struct CacheAware {
    n: usize,
    threshold: f32,
    tries: Vec<Mutex<Trie>>,
    sizes: Vec<AtomicU64>,
    rr: AtomicUsize,
}

impl CacheAware {
    pub fn new(n: usize, threshold: f32) -> Self {
        let mut tries = Vec::with_capacity(n);
        let mut sizes = Vec::with_capacity(n);
        for _ in 0..n {
            tries.push(Mutex::new(Trie::with_capacity(
                scorer::DEFAULT_PREFIX_MAX_NODES,
            )));
            sizes.push(AtomicU64::new(0));
        }
        Self {
            n,
            threshold,
            tries,
            sizes,
            rr: AtomicUsize::new(0),
        }
    }
}

impl LoadBalancer for CacheAware {
    fn name(&self) -> &'static str {
        "cache_aware"
    }

    fn pick(&self, ctx: &RouteContext, loads: &[u64]) -> Option<usize> {
        if self.n == 0 {
            return None;
        }
        if let Some(prompt) = ctx.prompt {
            if !prompt.is_empty() {
                let mut best = 0usize;
                let mut best_ratio = 0f32;
                for i in 0..self.n {
                    let matched = self.tries[i].lock().longest_prefix(prompt);
                    let ratio = matched as f32 / prompt.len() as f32;
                    if ratio > best_ratio {
                        best_ratio = ratio;
                        best = i;
                    }
                }
                if best_ratio >= self.threshold {
                    return Some(best);
                }
            }
        }
        // not enough cache affinity: prefer the least loaded target when known
        if loads.len() == self.n {
            let mut idx = 0;
            let mut min = loads[0];
            for (i, &l) in loads.iter().enumerate().skip(1) {
                if l < min {
                    min = l;
                    idx = i;
                }
            }
            return Some(idx);
        }
        // otherwise spread to the target with the smallest learned tree
        let mut idx = 0;
        let mut min = self.sizes[0].load(Relaxed);
        for i in 1..self.n {
            let s = self.sizes[i].load(Relaxed);
            if s < min {
                min = s;
                idx = i;
            }
        }
        if min == 0 {
            idx = self.rr.fetch_add(1, Relaxed) % self.n;
        }
        Some(idx)
    }

    fn observe(&self, target: usize, ctx: &RouteContext) {
        if target >= self.n {
            return;
        }
        if let Some(prompt) = ctx.prompt {
            if !prompt.is_empty() {
                self.tries[target].lock().insert(prompt);
                self.sizes[target].fetch_add(1, Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin_cycles() {
        let lb = RoundRobin::new(3);
        let c = RouteContext::default();
        assert_eq!(lb.pick(&c, &[]), Some(0));
        assert_eq!(lb.pick(&c, &[]), Some(1));
        assert_eq!(lb.pick(&c, &[]), Some(2));
        assert_eq!(lb.pick(&c, &[]), Some(0));
    }

    #[test]
    fn build_cheapest_ranks_by_cost() {
        let stats = TargetStats {
            cost_per_mtok: vec![8.0, 1.5, 3.0],
            ..Default::default()
        };
        let lb = build_with_stats(BalancingStrategy::Cheapest, &[1, 1, 1], &stats);
        assert_eq!(lb.name(), "cheapest");
        assert_eq!(lb.pick(&RouteContext::default(), &[]), Some(1));
    }

    #[test]
    fn build_cheapest_pads_missing_stats() {
        // stats shorter than the route: the tail is unknown (scored neutral),
        // no panic, and the known-cheapest target still wins
        let stats = TargetStats {
            cost_per_mtok: vec![5.0, 0.5],
            ..Default::default()
        };
        let lb = build_with_stats(BalancingStrategy::Cheapest, &[1, 1, 1], &stats);
        assert_eq!(lb.pick(&RouteContext::default(), &[]), Some(1));
    }

    #[test]
    fn build_fastest_without_source_degrades_to_least_load() {
        let lb = build(BalancingStrategy::Fastest, &[1, 1, 1]);
        assert_eq!(lb.name(), "fastest");
        // least-load fallback: the idle target wins
        assert_eq!(lb.pick(&RouteContext::default(), &[5, 0, 9]), Some(1));
    }

    #[test]
    fn consistent_hash_is_stable() {
        let lb = ConsistentHash::new(4);
        let ctx = RouteContext {
            session_key: Some("user-1"),
            prompt: None,
            token_ids: None,
            adapter: None,
        };
        let a = lb.pick(&ctx, &[]).unwrap();
        let b = lb.pick(&ctx, &[]).unwrap();
        assert_eq!(a, b);
        assert!(a < 4);
    }

    #[test]
    fn consistent_hash_ring_preserves_legacy_vnode_positions() {
        const VIRTUAL_NODES: usize = 100;
        let lb = ConsistentHash::new(4);
        let mut legacy = Vec::with_capacity(4 * VIRTUAL_NODES);
        for i in 0..4 {
            for v in 0..VIRTUAL_NODES {
                legacy.push((lb.hasher.hash_one(format!("{i}#{v}")), i));
            }
        }
        legacy.sort_by_key(|(hash, _)| *hash);
        assert_eq!(lb.ring, legacy);
    }

    #[test]
    fn cache_aware_pins_repeated_prefix() {
        let lb = CacheAware::new(2, 0.5);
        let ctx = RouteContext {
            session_key: None,
            prompt: Some("a long shared system prompt followed by a question"),
            token_ids: None,
            adapter: None,
        };
        let first = lb.pick(&ctx, &[]).unwrap();
        lb.observe(first, &ctx);
        let second = lb.pick(&ctx, &[]).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn empty_targets_return_none() {
        let lb = build(BalancingStrategy::RoundRobin, &[]);
        assert_eq!(lb.pick(&RouteContext::default(), &[]), None);
    }

    #[test]
    fn weighted_distributes_in_proportion() {
        // weights 3:1 over a full cycle of 4 picks -> target 0 thrice, target 1 once
        let lb = build(BalancingStrategy::Weighted, &[3, 1]);
        let ctx = RouteContext::default();
        let mut counts = [0usize; 2];
        for _ in 0..40 {
            counts[lb.pick(&ctx, &[]).unwrap()] += 1;
        }
        assert_eq!(counts[0], 30);
        assert_eq!(counts[1], 10);
    }

    #[test]
    fn weighted_is_smooth_not_bursty() {
        // smooth wrr interleaves rather than emitting 0,0,0,1 in a block
        let lb = build(BalancingStrategy::Weighted, &[3, 1]);
        let ctx = RouteContext::default();
        let seq: Vec<usize> = (0..4).map(|_| lb.pick(&ctx, &[]).unwrap()).collect();
        // the single low-weight pick lands in the middle of the cycle
        assert_eq!(seq, vec![0, 0, 1, 0]);
    }

    #[test]
    fn weighted_empty_returns_none() {
        let lb = build(BalancingStrategy::Weighted, &[]);
        assert_eq!(lb.pick(&RouteContext::default(), &[]), None);
    }

    #[test]
    fn pipeline_strategy_picks_least_loaded() {
        let lb = build(BalancingStrategy::Pipeline, &[1, 1, 1]);
        assert_eq!(lb.name(), "pipeline");
        // load scorer favours the idle target 1; equal weights, no prompt
        let got = lb.pick(&RouteContext::default(), &[9, 0, 4]);
        assert_eq!(got, Some(1));
    }

    #[test]
    fn pipeline_strategy_empty_returns_none() {
        let lb = build(BalancingStrategy::Pipeline, &[]);
        assert_eq!(lb.pick(&RouteContext::default(), &[]), None);
    }
}
