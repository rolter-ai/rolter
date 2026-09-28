//! Composable **filter → weighted-score → argmax** selection pipeline.
//!
//! Modeled on llm-d's Endpoint Picker: eligibility filtering drops targets that
//! must not receive traffic, every strategy becomes a [`Scorer`] producing a
//! per-target score in `[0.0, 1.0]`, the scores are combined as a weighted sum,
//! and the winner is the argmax with ties broken randomly. This turns the
//! monolithic [`LoadBalancer`] strategies into composable
//! plugins so cache/load/cost signals can be mixed per route.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use rand::RngExt;

use parking_lot::Mutex;

use crate::trie::Trie;
use crate::{LoadBalancer, RouteContext};

/// A per-target signal. Produces a score in `[0.0, 1.0]` (higher is better) for
/// each candidate target that survived filtering, index-aligned with the
/// `candidates` slice it is handed.
pub trait Scorer: Send + Sync {
    /// stable identifier of the scorer
    fn name(&self) -> &'static str;

    /// Score each candidate target. `candidates[k]` is a target index into the
    /// route; `loads[i]` (when `loads.len()` equals the route width) is the
    /// in-flight count for target `i`. Returns one score per candidate.
    fn score(&self, ctx: &RouteContext, candidates: &[usize], loads: &[u64]) -> Vec<f32>;

    /// Record that `target` served `ctx`. Scorers that learn from traffic
    /// (prefix cache) override this; stateless ones ignore it.
    fn observe(&self, _target: usize, _ctx: &RouteContext) {}
}

/// One scorer's contribution to a [`Pipeline`]'s blended score, over every
/// target of the route rather than a request's candidate set.
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    /// the scorer's stable identifier ("fastest", "cheapest", ...)
    pub name: &'static str,
    /// the weight it carries in the blend
    pub weight: f32,
    /// its score per target, route-order aligned
    pub scores: Vec<f32>,
}

/// A weighted stack of [`Scorer`]s selecting one target by argmax of the
/// weighted-sum score over the eligible candidates.
pub struct Pipeline {
    scorers: Vec<(Box<dyn Scorer>, f32)>,
    /// route width (number of targets)
    n: usize,
    /// strategy name surfaced in logs/metrics ("pipeline", "cheapest", ...)
    name: &'static str,
}

impl Pipeline {
    /// Empty pipeline over `n` targets. Add scorers with [`Pipeline::with`].
    pub fn new(n: usize) -> Self {
        Self {
            scorers: Vec::new(),
            n,
            name: "pipeline",
        }
    }

    /// Override the strategy name surfaced by [`LoadBalancer::name`].
    pub fn named(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Add a scorer contributing `weight` to the combined score. A non-positive
    /// weight is clamped to `0.0` (the scorer is retained but contributes
    /// nothing), keeping the weighted sum well defined.
    pub fn with(mut self, scorer: Box<dyn Scorer>, weight: f32) -> Self {
        self.scorers.push((scorer, weight.max(0.0)));
        self
    }

    /// Select one target. `eligible(i)` is the filter stage: it returns `false`
    /// for a target that must be skipped (model mismatch, unhealthy, cooling,
    /// already tried). Returns `None` when no target is eligible.
    pub fn select(
        &self,
        ctx: &RouteContext,
        loads: &[u64],
        eligible: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        // stage 1: filter to the eligible candidate set
        let candidates: Vec<usize> = (0..self.n).filter(|&i| eligible(i)).collect();
        if candidates.is_empty() {
            return None;
        }
        if candidates.len() == 1 {
            return Some(candidates[0]);
        }
        // stage 2: weighted sum of every scorer over the candidates
        let mut totals = vec![0f32; candidates.len()];
        for (scorer, weight) in &self.scorers {
            if *weight == 0.0 {
                continue;
            }
            let scores = scorer.score(ctx, &candidates, loads);
            for (k, s) in scores.iter().enumerate() {
                if let Some(t) = totals.get_mut(k) {
                    *t += weight * s;
                }
            }
        }
        // stage 3: argmax, ties broken randomly
        Some(candidates[argmax_tiebreak(&totals)])
    }

    /// Score every target with every scorer, without picking one. This is the
    /// read-only half of [`Pipeline::select`]: no filtering, no argmax and no
    /// tiebreak randomness, so the same call twice on an idle route returns
    /// the same numbers. Used by telemetry to explain *why* a route ranks its
    /// targets the way it does; never on the pick path.
    pub fn components(&self, ctx: &RouteContext, loads: &[u64]) -> Vec<Component> {
        let targets: Vec<usize> = (0..self.n).collect();
        self.scorers
            .iter()
            .map(|(scorer, weight)| Component {
                name: scorer.name(),
                weight: *weight,
                scores: scorer.score(ctx, &targets, loads),
            })
            .collect()
    }

    /// Fan an `observe` out to every scorer so learners update.
    pub fn observe(&self, target: usize, ctx: &RouteContext) {
        for (scorer, _) in &self.scorers {
            scorer.observe(target, ctx);
        }
    }

    /// The default composable stack over `weights.len()` targets: session
    /// affinity, configured per-target weight, in-flight load, and prefix-cache
    /// affinity, each contributing equally. The foundation strategy the
    /// roadmap's cost/latency scorers slot into.
    pub fn default_stack(weights: &[u32]) -> Self {
        let n = weights.len();
        Self::new(n)
            .with(Box::new(SessionAffinityScorer::new(n)), 1.0)
            .with(Box::new(StaticScorer::new(weights)), 1.0)
            .with(Box::new(LeastLoadScorer::new(n)), 1.0)
            .with(Box::new(PrefixCacheScorer::new(n)), 1.0)
    }

    /// The cost-aware stack: catalog price dominates, with in-flight load as a
    /// light tiebreaker so equal-cost targets still spread instead of piling
    /// onto one. `costs[i]` is any consistent per-token rate for target `i`
    /// (`<= 0` = unknown; see [`CheapestScorer`]).
    pub fn cheapest_stack(costs: &[f64]) -> Self {
        let n = costs.len();
        Self::new(n)
            .named("cheapest")
            .with(Box::new(CheapestScorer::new(costs)), 1.0)
            .with(Box::new(LeastLoadScorer::new(n)), 0.25)
    }

    /// The LoRA-aware stack (#853): adapter residency dominates, prefix
    /// affinity next, in-flight load last.
    ///
    /// Adapter residency outranks prefix affinity because the costs are not
    /// comparable — missing a warm prefix re-computes some tokens, while
    /// missing a resident adapter can force the engine to load adapter weights
    /// before it decodes anything at all. Load stays in the stack so a fleet
    /// where every target holds the adapter still balances rather than pinning.
    pub fn lora_stack(n: usize) -> Self {
        Self::new(n)
            .named("lora_aware")
            .with(Box::new(LoraScorer::new(n)), 1.0)
            .with(Box::new(PrefixCacheScorer::new(n)), 0.5)
            .with(Box::new(LeastLoadScorer::new(n)), 0.25)
    }

    /// The latency-aware stack: observed per-target latency dominates, with
    /// in-flight load as a light tiebreaker (and the only signal until latency
    /// samples arrive — a cold route behaves like least-load).
    pub fn fastest_stack(n: usize, source: std::sync::Arc<dyn LatencySource>) -> Self {
        Self::new(n)
            .named("fastest")
            .with(Box::new(FastestScorer::new(n, source)), 1.0)
            .with(Box::new(LeastLoadScorer::new(n)), 0.25)
    }

    /// The predicted-latency stack: the per-request model dominates, with
    /// in-flight load behind it.
    ///
    /// Load stays in the stack for two reasons. It carries the route while the
    /// models are cold, and once they are warm it is the tiebreaker between
    /// targets the model rates equally — which is the common case on a
    /// homogeneous fleet, where the honest answer is "either, take the emptier".
    pub fn predicted_latency_stack(
        n: usize,
        source: std::sync::Arc<dyn LatencyPredictionSource>,
    ) -> Self {
        Self::new(n)
            .named("predicted_latency")
            .with(Box::new(PredictedLatencyScorer::new(n, source)), 1.0)
            .with(Box::new(LeastLoadScorer::new(n)), 0.25)
    }
}

impl LoadBalancer for Pipeline {
    fn name(&self) -> &'static str {
        self.name
    }

    fn pick(&self, ctx: &RouteContext, loads: &[u64]) -> Option<usize> {
        // eligibility filtering (tried/cooling/unhealthy/breaker) is applied by
        // the caller; the pipeline scores every target
        self.select(ctx, loads, |_| true)
    }

    fn observe(&self, target: usize, ctx: &RouteContext) {
        Pipeline::observe(self, target, ctx);
    }
}

/// Index of the maximum score with ties broken uniformly at random. An empty
/// slice yields `0` (callers guarantee non-empty candidate sets).
fn argmax_tiebreak(scores: &[f32]) -> usize {
    if scores.is_empty() {
        return 0;
    }
    let mut best = f32::NEG_INFINITY;
    let mut ties = 0usize;
    let mut winner = 0usize;
    for (i, &s) in scores.iter().enumerate() {
        if s > best + f32::EPSILON {
            best = s;
            ties = 1;
            winner = i;
        } else if (s - best).abs() <= f32::EPSILON {
            // reservoir pick among equal-score candidates for uniform tiebreak
            ties += 1;
            if rand::rng().random_range(0..ties) == 0 {
                winner = i;
            }
        }
    }
    winner
}

/// Constant per-target preference from configured weights (e.g. `target.weight`
/// in `rolter.toml`). Normalized to `[0.0, 1.0]` against the largest weight so
/// it composes with the other scorers on a shared scale.
pub struct StaticScorer {
    /// per-target weight, index-aligned with the route targets
    weights: Vec<f32>,
    max: f32,
}

impl StaticScorer {
    pub fn new(weights: &[u32]) -> Self {
        let weights: Vec<f32> = weights.iter().map(|&w| w.max(1) as f32).collect();
        let max = weights.iter().cloned().fold(1.0f32, f32::max);
        Self { weights, max }
    }
}

impl Scorer for StaticScorer {
    fn name(&self) -> &'static str {
        "static_weight"
    }
    fn score(&self, _ctx: &RouteContext, candidates: &[usize], _loads: &[u64]) -> Vec<f32> {
        candidates
            .iter()
            .map(|&i| self.weights.get(i).copied().unwrap_or(1.0) / self.max)
            .collect()
    }
}

/// Prefer the cheapest target by catalog price. Costs are any consistent
/// per-token rate, index-aligned with the route targets; only relative order
/// matters. The cheapest candidate scores `1.0` down toward `0.0` for the most
/// expensive; a target with no known price (cost `<= 0`) scores a neutral
/// `0.5`, and all-equal (or all-unknown) costs score a flat `1.0` so the
/// scorer never skews a route the catalog doesn't cover.
pub struct CheapestScorer {
    /// per-target cost, index-aligned with the route targets (`<= 0` = unknown)
    costs: Vec<f64>,
}

impl CheapestScorer {
    pub fn new(costs: &[f64]) -> Self {
        Self {
            costs: costs.to_vec(),
        }
    }
}

impl Scorer for CheapestScorer {
    fn name(&self) -> &'static str {
        "cheapest"
    }
    fn score(&self, _ctx: &RouteContext, candidates: &[usize], _loads: &[u64]) -> Vec<f32> {
        rank_lower_better(&self.costs, candidates)
    }
}

/// Min-max normalize a lower-is-better signal over the candidate set: the
/// lowest known value scores `1.0` down to `0.0` for the highest; `<= 0`
/// (unknown) scores a neutral `0.5`, and an uninformative window (no or equal
/// known values) scores a flat `1.0`.
fn rank_lower_better(values: &[f64], candidates: &[usize]) -> Vec<f32> {
    let known: Vec<f64> = candidates
        .iter()
        .filter_map(|&i| values.get(i).copied())
        .filter(|&v| v > 0.0)
        .collect();
    let (min, max) = known
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    if known.is_empty() || min >= max {
        return vec![1.0; candidates.len()];
    }
    candidates
        .iter()
        .map(|&i| match values.get(i).copied() {
            Some(v) if v > 0.0 => (1.0 - (v - min) / (max - min)) as f32,
            _ => 0.5,
        })
        .collect()
}

/// A live, lock-cheap source of per-target latency the [`FastestScorer`] reads
/// at pick time. Implemented by the gateway over its request-completion
/// tracker; `latencies(n)` returns one entry per target (`<= 0` = no sample
/// yet).
pub trait LatencySource: Send + Sync {
    /// Smoothed latency in milliseconds for targets `0..n`, route-order aligned.
    fn latencies(&self, n: usize) -> Vec<f64>;
}

/// A live source of *predicted* per-request latency, as opposed to the observed
/// average [`LatencySource`] reports. Implemented by
/// [`crate::predictor::LatencyPredictor`]; `None` for a target means the model
/// has too little evidence to be trusted, which is not the same as "slow".
pub trait LatencyPredictionSource: Send + Sync {
    /// Predicted milliseconds for each target `0..n` given the current per-target
    /// `loads` and this request's prompt size.
    fn predict(&self, n: usize, loads: &[u64], prompt_tokens: usize) -> Vec<Option<f64>>;
}

/// Live exact KV-cache residency. `scores` returns route-order-aligned resident
/// prefix fractions, or `None` when token ids are absent or telemetry is stale.
pub trait KvCacheSource: Send + Sync {
    fn scores(&self, token_ids: &[u32]) -> Option<Vec<f32>>;
}

/// Live LMCache controller signal. Scores are route-order-aligned and already
/// bounded to `[0, 1]`; `None` means missing, failed, or stale telemetry.
pub trait LmCacheSource: Send + Sync {
    fn scores(&self) -> Option<Vec<f32>>;
}

pub struct PreciseKvScorer {
    source: std::sync::Arc<dyn KvCacheSource>,
}

impl PreciseKvScorer {
    pub fn new(source: std::sync::Arc<dyn KvCacheSource>) -> Self {
        Self { source }
    }
}

impl Scorer for PreciseKvScorer {
    fn name(&self) -> &'static str {
        "precise_kv_cache"
    }

    fn score(&self, ctx: &RouteContext, candidates: &[usize], _loads: &[u64]) -> Vec<f32> {
        let Some(token_ids) = ctx.token_ids.filter(|ids| !ids.is_empty()) else {
            return vec![0.0; candidates.len()];
        };
        let Some(scores) = self.source.scores(token_ids) else {
            return vec![0.0; candidates.len()];
        };
        candidates
            .iter()
            .map(|&index| scores.get(index).copied().unwrap_or(0.0).clamp(0.0, 1.0))
            .collect()
    }
}

pub struct LmCacheScorer {
    source: std::sync::Arc<dyn LmCacheSource>,
}

impl LmCacheScorer {
    pub fn new(source: std::sync::Arc<dyn LmCacheSource>) -> Self {
        Self { source }
    }
}

impl Scorer for LmCacheScorer {
    fn name(&self) -> &'static str {
        "lmcache"
    }

    fn score(&self, _ctx: &RouteContext, candidates: &[usize], _loads: &[u64]) -> Vec<f32> {
        let Some(scores) = self.source.scores() else {
            return vec![0.0; candidates.len()];
        };
        candidates
            .iter()
            .map(|&index| scores.get(index).copied().unwrap_or(0.0).clamp(0.0, 1.0))
            .collect()
    }
}

/// Prefer the target with the lowest observed latency. Reads a live
/// [`LatencySource`] on every pick (latency moves with traffic, unlike catalog
/// price), then ranks exactly like [`CheapestScorer`]: fastest candidate
/// `1.0`, slowest `0.0`, unsampled targets a neutral `0.5`, and a flat `1.0`
/// until at least two targets have distinct samples.
pub struct FastestScorer {
    source: std::sync::Arc<dyn LatencySource>,
    /// route width handed to the source
    n: usize,
}

impl FastestScorer {
    pub fn new(n: usize, source: std::sync::Arc<dyn LatencySource>) -> Self {
        Self { source, n }
    }
}

impl Scorer for FastestScorer {
    fn name(&self) -> &'static str {
        "fastest"
    }
    fn score(&self, _ctx: &RouteContext, candidates: &[usize], _loads: &[u64]) -> Vec<f32> {
        let latencies = self.source.latencies(self.n);
        rank_lower_better(&latencies, candidates)
    }
}

/// Prefer the target this request is predicted to finish soonest on, given the
/// queue it would join and its own prompt size (#853).
///
/// Unlike [`FastestScorer`] this reads `loads`: the whole point is that a
/// target's cost depends on how busy it is *right now*, so a stale average
/// cannot substitute. A target the model cannot yet predict scores as unknown,
/// which leaves the rest of the stack to place the request.
pub struct PredictedLatencyScorer {
    source: std::sync::Arc<dyn LatencyPredictionSource>,
    n: usize,
}

impl PredictedLatencyScorer {
    pub fn new(n: usize, source: std::sync::Arc<dyn LatencyPredictionSource>) -> Self {
        Self { source, n }
    }
}

impl Scorer for PredictedLatencyScorer {
    fn name(&self) -> &'static str {
        "predicted_latency"
    }
    fn score(&self, ctx: &RouteContext, candidates: &[usize], loads: &[u64]) -> Vec<f32> {
        let tokens = crate::predictor::prompt_tokens(ctx);
        // an unpredicted target becomes `0.0`, which `rank_lower_better` already
        // reads as "unknown" and scores neutrally — the same convention every
        // other lower-is-better signal in this file uses
        let predicted: Vec<f64> = self
            .source
            .predict(self.n, loads, tokens)
            .into_iter()
            .map(|p| p.unwrap_or(0.0))
            .collect();
        rank_lower_better(&predicted, candidates)
    }
}

/// Prefer the least in-flight-loaded target. Scores `1.0` for the least loaded
/// candidate down toward `0.0` for the most loaded; all-equal (or unknown) load
/// scores a flat `1.0` so this scorer stays neutral until load data arrives.
pub struct LeastLoadScorer {
    /// route width, used to validate the `loads` slice
    n: usize,
}

impl LeastLoadScorer {
    pub fn new(n: usize) -> Self {
        Self { n }
    }
}

impl Scorer for LeastLoadScorer {
    fn name(&self) -> &'static str {
        "least_load"
    }
    fn score(&self, _ctx: &RouteContext, candidates: &[usize], loads: &[u64]) -> Vec<f32> {
        if loads.len() != self.n {
            return vec![1.0; candidates.len()];
        }
        let max = candidates
            .iter()
            .map(|&i| loads[i])
            .max()
            .unwrap_or(0)
            .max(1) as f32;
        candidates
            .iter()
            .map(|&i| 1.0 - (loads[i] as f32 / max))
            .collect()
    }
}

/// Approximate prefix/KV-cache affinity. Each target keeps a byte trie of the
/// prompts it has served; a candidate scores by the fraction of the incoming
/// prompt's leading bytes already resident, so repeated prefixes pin to the
/// warm target. Absent a prompt every candidate scores `0.0` (neutral).
pub struct PrefixCacheScorer {
    n: usize,
    tries: Vec<Mutex<Trie>>,
    sizes: Vec<AtomicU64>,
}

/// Default per-target node cap for prefix-cache tries, bounding memory while
/// still holding a large working set of prompts.
pub const DEFAULT_PREFIX_MAX_NODES: usize = 1_000_000;

impl PrefixCacheScorer {
    pub fn new(n: usize) -> Self {
        let mut tries = Vec::with_capacity(n);
        let mut sizes = Vec::with_capacity(n);
        for _ in 0..n {
            tries.push(Mutex::new(Trie::with_capacity(DEFAULT_PREFIX_MAX_NODES)));
            sizes.push(AtomicU64::new(0));
        }
        Self { n, tries, sizes }
    }
}

impl Scorer for PrefixCacheScorer {
    fn name(&self) -> &'static str {
        "prefix_cache"
    }
    fn score(&self, ctx: &RouteContext, candidates: &[usize], _loads: &[u64]) -> Vec<f32> {
        let Some(prompt) = ctx.prompt.filter(|p| !p.is_empty()) else {
            return vec![0.0; candidates.len()];
        };
        let len = prompt.len() as f32;
        candidates
            .iter()
            .map(|&i| {
                if i >= self.n {
                    return 0.0;
                }
                let matched = self.tries[i].lock().longest_prefix(prompt);
                matched as f32 / len
            })
            .collect()
    }

    fn observe(&self, target: usize, ctx: &RouteContext) {
        if target >= self.n {
            return;
        }
        if let Some(prompt) = ctx.prompt.filter(|p| !p.is_empty()) {
            self.tries[target].lock().insert(prompt);
            self.sizes[target].fetch_add(1, Relaxed);
        }
    }
}

/// Default time-to-live for a session's affinity to its last-served target.
const DEFAULT_AFFINITY_TTL: Duration = Duration::from_secs(300);
/// Default cap on tracked sessions, bounding memory under churn.
const DEFAULT_AFFINITY_CAP: usize = 100_000;

/// Session affinity. Boosts the target that last served a given session so
/// repeat requests reuse its warm KV/prefix cache. The boost expires after a
/// TTL (so a session doesn't pin to a since-degraded node forever) and the
/// tracking map is capped (so unbounded distinct sessions can't grow it without
/// limit). Health/cooldown filtering happens upstream in the pipeline, so a
/// boosted-but-ineligible target is simply never a candidate.
pub struct SessionAffinityScorer {
    n: usize,
    ttl: Duration,
    cap: usize,
    /// session key to (last-served target, when it was recorded)
    last: Mutex<HashMap<String, (usize, Instant)>>,
}

impl SessionAffinityScorer {
    pub fn new(n: usize) -> Self {
        Self::with_ttl(n, DEFAULT_AFFINITY_TTL)
    }

    /// Construct with an explicit affinity TTL (a zero TTL makes every entry
    /// immediately stale, i.e. affinity off).
    pub fn with_ttl(n: usize, ttl: Duration) -> Self {
        Self {
            n,
            ttl,
            cap: DEFAULT_AFFINITY_CAP,
            last: Mutex::new(HashMap::new()),
        }
    }
}

impl Scorer for SessionAffinityScorer {
    fn name(&self) -> &'static str {
        "session_affinity"
    }

    fn score(&self, ctx: &RouteContext, candidates: &[usize], _loads: &[u64]) -> Vec<f32> {
        let mut scores = vec![0.0; candidates.len()];
        let Some(key) = ctx.session_key else {
            return scores;
        };
        let map = self.last.lock();
        if let Some((target, at)) = map.get(key) {
            if at.elapsed() < self.ttl {
                if let Some(k) = candidates.iter().position(|&c| c == *target) {
                    scores[k] = 1.0;
                }
            }
        }
        scores
    }

    fn observe(&self, target: usize, ctx: &RouteContext) {
        if target >= self.n {
            return;
        }
        let Some(key) = ctx.session_key else {
            return;
        };
        let mut map = self.last.lock();
        // bound the map: when at capacity and inserting a new session, drop an
        // arbitrary existing entry (cheap eviction; affinity is best-effort)
        if map.len() >= self.cap && !map.contains_key(key) {
            if let Some(evict) = map.keys().next().cloned() {
                map.remove(&evict);
            }
        }
        map.insert(key.to_string(), (target, Instant::now()));
    }
}

/// Adapter slots a single target can hold resident at once.
///
/// Mirrors vLLM's `--max-loras`: an accelerator holds a bounded number of LoRA
/// adapters, and serving one it does not hold costs a load. Modelling the bound
/// is the point — an unbounded "has ever served" set would claim residency long
/// after the adapter was evicted upstream, which is worse than not scoring at
/// all because it steers confidently to a cold target.
pub const DEFAULT_MAX_ADAPTERS_PER_TARGET: usize = 8;

/// LRU set of the adapters one target currently holds.
#[derive(Debug)]
struct AdapterSlots {
    capacity: usize,
    /// least-recently-used first, so eviction pops the front
    order: Vec<String>,
}

impl AdapterSlots {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            order: Vec::new(),
        }
    }

    fn holds(&self, adapter: &str) -> bool {
        self.order.iter().any(|a| a == adapter)
    }

    /// Record that this target just served `adapter`, evicting the least
    /// recently used one when the slots are full.
    fn touch(&mut self, adapter: &str) {
        if let Some(at) = self.order.iter().position(|a| a == adapter) {
            let existing = self.order.remove(at);
            self.order.push(existing);
            return;
        }
        if self.order.len() >= self.capacity {
            self.order.remove(0);
        }
        self.order.push(adapter.to_string());
    }
}

/// Prefer a target that already holds the requested LoRA adapter (#853).
///
/// llm-d routes model *variants* by which accelerator already has the adapter
/// resident; for a fleet serving many adapters off shared base weights, that is
/// the same class of win as prefix-cache affinity, and it composes with the
/// existing scorers rather than replacing them.
///
/// Residency is *learned from traffic*, not declared: rolter cannot see which
/// adapters an engine currently holds, and a static declaration would go stale
/// the moment the engine evicted one. Each target's slot set is bounded and
/// LRU, so it tracks what the engine plausibly still holds rather than
/// everything it has ever served.
///
/// When no candidate holds the adapter — a cold adapter, or a request with none
/// — every candidate scores `0.0`. That is deliberately *neutral* rather than a
/// penalty: an equal contribution cannot move the argmax, so the rest of the
/// pipeline decides and this scorer stays silent instead of guessing.
pub struct LoraScorer {
    resident: Vec<Mutex<AdapterSlots>>,
}

impl LoraScorer {
    /// Scorer over `n` targets, each holding up to
    /// [`DEFAULT_MAX_ADAPTERS_PER_TARGET`] adapters.
    pub fn new(n: usize) -> Self {
        Self::with_capacity(n, DEFAULT_MAX_ADAPTERS_PER_TARGET)
    }

    /// Scorer over `n` targets, each holding up to `capacity` adapters. Set
    /// this to the engine's `--max-loras` so residency tracks the real bound.
    pub fn with_capacity(n: usize, capacity: usize) -> Self {
        Self {
            resident: (0..n)
                .map(|_| Mutex::new(AdapterSlots::new(capacity)))
                .collect(),
        }
    }

    /// Whether `target` currently holds `adapter`. Exposed for tests and
    /// telemetry, not used on the pick path beyond `score`.
    pub fn holds(&self, target: usize, adapter: &str) -> bool {
        self.resident
            .get(target)
            .is_some_and(|slots| slots.lock().holds(adapter))
    }
}

impl Scorer for LoraScorer {
    fn name(&self) -> &'static str {
        "lora"
    }

    fn score(&self, ctx: &RouteContext, candidates: &[usize], _loads: &[u64]) -> Vec<f32> {
        let Some(adapter) = ctx.adapter else {
            return vec![0.0; candidates.len()];
        };
        candidates
            .iter()
            .map(|&i| {
                if self
                    .resident
                    .get(i)
                    .is_some_and(|slots| slots.lock().holds(adapter))
                {
                    1.0
                } else {
                    0.0
                }
            })
            .collect()
    }

    fn observe(&self, target: usize, ctx: &RouteContext) {
        let Some(adapter) = ctx.adapter else {
            return;
        };
        if let Some(slots) = self.resident.get(target) {
            slots.lock().touch(adapter);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argmax_picks_highest() {
        assert_eq!(argmax_tiebreak(&[0.1, 0.9, 0.3]), 1);
        assert_eq!(argmax_tiebreak(&[0.5]), 0);
    }

    #[test]
    fn argmax_tiebreak_stays_in_tied_set() {
        // three-way tie on the max: winner must be one of the tied indices
        for _ in 0..50 {
            let w = argmax_tiebreak(&[1.0, 1.0, 1.0, 0.2]);
            assert!(w < 3, "picked a non-max index {w}");
        }
    }

    #[test]
    fn filter_drops_ineligible_targets() {
        let p = Pipeline::new(3).with(Box::new(StaticScorer::new(&[1, 1, 1])), 1.0);
        // only target 2 is eligible -> it must win regardless of scores
        let got = p.select(&RouteContext::default(), &[], |i| i == 2);
        assert_eq!(got, Some(2));
    }

    #[test]
    fn no_eligible_targets_return_none() {
        let p = Pipeline::new(2).with(Box::new(StaticScorer::new(&[1, 1])), 1.0);
        assert_eq!(p.select(&RouteContext::default(), &[], |_| false), None);
    }

    #[test]
    fn static_weight_prefers_heavier_target() {
        let p = Pipeline::new(2).with(Box::new(StaticScorer::new(&[1, 9])), 1.0);
        // target 1 has 9x the weight; deterministic (no tie) so it always wins
        assert_eq!(p.select(&RouteContext::default(), &[], |_| true), Some(1));
    }

    #[test]
    fn cheapest_prefers_lowest_cost() {
        let p = Pipeline::cheapest_stack(&[3.0, 0.5, 10.0]);
        assert_eq!(p.select(&RouteContext::default(), &[], |_| true), Some(1));
        // eligibility still filters: cheapest surviving target wins
        assert_eq!(p.select(&RouteContext::default(), &[], |i| i != 1), Some(0));
    }

    #[test]
    fn cheapest_unknown_cost_scores_neutral() {
        let scorer = CheapestScorer::new(&[2.0, 0.0, 4.0]);
        let scores = scorer.score(&RouteContext::default(), &[0, 1, 2], &[]);
        assert_eq!(scores, vec![1.0, 0.5, 0.0]);
    }

    #[test]
    fn cheapest_all_unknown_or_equal_is_flat() {
        let scorer = CheapestScorer::new(&[0.0, 0.0]);
        assert_eq!(
            scorer.score(&RouteContext::default(), &[0, 1], &[]),
            vec![1.0, 1.0]
        );
        let scorer = CheapestScorer::new(&[5.0, 5.0]);
        assert_eq!(
            scorer.score(&RouteContext::default(), &[0, 1], &[]),
            vec![1.0, 1.0]
        );
    }

    #[test]
    fn cheapest_ties_break_by_load() {
        // equal cost: the 0.25-weight load scorer decides
        let p = Pipeline::cheapest_stack(&[1.0, 1.0]);
        assert_eq!(
            p.select(&RouteContext::default(), &[9, 0], |_| true),
            Some(1)
        );
    }

    struct FixedLatency(Vec<f64>);
    impl LatencySource for FixedLatency {
        fn latencies(&self, _n: usize) -> Vec<f64> {
            self.0.clone()
        }
    }

    struct FixedKv(Option<Vec<f32>>);
    impl KvCacheSource for FixedKv {
        fn scores(&self, _token_ids: &[u32]) -> Option<Vec<f32>> {
            self.0.clone()
        }
    }

    struct FixedLmCache(Option<Vec<f32>>);
    impl LmCacheSource for FixedLmCache {
        fn scores(&self) -> Option<Vec<f32>> {
            self.0.clone()
        }
    }

    #[test]
    fn precise_kv_prefers_largest_resident_prefix_and_fails_neutral() {
        let scorer = PreciseKvScorer::new(std::sync::Arc::new(FixedKv(Some(vec![0.25, 1.0]))));
        let ctx = RouteContext {
            token_ids: Some(&[1, 2, 3, 4]),
            ..Default::default()
        };
        assert_eq!(scorer.score(&ctx, &[0, 1], &[]), vec![0.25, 1.0]);
        let stale = PreciseKvScorer::new(std::sync::Arc::new(FixedKv(None)));
        assert_eq!(stale.score(&ctx, &[0, 1], &[]), vec![0.0, 0.0]);
        assert_eq!(
            scorer.score(&RouteContext::default(), &[0, 1], &[]),
            vec![0.0, 0.0]
        );
    }

    #[test]
    fn lmcache_scores_are_bounded_and_stale_is_neutral() {
        let scorer = LmCacheScorer::new(std::sync::Arc::new(FixedLmCache(Some(vec![1.5, -0.2]))));
        assert_eq!(
            scorer.score(&RouteContext::default(), &[0, 1], &[]),
            vec![1.0, 0.0]
        );
        let stale = LmCacheScorer::new(std::sync::Arc::new(FixedLmCache(None)));
        assert_eq!(
            stale.score(&RouteContext::default(), &[0, 1], &[]),
            vec![0.0, 0.0]
        );
    }

    #[test]
    fn fastest_prefers_lowest_latency() {
        let source = std::sync::Arc::new(FixedLatency(vec![300.0, 40.0, 900.0]));
        let p = Pipeline::fastest_stack(3, source);
        assert_eq!(p.name(), "fastest");
        assert_eq!(p.select(&RouteContext::default(), &[], |_| true), Some(1));
        // eligibility still filters: fastest surviving target wins
        assert_eq!(p.select(&RouteContext::default(), &[], |i| i != 1), Some(0));
    }

    #[test]
    fn fastest_cold_route_follows_load() {
        // no latency samples yet: the load tiebreaker decides alone
        let source = std::sync::Arc::new(FixedLatency(vec![0.0, 0.0]));
        let p = Pipeline::fastest_stack(2, source);
        assert_eq!(
            p.select(&RouteContext::default(), &[7, 0], |_| true),
            Some(1)
        );
    }

    #[test]
    fn fastest_unsampled_target_scores_neutral() {
        let source = std::sync::Arc::new(FixedLatency(vec![100.0, 0.0, 500.0]));
        let scorer = FastestScorer::new(3, source);
        let scores = scorer.score(&RouteContext::default(), &[0, 1, 2], &[]);
        assert_eq!(scores, vec![1.0, 0.5, 0.0]);
    }

    #[test]
    fn least_load_prefers_idle_target() {
        let p = Pipeline::new(3).with(Box::new(LeastLoadScorer::new(3)), 1.0);
        // target 1 carries the least in-flight load
        let got = p.select(&RouteContext::default(), &[10, 0, 7], |_| true);
        assert_eq!(got, Some(1));
    }

    #[test]
    fn prefix_cache_pins_repeated_prompt() {
        let scorer = Box::new(PrefixCacheScorer::new(2));
        let p = Pipeline::new(2).with(scorer, 1.0);
        let ctx = RouteContext {
            session_key: None,
            prompt: Some("a long shared system prompt then a question"),
            token_ids: None,
            adapter: None,
            ..Default::default()
        };
        // cold: prefix scores 0 for both, load scorer absent -> tie, any target ok
        let first = p.select(&ctx, &[], |_| true).unwrap();
        p.observe(first, &ctx);
        // warm: the served target now has the resident prefix and must win
        let second = p.select(&ctx, &[], |_| true).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn session_affinity_pins_to_last_target() {
        let scorer = Box::new(SessionAffinityScorer::new(3));
        let p = Pipeline::new(3).with(scorer, 1.0);
        let ctx = RouteContext {
            session_key: Some("user-1"),
            prompt: None,
            token_ids: None,
            adapter: None,
            ..Default::default()
        };
        // cold: no affinity, all candidates tie -> record whichever wins as served
        p.observe(2, &ctx);
        // warm: target 2 boosted -> it wins even though nothing else differs
        assert_eq!(p.select(&ctx, &[], |_| true), Some(2));
    }

    #[test]
    fn session_affinity_ignored_without_key() {
        let scorer = SessionAffinityScorer::new(2);
        let ctx = RouteContext::default();
        // no session key -> neutral zero scores, observe is a no-op
        scorer.observe(1, &ctx);
        assert_eq!(scorer.score(&ctx, &[0, 1], &[]), vec![0.0, 0.0]);
    }

    #[test]
    fn session_affinity_expires_after_ttl() {
        let scorer = SessionAffinityScorer::with_ttl(2, Duration::ZERO);
        let ctx = RouteContext {
            session_key: Some("s"),
            prompt: None,
            token_ids: None,
            adapter: None,
            ..Default::default()
        };
        scorer.observe(1, &ctx);
        // zero ttl -> entry is immediately stale, so no boost
        assert_eq!(scorer.score(&ctx, &[0, 1], &[]), vec![0.0, 0.0]);
    }

    #[test]
    fn session_affinity_skips_when_target_filtered_out() {
        let scorer = SessionAffinityScorer::new(3);
        let ctx = RouteContext {
            session_key: Some("s"),
            prompt: None,
            token_ids: None,
            adapter: None,
            ..Default::default()
        };
        scorer.observe(2, &ctx);
        // target 2 not in the candidate set -> no boost applied
        assert_eq!(scorer.score(&ctx, &[0, 1], &[]), vec![0.0, 0.0]);
    }

    fn with_adapter(adapter: &str) -> RouteContext<'_> {
        RouteContext {
            adapter: Some(adapter),
            ..Default::default()
        }
    }

    #[test]
    fn a_resident_adapter_outscores_a_cold_target() {
        let scorer = LoraScorer::new(3);
        scorer.observe(1, &with_adapter("summarizer"));
        assert_eq!(
            scorer.score(&with_adapter("summarizer"), &[0, 1, 2], &[]),
            vec![0.0, 1.0, 0.0]
        );
    }

    #[test]
    fn a_cold_adapter_scores_every_target_equally() {
        // neutral, not a penalty: an equal contribution cannot move the argmax,
        // so the rest of the pipeline decides instead of this scorer guessing
        let scorer = LoraScorer::new(3);
        scorer.observe(1, &with_adapter("summarizer"));
        assert_eq!(
            scorer.score(&with_adapter("translator"), &[0, 1, 2], &[]),
            vec![0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn a_request_without_an_adapter_is_inert() {
        let scorer = LoraScorer::new(2);
        scorer.observe(0, &with_adapter("summarizer"));
        assert_eq!(
            scorer.score(&RouteContext::default(), &[0, 1], &[]),
            vec![0.0, 0.0]
        );
        // and observing without an adapter records nothing
        scorer.observe(1, &RouteContext::default());
        assert!(!scorer.holds(1, "summarizer"));
    }

    #[test]
    fn residency_is_bounded_and_evicts_least_recently_used() {
        // the engine holds a bounded number of adapters (vLLM --max-loras);
        // claiming residency past that bound would confidently steer to a
        // target that has since evicted the adapter
        let scorer = LoraScorer::with_capacity(1, 2);
        for adapter in ["a", "b"] {
            scorer.observe(0, &with_adapter(adapter));
        }
        assert!(scorer.holds(0, "a") && scorer.holds(0, "b"));

        scorer.observe(0, &with_adapter("c"));
        assert!(!scorer.holds(0, "a"), "the oldest adapter must be evicted");
        assert!(scorer.holds(0, "b") && scorer.holds(0, "c"));
    }

    #[test]
    fn reuse_refreshes_an_adapter_against_eviction() {
        let scorer = LoraScorer::with_capacity(1, 2);
        scorer.observe(0, &with_adapter("a"));
        scorer.observe(0, &with_adapter("b"));
        // touching "a" again makes "b" the least recently used
        scorer.observe(0, &with_adapter("a"));
        scorer.observe(0, &with_adapter("c"));
        assert!(scorer.holds(0, "a"), "recently reused adapter was evicted");
        assert!(!scorer.holds(0, "b"));
    }

    #[test]
    fn repeated_adapter_use_does_not_consume_extra_slots() {
        let scorer = LoraScorer::with_capacity(1, 2);
        for _ in 0..10 {
            scorer.observe(0, &with_adapter("a"));
        }
        scorer.observe(0, &with_adapter("b"));
        // "a" served ten times must still occupy exactly one slot
        assert!(scorer.holds(0, "a") && scorer.holds(0, "b"));
    }

    #[test]
    fn the_lora_stack_pins_repeat_adapter_traffic_to_its_warm_target() {
        let stack = Pipeline::lora_stack(3);
        let ctx = with_adapter("summarizer");
        let first = stack.select(&ctx, &[], |_| true).unwrap();
        stack.observe(first, &ctx);
        // the warm target keeps winning while it is not overloaded
        for _ in 0..5 {
            let next = stack.select(&ctx, &[], |_| true).unwrap();
            assert_eq!(next, first, "adapter affinity did not hold");
            stack.observe(next, &ctx);
        }
    }

    #[test]
    fn adapter_affinity_outranks_prefix_affinity() {
        // the costs are not comparable: a cold prefix recomputes tokens, a cold
        // adapter can block decoding on an adapter load
        let stack = Pipeline::lora_stack(2);
        let warm_prefix = RouteContext {
            prompt: Some("a shared prompt prefix"),
            adapter: Some("translator"),
            ..Default::default()
        };
        // target 0 gets the prompt, target 1 gets the adapter
        stack.observe(
            0,
            &RouteContext {
                prompt: warm_prefix.prompt,
                ..Default::default()
            },
        );
        stack.observe(
            1,
            &RouteContext {
                adapter: warm_prefix.adapter,
                ..Default::default()
            },
        );
        assert_eq!(stack.select(&warm_prefix, &[], |_| true), Some(1));
    }

    #[test]
    fn adapter_affinity_pins_rather_than_spreading() {
        // worth pinning down explicitly: affinity is the *point*, so repeat
        // traffic for one adapter concentrates on one target instead of
        // spreading. That is why the gateway only sets an adapter when the
        // request addresses something other than the route's own model — on a
        // single-model route this behaviour would pin the entire route
        let stack = Pipeline::lora_stack(3);
        let ctx = with_adapter("only-adapter");
        let first = stack.select(&ctx, &[], |_| true).unwrap();
        stack.observe(first, &ctx);
        for _ in 0..4 {
            let next = stack.select(&ctx, &[], |_| true).unwrap();
            assert_eq!(next, first);
            stack.observe(next, &ctx);
        }
    }

    #[test]
    fn load_still_breaks_ties_once_several_targets_hold_the_adapter() {
        // a fleet where every target has the adapter must balance, not pin
        let stack = Pipeline::lora_stack(2);
        let ctx = with_adapter("shared");
        stack.observe(0, &ctx);
        stack.observe(1, &ctx);
        assert_eq!(stack.select(&ctx, &[9, 0], |_| true), Some(1));
        assert_eq!(stack.select(&ctx, &[0, 9], |_| true), Some(0));
    }

    #[test]
    fn an_unknown_target_index_is_ignored_rather_than_panicking() {
        let scorer = LoraScorer::new(2);
        scorer.observe(99, &with_adapter("a"));
        assert!(!scorer.holds(99, "a"));
        assert_eq!(
            scorer.score(&with_adapter("a"), &[0, 1], &[]),
            vec![0.0, 0.0]
        );
    }

    #[test]
    fn weighted_sum_blends_scorers() {
        // load says target 0 (idle); a strong static weight pulls toward target 1
        let p = Pipeline::new(2)
            .with(Box::new(LeastLoadScorer::new(2)), 1.0)
            .with(Box::new(StaticScorer::new(&[1, 100])), 5.0);
        // static contribution (5*1.0) dominates load's 1.0 swing -> target 1
        let got = p.select(&RouteContext::default(), &[0, 5], |_| true);
        assert_eq!(got, Some(1));
    }

    #[test]
    fn the_predicted_latency_stack_prefers_the_target_this_request_is_cheapest_on() {
        use crate::predictor::{Features, LatencyPredictor};

        let predictor = std::sync::Arc::new(LatencyPredictor::new(2));
        // target 0: queue-sensitive, cheap prefill. target 1: queue-insensitive,
        // expensive prefill. neither is "the fast one" — it depends on the request
        for i in 0..200u64 {
            let queue = i % 6;
            let tokens = ((i % 4) * 1000) as usize;
            predictor.observe(
                0,
                Features {
                    queue_depth: queue,
                    prompt_tokens: tokens,
                },
                10.0 + 100.0 * queue as f64,
            );
            predictor.observe(
                1,
                Features {
                    queue_depth: queue,
                    prompt_tokens: tokens,
                },
                10.0 + 300.0 * (tokens as f64 / 1000.0),
            );
        }
        let stack = Pipeline::predicted_latency_stack(2, predictor);

        // short prompt into deep queues: target 1 shrugs its queue off
        let short = RouteContext {
            prompt: Some("hi"),
            ..Default::default()
        };
        let picks = (0..40)
            .filter(|_| stack.pick(&short, &[5, 5]) == Some(1))
            .count();
        assert!(
            picks > 30,
            "short prompt should favour target 1, got {picks}/40"
        );

        // long prompt into idle queues: target 0 prefills far cheaper
        let long_prompt = "x".repeat(16_000);
        let long = RouteContext {
            prompt: Some(&long_prompt),
            ..Default::default()
        };
        let picks = (0..40)
            .filter(|_| stack.pick(&long, &[0, 0]) == Some(0))
            .count();
        assert!(
            picks > 30,
            "long prompt should favour target 0, got {picks}/40"
        );
    }

    #[test]
    fn a_cold_predictor_leaves_the_stack_behaving_like_least_load() {
        use crate::predictor::LatencyPredictor;

        let stack =
            Pipeline::predicted_latency_stack(3, std::sync::Arc::new(LatencyPredictor::new(3)));
        // no target predicts anything, so only the load scorer has an opinion
        let ctx = RouteContext::default();
        let picks = (0..40)
            .filter(|_| stack.pick(&ctx, &[9, 0, 9]) == Some(1))
            .count();
        assert!(picks > 30, "cold stack should follow load, got {picks}/40");
    }
}
