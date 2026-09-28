//! criterion benches for the CPU side of config-snapshot generation (#845).
//!
//! `/internal/snapshot` is polled by every gateway in the fleet, and its
//! latency *is* config-propagation delay. The database read dominates a cold
//! build, but `sanitize_for_snapshot`, `validate` and the JSON encode are the
//! parts rolter owns and the parts that scale with tenant size — a deployment
//! with a thousand routes pays them on every poll of every gateway.
//!
//! These are the three stages the `snapshot.sanitize` / `snapshot.encode` spans
//! now cover, benched here so a regression in them is visible without standing
//! up a database.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rolter_core::{BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, Target};
use std::hint::black_box;

fn provider(i: usize) -> ProviderConfig {
    ProviderConfig {
        name: format!("provider-{i}"),
        slug: Some(format!("provider-{i}")),
        api_base: format!("https://provider-{i}.example.test/v1"),
        api_key_env: Some(format!("PROVIDER_{i}_KEY")),
        ..Default::default()
    }
}

/// A config shaped like a real multi-tenant deployment: every route fans out
/// over several providers, which is what makes `sanitize_for_snapshot`'s
/// per-target provider lookup do real work.
fn config(routes: usize, providers: usize) -> GatewayConfig {
    let built_providers = (0..providers).map(provider).collect();
    let built_routes = (0..routes)
        .map(|r| ModelRoute {
            model: format!("model-{r}"),
            strategy: BalancingStrategy::RoundRobin,
            targets: (0..3)
                .map(|t| Target {
                    provider: format!("provider-{}", (r + t) % providers),
                    model: Some(format!("upstream-model-{r}")),
                    weight: 1,
                })
                .collect(),
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            tenancy: None,
        })
        .collect();
    GatewayConfig {
        providers: built_providers,
        routes: built_routes,
        ..Default::default()
    }
}

fn bench_snapshot(c: &mut Criterion) {
    // 10 routes is a small deployment, 1000 a large multi-tenant one; the
    // question these answer is whether any stage is worse than linear
    let sizes = [(10usize, 4usize), (100, 16), (1000, 64)];

    let mut group = c.benchmark_group("snapshot_sanitize");
    for (routes, providers) in sizes {
        let base = config(routes, providers);
        group.bench_with_input(BenchmarkId::from_parameter(routes), &base, |b, base| {
            b.iter_batched(
                || base.clone(),
                |mut config| black_box(config.sanitize_for_snapshot()),
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();

    let mut group = c.benchmark_group("snapshot_validate");
    for (routes, providers) in sizes {
        let config = config(routes, providers);
        group.bench_with_input(BenchmarkId::from_parameter(routes), &config, |b, config| {
            b.iter(|| black_box(config.validate().is_ok()));
        });
    }
    group.finish();

    // the encode is what every gateway transfers on every poll, and the stage
    // #845 made measurable by serializing explicitly rather than via `Json`
    let mut group = c.benchmark_group("snapshot_encode");
    for (routes, providers) in sizes {
        let config = config(routes, providers);
        group.bench_with_input(BenchmarkId::from_parameter(routes), &config, |b, config| {
            b.iter(|| {
                black_box(
                    serde_json::to_vec(&serde_json::json!({"version": 1, "config": config}))
                        .unwrap()
                        .len(),
                )
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_snapshot);
criterion_main!(benches);
