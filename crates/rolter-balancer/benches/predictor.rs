//! What predicted-latency scheduling costs on the request path (#853).
//!
//! Two things run per request: `observe` folds a completed request into one
//! target's model, and `predict` reads every target's model to rank them. Both
//! take the same mutex, so the numbers that matter are the per-call cost and how
//! it scales with route width.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rolter_balancer::predictor::{Features, LatencyPredictor};
use rolter_balancer::scorer::Pipeline;
use rolter_balancer::{LoadBalancer, RouteContext};
use std::hint::black_box;
use std::sync::Arc;

/// A predictor whose models are all past `MIN_SAMPLES`, i.e. the state a route
/// spends essentially all of its life in.
fn warm(n: usize) -> Arc<LatencyPredictor> {
    let p = Arc::new(LatencyPredictor::new(n));
    for target in 0..n {
        for i in 0..64 {
            p.observe(
                target,
                Features {
                    queue_depth: (i % 8) as u64,
                    prompt_tokens: (i % 4) * 700,
                },
                40.0 + 12.0 * (i % 8) as f64,
            );
        }
    }
    p
}

fn bench_observe(c: &mut Criterion) {
    let p = warm(8);
    c.bench_function("predictor/observe", |b| {
        b.iter(|| {
            p.observe(
                black_box(3),
                black_box(Features {
                    queue_depth: 5,
                    prompt_tokens: 1200,
                }),
                black_box(87.0),
            )
        })
    });
}

fn bench_predict(c: &mut Criterion) {
    let mut group = c.benchmark_group("predictor/predict");
    for n in [2_usize, 8, 32] {
        let p = warm(n);
        let loads: Vec<u64> = (0..n as u64).collect();
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| black_box(p.predict(n, black_box(&loads), black_box(1500))))
        });
    }
    group.finish();
}

/// The number that actually gates adoption: a full `pick` through the
/// predicted-latency stack, against the `fastest` stack it would replace.
fn bench_pick(c: &mut Criterion) {
    let mut group = c.benchmark_group("predictor/pick");
    let prompt = "summarize the following incident report: ".repeat(40);
    for n in [2_usize, 8, 32] {
        let stack = Pipeline::predicted_latency_stack(n, warm(n));
        let loads: Vec<u64> = (0..n as u64).collect();
        let ctx = RouteContext::new().with_prompt(Some(&prompt));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(stack.pick(black_box(&ctx), black_box(&loads))))
        });
    }
    group.finish();
}

criterion_group!(benches, bench_observe, bench_predict, bench_pick);
criterion_main!(benches);
