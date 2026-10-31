//! What the gateway does when a caller gives up mid-request (#1083).
//!
//! A disconnect is not an error the upstream produced, so it must not be
//! retried onto another provider and must not be logged as an upstream 200 that
//! nobody received. Every abandoned request lands as one status-499 row, bumps
//! `rolter_client_disconnects_total`, and — the part that actually costs
//! production traffic — releases its in-flight slot, so `rolter_inflight_requests`
//! returns to zero.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
};
use serde_json::{json, Value};

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// A stand-in for the ClickHouse HTTP interface that keeps every JSONEachRow
/// line the log writer posts, and separately every passive health event.
#[derive(Clone, Default)]
struct Rows(Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<Value>>>);

impl Rows {
    async fn serve(&self) -> SocketAddr {
        // the same endpoint receives payload captures too, which are not this
        // test's business
        async fn ingest(
            State(rows): State<Rows>,
            axum::extract::RawQuery(query): axum::extract::RawQuery,
            body: String,
        ) -> impl IntoResponse {
            let query = query.unwrap_or_default();
            let table = if query.contains("request_logs") {
                &rows.0
            } else if query.contains("provider_health_events") {
                &rows.1
            } else {
                return "ok";
            };
            for line in body.lines().filter(|line| !line.trim().is_empty()) {
                if let Ok(row) = serde_json::from_str::<Value>(line) {
                    table.lock().push(row);
                }
            }
            "ok"
        }
        serve(
            Router::new()
                .route("/", post(ingest))
                .with_state(self.clone()),
        )
        .await
    }

    /// Wait for a row the predicate accepts, so the test never sleeps for a
    /// fixed span it hopes is long enough.
    async fn wait_for(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..200 {
            if let Some(row) = self.0.lock().iter().find(|row| pred(row)).cloned() {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no {what} row arrived; saw {:?}", self.0.lock());
    }

    fn len(&self) -> usize {
        self.0.lock().len()
    }

    /// The passive health events received so far, leaving out any the prober
    /// wrote, since only real traffic is under test here.
    fn health_events(&self) -> Vec<Value> {
        self.1
            .lock()
            .iter()
            .filter(|event| event["source"] == "passive")
            .cloned()
            .collect()
    }
}

fn config(upstream: SocketAddr, clickhouse: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
    config.providers.push(ProviderConfig {
        name: "slow".into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{upstream}"),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: "slow-chat".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "slow".into(),
            model: Some("slow-chat".into()),
            ..Default::default()
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    config
}

async fn metrics(gateway: SocketAddr) -> String {
    reqwest::get(format!("http://{gateway}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

/// Read one `name value` sample out of the Prometheus text exposition.
fn sample(body: &str, name: &str) -> f64 {
    body.lines()
        .find_map(|line| line.strip_prefix(name)?.trim().parse().ok())
        .unwrap_or_else(|| panic!("no {name} sample in:\n{body}"))
}

#[tokio::test]
async fn a_caller_that_hangs_up_before_the_answer_is_logged_499_and_never_retried() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post({
            let attempts = attempts.clone();
            move || async move {
                attempts.fetch_add(1, Ordering::Relaxed);
                // longer than the client is willing to wait, so the hang-up
                // always lands while the request is in flight upstream
                tokio::time::sleep(Duration::from_secs(30)).await;
                axum::Json(json!({"choices": []}))
            }
        }),
    ))
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(
        upstream, clickhouse,
    )))
    .await;

    let hung_up = reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({"model": "slow-chat", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await;
    assert!(hung_up.is_err(), "the client was supposed to give up");

    let row = rows
        .wait_for("client-disconnect", |row| row["status"] == 499)
        .await;
    assert_eq!(row["model"], "slow-chat");
    assert_eq!(row["error"], "client disconnected");
    // the row names the target the caller gave up waiting on, which is the
    // one an operator is looking for when callers time out (#1816)
    assert_eq!(row["provider"], "slow");
    assert_eq!(row["target"], "slow-chat");
    // a disconnect is the caller's decision, not an upstream failure: burning a
    // second provider on it would double the cost of every abandoned request
    assert_eq!(attempts.load(Ordering::Relaxed), 1);
    assert_eq!(rows.len(), 1, "one row per abandoned request");

    let body = metrics(gateway).await;
    assert_eq!(sample(&body, "rolter_client_disconnects_total"), 1.0);
    assert_eq!(
        sample(&body, "rolter_inflight_requests"),
        0.0,
        "the abandoned request must give its slot back"
    );
}

/// A failed attempt is charged to its target before the loop backs off
/// (#1646). A caller who then leaves during the backoff gets a row naming that
/// target, but the one upstream failure must still count once, both in the
/// per-target error counter and in the health rollup that drives flap alerts.
#[tokio::test]
async fn a_caller_that_leaves_during_a_retry_backoff_is_not_charged_to_the_target_twice() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post({
            let attempts = attempts.clone();
            move || async move {
                attempts.fetch_add(1, Ordering::Relaxed);
                // honoured as the backoff, and far longer than the client waits,
                // so the hang-up always lands while the loop sleeps
                (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "20")],
                    axum::Json(json!({"error": {"message": "slow down"}})),
                )
            }
        }),
    ))
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let mut config = config(upstream, clickhouse);
    // a sibling to fail over to: the loop only backs off when there is a next
    // attempt to wait for (#2835), so with one target the 429 would be answered
    // at once and there would be no backoff for the caller to leave during
    config.providers.push(ProviderConfig {
        name: "sibling".into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{upstream}"),
        api_key: Some("test-key".into()),
        ..Default::default()
    });
    config.routes[0].targets.push(Target {
        provider: "sibling".into(),
        model: Some("slow-chat".into()),
        ..Default::default()
    });
    let gateway = serve(rolter_gateway::build_router_from_config(&config)).await;

    let hung_up = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
        .unwrap()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({"model": "slow-chat", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await;
    assert!(hung_up.is_err(), "the client was supposed to give up");

    let row = rows
        .wait_for("client-disconnect", |row| row["status"] == 499)
        .await;
    assert_eq!(row["provider"], "slow");
    assert_eq!(row["target"], "slow-chat");
    assert_eq!(
        attempts.load(Ordering::Relaxed),
        1,
        "the backoff never ended"
    );

    // the row is written after the guard has done its counting, so the
    // counters are final by the time it arrives
    let body = metrics(gateway).await;
    assert_eq!(sample(&body, "rolter_client_disconnects_total"), 1.0);
    assert_eq!(
        sample(
            &body,
            r#"rolter_target_requests_total{provider="slow",target="slow-chat",outcome="error"}"#
        ),
        1.0,
        "the 429 counts once; the caller leaving afterwards is not a second failure"
    );

    // health events flush on their own timer: wait for the 429's, then give a
    // second one the same chance to arrive before asserting it never did
    for _ in 0..200 {
        if !rows.health_events().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = rows.health_events();
    assert_eq!(
        events.len(),
        1,
        "one upstream failure, one event: {events:?}"
    );
    assert_eq!(events[0]["status_code"], 429);
}

/// A route split into variants picks its target in its own loop, and the row
/// names the variant as well as the target (#1816).
#[tokio::test]
async fn a_request_abandoned_on_a_variant_route_names_its_variant_and_target() {
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            axum::Json(json!({"choices": []}))
        }),
    ))
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let mut config = config(upstream, clickhouse);
    let route = &mut config.routes[0];
    route.variants = vec![rolter_core::Variant {
        name: "canary".into(),
        weight: 1,
        targets: std::mem::take(&mut route.targets),
        params: Default::default(),
    }];
    let gateway = serve(rolter_gateway::build_router_from_config(&config)).await;

    let hung_up = reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({"model": "slow-chat", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await;
    assert!(hung_up.is_err(), "the client was supposed to give up");

    let row = rows
        .wait_for("client-disconnect", |row| row["status"] == 499)
        .await;
    assert_eq!(row["provider"], "slow");
    assert_eq!(row["target"], "slow-chat");
    assert_eq!(row["variant"], "canary");
    assert_eq!(row["first_variant"], "canary");
    // the cancel row carries the split's sample too (#3547)
    assert_eq!(row["sampled_variant"], "canary");
}

#[tokio::test]
async fn a_stream_abandoned_mid_body_is_logged_499_with_the_tokens_it_did_bill() {
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            use futures_util::StreamExt;
            // usage first, then a frame the caller never reads: the provider has
            // already billed the tokens by the time the client walks away
            let frames = futures_util::stream::iter(vec![
                Ok::<_, std::io::Error>(axum::body::Bytes::from_static(
                    b"data: {\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":6,\"total_tokens\":10}}\n\n",
                )),
            ])
            .chain(futures_util::stream::once(async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(axum::body::Bytes::from_static(b"data: [DONE]\n\n"))
            }));
            (
                [("content-type", "text/event-stream")],
                Body::from_stream(frames),
            )
        }),
    ))
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(
        upstream, clickhouse,
    )))
    .await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({
            "model": "slow-chat",
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    // read the frames the provider did send, then drop the response — the shape
    // of a browser tab closing part-way through a completion
    let mut stream = response.bytes_stream();
    {
        use futures_util::StreamExt;
        let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("no first frame")
            .expect("stream ended early")
            .unwrap();
        assert!(String::from_utf8_lossy(&first).contains("usage"));
    }
    drop(stream);

    let row = rows
        .wait_for("client-disconnect", |row| row["status"] == 499)
        .await;
    assert_eq!(row["error"], "client disconnected");
    // the tokens are real spend even though nobody read the answer, so they stay
    // on the row: dropping them would under-bill every abandoned stream
    assert_eq!(row["total_tokens"], 10);
    // clickhouse takes booleans as 0/1
    assert_eq!(row["stream"], 1);

    let body = metrics(gateway).await;
    assert_eq!(sample(&body, "rolter_client_disconnects_total"), 1.0);
    assert_eq!(sample(&body, "rolter_inflight_requests"), 0.0);
}

#[tokio::test]
async fn a_completed_request_is_neither_counted_nor_marked() {
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            axum::Json(json!({
                "choices": [{"message": {"role": "assistant", "content": "ok"}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }))
        }),
    ))
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(
        upstream, clickhouse,
    )))
    .await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({"model": "slow-chat", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let row = rows.wait_for("request", |row| row["status"] == 200).await;
    assert_eq!(row["error"], "");
    assert_eq!(rows.len(), 1);

    let body = metrics(gateway).await;
    assert_eq!(sample(&body, "rolter_client_disconnects_total"), 0.0);
    assert_eq!(sample(&body, "rolter_inflight_requests"), 0.0);
}

/// Every `rolter_target_requests_total` series in the exposition, with its value.
fn target_requests(body: &str) -> Vec<(String, f64)> {
    body.lines()
        .filter(|line| line.starts_with("rolter_target_requests_total{"))
        .filter_map(|line| {
            let (series, value) = line.rsplit_once(' ')?;
            Some((series.to_string(), value.parse().ok()?))
        })
        .collect()
}

/// A caller leaving is not the target's failure (#1933): nothing was charged to
/// the target it was waiting on, neither as an error nor as a success, and no
/// passive health event was emitted, so the uptime rollup and the
/// `provider_health_flaps` alert never see it. The disconnect stays visible in
/// `rolter_client_disconnects_total`.
async fn assert_target_not_charged(gateway: SocketAddr, rows: &Rows) {
    // health events flush on their own timer; give one the chance to arrive
    // before asserting it never did
    tokio::time::sleep(Duration::from_millis(400)).await;
    let body = metrics(gateway).await;
    assert_eq!(
        sample(&body, "rolter_client_disconnects_total"),
        1.0,
        "the disconnect itself stays countable"
    );
    let charged: Vec<_> = target_requests(&body)
        .into_iter()
        .filter(|(_, value)| *value > 0.0)
        .collect();
    assert!(
        charged.is_empty(),
        "a client disconnect was counted against its target: {charged:?}"
    );
    let events = rows.health_events();
    assert!(
        events.is_empty(),
        "a client disconnect became a passive health event: {events:?}"
    );
}

/// In flight: the caller leaves while the upstream is still working.
#[tokio::test]
async fn a_disconnect_while_the_upstream_works_is_not_charged_to_the_target() {
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            axum::Json(json!({"choices": []}))
        }),
    ))
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(
        upstream, clickhouse,
    )))
    .await;

    let hung_up = reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({"model": "slow-chat", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await;
    assert!(hung_up.is_err(), "the client was supposed to give up");

    let row = rows
        .wait_for("client-disconnect", |row| row["status"] == 499)
        .await;
    // the row still names the target, so an operator can see where callers give up
    assert_eq!(row["provider"], "slow");
    assert_eq!(row["target"], "slow-chat");
    assert_target_not_charged(gateway, &rows).await;
}

/// Queued: the caller leaves while its job still waits in rolter's own provider
/// queue, before any upstream I/O. A healthy target must not be charged for
/// rolter's queueing.
#[tokio::test]
async fn a_disconnect_while_queued_in_rolter_is_not_charged_to_the_target() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post({
            let attempts = attempts.clone();
            move || async move {
                attempts.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_secs(30)).await;
                axum::Json(json!({"choices": []}))
            }
        }),
    ))
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let mut config = config(upstream, clickhouse);
    // one worker: the first request holds it, the second waits in the queue
    config.queue = rolter_core::QueueConfig {
        enabled: true,
        capacity: 4,
        workers: 1,
        backpressure: rolter_core::BackpressurePolicy::Block,
        block_timeout_ms: 30_000,
    };
    let gateway = serve(rolter_gateway::build_router_from_config(&config)).await;

    let url = format!("http://{gateway}/v1/chat/completions");
    let body = json!({"model": "slow-chat", "messages": [{"role": "user", "content": "hi"}]});
    let holder = tokio::spawn({
        let (url, body) = (url.clone(), body.clone());
        async move { reqwest::Client::new().post(url).json(&body).send().await }
    });
    // the holder reaches the upstream before the second request is sent
    for _ in 0..200 {
        if attempts.load(Ordering::Relaxed) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        attempts.load(Ordering::Relaxed),
        1,
        "the holder never started"
    );

    let queued = reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap()
        .post(url)
        .json(&body)
        .send()
        .await;
    assert!(queued.is_err(), "the queued client was supposed to give up");

    let row = rows
        .wait_for("queued client-disconnect", |row| row["status"] == 499)
        .await;
    // the row names the target it was queued for, which never saw the request
    assert_eq!(row["provider"], "slow");
    assert_eq!(
        attempts.load(Ordering::Relaxed),
        1,
        "the queued job reached the upstream"
    );
    assert_target_not_charged(gateway, &rows).await;
    holder.abort();
}

/// Mid-stream: the caller leaves after the upstream began answering. The target
/// did answer, but the row is rewritten to 499 and is not the target's verdict
/// either way.
#[tokio::test]
async fn a_disconnect_mid_stream_is_not_charged_to_the_target() {
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            use futures_util::StreamExt;
            let frames = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(
                axum::body::Bytes::from_static(
                    b"data: {\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":6,\"total_tokens\":10}}\n\n",
                ),
            )])
            .chain(futures_util::stream::once(async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(axum::body::Bytes::from_static(b"data: [DONE]\n\n"))
            }));
            (
                [("content-type", "text/event-stream")],
                Body::from_stream(frames),
            )
        }),
    ))
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(
        upstream, clickhouse,
    )))
    .await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({
            "model": "slow-chat",
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut stream = response.bytes_stream();
    {
        use futures_util::StreamExt;
        tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("no first frame")
            .expect("stream ended early")
            .unwrap();
    }
    drop(stream);

    rows.wait_for("client-disconnect", |row| row["status"] == 499)
        .await;
    assert_target_not_charged(gateway, &rows).await;
}

/// The rule is about the caller leaving, not about the number 499: an upstream
/// that answers 499 itself (a proxy in front of it closing on its own client)
/// still counts against the target it came from.
#[tokio::test]
async fn an_upstream_that_answers_499_is_still_that_targets_failure() {
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            (
                axum::http::StatusCode::from_u16(499).unwrap(),
                axum::Json(json!({"error": {"message": "closed"}})),
            )
        }),
    ))
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(
        upstream, clickhouse,
    )))
    .await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({"model": "slow-chat", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 499);
    rows.wait_for("upstream 499", |row| row["status"] == 499)
        .await;

    let body = metrics(gateway).await;
    assert_eq!(sample(&body, "rolter_client_disconnects_total"), 0.0);
    assert_eq!(
        sample(
            &body,
            r#"rolter_target_requests_total{provider="slow",target="slow-chat",outcome="error"}"#
        ),
        1.0
    );
}
