//! The `upstream.request` span of every kind of attempt (#3535).
//!
//! Each dialled attempt opens one OTel GenAI span, child of the gateway's
//! request span, and injects its trace context from inside it, so the
//! provider call parents to the gateway rather than to the caller (#805).
//! Before #3535 only a JSON attempt on a route's classic pool did: the
//! weighted-variant chain and every audio upload sent the caller's trace
//! headers as they came and exported no GenAI span. These tests install an
//! in-memory OTLP pipeline, send a request under a trace id they own, and
//! check the span the gateway exported against the `traceparent` the mock
//! provider received.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::telemetry::test_pipeline::{self, ExportedSpan};
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target, Variant,
};
use serde_json::{json, Value};

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// What the mock provider saw of each call: the `traceparent` it was sent and
/// the model the call named.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<(String, String)>>>);

impl Seen {
    fn traceparent(headers: &HeaderMap) -> String {
        headers
            .get("traceparent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    async fn serve(&self) -> SocketAddr {
        async fn chat(
            State(seen): State<Seen>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            let model = body["model"].as_str().unwrap_or_default().to_string();
            seen.0
                .lock()
                .push((Seen::traceparent(&headers), model.clone()));
            Json(json!({
                "id": "chatcmpl-1", "object": "chat.completion", "model": model,
                "choices": [{"index": 0, "finish_reason": "stop",
                    "message": {"role": "assistant", "content": "hello"}}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 1, "total_tokens": 4}
            }))
        }
        async fn transcribe(State(seen): State<Seen>, headers: HeaderMap) -> Json<Value> {
            // the form goes as it came, so it names the caller's model
            seen.0
                .lock()
                .push((Seen::traceparent(&headers), String::new()));
            Json(json!({"text": "hello"}))
        }
        serve(
            Router::new()
                .route("/v1/chat/completions", post(chat))
                .route("/v1/audio/transcriptions", post(transcribe))
                .with_state(self.clone()),
        )
        .await
    }

    fn only(&self) -> (String, String) {
        let seen = self.0.lock();
        assert_eq!(seen.len(), 1, "one attempt: {seen:?}");
        seen[0].clone()
    }
}

fn target(model: &str) -> Target {
    Target {
        provider: "up".into(),
        model: Some(model.into()),
        ..Default::default()
    }
}

fn route(model: &str, targets: Vec<Target>, variants: Vec<Variant>) -> ModelRoute {
    ModelRoute {
        model: model.into(),
        strategy: BalancingStrategy::RoundRobin,
        targets,
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants,
        tenancy: None,
    }
}

/// A classic route `classic` and a route `variants` whose one variant holds
/// one target, so which target an attempt dials is known in advance.
fn config(up: SocketAddr, upstream_model: &str) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "up".into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{up}"),
        ..Default::default()
    });
    config
        .routes
        .push(route("classic", vec![target(upstream_model)], vec![]));
    config.routes.push(route(
        "variants",
        vec![],
        vec![Variant {
            name: "b".into(),
            weight: 100,
            targets: vec![target(upstream_model)],
            params: Default::default(),
        }],
    ));
    config
}

/// A trace id no other test in the process uses.
fn trace_id(seed: u8) -> String {
    format!("{:032x}", 0x3535_0000_u128 + u128::from(seed))
}

const INBOUND_SPAN: &str = "00f067aa0ba902b7";

/// The spans exported under `trace`, waiting for the attempt's span to end:
/// it rides on the response stream, which drops a moment after the body.
async fn spans_of(trace: &str) -> Vec<ExportedSpan> {
    let pipeline = test_pipeline::install();
    for _ in 0..200 {
        let spans: Vec<_> = pipeline
            .spans()
            .into_iter()
            .filter(|span| span.trace_id == trace)
            .collect();
        if spans.iter().any(|span| {
            span.attribute("gen_ai.operation.name").is_some() && span.name != "gateway.request"
        }) {
            return spans;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no upstream.request span exported under trace {trace}");
}

/// The attempt's span and the gateway span it hangs off, checked against the
/// `traceparent` the provider received.
fn assert_attempt(spans: &[ExportedSpan], traceparent: &str, model: &str) -> ExportedSpan {
    let attempts: Vec<_> = spans
        .iter()
        .filter(|span| span.attribute("attempt").is_some())
        .collect();
    assert_eq!(attempts.len(), 1, "one attempt span: {spans:#?}");
    let attempt = attempts[0].clone();
    assert_eq!(attempt.attribute("gen_ai.request.model"), Some(model));
    assert_eq!(attempt.attribute("http.response.status_code"), Some("200"));
    // parented to the gateway's request span, itself a child of the caller
    let parent = spans
        .iter()
        .find(|span| span.span_id == attempt.parent_span_id)
        .unwrap_or_else(|| panic!("the attempt's parent was not exported: {spans:#?}"));
    assert_eq!(parent.parent_span_id, INBOUND_SPAN, "{parent:#?}");
    // and the provider call is the attempt's child, not the caller's
    let fields: Vec<&str> = traceparent.split('-').collect();
    assert_eq!(fields.len(), 4, "malformed traceparent {traceparent:?}");
    assert_eq!(fields[1], attempt.trace_id);
    assert_eq!(
        fields[2], attempt.span_id,
        "the provider call must parent to the attempt span"
    );
    attempt
}

async fn gateway(config: &GatewayConfig) -> SocketAddr {
    test_pipeline::install();
    let state = rolter_gateway::AppState::with_logging(config, None);
    serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await
}

async fn chat(gw: SocketAddr, model: &str, trace: &str) {
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .header("traceparent", format!("00-{trace}-{INBOUND_SPAN}-01"))
        .json(&json!({"model": model, "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.text().await.unwrap();
}

async fn transcribe(gw: SocketAddr, model: &str, trace: &str) {
    let b = "SPANBOUND";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\nRIFFxxxxWAVE\r\n--{b}--\r\n"
    );
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/audio/transcriptions"))
        .header("traceparent", format!("00-{trace}-{INBOUND_SPAN}-01"))
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={b}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.text().await.unwrap();
}

#[tokio::test]
async fn a_variant_attempt_opens_the_upstream_request_span() {
    let seen = Seen::default();
    let gw = gateway(&config(seen.serve().await, "model-b")).await;
    let trace = trace_id(1);
    chat(gw, "variants", &trace).await;

    let (traceparent, model) = seen.only();
    assert_eq!(model, "model-b");
    let attempt = assert_attempt(&spans_of(&trace).await, &traceparent, "model-b");
    assert_eq!(attempt.attribute("gen_ai.operation.name"), Some("chat"));
    assert_eq!(attempt.name, "chat model-b");
    // the usage the answer reported lands on the span the outcome handed on
    assert_eq!(attempt.attribute("gen_ai.usage.input_tokens"), Some("3"));
}

#[tokio::test]
async fn a_classic_chat_attempt_still_opens_it() {
    let seen = Seen::default();
    let gw = gateway(&config(seen.serve().await, "model-a")).await;
    let trace = trace_id(2);
    chat(gw, "classic", &trace).await;

    let (traceparent, _) = seen.only();
    let attempt = assert_attempt(&spans_of(&trace).await, &traceparent, "model-a");
    assert_eq!(attempt.attribute("gen_ai.operation.name"), Some("chat"));
}

#[tokio::test]
async fn an_upload_attempt_opens_it_under_the_audio_operation() {
    let seen = Seen::default();
    let gw = gateway(&config(seen.serve().await, "whisper-a")).await;
    let trace = trace_id(3);
    transcribe(gw, "classic", &trace).await;

    let (traceparent, _) = seen.only();
    let attempt = assert_attempt(&spans_of(&trace).await, &traceparent, "whisper-a");
    assert_eq!(
        attempt.attribute("gen_ai.operation.name"),
        Some("generate_content")
    );
    assert_eq!(attempt.attribute("gen_ai.output.type"), Some("text"));
}

#[tokio::test]
async fn an_upload_through_a_variant_opens_it_too() {
    let seen = Seen::default();
    let gw = gateway(&config(seen.serve().await, "whisper-b")).await;
    let trace = trace_id(4);
    transcribe(gw, "variants", &trace).await;

    let (traceparent, _) = seen.only();
    let attempt = assert_attempt(&spans_of(&trace).await, &traceparent, "whisper-b");
    assert_eq!(
        attempt.attribute("gen_ai.operation.name"),
        Some("generate_content")
    );
}
