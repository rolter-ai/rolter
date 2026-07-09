//! Pluggable load-balancing strategies for rolter routes.
//!
//! Every strategy implements [`LoadBalancer`]. The [`build`] factory turns a
//! [`BalancingStrategy`] from the config into a boxed balancer. New strategies
//! (precise KV-cache aware, lmcache aware, latency based, ...) only need to
//! implement the trait and be wired into [`build`].

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

use ahash::RandomState;
use parking_lot::Mutex;
use rolter_core::BalancingStrategy;

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
}

/// Build a boxed [`LoadBalancer`] from a configured strategy and the route's
/// per-target `weights` (index-aligned with the route targets). Strategies that
/// ignore weights only use `weights.len()` as the target count.
pub fn build(strategy: BalancingStrategy, weights: &[u32]) -> Box<dyn LoadBalancer> {
    let n = weights.len();
    match strategy {
        BalancingStrategy::RoundRobin => Box::new(RoundRobin::new(n)),
        BalancingStrategy::Random => Box::new(Random::new(n)),
        BalancingStrategy::PowerOfTwo => Box::new(PowerOfTwo::new(n)),
        BalancingStrategy::ConsistentHash => Box::new(ConsistentHash::new(n)),
        BalancingStrategy::CacheAware => Box::new(CacheAware::new(n, 0.5)),
        BalancingStrategy::Weighted => Box::new(WeightedRoundRobin::new(weights)),
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
        Some(rand::random::<usize>() % self.n)
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
        let a = rand::random::<usize>() % self.n;
        let mut b = rand::random::<usize>() % self.n;
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

impl ConsistentHash {
    pub fn new(n: usize) -> Self {
        // fixed seeds keep the ring and key hashing deterministic in-process
        let hasher = RandomState::with_seeds(0x1234, 0x5678, 0x9abc, 0xdef0);
        let mut ring = Vec::new();
        const VIRTUAL_NODES: usize = 100;
        for i in 0..n {
            for v in 0..VIRTUAL_NODES {
                ring.push((hasher.hash_one(format!("{i}#{v}")), i));
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
            tries.push(Mutex::new(Trie::default()));
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
    fn consistent_hash_is_stable() {
        let lb = ConsistentHash::new(4);
        let ctx = RouteContext {
            session_key: Some("user-1"),
            prompt: None,
        };
        let a = lb.pick(&ctx, &[]).unwrap();
        let b = lb.pick(&ctx, &[]).unwrap();
        assert_eq!(a, b);
        assert!(a < 4);
    }

    #[test]
    fn cache_aware_pins_repeated_prefix() {
        let lb = CacheAware::new(2, 0.5);
        let ctx = RouteContext {
            session_key: None,
            prompt: Some("a long shared system prompt followed by a question"),
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
}
