//! criterion benches for the prefix trie backing cache-aware affinity.
//!
//! `insert` runs on every observed request and `longest_prefix` on every pick
//! for prefix-scoring strategies, so both sit on the gateway hot path. we bench
//! them against a bounded trie (LRU eviction on) with realistic prompt-length
//! keys so eviction churn is part of the measurement.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rolter_balancer::trie::Trie;
use std::hint::black_box;

/// build a pool of distinct, prompt-like keys sharing common prefixes so the
/// trie exercises real branching rather than degenerate single paths
fn sample_keys(count: usize) -> Vec<String> {
    let prefixes = [
        "summarize the following document: ",
        "translate this text to french: ",
        "you are a helpful assistant. answer: ",
        "given the code below, find the bug: ",
    ];
    (0..count)
        .map(|i| {
            format!(
                "{}request payload number {i} with trailing entropy",
                prefixes[i % prefixes.len()]
            )
        })
        .collect()
}

fn bench_trie(c: &mut Criterion) {
    let keys = sample_keys(4096);

    // insert: cap the trie so most inserts also trigger LRU eviction, the
    // realistic steady state for a warm cache-aware route
    let mut group = c.benchmark_group("trie_insert");
    for cap in [0usize, 50_000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(if cap == 0 {
                "unbounded".into()
            } else {
                format!("cap_{cap}")
            }),
            &cap,
            |b, &cap| {
                b.iter_batched(
                    || Trie::with_capacity(cap),
                    |mut trie| {
                        for k in &keys {
                            trie.insert(black_box(k));
                        }
                        trie
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();

    // longest_prefix: query a warm trie with both resident and novel keys
    let mut trie = Trie::with_capacity(0);
    for k in &keys {
        trie.insert(k);
    }
    let queries = sample_keys(512);
    c.bench_function("trie_longest_prefix", |b| {
        b.iter(|| {
            let mut acc = 0usize;
            for q in &queries {
                acc += black_box(trie.longest_prefix(black_box(q)));
            }
            acc
        });
    });
}

criterion_group!(benches, bench_trie);
criterion_main!(benches);
