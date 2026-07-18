//! criterion benches for the balancer hot path: `LoadBalancer::pick`.
//!
//! `pick` runs once per proxied request, so its cost is pure gateway overhead.
//! we bench each built-in strategy across a realistic target-pool size with a
//! populated `RouteContext` (session key + prompt) so cache/hash strategies do
//! their real work rather than hitting an empty fast path.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rolter_balancer::{build, RouteContext};
use rolter_core::BalancingStrategy;
use std::hint::black_box;

/// strategies that only need a target count and weights (i.e. buildable via the
/// plain `build` factory without per-target stats)
const STRATEGIES: &[(&str, BalancingStrategy)] = &[
    ("round_robin", BalancingStrategy::RoundRobin),
    ("random", BalancingStrategy::Random),
    ("power_of_two", BalancingStrategy::PowerOfTwo),
    ("consistent_hash", BalancingStrategy::ConsistentHash),
    ("cache_aware", BalancingStrategy::CacheAware),
    ("weighted", BalancingStrategy::Weighted),
    ("pipeline", BalancingStrategy::Pipeline),
];

fn bench_pick(c: &mut Criterion) {
    // representative self-hosted pool size (e.g. ~24 vLLM replicas)
    let n = 24usize;
    let weights: Vec<u32> = (0..n).map(|i| 1 + (i as u32 % 4)).collect();
    // in-flight load snapshot the load-aware strategies rank on
    let loads: Vec<u64> = (0..n).map(|i| (i as u64 * 7) % 13).collect();
    let ctx = RouteContext {
        session_key: Some("user-42-session-abcdef0123456789"),
        prompt: Some(
            "summarize the following document for me in three concise bullet points: lorem \
             ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor",
        ),
        token_ids: None,
    };

    let mut group = c.benchmark_group("pick");
    for (label, strategy) in STRATEGIES {
        let lb = build(*strategy, &weights);
        // warm strategies that learn from traffic so the bench measures the
        // steady state, not a cold first pick
        for i in 0..n {
            lb.observe(i, &ctx);
        }
        group.bench_with_input(BenchmarkId::from_parameter(label), strategy, |b, _| {
            b.iter(|| black_box(lb.pick(black_box(&ctx), black_box(&loads))));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_pick);
criterion_main!(benches);
