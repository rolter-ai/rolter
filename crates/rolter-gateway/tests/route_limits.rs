//! What a route's `advanced.limits.rpm`, `tpm` and `concurrency` do to a
//! request (#2936).
//!
//! Each test drives the gateway over HTTP against a mock upstream, on a route
//! that sets one cap and a deployment that sets none, so a test passes only if
//! the route's own cap is what refused or admitted. `rpm` and `tpm` count in
//! Redis and run only when `ROLTER_TEST_REDIS_URL` names a server; every route
//! they use has a name no other run shares, since the counters outlive a test.
//! `concurrency` is counted in process and needs no Redis.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::StreamExt;
use rolter_core::{
    AdvancedModelConfig, BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig,
    ProviderKind, Target,
};
use serde_json::{json, Value};
use tokio::sync::Semaphore;

/// A mock upstream. When `gated`, every answer waits for a permit on `gate`,
/// which is how a test keeps a request in flight for exactly as long as it
/// wants to.
#[derive(Clone)]
struct Upstream {
    calls: Arc<AtomicU32>,
    gate: Arc<Semaphore>,
    gated: bool,
    /// answer as an SSE stream: one chunk at once, the rest after the gate
    streaming: bool,
}

fn completion() -> Value {
    json!({
        "id": "chatcmpl-1", "object": "chat.completion", "model": "up-model",
        "choices": [{"index": 0, "finish_reason": "stop",
                     "message": {"role": "assistant", "content": "ok"}}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    })
}

async fn answer(State(up): State<Upstream>) -> Response {
    up.calls.fetch_add(1, Ordering::SeqCst);
    if up.streaming {
        let first = format!(
            "data: {}\n\n",
            json!({"id": "c", "object": "chat.completion.chunk", "model": "up-model",
                   "choices": [{"index": 0, "delta": {"content": "o"}}]})
        );
        let last = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"id": "c", "object": "chat.completion.chunk", "model": "up-model",
                   "choices": [{"index": 0, "delta": {"content": "k"}, "finish_reason": "stop"}],
                   "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}})
        );
        let gate = up.gate.clone();
        let stream = futures_util::stream::once(async move {
            Ok::<_, std::convert::Infallible>(Bytes::from(first))
        })
        .chain(futures_util::stream::once(async move {
            gate.acquire().await.unwrap().forget();
            Ok(Bytes::from(last))
        }));
        return Response::builder()
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap();
    }
    if up.gated {
        up.gate.acquire().await.unwrap().forget();
    }
    Json(completion()).into_response()
}

async fn upstream(gated: bool, streaming: bool) -> (SocketAddr, Upstream) {
    let up = Upstream {
        calls: Arc::default(),
        gate: Arc::new(Semaphore::new(0)),
        gated,
        streaming,
    };
    let app = Router::new()
        .route("/v1/chat/completions", post(answer))
        .route("/v1/audio/transcriptions", post(answer))
        .with_state(up.clone());
    (serve(app).await, up)
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

fn limited(rpm: Option<u32>, tpm: Option<u32>, concurrency: Option<u32>) -> AdvancedModelConfig {
    let mut advanced = AdvancedModelConfig::default();
    advanced.limits.rpm = rpm;
    advanced.limits.tpm = tpm;
    advanced.limits.concurrency = concurrency;
    advanced
}

/// A route named `model` over one target at `addr`, carrying `advanced`.
fn route(model: &str, _addr: SocketAddr, advanced: AdvancedModelConfig) -> ModelRoute {
    ModelRoute {
        model: model.into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "up".into(),
            model: Some("up-model".into()),
            ..Default::default()
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced,
        cache: None,
        variants: Default::default(),
        tenancy: None,
    }
}

fn config(routes: Vec<ModelRoute>, addr: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "up".into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{addr}"),
        api_key: Some("sk-provider".into()),
        ..Default::default()
    });
    config.routes = routes;
    // no retries: a refusal or a failure must be the one answer
    config.retry.max_retries = 0;
    config
}

async fn gateway(config: &GatewayConfig, redis: Option<&str>) -> SocketAddr {
    let state = rolter_gateway::AppState::with_logging(config, redis);
    serve(rolter_gateway::build_router(
        state,
        &config.server.metrics_path,
        config.server.max_body_bytes,
    ))
    .await
}

fn redis_url() -> Option<String> {
    let url = std::env::var("ROLTER_TEST_REDIS_URL").ok()?;
    if url.trim().is_empty() {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return None;
    }
    // a logical database no other suite selects
    Some(format!("{}/9", url.trim().trim_end_matches('/')))
}

/// A route name no other run shares.
fn unique(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

async fn chat(gw: SocketAddr, model: &str, stream: bool) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": model, "stream": stream,
                      "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap()
}

async fn message(gw: SocketAddr, model: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}/v1/messages"))
        .header("anthropic-version", "2023-06-01")
        .json(&json!({"model": model, "max_tokens": 8,
                      "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap()
}

/// Wait until `up` has been called `n` times, so a request is known to be in
/// flight before the next one is sent.
async fn wait_for_calls(up: &Upstream, n: u32) {
    for _ in 0..200 {
        if up.calls.load(Ordering::SeqCst) >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("upstream never saw {n} calls");
}

fn retry_after(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

// --- concurrency: in process, no Redis ---

#[tokio::test]
async fn a_route_at_its_concurrency_cap_refuses_the_next_request_with_a_429() {
    let (addr, up) = upstream(true, false).await;
    let model = unique("conc");
    let gw = gateway(
        &config(
            vec![route(&model, addr, limited(None, None, Some(1)))],
            addr,
        ),
        None,
    )
    .await;

    let held = tokio::spawn({
        let model = model.clone();
        async move { chat(gw, &model, false).await }
    });
    wait_for_calls(&up, 1).await;

    let refused = chat(gw, &model, false).await;
    assert_eq!(refused.status(), 429);
    assert!(retry_after(&refused).is_some(), "a 429 says when to retry");
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "rate_limit_exceeded", "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&format!("route '{model}'")),
        "{body}"
    );
    assert_eq!(
        up.calls.load(Ordering::SeqCst),
        1,
        "never reached the upstream"
    );

    // the slot is released when the response is delivered
    up.gate.add_permits(1);
    assert_eq!(held.await.unwrap().status(), 200);
    up.gate.add_permits(1);
    assert_eq!(chat(gw, &model, false).await.status(), 200);
}

#[tokio::test]
async fn a_streamed_response_keeps_its_concurrency_slot_until_the_last_byte() {
    let (addr, up) = upstream(false, true).await;
    let model = unique("conc-stream");
    let gw = gateway(
        &config(
            vec![route(&model, addr, limited(None, None, Some(1)))],
            addr,
        ),
        None,
    )
    .await;

    // the headers and the first chunk are here; the rest is held upstream
    let streaming = chat(gw, &model, true).await;
    assert_eq!(streaming.status(), 200);
    let mut body = streaming.bytes_stream();
    let first = body.next().await.unwrap().unwrap();
    assert!(!first.is_empty());

    assert_eq!(
        chat(gw, &model, false).await.status(),
        429,
        "the route is still serving the stream"
    );

    up.gate.add_permits(1);
    while body.next().await.is_some() {}
    drop(body);

    // the permit follows the response body, which the gateway drops a moment
    // after the client has read the end
    let mut status = 429;
    for _ in 0..40 {
        up.gate.add_permits(1);
        status = chat(gw, &model, false).await.status().as_u16();
        if status == 200 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(status, 200, "the slot is free once the stream has ended");
}

#[tokio::test]
async fn a_client_that_leaves_mid_stream_frees_the_slot() {
    let (addr, up) = upstream(false, true).await;
    let model = unique("conc-leave");
    let gw = gateway(
        &config(
            vec![route(&model, addr, limited(None, None, Some(1)))],
            addr,
        ),
        None,
    )
    .await;

    let streaming = chat(gw, &model, true).await;
    let mut body = streaming.bytes_stream();
    body.next().await.unwrap().unwrap();
    assert_eq!(chat(gw, &model, false).await.status(), 429);

    drop(body); // the client disconnects with the upstream still holding the rest
    let mut status = 429;
    for _ in 0..80 {
        up.gate.add_permits(1);
        status = chat(gw, &model, false).await.status().as_u16();
        if status == 200 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(status, 200, "a disconnect must not leak the slot");
}

#[tokio::test]
async fn a_concurrency_cap_belongs_to_its_route_alone() {
    let (addr, up) = upstream(true, false).await;
    let (capped, other) = (unique("conc-a"), unique("conc-b"));
    let gw = gateway(
        &config(
            vec![
                route(&capped, addr, limited(None, None, Some(1))),
                route(&other, addr, AdvancedModelConfig::default()),
            ],
            addr,
        ),
        None,
    )
    .await;

    let held = tokio::spawn({
        let capped = capped.clone();
        async move { chat(gw, &capped, false).await }
    });
    wait_for_calls(&up, 1).await;
    assert_eq!(chat(gw, &capped, false).await.status(), 429);

    // a route with no cap is not slowed by a neighbour's
    let neighbour = tokio::spawn({
        let other = other.clone();
        async move { chat(gw, &other, false).await }
    });
    wait_for_calls(&up, 2).await;
    up.gate.add_permits(2);
    assert_eq!(neighbour.await.unwrap().status(), 200);
    assert_eq!(held.await.unwrap().status(), 200);
}

/// An upload runs its own forwarding loop, and holds the slot like any other
/// request.
#[tokio::test]
async fn an_upload_holds_and_is_refused_by_the_route_concurrency_cap() {
    let (addr, up) = upstream(true, false).await;
    let model = unique("conc-upload");
    let gw = gateway(
        &config(
            vec![route(&model, addr, limited(None, None, Some(1)))],
            addr,
        ),
        None,
    )
    .await;
    let upload = |model: String| async move {
        let boundary = "ROLTERBOUND";
        let body = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n\
             --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
             Content-Type: audio/wav\r\n\r\nRIFFxxxxWAVE\r\n--{b}--\r\n",
            b = boundary
        );
        reqwest::Client::new()
            .post(format!("http://{gw}/v1/audio/transcriptions"))
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await
            .unwrap()
    };
    let held = tokio::spawn(upload(model.clone()));
    wait_for_calls(&up, 1).await;

    assert_eq!(upload(model.clone()).await.status(), 429);
    assert_eq!(
        chat(gw, &model, false).await.status(),
        429,
        "one route, one cap"
    );
    assert_eq!(up.calls.load(Ordering::SeqCst), 1);

    up.gate.add_permits(1);
    held.await.unwrap();
}

/// The Anthropic surface refuses with the same envelope and cause.
#[tokio::test]
async fn an_anthropic_caller_gets_the_same_concurrency_refusal() {
    let (addr, up) = upstream(true, false).await;
    let model = unique("conc-anthropic");
    let gw = gateway(
        &config(
            vec![route(&model, addr, limited(None, None, Some(1)))],
            addr,
        ),
        None,
    )
    .await;
    let held = tokio::spawn({
        let model = model.clone();
        async move { chat(gw, &model, false).await }
    });
    wait_for_calls(&up, 1).await;

    let refused = message(gw, &model).await;
    assert_eq!(refused.status(), 429);
    assert!(retry_after(&refused).is_some());
    let body: Value = refused.json().await.unwrap();
    // the gateway's own refusals share one envelope on both surfaces, the
    // same one a caller's key limit answers with
    assert_eq!(body["error"]["type"], "rate_limit_error", "{body}");
    assert_eq!(body["error"]["code"], "rate_limit_exceeded", "{body}");

    up.gate.add_permits(1);
    held.await.unwrap();
}

#[tokio::test]
async fn a_refusal_is_counted_per_route_and_cap() {
    let (addr, up) = upstream(true, false).await;
    let model = unique("conc-metric");
    let gw = gateway(
        &config(
            vec![route(&model, addr, limited(None, None, Some(1)))],
            addr,
        ),
        None,
    )
    .await;
    let held = tokio::spawn({
        let model = model.clone();
        async move { chat(gw, &model, false).await }
    });
    wait_for_calls(&up, 1).await;
    assert_eq!(chat(gw, &model, false).await.status(), 429);
    assert_eq!(chat(gw, &model, false).await.status(), 429);

    let metrics = reqwest::get(format!("http://{gw}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metrics.contains(&format!(
            "rolter_route_limit_blocks_total{{route=\"{model}\",kind=\"concurrency\"}} 2"
        )),
        "{metrics}"
    );
    up.gate.add_permits(1);
    held.await.unwrap();
}

// --- rpm and tpm: Redis ---

#[tokio::test]
async fn a_route_at_its_rpm_refuses_the_next_request() {
    let Some(url) = redis_url() else { return };
    let (addr, up) = upstream(false, false).await;
    let model = unique("rpm");
    let gw = gateway(
        &config(
            vec![route(&model, addr, limited(Some(2), None, None))],
            addr,
        ),
        Some(&url),
    )
    .await;

    assert_eq!(chat(gw, &model, false).await.status(), 200);
    assert_eq!(chat(gw, &model, false).await.status(), 200);
    let refused = chat(gw, &model, false).await;
    assert_eq!(refused.status(), 429);
    let wait = retry_after(&refused).expect("a 429 says when to retry");
    assert!((1..=60).contains(&wait), "{wait}");
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "rate_limit_exceeded", "{body}");
    assert!(
        body["error"]["message"].as_str().unwrap().contains("rpm"),
        "{body}"
    );
    assert_eq!(up.calls.load(Ordering::SeqCst), 2, "the third never left");

    let metrics = reqwest::get(format!("http://{gw}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metrics.contains(&format!(
            "rolter_route_limit_blocks_total{{route=\"{model}\",kind=\"rpm\"}} 1"
        )),
        "{metrics}"
    );
}

/// The count is shared: a second gateway on the same Redis sees the first's
/// requests, so the cap holds across replicas, not per replica.
#[tokio::test]
async fn the_rpm_cap_is_shared_by_every_replica() {
    let Some(url) = redis_url() else { return };
    let (addr, _) = upstream(false, false).await;
    let model = unique("rpm-fleet");
    let cfg = config(
        vec![route(&model, addr, limited(Some(2), None, None))],
        addr,
    );
    let (one, two) = (
        gateway(&cfg, Some(&url)).await,
        gateway(&cfg, Some(&url)).await,
    );

    assert_eq!(chat(one, &model, false).await.status(), 200);
    assert_eq!(chat(two, &model, false).await.status(), 200);
    assert_eq!(chat(one, &model, false).await.status(), 429);
    assert_eq!(chat(two, &model, false).await.status(), 429);
}

#[tokio::test]
async fn a_route_rpm_is_not_another_routes() {
    let Some(url) = redis_url() else { return };
    let (addr, _) = upstream(false, false).await;
    let (capped, other) = (unique("rpm-a"), unique("rpm-b"));
    let gw = gateway(
        &config(
            vec![
                route(&capped, addr, limited(Some(1), None, None)),
                route(&other, addr, AdvancedModelConfig::default()),
            ],
            addr,
        ),
        Some(&url),
    )
    .await;

    assert_eq!(chat(gw, &capped, false).await.status(), 200);
    assert_eq!(chat(gw, &capped, false).await.status(), 429);
    for _ in 0..3 {
        assert_eq!(chat(gw, &other, false).await.status(), 200);
    }
}

#[tokio::test]
async fn an_anthropic_caller_is_held_to_the_route_rpm_too() {
    let Some(url) = redis_url() else { return };
    let (addr, _) = upstream(false, false).await;
    let model = unique("rpm-anthropic");
    let gw = gateway(
        &config(
            vec![route(&model, addr, limited(Some(1), None, None))],
            addr,
        ),
        Some(&url),
    )
    .await;

    assert_eq!(chat(gw, &model, false).await.status(), 200);
    let refused = message(gw, &model).await;
    assert_eq!(refused.status(), 429);
    assert!(retry_after(&refused).is_some());
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["type"], "rate_limit_error", "{body}");
}

/// `tpm` is reactive: a response's usage lands after it is delivered, and the
/// next request is refused once the window holds the cap.
#[tokio::test]
async fn a_route_whose_tokens_fill_its_tpm_refuses_the_next_request() {
    let Some(url) = redis_url() else { return };
    let (addr, up) = upstream(false, false).await;
    let model = unique("tpm");
    // each response reports two tokens
    let gw = gateway(
        &config(
            vec![route(&model, addr, limited(None, Some(2), None))],
            addr,
        ),
        Some(&url),
    )
    .await;

    assert_eq!(chat(gw, &model, false).await.status(), 200);
    let mut refused = None;
    for _ in 0..80 {
        let response = chat(gw, &model, false).await;
        if response.status() == 429 {
            refused = Some(response);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let refused = refused.expect("recorded tokens must fill the window");
    assert!(retry_after(&refused).is_some());
    let body: Value = refused.json().await.unwrap();
    assert!(
        body["error"]["message"].as_str().unwrap().contains("tpm"),
        "{body}"
    );
    assert!(up.calls.load(Ordering::SeqCst) >= 1);
}

// --- no Redis ---

/// Without Redis `rpm` and `tpm` cannot be counted and fail open, like every
/// other throughput limit; `concurrency` still holds. The first half is the
/// documented limit, pinned so a change to it is a decision.
#[tokio::test]
async fn without_redis_rpm_fails_open_and_concurrency_still_holds() {
    let (addr, up) = upstream(true, false).await;
    let model = unique("no-redis");
    let gw = gateway(
        &config(
            vec![route(&model, addr, limited(Some(1), Some(1), Some(1)))],
            addr,
        ),
        None,
    )
    .await;

    let held = tokio::spawn({
        let model = model.clone();
        async move { chat(gw, &model, false).await }
    });
    wait_for_calls(&up, 1).await;
    assert_eq!(chat(gw, &model, false).await.status(), 429, "concurrency");
    up.gate.add_permits(1);
    assert_eq!(held.await.unwrap().status(), 200);

    // sequential requests are far past rpm = 1 and are still admitted
    for _ in 0..3 {
        up.gate.add_permits(1);
        assert_eq!(chat(gw, &model, false).await.status(), 200);
    }
}
