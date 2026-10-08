//! The wait between upstream attempts, and when it is pointless (#2835).
//!
//! A forward loop backs off before its next attempt, and for a `429` the wait
//! is the upstream's own `Retry-After`, up to 30 seconds. On a route with one
//! target the next attempt does not exist: the loop wakes, finds no untried
//! target and ends the request with the answer it already had. The caller used
//! to wait the whole delay for that.
//!
//! These tests drive the gateway over HTTP against a mock upstream that always
//! answers `429 Retry-After: 30`, and bound each request well under that.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{
    ApiKeyConfig, BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind,
    Target, Variant,
};
use serde_json::{json, Value};

/// What the upstream's `Retry-After` asks the caller to wait, in seconds.
const ASK_SECS: &str = "30";

/// A request that regresses to waiting out [`ASK_SECS`] is failed after this
/// long instead of holding the suite for half a minute.
const PROMPT: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct Canned {
    retry_after: &'static str,
    calls: Arc<AtomicU32>,
}

async fn canned(State(canned): State<Canned>) -> axum::response::Response {
    canned.calls.fetch_add(1, Ordering::SeqCst);
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({"error": {"message": "slow down", "type": "rate_limit_error"}})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, canned.retry_after.parse().unwrap());
    response
}

/// An upstream that answers every call to `path` with `429` and `retry_after`.
async fn upstream(path: &str, retry_after: &'static str) -> (SocketAddr, Arc<AtomicU32>) {
    let calls = Arc::new(AtomicU32::new(0));
    let app = Router::new().route(path, post(canned)).with_state(Canned {
        retry_after,
        calls: calls.clone(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, calls)
}

fn target() -> Target {
    Target {
        provider: "up".into(),
        model: Some("test-model".into()),
        weight: 1,
    }
}

/// One route, `test-model`, with one target on one provider, the default
/// retry policy and cooldowns off, so only the wait is under test.
fn config_over(addr: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "up".into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{addr}"),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: "test-model".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![target()],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    config
}

async fn gateway(config: &GatewayConfig) -> SocketAddr {
    let app = rolter_gateway::build_router_from_config(config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

async fn chat(gw: SocketAddr) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap()
}

/// Send `request`, failing the test rather than waiting if it takes
/// [`PROMPT`], and say how long it took.
async fn promptly(
    request: impl std::future::Future<Output = reqwest::Response>,
) -> (reqwest::Response, Duration) {
    let started = Instant::now();
    let response = tokio::time::timeout(PROMPT, request).await.expect(
        "the request waited out the upstream's Retry-After for an attempt that cannot happen",
    );
    (response, started.elapsed())
}

/// What the caller is told is the upstream's own `429` with its `Retry-After`,
/// so a client that backs off on rate limits still does.
async fn assert_rate_limited(response: reqwest::Response) {
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers()[header::RETRY_AFTER], ASK_SECS);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "upstream_rate_limited");
}

#[tokio::test]
async fn a_single_target_route_answers_without_waiting_out_retry_after() {
    let (addr, calls) = upstream("/v1/chat/completions", ASK_SECS).await;
    let gw = gateway(&config_over(addr)).await;

    let (response, elapsed) = promptly(chat(gw)).await;

    assert_rate_limited(response).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one target, one try");
    assert!(elapsed < PROMPT, "{elapsed:?}");
}

#[tokio::test]
async fn a_variant_route_with_one_target_answers_without_waiting() {
    let (addr, calls) = upstream("/v1/chat/completions", ASK_SECS).await;
    let mut config = config_over(addr);
    let route = &mut config.routes[0];
    route.variants = vec![Variant {
        name: "canary".into(),
        weight: 1,
        targets: std::mem::take(&mut route.targets),
        params: Default::default(),
    }];
    let gw = gateway(&config).await;

    let (response, _) = promptly(chat(gw)).await;

    assert_rate_limited(response).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one candidate, one try");
}

#[tokio::test]
async fn an_upload_to_a_single_target_answers_without_waiting() {
    let (addr, calls) = upstream("/v1/audio/transcriptions", ASK_SECS).await;
    let gw = gateway(&config_over(addr)).await;
    let boundary = "ROLTERBOUND";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ntest-model\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\nRIFFxxxxWAVE\r\n--{b}--\r\n",
        b = boundary
    );
    let upload = reqwest::Client::new()
        .post(format!("http://{gw}/v1/audio/transcriptions"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send();
    let upload = async { upload.await.unwrap() };

    let (response, _) = promptly(upload).await;

    assert_rate_limited(response).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one target, one try");
}

/// A failover to a sibling target still waits first, because the sibling may
/// be the same upstream capacity that just said no. What goes is the wait after
/// the last target, which has nothing to wake up for.
#[tokio::test]
async fn a_failover_to_a_sibling_target_still_waits() {
    let (addr, calls) = upstream("/v1/chat/completions", "1").await;
    let mut config = config_over(addr);
    config.providers.push(ProviderConfig {
        name: "sibling".into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{addr}"),
        ..Default::default()
    });
    config.routes[0].targets.push(Target {
        provider: "sibling".into(),
        model: Some("test-model".into()),
        weight: 1,
    });
    let gw = gateway(&config).await;

    let started = Instant::now();
    let response = chat(gw).await;
    let elapsed = started.elapsed();

    assert_eq!(response.status(), 429);
    assert_eq!(calls.load(Ordering::SeqCst), 2, "both targets were tried");
    // one wait, between the first target and the second; none after the last
    assert!(
        elapsed >= Duration::from_millis(900),
        "the wait before the failover was skipped: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
}

/// A `429` on a provider with several keys parks the key and retries the same
/// target on a sibling key, so there is a next attempt and the wait stays.
#[tokio::test]
async fn a_multi_key_provider_still_waits_for_its_key_to_cool_down() {
    let (addr, calls) = upstream("/v1/chat/completions", "1").await;
    let mut config = config_over(addr);
    config.retry.max_retries = 1;
    config.providers[0].api_keys = vec![
        ApiKeyConfig {
            key: Some("key-one".to_string()),
            env: None,
            weight: 1,
        },
        ApiKeyConfig {
            key: Some("key-two".to_string()),
            env: None,
            weight: 1,
        },
    ];
    let gw = gateway(&config).await;

    let started = Instant::now();
    let response = chat(gw).await;
    let elapsed = started.elapsed();

    assert_eq!(response.status(), 429);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the target was tried twice"
    );
    assert!(
        elapsed >= Duration::from_millis(900),
        "the multi-key wait was skipped: {elapsed:?}"
    );
}
