//! Adaptive routing (#544): a weighted blend of observed latency, catalog cost
//! and in-flight load that only takes over once it can justify itself.
//!
//! The strategy is deliberately conservative. Three independent conditions must
//! all hold before a single request is routed by the blend:
//!
//! 1. the operator turned it on ([`AdaptiveRoutingConfig::enabled`] — the kill
//!    switch),
//! 2. the blend has some non-zero weight to rank with, and
//! 3. the route has served `min_samples` requests *and* the dominant signal has
//!    real evidence (at least two targets with a latency sample).
//!
//! Otherwise every pick goes to the same `pipeline` stack the route would have
//! used without the strategy, so switching a route to `adaptive` never moves
//! traffic on its own. When the blend is engaged, a bounded `exploration_ratio`
//! share of picks is made uniformly at random so a target the blend has learned
//! to avoid keeps producing fresh latency samples instead of going dark.
//!
//! Condition 3 cannot be reached by waiting alone (#1645). The fallback stack
//! is free to send every pick to one target — configured weight, session
//! affinity or prefix affinity all do this legitimately — and the peer it
//! skips then never earns the latency sample the evidence check is counting,
//! so the route stays on the fallback forever and `adaptive` is silently equal
//! to the strategy it replaced. A bounded warm-up therefore runs *before*
//! engagement: while the evidence is thin, up to `MAX_WARMUP_PROBES` picks
//! per target are steered to the targets that have no latency sample yet. The
//! budget is what keeps an unreachable target from swallowing the route — it
//! costs at most `MAX_WARMUP_PROBES` picks in total, not a permanent share.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Instant;

use rand::RngExt;
use rolter_core::AdaptiveRoutingConfig;

use crate::scorer::{
    CheapestScorer, FastestScorer, LatencySource, LeastLoadScorer, Pipeline, Scorer,
};
use crate::{AdaptiveTelemetry, DecisionCounts, LoadBalancer, RouteContext, TargetTelemetry};

/// Latency samples needed before the blend is trusted to rank targets: ranking
/// is relative, so a single sampled target says nothing about the others.
const MIN_LATENCY_SAMPLES: usize = 2;

/// Warm-up picks a single target may be given before the route gives up on
/// sampling it and leaves it to the fallback stack.
///
/// A latency sample lands only when the request *finishes*, so a budget of one
/// or two would be spent on picks that are still in flight and the warm-up
/// would give up on a healthy target under any concurrency. Eight is enough to
/// outlast that lag on a busy route while keeping the cost of a target that
/// never answers bounded and small.
const MAX_WARMUP_PROBES: u64 = 8;

pub struct Adaptive {
    cfg: AdaptiveRoutingConfig,
    /// route width
    n: usize,
    /// the weighted latency/cost/load blend
    blend: Pipeline,
    /// the deterministic stack served while the blend is not engaged
    fallback: Pipeline,
    /// live latency handle, also used as the evidence check
    latency: Option<Arc<dyn LatencySource>>,
    /// requests this route has observed since the balancer was built
    served: AtomicU64,
    /// picks routed by the blend, by exploration, and by the fallback stack,
    /// exported as routing telemetry so an operator can see what adaptive
    /// routing is actually doing before and after they enable it
    blend_picks: AtomicU64,
    exploration_picks: AtomicU64,
    fallback_picks: AtomicU64,
    /// whether the last pick found the blend engaged
    engaged_now: AtomicBool,
    /// catalog price per target, kept so telemetry can report the raw signal
    /// next to the score derived from it
    costs: Vec<f64>,
    /// picks each target has served, index-aligned with the route targets
    target_samples: Vec<AtomicU64>,
    /// warm-up picks already spent on each target, bounded by
    /// `MAX_WARMUP_PROBES` so an unreachable target cannot absorb the route
    warmup_probes: Vec<AtomicU64>,
    /// milliseconds since [`Adaptive::built`] at each target's last pick;
    /// meaningless until that target's sample count is non-zero
    target_last_seen: Vec<AtomicU64>,
    /// baseline for the ages above. Monotonic, so telemetry stays honest
    /// across a wall-clock adjustment
    built: Instant,
}

impl Adaptive {
    /// Build over a route's `weights`, per-target `costs` (`<= 0` = unknown,
    /// see [`CheapestScorer`]) and optional live latency handle.
    pub fn new(
        weights: &[u32],
        costs: &[f64],
        latency: Option<Arc<dyn LatencySource>>,
        cfg: &AdaptiveRoutingConfig,
    ) -> Self {
        let cfg = cfg.sanitized();
        let n = weights.len();
        let mut costs = costs.to_vec();
        costs.resize(n, 0.0);
        let mut blend = Pipeline::new(n).named("adaptive");
        if let (Some(source), true) = (latency.as_ref(), cfg.latency_weight > 0.0) {
            blend = blend.with(
                Box::new(FastestScorer::new(n, source.clone())) as Box<dyn Scorer>,
                cfg.latency_weight,
            );
        }
        if cfg.cost_weight > 0.0 {
            blend = blend.with(Box::new(CheapestScorer::new(&costs)), cfg.cost_weight);
        }
        if cfg.load_weight > 0.0 {
            blend = blend.with(Box::new(LeastLoadScorer::new(n)), cfg.load_weight);
        }
        Self {
            cfg,
            n,
            blend,
            fallback: Pipeline::default_stack(weights),
            latency,
            served: AtomicU64::new(0),
            blend_picks: AtomicU64::new(0),
            exploration_picks: AtomicU64::new(0),
            fallback_picks: AtomicU64::new(0),
            engaged_now: AtomicBool::new(false),
            costs,
            target_samples: (0..n).map(|_| AtomicU64::new(0)).collect(),
            target_last_seen: (0..n).map(|_| AtomicU64::new(0)).collect(),
            warmup_probes: (0..n).map(|_| AtomicU64::new(0)).collect(),
            built: Instant::now(),
        }
    }

    /// Per-target signals as the blend currently sees them (#751).
    ///
    /// Sampled, not recorded: everything here is read off live state at call
    /// time, so nothing is written on the request path beyond the two atomics
    /// [`LoadBalancer::observe`] already touches. `loads` is the same
    /// in-flight slice [`LoadBalancer::pick`] takes.
    pub fn telemetry(&self, loads: &[u64]) -> AdaptiveTelemetry {
        // the blend's scorers rank on latency/cost/load only — none of them
        // reads the request context — so a default one describes the route
        let ctx = RouteContext::default();
        let components = self.blend.components(&ctx, loads);
        let elapsed = self.built.elapsed().as_millis() as u64;
        let targets = (0..self.n)
            .map(|i| {
                let mut target = TargetTelemetry {
                    target: i,
                    latency_ms: self
                        .latency
                        .as_ref()
                        .and_then(|source| source.latencies(self.n).get(i).copied())
                        .unwrap_or_default(),
                    cost_per_mtok: self.costs.get(i).copied().unwrap_or_default(),
                    in_flight: loads.get(i).copied().unwrap_or_default(),
                    samples: self.target_samples[i].load(Relaxed),
                    ..Default::default()
                };
                if target.samples > 0 {
                    // saturating: `elapsed` is read after the timestamp, but a
                    // concurrent `observe` can still land in between
                    target.last_sample_age_ms =
                        Some(elapsed.saturating_sub(self.target_last_seen[i].load(Relaxed)));
                }
                for component in &components {
                    let score = component.scores.get(i).copied().unwrap_or_default();
                    target.score += component.weight * score;
                    match component.name {
                        "fastest" => target.latency_score = score,
                        "cheapest" => target.cost_score = score,
                        "least_load" => target.load_score = score,
                        _ => {}
                    }
                }
                target
            })
            .collect();
        AdaptiveTelemetry {
            engaged: self.engaged(),
            observed: self.served.load(Relaxed),
            decisions: self.decision_counts(),
            policy: self.cfg.clone(),
            targets,
        }
    }

    fn decision_counts(&self) -> DecisionCounts {
        DecisionCounts {
            blend: self.blend_picks.load(Relaxed),
            exploration: self.exploration_picks.load(Relaxed),
            fallback: self.fallback_picks.load(Relaxed),
            engaged: self.engaged_now.load(Relaxed),
        }
    }

    /// Whether the blend may route this request.
    fn engaged(&self) -> bool {
        self.cfg.enabled
            && self.cfg.has_signal()
            && self.served.load(Relaxed) >= u64::from(self.cfg.min_samples)
            && self.evidence_ready()
    }

    /// Whether the dominant signal can actually separate two targets. Cost is a
    /// build-time constant and load is always readable, so only the latency
    /// signal — the one that needs traffic to exist — is checked here.
    fn evidence_ready(&self) -> bool {
        if self.cfg.latency_weight <= 0.0 {
            return true;
        }
        let Some(source) = &self.latency else {
            return false;
        };
        source
            .latencies(self.n)
            .iter()
            .filter(|v| **v > 0.0)
            .count()
            >= MIN_LATENCY_SAMPLES
    }

    /// A target to spend this pre-engagement pick on so the blend can ever
    /// gather the evidence [`Adaptive::evidence_ready`] demands, or `None` to
    /// leave the pick to the fallback stack.
    ///
    /// Returns the least-probed target that still has no latency sample, so a
    /// wide route warms its targets evenly instead of draining one budget at a
    /// time. Only reached while the blend is disengaged, and only until the
    /// evidence is in, so it costs the hot path nothing in steady state.
    fn warmup_probe(&self) -> Option<usize> {
        if !self.cfg.enabled || !self.cfg.has_signal() || self.cfg.latency_weight <= 0.0 {
            return None;
        }
        let latencies = self.latency.as_ref()?.latencies(self.n);
        if latencies.iter().filter(|v| **v > 0.0).count() >= MIN_LATENCY_SAMPLES {
            // disengaged for some other reason (min_samples, say) — the
            // evidence is already there, so steering traffic buys nothing
            return None;
        }
        let candidate = latencies
            .iter()
            .enumerate()
            .filter(|(_, ms)| **ms <= 0.0)
            .filter_map(|(i, _)| {
                let spent = self.warmup_probes.get(i)?.load(Relaxed);
                (spent < MAX_WARMUP_PROBES).then_some((spent, i))
            })
            .min()?;
        let (_, target) = candidate;
        if let Some(spent) = self.warmup_probes.get(target) {
            spent.fetch_add(1, Relaxed);
        }
        Some(target)
    }

    /// Whether this pick is spent on exploration rather than exploitation.
    fn explore(&self) -> bool {
        self.cfg.exploration_ratio > 0.0
            && self.n > 1
            && rand::rng().random::<f32>() < self.cfg.exploration_ratio
    }
}

impl LoadBalancer for Adaptive {
    fn name(&self) -> &'static str {
        "adaptive"
    }

    fn pick(&self, ctx: &RouteContext, loads: &[u64]) -> Option<usize> {
        let engaged = self.engaged();
        self.engaged_now.store(engaged, Relaxed);
        if !engaged {
            // a warm-up probe is exploration in the same sense as the ratio
            // below — a pick spent on evidence rather than on the best target
            if let Some(target) = self.warmup_probe() {
                self.exploration_picks.fetch_add(1, Relaxed);
                return Some(target);
            }
            self.fallback_picks.fetch_add(1, Relaxed);
            return self.fallback.pick(ctx, loads);
        }
        if self.explore() {
            self.exploration_picks.fetch_add(1, Relaxed);
            return Some(rand::rng().random_range(0..self.n));
        }
        self.blend_picks.fetch_add(1, Relaxed);
        self.blend.pick(ctx, loads)
    }

    fn decisions(&self) -> Option<DecisionCounts> {
        Some(self.decision_counts())
    }

    fn telemetry(&self, loads: &[u64]) -> Option<AdaptiveTelemetry> {
        Some(Adaptive::telemetry(self, loads))
    }

    fn observe(&self, target: usize, ctx: &RouteContext) {
        self.served.fetch_add(1, Relaxed);
        // per-target attribution for telemetry: two relaxed atomics on a path
        // that already walks the scorer stack
        if let (Some(samples), Some(last_seen)) = (
            self.target_samples.get(target),
            self.target_last_seen.get(target),
        ) {
            samples.fetch_add(1, Relaxed);
            last_seen.store(self.built.elapsed().as_millis() as u64, Relaxed);
        }
        // the fallback stack keeps learning while the blend is engaged, so a
        // config change that disengages adaptive routing lands on a warm
        // session/prefix cache rather than a cold one
        self.fallback.observe(target, ctx);
        self.blend.observe(target, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedLatency(Vec<f64>);

    impl LatencySource for FixedLatency {
        fn latencies(&self, n: usize) -> Vec<f64> {
            let mut out = self.0.clone();
            out.resize(n, 0.0);
            out
        }
    }

    fn cfg(enabled: bool, min_samples: u32) -> AdaptiveRoutingConfig {
        AdaptiveRoutingConfig {
            enabled,
            exploration_ratio: 0.0,
            min_samples,
            ..Default::default()
        }
    }

    fn warm(lb: &Adaptive, picks: u32) {
        for _ in 0..picks {
            lb.observe(0, &RouteContext::default());
        }
    }

    /// target 1 is both faster and cheaper, so an engaged blend always picks it
    fn lopsided(cfg: &AdaptiveRoutingConfig) -> Adaptive {
        Adaptive::new(
            &[1, 1],
            &[10.0, 1.0],
            Some(Arc::new(FixedLatency(vec![500.0, 10.0]))),
            cfg,
        )
    }

    /// A latency source that only knows a target after it has served a
    /// request, which is what a real one does.
    #[derive(Default)]
    struct WarmingLatency(Vec<AtomicU64>);

    impl WarmingLatency {
        fn new(n: usize) -> Self {
            Self((0..n).map(|_| AtomicU64::new(0)).collect())
        }

        fn record(&self, target: usize, ms: u64) {
            if let Some(slot) = self.0.get(target) {
                slot.store(ms, Relaxed);
            }
        }
    }

    impl LatencySource for WarmingLatency {
        fn latencies(&self, n: usize) -> Vec<f64> {
            let mut out: Vec<f64> = self.0.iter().map(|v| v.load(Relaxed) as f64).collect();
            out.resize(n, 0.0);
            out
        }
    }

    /// #1645: the fallback stack can pin every pick to one target, and the
    /// unsampled peer then never earns the latency sample `evidence_ready`
    /// needs — so the blend could never engage on its own.
    #[test]
    fn warm_up_samples_every_target_when_the_fallback_pins_one() {
        let latency = Arc::new(WarmingLatency::new(2));
        // target 0 outweighs target 1 by 100x, so every fallback pick is 0
        let lb = Adaptive::new(
            &[100, 1],
            &[1.0, 1.0],
            Some(latency.clone() as Arc<dyn LatencySource>),
            &cfg(true, 5),
        );
        let mut picks = [0usize; 2];
        for _ in 0..50 {
            let Some(target) = lb.pick(&RouteContext::default(), &[]) else {
                panic!("adaptive returned no target");
            };
            picks[target] += 1;
            lb.observe(target, &RouteContext::default());
            latency.record(target, if target == 0 { 3300 } else { 200 });
        }
        assert!(
            picks[1] > 0,
            "the unsampled target never received a warm-up probe: {picks:?}"
        );
        assert!(
            lb.engaged(),
            "adaptive never engaged after 50 requests: {:?}",
            lb.decision_counts()
        );
    }

    /// The warm-up must not become a permanent tax on a route whose target is
    /// simply gone: a target that never answers never earns a latency sample,
    /// so only the probe budget stops it absorbing every pick.
    #[test]
    fn an_unreachable_target_costs_a_bounded_number_of_probes() {
        let latency = Arc::new(WarmingLatency::new(2));
        latency.record(0, 200);
        let lb = Adaptive::new(
            &[1, 1],
            &[1.0, 1.0],
            Some(latency.clone() as Arc<dyn LatencySource>),
            &cfg(true, 1_000),
        );
        // target 1 is dead: picked, but never reports a latency. The fallback
        // stack still sends it its share, so only the warm-up picks are
        // counted here — those are the ones the route steers deliberately
        for _ in 0..200 {
            lb.pick(&RouteContext::default(), &[]);
            lb.observe(0, &RouteContext::default());
        }
        let probes = lb.decision_counts().exploration;
        assert!(
            probes <= MAX_WARMUP_PROBES,
            "a dead target absorbed {probes} warm-up picks, budget is {MAX_WARMUP_PROBES}"
        );
        assert_eq!(probes, MAX_WARMUP_PROBES, "the budget was not spent at all");
    }

    /// The kill switch outranks the warm-up: a disabled policy must move no
    /// traffic at all, evidence or no evidence.
    #[test]
    fn a_disabled_policy_never_probes() {
        let latency = Arc::new(WarmingLatency::new(2));
        let lb = Adaptive::new(
            &[100, 1],
            &[1.0, 1.0],
            Some(latency as Arc<dyn LatencySource>),
            &cfg(false, 0),
        );
        for _ in 0..50 {
            assert_eq!(lb.pick(&RouteContext::default(), &[]), Some(0));
        }
        assert_eq!(lb.decision_counts().exploration, 0);
    }

    #[test]
    fn kill_switch_keeps_traffic_on_the_fallback_stack() {
        let lb = lopsided(&cfg(false, 0));
        warm(&lb, 100);
        // the fallback stack is weight+load+prefix based and both targets are
        // identical there, so the disabled blend cannot pin traffic to target 1
        let mut picks = [0usize; 2];
        for _ in 0..200 {
            picks[lb.pick(&RouteContext::default(), &[]).unwrap()] += 1;
        }
        assert!(
            picks[0] > 0,
            "disabled adaptive routing still shifted all traffic: {picks:?}"
        );
    }

    #[test]
    fn engages_only_after_min_samples() {
        let lb = lopsided(&cfg(true, 10));
        assert!(!lb.engaged());
        warm(&lb, 9);
        assert!(!lb.engaged());
        warm(&lb, 1);
        assert!(lb.engaged());
        for _ in 0..50 {
            assert_eq!(lb.pick(&RouteContext::default(), &[]), Some(1));
        }
    }

    #[test]
    fn thin_latency_evidence_falls_back_deterministically() {
        // only one target has ever been sampled: nothing to rank against
        let lb = Adaptive::new(
            &[1, 1],
            &[10.0, 1.0],
            Some(Arc::new(FixedLatency(vec![500.0, 0.0]))),
            &cfg(true, 0),
        );
        assert!(!lb.engaged());
        // and with no latency handle at all the blend never engages either
        let blind = Adaptive::new(&[1, 1], &[10.0, 1.0], None, &cfg(true, 0));
        assert!(!blind.engaged());
    }

    #[test]
    fn cost_only_blend_needs_no_latency_evidence() {
        let policy = AdaptiveRoutingConfig {
            enabled: true,
            latency_weight: 0.0,
            load_weight: 0.0,
            exploration_ratio: 0.0,
            min_samples: 0,
            ..Default::default()
        };
        let lb = Adaptive::new(&[1, 1], &[10.0, 1.0], None, &policy);
        assert!(lb.engaged());
        assert_eq!(lb.pick(&RouteContext::default(), &[]), Some(1));
    }

    #[test]
    fn all_zero_weights_are_not_a_random_balancer() {
        let policy = AdaptiveRoutingConfig {
            enabled: true,
            latency_weight: 0.0,
            cost_weight: 0.0,
            load_weight: 0.0,
            min_samples: 0,
            ..Default::default()
        };
        let lb = lopsided(&policy);
        assert!(!lb.engaged());
    }

    #[test]
    fn exploration_ratio_is_bounded() {
        let policy = AdaptiveRoutingConfig {
            enabled: true,
            exploration_ratio: 9.0,
            min_samples: 0,
            ..Default::default()
        };
        let lb = lopsided(&policy);
        assert_eq!(lb.cfg.exploration_ratio, rolter_core::MAX_EXPLORATION_RATIO);
        // an engaged blend with capped exploration still favours the good
        // target by a wide margin
        let mut best = 0;
        for _ in 0..400 {
            if lb.pick(&RouteContext::default(), &[]) == Some(1) {
                best += 1;
            }
        }
        assert!(best > 250, "exploration swamped exploitation: {best}/400");
    }

    #[test]
    fn decision_counters_split_fallback_from_blend_and_exploration() {
        let lb = lopsided(&cfg(true, 5));
        // below min_samples every pick is served by the fallback stack
        for _ in 0..3 {
            lb.pick(&RouteContext::default(), &[]);
        }
        let counts = lb.decisions().unwrap();
        assert_eq!(
            (counts.blend, counts.exploration, counts.fallback),
            (0, 0, 3)
        );
        assert!(!counts.engaged);

        warm(&lb, 5);
        for _ in 0..10 {
            lb.pick(&RouteContext::default(), &[]);
        }
        let counts = lb.decisions().unwrap();
        assert!(counts.engaged);
        // exploration is off in this config, so every engaged pick is a blend one
        assert_eq!(counts.blend, 10);
        assert_eq!(counts.exploration, 0);
        assert_eq!(counts.fallback, 3);
    }

    #[test]
    fn exploration_picks_are_counted_separately() {
        let policy = AdaptiveRoutingConfig {
            enabled: true,
            exploration_ratio: rolter_core::MAX_EXPLORATION_RATIO,
            min_samples: 0,
            ..Default::default()
        };
        let lb = lopsided(&policy);
        for _ in 0..400 {
            lb.pick(&RouteContext::default(), &[]);
        }
        let counts = lb.decisions().unwrap();
        assert_eq!(counts.fallback, 0);
        assert_eq!(counts.blend + counts.exploration, 400);
        assert!(
            counts.exploration > 0 && counts.blend > 0,
            "expected both modes to be exercised: {counts:?}"
        );
    }

    #[test]
    fn telemetry_reports_the_signals_behind_the_ranking() {
        let lb = lopsided(&cfg(true, 0));
        let telemetry = lb.telemetry(&[3, 1]);
        assert_eq!(telemetry.targets.len(), 2);
        let (slow, fast) = (&telemetry.targets[0], &telemetry.targets[1]);
        // raw observations travel with the scores they produced
        assert_eq!((slow.latency_ms, fast.latency_ms), (500.0, 10.0));
        assert_eq!((slow.cost_per_mtok, fast.cost_per_mtok), (10.0, 1.0));
        assert_eq!((slow.in_flight, fast.in_flight), (3, 1));
        // and the cheaper, faster, less loaded target outranks the other on
        // every component as well as on the blend
        assert!(fast.latency_score > slow.latency_score);
        assert!(fast.cost_score > slow.cost_score);
        assert!(fast.load_score > slow.load_score);
        assert!(fast.score > slow.score);
        // the reported policy is the sanitized one the balancer actually runs
        assert_eq!(telemetry.policy, lb.cfg);
    }

    #[test]
    fn telemetry_attributes_samples_to_the_target_that_served_them() {
        let lb = lopsided(&cfg(true, 0));
        // a target nothing has been routed to has no sample and no age
        let cold = lb.telemetry(&[]);
        assert!(cold.targets.iter().all(|t| t.samples == 0));
        assert!(cold.targets.iter().all(|t| t.last_sample_age_ms.is_none()));
        assert_eq!(cold.observed, 0);

        for _ in 0..3 {
            lb.observe(1, &RouteContext::default());
        }
        lb.observe(0, &RouteContext::default());
        let warm = lb.telemetry(&[]);
        assert_eq!(warm.observed, 4);
        assert_eq!(warm.targets[0].samples, 1);
        assert_eq!(warm.targets[1].samples, 3);
        assert!(warm.targets.iter().all(|t| t.last_sample_age_ms.is_some()));
    }

    #[test]
    fn telemetry_carries_the_decision_split_and_engagement() {
        let lb = lopsided(&cfg(true, 5));
        for _ in 0..3 {
            lb.pick(&RouteContext::default(), &[]);
        }
        // still warming up: the fallback stack is serving and the blend is off
        let cold = lb.telemetry(&[]);
        assert!(!cold.engaged);
        assert_eq!(cold.decisions.fallback, 3);

        warm(&lb, 5);
        lb.pick(&RouteContext::default(), &[]);
        let hot = lb.telemetry(&[]);
        assert!(hot.engaged);
        assert_eq!(hot.decisions.blend, 1);
    }

    #[test]
    fn a_zero_weight_signal_scores_zero_in_the_blend() {
        let policy = AdaptiveRoutingConfig {
            enabled: true,
            latency_weight: 0.0,
            load_weight: 0.0,
            min_samples: 0,
            ..Default::default()
        };
        let telemetry = lopsided(&policy).telemetry(&[9, 0]);
        // a signal carrying no weight is not built into the blend at all, so
        // it reports zero rather than a score nothing acts on
        assert!(telemetry
            .targets
            .iter()
            .all(|t| t.latency_score == 0.0 && t.load_score == 0.0));
        assert!(telemetry.targets[1].cost_score > telemetry.targets[0].cost_score);
        // ...while the raw signals are still reported, because the operator
        // may be deciding whether to give them weight
        assert_eq!(telemetry.targets[0].latency_ms, 500.0);
        assert_eq!(telemetry.targets[0].in_flight, 9);
    }

    #[test]
    fn negative_weights_are_clamped() {
        let policy = AdaptiveRoutingConfig {
            enabled: true,
            cost_weight: -3.0,
            exploration_ratio: -1.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(policy.cost_weight, 0.0);
        assert_eq!(policy.exploration_ratio, 0.0);
    }
}
