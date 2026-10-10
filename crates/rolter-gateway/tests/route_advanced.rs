//! What a route's `advanced` block does to an upstream call (#2924).
//!
//! The block used to carry a dozen settings the dashboard and the docs
//! described as working and the gateway read none of. These tests drive the
//! gateway over HTTP against mock upstreams and pin the ones it applies now:
//! the route's own retry budget, its own bound on the wait for response
//! headers, and its static headers with their lock. Each one is a setting a
//! route sets and the deployment does not, so a test passes only if the
//! route's value is the one in effect.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::{
    AdvancedModelConfig, BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig,
    ProviderKind, Target,
};
use serde_json::{json, Value};

#[derive(Clone, Default)]
struct Upstream {
    calls: Arc<AtomicU32>,
    heads: Arc<Mutex<Vec<HeaderMap>>>,
    /// how long to hold the response headers back
    delay: Duration,
    status: u16,
}

async fn answer(State(up): State<Upstream>, headers: HeaderMap) -> axum::response::Response {
    up.calls.fetch_add(1, Ordering::SeqCst);
    up.heads.lock().push(headers);
    tokio::time::sleep(up.delay).await;
    let status = StatusCode::from_u16(up.status).unwrap();
    if status.is_success() {
        return Json(json!({
            "id": "chatcmpl-1", "object": "chat.completion", "model": "up-model",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": "ok"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }))
        .into_response();
    }
    (
        status,
        Json(json!({"error": {"message": "no", "type": "server_error"}})),
    )
        .into_response()
}

async fn upstream(status: u16, delay: Duration) -> (SocketAddr, Upstream) {
    let up = Upstream {
        status,
        delay,
        ..Default::default()
    };
    let app = Router::new()
        .route("/v1/chat/completions", post(answer))
        .with_state(up.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, up)
}

/// One route, `test-model`, over `providers` (one target each), carrying
/// `advanced`.
fn config_over(providers: &[SocketAddr], advanced: AdvancedModelConfig) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    let mut targets = Vec::new();
    for (i, addr) in providers.iter().enumerate() {
        config.providers.push(ProviderConfig {
            name: format!("up{i}"),
            kind: ProviderKind::OpenaiCompatible,
            api_base: format!("http://{addr}"),
            api_key: Some("sk-provider".into()),
            ..Default::default()
        });
        targets.push(Target {
            provider: format!("up{i}"),
            model: Some("up-model".into()),
            weight: 1,
        });
    }
    config.routes.push(ModelRoute {
        model: "test-model".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets,
        params: Default::default(),
        param_policy: Default::default(),
        advanced,
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

async fn chat_with(gw: SocketAddr, extra: &[(&str, &str)]) -> reqwest::Response {
    let mut request = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "hi"}]}));
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    request.send().await.unwrap()
}

async fn chat(gw: SocketAddr) -> reqwest::Response {
    chat_with(gw, &[]).await
}

fn with_retries(retries: Option<u32>) -> AdvancedModelConfig {
    let mut advanced = AdvancedModelConfig::default();
    advanced.limits.retries = retries;
    advanced
}

/// Three targets that all fail. Every attempt moves to a target not yet tried,
/// so the number of upstream calls is the retry budget plus the first attempt,
/// up to three.
async fn calls_to_three_failing_targets(
    retries: Option<u32>,
    deployment_retries: Option<u32>,
) -> u32 {
    let mut ups = Vec::new();
    let mut addrs = Vec::new();
    for _ in 0..3 {
        let (addr, up) = upstream(503, Duration::ZERO).await;
        addrs.push(addr);
        ups.push(up);
    }
    let mut config = config_over(&addrs, with_retries(retries));
    if let Some(n) = deployment_retries {
        config.retry.max_retries = n;
    }
    config.retry.base_backoff_ms = 1;
    config.retry.max_backoff_ms = 1;
    let gw = gateway(&config).await;

    let response = chat(gw).await;
    assert_eq!(response.status(), 503, "the last failure is handed back");
    ups.iter().map(|up| up.calls.load(Ordering::SeqCst)).sum()
}

#[tokio::test]
async fn a_route_without_a_retry_budget_uses_the_deployments() {
    // the default is two retries: all three targets are tried
    assert_eq!(calls_to_three_failing_targets(None, None).await, 3);
    assert_eq!(calls_to_three_failing_targets(None, Some(1)).await, 2);
}

#[tokio::test]
async fn a_route_can_turn_retries_off_on_a_deployment_that_retries() {
    assert_eq!(calls_to_three_failing_targets(Some(0), None).await, 1);
}

#[tokio::test]
async fn a_route_can_cut_its_retries_below_the_deployments() {
    assert_eq!(calls_to_three_failing_targets(Some(1), Some(2)).await, 2);
}

#[tokio::test]
async fn a_route_can_retry_on_a_deployment_that_does_not() {
    assert_eq!(calls_to_three_failing_targets(Some(2), Some(0)).await, 3);
}

#[tokio::test]
async fn a_route_retry_budget_applies_to_an_upload_too() {
    // the multipart path has its own loop; two targets, one retry budget of zero
    let mut ups = Vec::new();
    let mut addrs = Vec::new();
    for _ in 0..2 {
        let up = Upstream {
            status: 503,
            ..Default::default()
        };
        let app = Router::new()
            .route("/v1/audio/transcriptions", post(answer))
            .with_state(up.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        addrs.push(listener.local_addr().unwrap());
        ups.push(up);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    }
    let mut config = config_over(&addrs, with_retries(Some(0)));
    config.retry.base_backoff_ms = 1;
    config.retry.max_backoff_ms = 1;
    let gw = gateway(&config).await;
    let boundary = "ROLTERBOUND";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ntest-model\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\nRIFFxxxxWAVE\r\n--{b}--\r\n",
        b = boundary
    );
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/audio/transcriptions"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    let calls: u32 = ups.iter().map(|up| up.calls.load(Ordering::SeqCst)).sum();
    assert_eq!(calls, 1, "a budget of zero makes one attempt, not two");
}

#[tokio::test]
async fn a_route_timeout_cuts_a_slow_upstream_short() {
    // answers after 4s; the deployment waits 60s by default
    let (addr, up) = upstream(200, Duration::from_secs(4)).await;
    let mut advanced = AdvancedModelConfig::default();
    advanced.limits.timeout_secs = Some(1);
    advanced.limits.retries = Some(0);
    let gw = gateway(&config_over(&[addr], advanced)).await;

    let started = Instant::now();
    let response = chat(gw).await;
    let elapsed = started.elapsed();

    assert_eq!(response.status(), 502, "a timeout is an upstream failure");
    let body: Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("timed out after 1s"),
        "{body}"
    );
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    assert_eq!(up.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_route_without_a_timeout_keeps_the_deployments() {
    // the same 1.5s answer that a 1s route timeout would cut off
    let (addr, _) = upstream(200, Duration::from_millis(1500)).await;
    let gw = gateway(&config_over(&[addr], AdvancedModelConfig::default())).await;
    assert_eq!(chat(gw).await.status(), 200);
}

fn with_headers(headers: &[(&str, &str)], locked: &[&str]) -> AdvancedModelConfig {
    AdvancedModelConfig {
        headers: headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
        locked_headers: locked.iter().map(|name| name.to_string()).collect(),
        ..Default::default()
    }
}

fn header<'a>(heads: &'a [HeaderMap], name: &str) -> Option<&'a str> {
    heads.last()?.get(name)?.to_str().ok()
}

#[tokio::test]
async fn route_headers_reach_the_upstream_and_never_replace_the_credential() {
    let (addr, up) = upstream(200, Duration::ZERO).await;
    // the route is written around the control plane's check: both of the
    // credential headers are in the stored row
    let advanced = with_headers(
        &[
            ("x-model-region", "eu"),
            ("authorization", "Bearer sk-route"),
            ("x-api-key", "sk-route"),
        ],
        &[],
    );
    let gw = gateway(&config_over(&[addr], advanced)).await;

    assert_eq!(chat(gw).await.status(), 200);

    let heads = up.heads.lock();
    assert_eq!(header(&heads, "x-model-region"), Some("eu"));
    assert_eq!(header(&heads, "authorization"), Some("Bearer sk-provider"));
    assert_eq!(header(&heads, "x-api-key"), None);
}

#[tokio::test]
async fn a_locked_route_header_beats_a_forwarded_caller_header_and_an_unlocked_one_loses() {
    let (addr, up) = upstream(200, Duration::ZERO).await;
    let advanced = with_headers(
        &[("x-tenant", "from-route"), ("x-region", "from-route")],
        &["x-tenant"],
    );
    let mut config = config_over(&[addr], advanced);
    // a caller's header reaches the upstream only when the deployment forwards it
    config.client.forwarded_headers = vec!["x-tenant".into(), "x-region".into()];
    let gw = gateway(&config).await;

    let response = chat_with(
        gw,
        &[("x-tenant", "from-caller"), ("x-region", "from-caller")],
    )
    .await;
    assert_eq!(response.status(), 200);

    let heads = up.heads.lock();
    let sent = heads.last().unwrap();
    let all = |name: &str| -> Vec<String> {
        sent.get_all(name)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        all("x-tenant"),
        ["from-route"],
        "locked: the route's value only"
    );
    assert_eq!(
        all("x-region"),
        ["from-caller"],
        "unlocked: the caller's choice"
    );
}

#[tokio::test]
async fn a_route_header_nobody_forwards_is_sent_as_it_is() {
    // a route header nobody forwards is sent as it is; the lock changes nothing
    // for a header the caller was never going to get through
    let (addr, up) = upstream(200, Duration::ZERO).await;
    let advanced = with_headers(&[("x-tenant", "from-route")], &["x-tenant"]);
    let gw = gateway(&config_over(&[addr], advanced)).await;

    assert_eq!(
        chat_with(gw, &[("x-tenant", "from-caller")]).await.status(),
        200
    );

    let heads = up.heads.lock();
    assert_eq!(header(&heads, "x-tenant"), Some("from-route"));
}
