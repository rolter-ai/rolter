//! criterion benches for cross-dialect request translation.
//!
//! `translate_request` runs once per request on every route whose provider
//! speaks a different dialect than the caller, so it is squarely on the gateway
//! hot path. the gemini pairs are benched specifically because #882 turned
//! their content-part loops from an infallible `filter_map` into a fallible
//! `collect::<Result<Vec<_>>>`, and that has to stay free.

use bytes::Bytes;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rolter_core::{ProviderKind, RoleProfile};
use rolter_proxy::TranslationPlan;
use serde_json::json;
use std::hint::black_box;

/// a chat body with `turns` conversational turns, each carrying a text part and
/// (every third turn) an image part — the mixed-part shape that exercises the
/// per-part match rather than the string fast path
fn chat_body(turns: usize) -> Bytes {
    let messages: Vec<_> = (0..turns)
        .map(|i| {
            let mut content = vec![json!({
                "type": "text",
                "text": format!("turn {i}: summarize the attached material in two sentences")
            })];
            if i % 3 == 0 {
                content.push(json!({
                    "type": "image_url",
                    "image_url": {"url": "https://example.test/page.png"}
                }));
            }
            json!({
                "role": if i % 2 == 0 { "user" } else { "assistant" },
                "content": content
            })
        })
        .collect();
    Bytes::from(
        serde_json::to_vec(&json!({
            "model": "bench-model",
            "max_tokens": 256,
            "messages": messages
        }))
        .unwrap(),
    )
}

/// a body whose content is a plain string, the common single-turn shape — it
/// skips the part loop entirely and is the floor the mixed-part case is read
/// against
fn plain_body() -> Bytes {
    Bytes::from(
        serde_json::to_vec(&json!({
            "model": "bench-model",
            "max_tokens": 256,
            "messages": [{"role": "user", "content": "what is the capital of france?"}]
        }))
        .unwrap(),
    )
}

fn bench_translate(c: &mut Criterion) {
    let targets = [
        ("to_interactions", ProviderKind::GeminiInteractions),
        ("to_gemini_generate", ProviderKind::GeminiNative),
        ("to_anthropic", ProviderKind::Anthropic),
    ];

    let mut group = c.benchmark_group("translate_request_parts");
    for (label, kind) in targets {
        let plan = TranslationPlan::resolve("/v1/chat/completions", kind, RoleProfile::Openai);
        for turns in [1usize, 8, 64] {
            let body = chat_body(turns);
            group.bench_with_input(BenchmarkId::new(label, turns), &body, |b, body| {
                b.iter(|| black_box(plan.translate_request(black_box(body.clone())).unwrap()));
            });
        }
    }
    group.finish();

    let mut group = c.benchmark_group("translate_request_plain");
    let body = plain_body();
    for (label, kind) in targets {
        let plan = TranslationPlan::resolve("/v1/chat/completions", kind, RoleProfile::Openai);
        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| black_box(plan.translate_request(black_box(body.clone())).unwrap()));
        });
    }
    group.finish();
}

/// a plain body carrying parameters every non-openai target drops, so the
/// adjustment record is filled (#1586); read against `translate_request_plain`
fn adjusted_body() -> Bytes {
    Bytes::from(
        serde_json::to_vec(&json!({
            "model": "bench-model",
            "max_tokens": 256,
            "messages": [{"role": "user", "content": "what is the capital of france?"}],
            "presence_penalty": 0.5,
            "frequency_penalty": 0.5,
            "logprobs": true,
            "seed": 7
        }))
        .unwrap(),
    )
}

fn bench_adjusted(c: &mut Criterion) {
    let compat = rolter_core::CompatibilityConfig::default();
    let mut group = c.benchmark_group("translate_request_adjusted");
    let body = adjusted_body();
    for (label, kind) in [
        ("to_interactions", ProviderKind::GeminiInteractions),
        ("to_gemini_generate", ProviderKind::GeminiNative),
        ("to_anthropic", ProviderKind::Anthropic),
    ] {
        let plan = TranslationPlan::resolve("/v1/chat/completions", kind, RoleProfile::Openai);
        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| {
                black_box(
                    plan.translate_request_reporting(black_box(body.clone()), &compat)
                        .unwrap(),
                )
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_translate, bench_adjusted);
criterion_main!(benches);
