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
//! [ADR-0032](https://github.com/rolter-ai/rolter/blob/master/docs/dev-docs/adr/2026-09-09-one-point-oh-compatibility-guarantees.md).

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
    /// request prompt used for prefix/cache affinity scoring. The gateway may
    /// hand over only the leading bytes of a long prompt, since those are what
    /// decide prefix affinity; `prompt_len` and `prompt_digest` then describe
    /// the whole of it
    pub prompt: Option<&'a str>,
    /// Byte length of the whole prompt `prompt` was taken from, for a reader
    /// that needs the prompt's size rather than its leading bytes (the latency
    /// predictor's token estimate). `None` means `prompt` is the whole prompt.
    pub prompt_len: Option<usize>,
    /// A 64-bit digest of the whole prompt `prompt` was taken from, for a
    /// reader that keys on the prompt's identity (`consistent_hash` without a
    /// session). Hashing a bounded prefix instead would send every request
    /// that shares a long system prompt to one target. `None` means the reader
    /// hashes `prompt` itself.
    pub prompt_digest: Option<u64>,
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

    /// Pick as [`LoadBalancer::pick`] does, told which targets the caller can
    /// use for this attempt: `eligible(i)` is `false` for a target that is
    /// already tried, parked on a cooldown, unhealthy, breaker-open or refused
    /// to the caller's key.
    ///
    /// The default ignores `eligible` and calls `pick`, since the caller skips
    /// an ineligible pick itself. A strategy whose decision weighs targets
    /// against each other overrides it, so a target that takes no traffic
    /// cannot shape the comparison: `cache_aware`'s load guard would otherwise
    /// read a dead replica's empty queue as the pool's least-loaded target.
    /// When no target is eligible an override behaves as `pick`.
    fn pick_eligible(
        &self,
        ctx: &RouteContext,
        loads: &[u64],
        eligible: &dyn Fn(usize) -> bool,
    ) -> Option<usize> {
        let _ = eligible;
        self.pick(ctx, loads)
    }

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
        self.pick_hash(self.hasher.hash_one(key))
    }

    /// The target owning ring position `h`: the first virtual node at or past
    /// it, wrapping to the start of the ring.
    fn pick_hash(&self, h: u64) -> usize {
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
        // the whole prompt's digest before `prompt`, which may be only its
        // leading bytes: prompts that share a long system prompt and differ
        // after it must not all hash to one target
        if let Some(digest) = ctx.prompt_digest {
            return Some(self.pick_hash(self.hasher.hash_one(digest)));
        }
        if let Some(prompt) = ctx.prompt {
            return Some(self.pick_key(prompt));
        }
        Some(self.rr.fetch_add(1, Relaxed) % self.n)
    }
}

/// Balance margins for [`CacheAware`], after SGLang's cache-aware router:
/// prefix affinity wins until the warm target is busier than the least-loaded
/// one by **both** margins, and then the request spreads. The absolute margin
/// lets a warm replica take a short queue for the sake of its cache; the
/// relative one keeps a busy pool from reading a gap of a request or two as an
/// imbalance. Without them the first replica to serve a shared system prompt
/// took every request that followed, however deep its queue grew (#1851).
const BALANCE_ABS_THRESHOLD: u64 = 2;
const BALANCE_REL_THRESHOLD: f64 = 1.5;

/// Whether `target` is busier than the least-loaded usable target by both
/// balance margins. Only targets `usable` admits count towards the minimum: a
/// replica that takes no traffic drains to an empty queue, and reading that as
/// the pool's least-loaded target would call every warm replica overloaded.
/// With no load known there is nothing to balance against.
fn overloaded(target: usize, loads: &[u64], usable: &dyn Fn(usize) -> bool) -> bool {
    let Some(&load) = loads.get(target) else {
        return false;
    };
    let Some(min) = (0..loads.len())
        .filter(|&i| usable(i))
        .map(|i| loads[i])
        .min()
    else {
        return false;
    };
    load > min.saturating_add(BALANCE_ABS_THRESHOLD)
        && load as f64 > min as f64 * BALANCE_REL_THRESHOLD
}

/// The targets one [`CacheAware`] pick may choose from, asked of the caller
/// once each, since the caller's answer may take locks. A route with up to 64
/// targets keeps the answer in a word, so a pick allocates nothing.
enum Usable {
    Word(u64),
    Wide(Vec<bool>),
}

impl Usable {
    /// Ask `eligible` about each of `n > 0` targets. With nothing eligible the
    /// caller fails open over every target, so all of them are weighed, as a
    /// plain pick does.
    fn ask(n: usize, eligible: &dyn Fn(usize) -> bool) -> Self {
        if n <= 64 {
            let mut word = 0u64;
            for i in (0..n).filter(|&i| eligible(i)) {
                word |= 1 << i;
            }
            if word == 0 {
                word = u64::MAX >> (64 - n);
            }
            return Usable::Word(word);
        }
        let mut mask: Vec<bool> = (0..n).map(eligible).collect();
        if !mask.contains(&true) {
            mask.fill(true);
        }
        Usable::Wide(mask)
    }

    fn contains(&self, i: usize) -> bool {
        match self {
            Usable::Word(word) => i < 64 && word & (1 << i) != 0,
            Usable::Wide(mask) => mask.get(i).copied().unwrap_or(false),
        }
    }
}

/// Approximate cache-aware routing.
///
/// Each target keeps a byte trie of prompts it has served. Incoming prompts are
/// scored by the fraction of their leading bytes already present on each target.
/// When the best match clears `threshold` the request is pinned there to reuse
/// the upstream KV cache, unless that target is busier than the least-loaded
/// one by both balance margins. Otherwise it spreads to the least-loaded target,
/// or to the least-warmed one when no load is known. Given the caller's
/// eligibility ([`LoadBalancer::pick_eligible`]), every one of those
/// comparisons runs over the targets the caller can use.
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
        self.pick_eligible(ctx, loads, &|_| true)
    }

    fn pick_eligible(
        &self,
        ctx: &RouteContext,
        loads: &[u64],
        eligible: &dyn Fn(usize) -> bool,
    ) -> Option<usize> {
        if self.n == 0 {
            return None;
        }
        let mask = Usable::ask(self.n, eligible);
        let usable = |i: usize| mask.contains(i);
        if let Some(prompt) = ctx.prompt {
            if !prompt.is_empty() {
                let mut best: Option<(usize, f32)> = None;
                for i in (0..self.n).filter(|&i| usable(i)) {
                    let matched = self.tries[i].lock().longest_prefix(prompt);
                    let ratio = matched as f32 / prompt.len() as f32;
                    if best.is_none_or(|(_, best_ratio)| ratio > best_ratio) {
                        best = Some((i, ratio));
                    }
                }
                // affinity, unless the warm target is the one queueing
                if let Some((best, ratio)) = best {
                    if ratio >= self.threshold
                        && !(loads.len() == self.n && overloaded(best, loads, &usable))
                    {
                        return Some(best);
                    }
                }
            }
        }
        // not enough cache affinity: prefer the least loaded target when known
        if loads.len() == self.n {
            return (0..self.n).filter(|&i| usable(i)).min_by_key(|&i| loads[i]);
        }
        // otherwise spread to the target with the smallest learned tree, and
        // round-robin while none has learned anything
        let (idx, min) = (0..self.n)
            .filter(|&i| usable(i))
            .map(|i| (i, self.sizes[i].load(Relaxed)))
            .min_by_key(|&(_, size)| size)?;
        if min == 0 {
            let start = self.rr.fetch_add(1, Relaxed) % self.n;
            return (0..self.n)
                .map(|k| (start + k) % self.n)
                .find(|&i| usable(i));
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
            ..Default::default()
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
            ..Default::default()
        };
        let first = lb.pick(&ctx, &[]).unwrap();
        lb.observe(first, &ctx);
        let second = lb.pick(&ctx, &[]).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn cache_aware_spreads_when_the_warm_target_is_busy() {
        let lb = CacheAware::new(3, 0.5);
        let ctx = RouteContext {
            prompt: Some("system: you are a careful assistant\nuser: hi\n"),
            ..Default::default()
        };
        lb.observe(0, &ctx);
        // an idle pool keeps the affinity
        assert_eq!(lb.pick(&ctx, &[0, 0, 0]), Some(0));
        // a short queue on the warm replica is worth its cache
        assert_eq!(lb.pick(&ctx, &[2, 0, 0]), Some(0));
        // past both margins the request goes to the least-loaded replica
        assert_eq!(lb.pick(&ctx, &[3, 1, 0]), Some(2));
        // in a busy pool a gap of a couple of requests is not an imbalance
        assert_eq!(lb.pick(&ctx, &[8, 6, 6]), Some(0));
        // with no load known there is nothing to balance against
        assert_eq!(lb.pick(&ctx, &[]), Some(0));
    }

    /// The load of #1851's dogfood run: concurrent requests with distinct
    /// questions behind one shared system prompt used to land 24 / 0 / 0 on
    /// three replicas. Each pick holds its slot, as a request in flight does.
    #[test]
    fn concurrent_prompts_sharing_a_system_prompt_spread() {
        let lb = CacheAware::new(3, 0.5);
        let system = "system: you are a careful assistant who answers in detail\n";
        let prompts: Vec<String> = (0..24)
            .map(|i| format!("{system}user: question {i}\n"))
            .collect();
        let mut loads = [0u64; 3];
        for prompt in &prompts {
            let ctx = RouteContext {
                prompt: Some(prompt),
                ..Default::default()
            };
            let target = lb.pick(&ctx, &loads).unwrap();
            lb.observe(target, &ctx);
            loads[target] += 1;
        }
        assert!(loads.iter().all(|&n| (6..=10).contains(&n)), "{loads:?}");
    }

    /// The other half: one conversation's turns, arriving one at a time, keep
    /// the replica that holds their prefix.
    #[test]
    fn a_conversations_turns_stay_on_one_replica() {
        let lb = CacheAware::new(3, 0.5);
        let mut conversation = String::from("system: be brief\n");
        let mut served = Vec::new();
        for turn in 0..6 {
            conversation.push_str(&format!(
                "user: question {turn}\nassistant: answer {turn}\n"
            ));
            let ctx = RouteContext {
                prompt: Some(&conversation),
                ..Default::default()
            };
            let target = lb.pick(&ctx, &[0, 0, 0]).unwrap();
            lb.observe(target, &ctx);
            served.push(target);
        }
        assert!(served.windows(2).all(|w| w[0] == w[1]), "{served:?}");
    }

    /// A replica that takes no traffic (unhealthy, parked, breaker-open)
    /// drains to an empty queue. Its zero must not make the warm replica look
    /// overloaded, or the spill lands on the dead replica and the gateway then
    /// swaps in whichever target comes first.
    #[test]
    fn cache_aware_balances_only_against_eligible_targets() {
        let lb = CacheAware::new(3, 0.5);
        let ctx = RouteContext {
            prompt: Some("system: you are a careful assistant\nuser: hi\n"),
            ..Default::default()
        };
        lb.observe(1, &ctx);
        let alive = |i: usize| i != 2;
        // counted against the dead replica, three in flight reads as overloaded
        assert_eq!(lb.pick(&ctx, &[10, 3, 0]), Some(2));
        // against the live pool the warm replica is the least loaded
        assert_eq!(lb.pick_eligible(&ctx, &[10, 3, 0], &alive), Some(1));
        // and a real imbalance among live replicas still spreads
        assert_eq!(lb.pick_eligible(&ctx, &[1, 9, 0], &alive), Some(0));
        // the least-loaded fallback skips the dead replica too
        let cold = RouteContext {
            prompt: Some("user: something nobody asked before\n"),
            ..Default::default()
        };
        assert_eq!(lb.pick_eligible(&cold, &[4, 3, 0], &alive), Some(1));
        // a warm replica that is itself ineligible leaves the affinity
        assert_eq!(lb.pick_eligible(&ctx, &[0, 0, 0], &|i| i != 1), Some(0));
    }

    /// Eligibility is kept in a word for up to 64 targets and in a list past
    /// that; both ends of the word and the list answer the same way.
    #[test]
    fn cache_aware_eligibility_holds_for_any_number_of_targets() {
        let cold = RouteContext::default();
        for n in [1usize, 63, 64, 65, 130] {
            let lb = CacheAware::new(n, 0.5);
            let last = n - 1;
            // the last target is the least loaded but dead; the one before it
            // is the least loaded live one
            let mut loads = vec![5u64; n];
            loads[last] = 0;
            if n > 1 {
                loads[last - 1] = 1;
                assert_eq!(
                    lb.pick_eligible(&cold, &loads, &|i| i != last),
                    Some(last - 1),
                    "n = {n}"
                );
            }
            assert_eq!(
                lb.pick_eligible(&cold, &loads, &|i| i == last),
                Some(last),
                "n = {n}"
            );
            // nothing eligible weighs every target
            assert_eq!(
                lb.pick_eligible(&cold, &loads, &|_| false),
                Some(last),
                "n = {n}"
            );
        }
    }

    #[test]
    fn cache_aware_with_nothing_eligible_picks_as_before() {
        let lb = CacheAware::new(3, 0.5);
        let ctx = RouteContext {
            prompt: Some("system: be brief\nuser: hi\n"),
            ..Default::default()
        };
        lb.observe(1, &ctx);
        let none = |_: usize| false;
        for loads in [[0, 0, 0], [10, 3, 0], [0, 9, 4]] {
            assert_eq!(
                lb.pick_eligible(&ctx, &loads, &none),
                lb.pick(&ctx, &loads),
                "{loads:?}"
            );
        }
        // with no load known either, the untrained fallback still answers
        let fresh = CacheAware::new(2, 0.5);
        assert!(fresh.pick_eligible(&ctx, &[], &none).is_some());
    }

    #[test]
    fn the_balance_margin_cannot_overflow() {
        let all = |_: usize| true;
        assert!(!overloaded(0, &[u64::MAX, u64::MAX], &all));
        assert!(overloaded(0, &[u64::MAX, 0], &all));
        // no usable target leaves nothing to balance against
        assert!(!overloaded(0, &[9, 0], &|_| false));
    }

    #[test]
    fn consistent_hash_keys_on_the_whole_prompt_digest() {
        let lb = ConsistentHash::new(4);
        // one bounded prefix, many whole prompts behind it: the digest decides
        let targets: std::collections::HashSet<usize> = (0..64u64)
            .map(|digest| {
                let ctx = RouteContext {
                    prompt: Some("system: a long shared prefix"),
                    prompt_digest: Some(digest.wrapping_mul(0x9e37_79b9_7f4a_7c15)),
                    ..Default::default()
                };
                lb.pick(&ctx, &[]).unwrap()
            })
            .collect();
        assert!(targets.len() > 1, "{targets:?}");
        // the same whole prompt keeps its target whatever prefix it carries
        let a = RouteContext {
            prompt: Some("one prefix"),
            prompt_digest: Some(42),
            ..Default::default()
        };
        let b = RouteContext {
            prompt: Some("another prefix"),
            prompt_digest: Some(42),
            ..Default::default()
        };
        assert_eq!(lb.pick(&a, &[]), lb.pick(&b, &[]));
        // a session still wins over the prompt
        let session = RouteContext {
            session_key: Some("user-1"),
            ..a.clone()
        };
        let plain = RouteContext {
            session_key: Some("user-1"),
            ..Default::default()
        };
        assert_eq!(lb.pick(&session, &[]), lb.pick(&plain, &[]));
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
