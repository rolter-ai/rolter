//! Token usage, cost and budget spend for streamed Responses API answers
//! (#2819), and the prompt-cache discount on them (#2847).
//!
//! A buffered Responses answer reports `usage` at the top level of the body,
//! which the request log has always read. A streamed one reports it only on
//! the terminal event, nested under `response`, and that was never read: a
//! streamed `/v1/responses` request through a real provider was logged with
//! zero tokens, zero cost and `usage_unknown = 1`, and its spend never reached
//! a budget.
//!
//! These tests drive the gateway over HTTP against mock upstreams and an
//! in-process stand-in for the ClickHouse HTTP interface. The budget counter
//! lives in Redis, so that test reads `ROLTER_TEST_REDIS_URL` and skips when it
//! is unset, the same contract the postgres suites keep.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
    VirtualKeyRecord,
};
use serde_json::{json, Value};

const KEY_ID: &str = "key-responses-stream";

/// The caller's key, made up per run rather than written out.
fn key() -> String {
    format!("sk-responses-stream-{:x}", std::process::id())
}

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

    /// The one row logged for `request_id`, once it has arrived.
    async fn row_for(&self, request_id: &str) -> Value {
        for _ in 0..200 {
            let found = self.matching(request_id);
            if !found.is_empty() {
                // one more flush interval, so a second row that should not
                // exist has had its chance to arrive
                tokio::time::sleep(Duration::from_millis(100)).await;
                let mut found = self.matching(request_id);
                assert_eq!(found.len(), 1, "a request must leave exactly one row");
                return found.remove(0);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no request-log row arrived for {request_id}");
    }

    fn matching(&self, request_id: &str) -> Vec<Value> {
        self.logs
            .lock()
            .iter()
            .filter(|row| row["request_id"] == request_id)
            .cloned()
            .collect()
    }
}

/// A Responses API event stream the way OpenAI frames it: `created` and
/// `in_progress` carry `usage: null`, the terminal event carries the counts.
fn responses_stream(terminal: &str, status: &str, usage: Value) -> String {
    let event = |name: &str, data: Value| format!("event: {name}\ndata: {data}\n\n");
    [
        event(
            "response.created",
            json!({"type": "response.created", "response": {
                "id": "resp_1", "status": "in_progress", "usage": null
            }}),
        ),
        event(
            "response.output_text.delta",
            json!({"type": "response.output_text.delta", "delta": "pong"}),
        ),
        event(
            terminal,
            json!({"type": terminal, "response": {
                "id": "resp_1", "object": "response", "status": status, "usage": usage
            }}),
        ),
    ]
    .concat()
}

fn counts() -> Value {
    json!({
        "input_tokens": 7,
        "input_tokens_details": {"cached_tokens": 0},
        "output_tokens": 3,
        "output_tokens_details": {"reasoning_tokens": 0},
        "total_tokens": 10
    })
}

/// The same answer with most of the prompt served from the provider's cache:
/// `cached_tokens` is part of `input_tokens`, as OpenAI reports it.
fn cached_counts() -> Value {
    json!({
        "input_tokens": 2000,
        "input_tokens_details": {"cached_tokens": 1500},
        "output_tokens": 3,
        "output_tokens_details": {"reasoning_tokens": 0},
        "total_tokens": 2003
    })
}

/// An upstream whose `/v1/responses` answers with `body` as `content_type`.
async fn responses_upstream(content_type: &'static str, body: String) -> SocketAddr {
    async fn handler(
        State((content_type, body)): State<(&'static str, String)>,
    ) -> impl IntoResponse {
        ([(header::CONTENT_TYPE, content_type)], body)
    }
    serve(
        Router::new()
            .route("/v1/responses", post(handler))
            .with_state((content_type, body)),
    )
    .await
}

/// A config with one `test-model` route over a single provider of `kind`, a
/// price of one dollar per token (a tenth of that for a cached input token), an
/// org budget, and request logs going to `clickhouse`.
fn config(
    kind: ProviderKind,
    upstream: SocketAddr,
    clickhouse: SocketAddr,
    org: &str,
) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
    config.retry.base_backoff_ms = 0;
    config.retry.max_backoff_ms = 0;
    config.providers.push(ProviderConfig {
        name: "up".into(),
        kind,
        api_base: format!("http://{upstream}"),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: "test-model".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "up".into(),
            model: Some("test-model".into()),
            weight: 1,
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    config.db_virtual_keys.push(VirtualKeyRecord {
        access_policy: None,
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), &key()),
        id: KEY_ID.into(),
        org_id: org.into(),
        team_id: String::new(),
        project_id: String::new(),
        user_id: String::new(),
        models: vec![],
        providers: vec![],
        disabled: false,
        expires_at: None,
        cache: None,
        business_unit_id: String::new(),
        customer_id: String::new(),
    });
    config.model_prices.push(
        serde_json::from_value(json!({
            "model": "test-model",
            "input_per_mtok": 1_000_000,
            "output_per_mtok": 1_000_000,
            "cached_input_per_mtok": 100_000
        }))
        .unwrap(),
    );
    config.budgets.push(
        serde_json::from_value(json!({
            "scope": "org", "id": org, "limit_usd": 100, "period": "monthly"
        }))
        .unwrap(),
    );
    config
}

async fn gateway(config: &GatewayConfig, redis: Option<&str>) -> SocketAddr {
    let state = rolter_gateway::AppState::with_logging(config, redis);
    serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await
}

/// POST a Responses request and read the answer to its end, so the stream has
/// finished and its row has been emitted.
async fn respond(gw: SocketAddr, request_id: &str, stream: bool) -> String {
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/responses"))
        .bearer_auth(key())
        .header("x-request-id", request_id)
        .json(&json!({"model": "test-model", "stream": stream, "input": "ping"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.text().await.unwrap()
}

/// The row the issue says was logged as a free, unknown request.
fn assert_billed(row: &Value) {
    assert_eq!(row["prompt_tokens"], 7, "request-log row");
    assert_eq!(row["completion_tokens"], 3, "request-log row");
    assert_eq!(row["total_tokens"], 10, "request-log row");
    assert_eq!(row["cost_usd"], 10.0, "request-log row");
    assert_eq!(row["unpriced"], 0, "request-log row");
    assert_eq!(row["usage_unknown"], 0, "request-log row");
}

// ── request-log rows: always run ────────────────────────────────────────────

/// The case from the issue: a streamed answer passed through to a provider
/// that speaks the Responses API.
#[tokio::test]
async fn a_streamed_responses_answer_is_logged_with_its_tokens_and_cost() {
    let upstream = responses_upstream(
        "text/event-stream",
        responses_stream("response.completed", "completed", counts()),
    )
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gw = gateway(
        &config(ProviderKind::Openai, upstream, clickhouse, "org-stream"),
        None,
    )
    .await;

    let body = respond(gw, "streamed-completed", true).await;

    // the client still gets the stream untouched
    assert!(body.contains("event: response.completed"), "answer body");
    let row = rows.row_for("streamed-completed").await;
    assert_eq!(row["status"], 200, "request-log row");
    assert_billed(&row);
}

/// A buffered answer reports the same numbers at the top level of the body, so
/// the two shapes of one answer must produce the same row.
#[tokio::test]
async fn a_buffered_responses_answer_is_logged_with_the_same_tokens_and_cost() {
    let upstream = responses_upstream(
        "application/json",
        json!({
            "id": "resp_1", "object": "response", "status": "completed", "usage": counts()
        })
        .to_string(),
    )
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gw = gateway(
        &config(ProviderKind::Openai, upstream, clickhouse, "org-buffered"),
        None,
    )
    .await;

    respond(gw, "buffered-completed", false).await;

    assert_billed(&rows.row_for("buffered-completed").await);
}

/// An answer cut short by `max_output_tokens` ends on `response.incomplete`,
/// not `response.completed`, and was still billed.
#[tokio::test]
async fn a_streamed_incomplete_responses_answer_is_logged_with_its_tokens() {
    let upstream = responses_upstream(
        "text/event-stream",
        responses_stream("response.incomplete", "incomplete", counts()),
    )
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gw = gateway(
        &config(ProviderKind::Openai, upstream, clickhouse, "org-incomplete"),
        None,
    )
    .await;

    respond(gw, "streamed-incomplete", true).await;

    assert_billed(&rows.row_for("streamed-incomplete").await);
}

/// A stream that ends without any usage is still logged as unknown rather than
/// as a free request: reading `response.usage` must not turn the `null` on
/// `response.created` into a report.
#[tokio::test]
async fn a_streamed_responses_answer_without_usage_stays_unknown() {
    let upstream = responses_upstream(
        "text/event-stream",
        responses_stream("response.completed", "completed", Value::Null),
    )
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gw = gateway(
        &config(ProviderKind::Openai, upstream, clickhouse, "org-silent"),
        None,
    )
    .await;

    respond(gw, "streamed-silent", true).await;

    let row = rows.row_for("streamed-silent").await;
    assert_eq!(row["total_tokens"], 0, "request-log row");
    assert_eq!(row["usage_unknown"], 1, "request-log row");
}

/// A Responses client on a provider that only speaks Chat Completions: the
/// gateway translates the stream, and what it logs is the translated
/// `response.completed`, whose usage sits under `response` too.
#[tokio::test]
async fn a_responses_stream_translated_from_chat_completions_is_logged_with_its_tokens() {
    let chat_stream = [
        json!({"id": "chat_1", "model": "up", "choices": [{"index": 0, "delta": {"content": "pong"}}]}),
        json!({"id": "chat_1", "model": "up", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}),
        json!({"id": "chat_1", "model": "up", "choices": [], "usage": {
            "prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10
        }}),
    ]
    .iter()
    .map(|chunk| format!("data: {chunk}\n\n"))
    .chain(["data: [DONE]\n\n".to_string()])
    .collect::<String>();
    async fn handler(State(body): State<String>) -> impl IntoResponse {
        ([(header::CONTENT_TYPE, "text/event-stream")], body)
    }
    let upstream = serve(
        Router::new()
            .route("/v1/chat/completions", post(handler))
            .with_state(chat_stream),
    )
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gw = gateway(
        &config(
            ProviderKind::OpenaiCompatible,
            upstream,
            clickhouse,
            "org-translated",
        ),
        None,
    )
    .await;

    let body = respond(gw, "streamed-translated", true).await;

    assert!(body.contains("event: response.completed"), "answer body");
    assert_billed(&rows.row_for("streamed-translated").await);
}

/// 500 fresh input tokens at one dollar, 1500 cached ones at ten cents and 3
/// output tokens at one dollar. Priced as if the cache had not been hit it is
/// 2003 dollars, which is what the row said before #2847.
fn assert_billed_at_the_cached_rate(row: &Value) {
    assert_eq!(row["prompt_tokens"], 2000, "request-log row");
    assert_eq!(row["cache_read_tokens"], 1500, "request-log row");
    assert_eq!(row["completion_tokens"], 3, "request-log row");
    assert_eq!(row["cost_usd"], 653.0, "request-log row");
    assert_eq!(row["unpriced"], 0, "request-log row");
    assert_eq!(row["usage_unknown"], 0, "request-log row");
}

/// #2847, buffered: the Responses API reports a prompt-cache hit as
/// `usage.input_tokens_details.cached_tokens`, which a price row with
/// `cached_input_per_mtok` turns into a discount.
#[tokio::test]
async fn a_buffered_responses_answer_with_cached_input_is_priced_at_the_cached_rate() {
    let upstream = responses_upstream(
        "application/json",
        json!({
            "id": "resp_1", "object": "response", "status": "completed",
            "usage": cached_counts()
        })
        .to_string(),
    )
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gw = gateway(
        &config(
            ProviderKind::Openai,
            upstream,
            clickhouse,
            "org-cached-buffered",
        ),
        None,
    )
    .await;

    respond(gw, "buffered-cached", false).await;

    assert_billed_at_the_cached_rate(&rows.row_for("buffered-cached").await);
}

/// #2847, streamed: the same figure on `response.completed`, under `response`.
#[tokio::test]
async fn a_streamed_responses_answer_with_cached_input_is_priced_at_the_cached_rate() {
    let upstream = responses_upstream(
        "text/event-stream",
        responses_stream("response.completed", "completed", cached_counts()),
    )
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gw = gateway(
        &config(
            ProviderKind::Openai,
            upstream,
            clickhouse,
            "org-cached-streamed",
        ),
        None,
    )
    .await;

    respond(gw, "streamed-cached", true).await;

    assert_billed_at_the_cached_rate(&rows.row_for("streamed-cached").await);
}

// ── budget counters: need redis ─────────────────────────────────────────────

/// Read the org's monthly spend counter, waiting for the asynchronous
/// recorder to land it. `None` once `settle` passes with nothing written.
async fn org_spend(redis: &str, org: &str, settle: Duration) -> Option<f64> {
    use redis::AsyncCommands;
    let client = redis::Client::open(redis).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let key = format!(
        "rolter:budget:org:{org}:{}",
        chrono::Utc::now().format("%Y%m")
    );
    let deadline = tokio::time::Instant::now() + settle;
    loop {
        let value: Option<String> = conn.get(&key).await.unwrap();
        if let Some(value) = value {
            return value.parse().ok();
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A per-run suffix, so tests sharing one redis never read each other's
/// counters.
fn unique(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

/// The other half of the issue: a streamed answer's cost reaches the budget it
/// is charged to.
#[tokio::test]
async fn a_streamed_responses_answer_advances_budget_spend() {
    let Some(redis) = std::env::var("ROLTER_TEST_REDIS_URL")
        .ok()
        .filter(|url| !url.is_empty())
    else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let upstream = responses_upstream(
        "text/event-stream",
        responses_stream("response.completed", "completed", counts()),
    )
    .await;
    let org = unique("org-stream-budget");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gw = gateway(
        &config(ProviderKind::Openai, upstream, clickhouse, &org),
        Some(&redis),
    )
    .await;

    respond(gw, "streamed-budget", true).await;

    assert_eq!(
        org_spend(&redis, &org, Duration::from_secs(5)).await,
        Some(10.0),
        "the streamed answer's cost must reach the org budget"
    );
}
