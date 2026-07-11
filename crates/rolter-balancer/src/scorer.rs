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
            if rand::random::<usize>().is_multiple_of(ties) {
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
        // known costs among the candidates bound the normalization window
        let known: Vec<f64> = candidates
            .iter()
            .filter_map(|&i| self.costs.get(i).copied())
            .filter(|&c| c > 0.0)
            .collect();
        let (min, max) = known
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &c| {
                (lo.min(c), hi.max(c))
            });
        if known.is_empty() || min >= max {
            return vec![1.0; candidates.len()];
        }
        candidates
            .iter()
            .map(|&i| match self.costs.get(i).copied() {
                Some(c) if c > 0.0 => (1.0 - (c - min) / (max - min)) as f32,
                _ => 0.5,
            })
            .collect()
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
        };
        scorer.observe(2, &ctx);
        // target 2 not in the candidate set -> no boost applied
        assert_eq!(scorer.score(&ctx, &[0, 1], &[]), vec![0.0, 0.0]);
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
}
