//! Fallback targets that take over only the failures they declare (#1584).
//!
//! A target with `takes_over` is never dialled first and is reached only as
//! the successor of an attempt whose failure it covers. These tests drive the
//! gateway over HTTP against mock upstreams and an in-process stand-in for the
//! ClickHouse HTTP interface, and read the reason the request-log row records.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
};
use serde_json::{json, Value};

const MODEL: &str = "test-model";

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// A stand-in for the ClickHouse HTTP interface keeping every `request_logs`
/// `JSONEachRow` line.
#[derive(Clone, Default)]
struct Rows {
    logs: Arc<Mutex<Vec<Value>>>,
}

impl Rows {
    async fn serve(&self) -> SocketAddr {
        async fn ingest(
            State(rows): State<Rows>,
            axum::extract::RawQuery(query): axum::extract::RawQuery,
            body: String,
        ) -> &'static str {
            if query.unwrap_or_default().contains("request_logs") {
                for line in body.lines().filter(|line| !line.trim().is_empty()) {
                    if let Ok(row) = serde_json::from_str::<Value>(line) {
                        rows.logs.lock().push(row);
                    }
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

    /// The request row logged for `request_id`, once it has arrived.
    async fn row_for(&self, request_id: &str) -> Value {
        for _ in 0..200 {
            let found = self
                .logs
                .lock()
                .iter()
                .find(|row| row["request_id"] == request_id)
                .cloned();
            if let Some(row) = found {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no request-log row for {request_id}");
    }
}

#[derive(Clone)]
struct Canned {
    status: StatusCode,
    body: Value,
    calls: Arc<AtomicU32>,
}

async fn canned(State(canned): State<Canned>) -> axum::response::Response {
    canned.calls.fetch_add(1, Ordering::SeqCst);
    (canned.status, Json(canned.body)).into_response()
}

/// A mock upstream answering every chat completion with `status` and `body`.
struct Upstream {
    addr: SocketAddr,
    calls: Arc<AtomicU32>,
}

impl Upstream {
    async fn start(status: StatusCode, body: Value) -> Self {
        let calls = Arc::new(AtomicU32::new(0));
        let addr = serve(
            Router::new()
                .route("/v1/chat/completions", post(canned))
                .with_state(Canned {
                    status,
                    body,
                    calls: calls.clone(),
                }),
        )
        .await;
        Self { addr, calls }
    }

    async fn ok(content: &str) -> Self {
        Self::start(
            StatusCode::OK,
            json!({
                "id": "chatcmpl-1",
                "object": "chat.completion",
                "model": MODEL,
                "choices": [{"index": 0, "finish_reason": "stop",
                    "message": {"role": "assistant", "content": content}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
        )
        .await
    }

    fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }
}

fn content_refusal() -> Value {
    json!({"error": {
        "message": "Your request was rejected as a result of our safety system.",
        "type": "invalid_request_error",
        "code": "content_policy_violation"
    }})
}

/// One route over `(provider name, upstream, takes_over)` in order, request
/// logs going to `clickhouse`, and no backoff.
fn config(clickhouse: SocketAddr, targets: &[(&str, &Upstream, &[&str])]) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
    config.retry.base_backoff_ms = 0;
    config.retry.max_backoff_ms = 0;
    for (name, upstream, _) in targets {
        config.providers.push(ProviderConfig {
            name: (*name).into(),
            kind: ProviderKind::Openai,
            api_base: format!("http://{}", upstream.addr),
            ..Default::default()
        });
    }
    config.routes.push(ModelRoute {
        model: MODEL.into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: targets
            .iter()
            .map(|(name, _, takes_over)| Target {
                provider: (*name).into(),
                takes_over: takes_over.iter().map(|t| t.parse().unwrap()).collect(),
                ..Default::default()
            })
            .collect(),
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    assert_eq!(config.validate(), Ok(()));
    config
}

async fn gateway(config: &GatewayConfig) -> SocketAddr {
    let state = rolter_gateway::AppState::with_logging(config, None);
    serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await
}

async fn chat(gw: SocketAddr, id: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .header("x-request-id", id)
        .json(&json!({"model": MODEL, "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap()
}

/// A restricted target takes no share of ordinary traffic: with the ordinary
/// target healthy, every request goes there, round-robin notwithstanding.
#[tokio::test]
async fn a_restricted_target_takes_no_ordinary_traffic() {
    let rows = Rows::default();
    let ch = rows.serve().await;
    let cheap = Upstream::ok("cheap").await;
    let premium = Upstream::ok("premium").await;
    let gw = gateway(&config(
        ch,
        &[("cheap", &cheap, &[]), ("premium", &premium, &["refusal"])],
    ))
    .await;

    for i in 0..6 {
        let id = format!("ordinary-{i}");
        assert_eq!(chat(gw, &id).await.status(), 200);
        assert_eq!(rows.row_for(&id).await.get("fallback_reason"), None);
    }
    assert_eq!(cheap.calls(), 6);
    assert_eq!(premium.calls(), 0);
}

/// The case the issue names: the cheap target refuses on content policy, the
/// expensive one that takes refusals over answers, and the row says why the
/// request reached it.
#[tokio::test]
async fn a_content_refusal_fails_over_to_the_target_that_takes_it_over() {
    let rows = Rows::default();
    let ch = rows.serve().await;
    let cheap = Upstream::start(StatusCode::BAD_REQUEST, content_refusal()).await;
    let premium = Upstream::ok("premium").await;
    let gw = gateway(&config(
        ch,
        &[("cheap", &cheap, &[]), ("premium", &premium, &["refusal"])],
    ))
    .await;

    let response = chat(gw, "refused").await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "premium");
    assert_eq!((cheap.calls(), premium.calls()), (1, 1));

    let row = rows.row_for("refused").await;
    assert_eq!(row["provider"], "premium", "{row}");
    assert_eq!(row["attempts"], 2, "{row}");
    assert_eq!(
        row["fallback_reason"], "refusal:content_policy_violation",
        "{row}"
    );
}

/// A refusal narrowed to a category the provider did not report is not taken
/// over, and the caller gets the refusal exactly as the upstream sent it.
#[tokio::test]
async fn a_refusal_outside_the_narrowed_category_reaches_the_caller_untouched() {
    let rows = Rows::default();
    let ch = rows.serve().await;
    let cheap = Upstream::start(StatusCode::BAD_REQUEST, content_refusal()).await;
    let premium = Upstream::ok("premium").await;
    let gw = gateway(&config(
        ch,
        &[
            ("cheap", &cheap, &[]),
            ("premium", &premium, &["refusal:hate"]),
        ],
    ))
    .await;

    let response = chat(gw, "narrowed").await;
    assert_eq!(response.status(), 400);
    assert_eq!(response.json::<Value>().await.unwrap(), content_refusal());
    assert_eq!(premium.calls(), 0);
}

/// An ordinary client error is final, even on a route with a target that
/// takes refusals over.
#[tokio::test]
async fn an_ordinary_client_error_is_not_taken_over() {
    let rows = Rows::default();
    let ch = rows.serve().await;
    let invalid = json!({"error": {"message": "bad value", "type": "invalid_request_error",
        "code": "invalid_value"}});
    let cheap = Upstream::start(StatusCode::BAD_REQUEST, invalid.clone()).await;
    let premium = Upstream::ok("premium").await;
    let gw = gateway(&config(
        ch,
        &[("cheap", &cheap, &[]), ("premium", &premium, &["refusal"])],
    ))
    .await;

    let response = chat(gw, "invalid").await;
    assert_eq!(response.status(), 400);
    assert_eq!(response.json::<Value>().await.unwrap(), invalid);
    assert_eq!(premium.calls(), 0);
}

/// A burst target that takes over throttling is reached on a `429` and
/// passed over on a `503`.
#[tokio::test]
async fn a_burst_target_follows_throttling_and_not_a_server_error() {
    let rows = Rows::default();
    let ch = rows.serve().await;

    let throttled = Upstream::start(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error": {"message": "slow down"}}),
    )
    .await;
    let burst = Upstream::ok("burst").await;
    let gw = gateway(&config(
        ch,
        &[("main", &throttled, &[]), ("burst", &burst, &["throttled"])],
    ))
    .await;
    assert_eq!(chat(gw, "throttled").await.status(), 200);
    assert_eq!(burst.calls(), 1);
    let row = rows.row_for("throttled").await;
    assert_eq!(row["provider"], "burst", "{row}");
    assert_eq!(row["fallback_reason"], "throttled", "{row}");

    let broken = Upstream::start(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error": {"message": "down"}}),
    )
    .await;
    let idle_burst = Upstream::ok("burst").await;
    let gw = gateway(&config(
        ch,
        &[
            ("main", &broken, &[]),
            ("burst", &idle_burst, &["throttled"]),
        ],
    ))
    .await;
    assert_eq!(chat(gw, "server-error").await.status(), 503);
    assert_eq!(idle_burst.calls(), 0);
}

/// A route whose every target is restricted could never dial anything, and
/// configuration validation refuses it by name.
#[test]
fn a_route_of_only_restricted_targets_is_refused_at_validation() {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "premium".into(),
        kind: ProviderKind::Openai,
        api_base: "http://127.0.0.1:1".into(),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: MODEL.into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "premium".into(),
            takes_over: vec!["refusal".parse().unwrap()],
            ..Default::default()
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    let problems = config.validate().unwrap_err();
    assert!(
        problems
            .iter()
            .any(|p| p.contains(rolter_core::failover::ALL_TARGETS_RESTRICTED)),
        "{problems:?}"
    );
}
