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
/// line the log writer posts.
#[derive(Clone, Default)]
struct Rows(Arc<Mutex<Vec<Value>>>);

impl Rows {
    async fn serve(&self) -> SocketAddr {
        // the same endpoint receives health events and payload captures; only
        // the request_logs table is this test's business
        async fn ingest(
            State(rows): State<Rows>,
            axum::extract::RawQuery(query): axum::extract::RawQuery,
            body: String,
        ) -> impl IntoResponse {
            if !query.unwrap_or_default().contains("request_logs") {
                return "ok";
            }
            for line in body.lines().filter(|line| !line.trim().is_empty()) {
                if let Ok(row) = serde_json::from_str::<Value>(line) {
                    rows.0.lock().push(row);
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
            weight: 1,
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
