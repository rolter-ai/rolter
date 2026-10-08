//! Request-log rows for the calls that manage a stored response (#2836).
//!
//! `GET`, `DELETE`, `cancel` and `input_items` on `/v1/responses/{id}` used to
//! answer and leave nothing behind, so a lifecycle call that failed upstream
//! was visible only as the `502` its caller saw. They now follow the rule every
//! other request from an identified caller follows: one row, whatever
//! happened. The row names the model and the provider the response was created
//! on, says which operation it was, and carries no tokens and no cost.
//!
//! These tests drive the gateway over HTTP against a mock OpenAI upstream and
//! an in-process stand-in for the ClickHouse HTTP interface.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
    VirtualKeyRecord,
};
use serde_json::{json, Value};

const KEY: &str = "sk-lifecycle-log";
const KEY_ID: &str = "key-lifecycle";
const ORG: &str = "org-lifecycle";
const TEAM: &str = "team-lifecycle";
const PROJECT: &str = "project-lifecycle";
const MODEL: &str = "life-model";
const RESPONSE_ID: &str = "resp_life";

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// An address nothing listens on: bound to learn a free port, then released.
async fn dead_address() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap()
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

    fn matching(&self, request_id: &str) -> Vec<Value> {
        self.logs
            .lock()
            .iter()
            .filter(|row| row["request_id"] == request_id)
            .cloned()
            .collect()
    }

    /// The one row logged for `request_id`, once it has arrived and a flush
    /// interval later, so a second row that should not exist has had its
    /// chance to arrive.
    async fn row_for(&self, request_id: &str) -> Value {
        for _ in 0..200 {
            if !self.matching(request_id).is_empty() {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let mut rows = self.matching(request_id);
                assert_eq!(rows.len(), 1, "{request_id} must leave exactly one row");
                return rows.remove(0);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "no request-log row for {request_id}; saw {:?}",
            self.logs.lock()
        );
    }

    /// Wait out a flush interval and say how many rows have arrived, for
    /// requests that must leave none.
    async fn row_count_after_settling(&self) -> usize {
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.logs.lock().len()
    }
}

/// What a lifecycle call on the mock upstream answers with.
#[derive(Clone)]
struct Lifecycle {
    status: StatusCode,
}

async fn lifecycle(State(life): State<Lifecycle>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let body = if life.status.is_success() {
        json!({"id": RESPONSE_ID, "object": "response", "data": []})
    } else {
        json!({"error": {"message": "the upstream said no", "type": "server_error"}})
    };
    (life.status, Json(body)).into_response()
}

/// A native OpenAI upstream that creates `RESPONSE_ID` and answers every
/// lifecycle call with `status`.
async fn upstream(status: StatusCode) -> SocketAddr {
    async fn create() -> Json<Value> {
        Json(json!({
            "id": RESPONSE_ID,
            "object": "response",
            "status": "completed",
            "usage": {"input_tokens": 3, "output_tokens": 4, "total_tokens": 7}
        }))
    }
    async fn chat() -> Json<Value> {
        Json(json!({
            "id": "chatcmpl-life",
            "object": "chat.completion",
            "model": "upstream-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hello"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7}
        }))
    }
    serve(
        Router::new()
            .route("/v1/chat/completions", post(chat))
            .route("/v1/responses", post(create))
            .route("/v1/responses/{id}", get(lifecycle).delete(lifecycle))
            .route("/v1/responses/{id}/cancel", post(lifecycle))
            .route("/v1/responses/{id}/input_items", get(lifecycle))
            .with_state(Lifecycle { status }),
    )
    .await
}

fn key(config: &GatewayConfig) -> VirtualKeyRecord {
    VirtualKeyRecord {
        access_policy: None,
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), KEY),
        id: KEY_ID.into(),
        org_id: ORG.into(),
        team_id: TEAM.into(),
        project_id: PROJECT.into(),
        user_id: String::new(),
        models: vec![],
        providers: vec![],
        disabled: false,
        expires_at: None,
        cache: None,
        business_unit_id: String::new(),
        customer_id: String::new(),
    }
}

/// One route, `MODEL`, over one provider named `native`, with request logs
/// going to `clickhouse`.
fn config_over(clickhouse: SocketAddr, kind: ProviderKind, api_base: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
    config.providers.push(ProviderConfig {
        name: "native".into(),
        kind,
        api_base: format!("http://{api_base}"),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: MODEL.into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "native".into(),
            model: Some("upstream-model".into()),
            weight: 1,
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    let owner = key(&config);
    config.db_virtual_keys.push(owner);
    config
}

struct Gateway {
    addr: SocketAddr,
    state: rolter_gateway::AppState,
}

async fn gateway(config: &GatewayConfig) -> Gateway {
    let state = rolter_gateway::AppState::with_logging(config, None);
    let addr = serve(rolter_gateway::build_router(
        state.clone(),
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    Gateway { addr, state }
}

/// POST a Responses API request for `MODEL`. The body is read to the end,
/// because the response is only registered for lifecycle calls once it has
/// been forwarded in full.
async fn create(gw: &Gateway, request: &str) -> (StatusCode, Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/responses", gw.addr))
        .bearer_auth(KEY)
        .header("x-request-id", request)
        .json(&json!({"model": MODEL, "input": "hello"}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// Create `RESPONSE_ID` through the gateway, and wait until its own row has
/// arrived so that the rows a test counts afterwards are the lifecycle ones.
async fn create_response(gw: &Gateway, rows: &Rows) {
    let (status, created) = create(gw, "create").await;
    assert_eq!(status, 200);
    assert_eq!(created["id"], RESPONSE_ID);
    let row = rows.row_for("create").await;
    assert_eq!(
        row["total_tokens"], 7,
        "the create call is the billable one"
    );
    assert!(
        row.get("lifecycle_operation").is_none(),
        "a model request's row does not carry the column: {row}"
    );
}

/// The four lifecycle operations, `delete` last since it forgets the response.
const OPERATIONS: [&str; 4] = ["retrieve", "input_items", "cancel", "delete"];

/// Make the lifecycle call `operation` on `id`, with the request id `request`.
async fn call(gw: &Gateway, operation: &str, id: &str, request: &str) -> reqwest::Response {
    let client = reqwest::Client::new();
    let base = format!("http://{}/v1/responses/{id}", gw.addr);
    let builder = match operation {
        "retrieve" => client.get(base),
        "input_items" => client.get(format!("{base}/input_items")),
        "cancel" => client.post(format!("{base}/cancel")),
        "delete" => client.delete(base),
        other => panic!("unknown operation {other}"),
    };
    builder
        .bearer_auth(KEY)
        .header("x-request-id", request)
        .send()
        .await
        .unwrap()
}

/// What every lifecycle row has in common: the caller's attribution, the
/// operation, and nothing a billable request would carry.
fn assert_lifecycle_row(row: &Value, operation: &str) {
    assert_eq!(row["virtual_key_id"], KEY_ID, "{row}");
    assert_eq!(row["org_id"], ORG, "{row}");
    assert_eq!(row["team_id"], TEAM, "{row}");
    assert_eq!(row["project_id"], PROJECT, "{row}");
    assert_eq!(row["model"], MODEL, "{row}");
    assert_eq!(row["lifecycle_operation"], operation, "{row}");
    assert_eq!(row["variant"], "", "{row}");
    assert_eq!(row["stream"], 0, "{row}");
    for tokens in [
        "prompt_tokens",
        "completion_tokens",
        "total_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
    ] {
        assert_eq!(row[tokens], 0, "{tokens}: {row}");
    }
    assert_eq!(row["cost_usd"], 0.0, "{row}");
    // a call that generates nothing has no usage to be unknown and no price to
    // be missing
    assert_eq!(row["unpriced"], 0, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
    assert_eq!(row["withheld"], 0, "{row}");
    assert!(row["latency_ms"].is_number(), "{row}");
}

#[tokio::test]
async fn every_lifecycle_call_leaves_one_row_naming_its_operation() {
    let up = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let config = config_over(rows.serve().await, ProviderKind::Openai, up);
    let gw = gateway(&config).await;
    create_response(&gw, &rows).await;

    for operation in OPERATIONS {
        let request = format!("life-{operation}");
        let response = call(&gw, operation, RESPONSE_ID, &request).await;
        assert_eq!(response.status(), 200, "{operation}");
        let _ = response.bytes().await.unwrap();

        let row = rows.row_for(&request).await;
        assert_lifecycle_row(&row, operation);
        assert_eq!(row["status"], 200, "{row}");
        // the provider and target are the ones the response was created on
        assert_eq!(row["provider"], "native", "{row}");
        assert_eq!(row["target"], "upstream-model", "{row}");
        assert_eq!(row["upstream_status"], 200, "{row}");
        assert_eq!(row["attempts"], 1, "{row}");
        assert_eq!(row["error"], "", "{row}");
    }
}

/// A provider that answers a lifecycle call with an error: the caller gets
/// the provider's own answer, as before, and the operator finds the call.
#[tokio::test]
async fn a_lifecycle_call_the_provider_fails_is_logged_with_its_status() {
    let up = upstream(StatusCode::INTERNAL_SERVER_ERROR).await;
    let rows = Rows::default();
    let config = config_over(rows.serve().await, ProviderKind::Openai, up);
    let gw = gateway(&config).await;
    create_response(&gw, &rows).await;

    for operation in OPERATIONS {
        let request = format!("failed-{operation}");
        let response = call(&gw, operation, RESPONSE_ID, &request).await;
        assert_eq!(response.status(), 500, "{operation}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["message"], "the upstream said no");

        let row = rows.row_for(&request).await;
        assert_lifecycle_row(&row, operation);
        assert_eq!(row["status"], 500, "{row}");
        assert_eq!(row["upstream_status"], 500, "{row}");
        assert_eq!(row["attempts"], 1, "{row}");
        assert_eq!(row["provider"], "native", "{row}");
        assert!(
            row["error"]
                .as_str()
                .unwrap()
                .contains("upstream returned 500"),
            "{row}"
        );
    }
}

/// A failed `DELETE` leaves the response in the registry, and a `404` from the
/// provider is logged as the provider's answer rather than a gateway one.
#[tokio::test]
async fn a_provider_that_has_forgotten_the_response_is_logged_as_its_404() {
    let up = upstream(StatusCode::NOT_FOUND).await;
    let rows = Rows::default();
    let config = config_over(rows.serve().await, ProviderKind::Openai, up);
    let gw = gateway(&config).await;
    create_response(&gw, &rows).await;

    let response = call(&gw, "retrieve", RESPONSE_ID, "forgotten").await;
    assert_eq!(response.status(), 404);

    let row = rows.row_for("forgotten").await;
    assert_lifecycle_row(&row, "retrieve");
    assert_eq!(row["status"], 404, "{row}");
    assert_eq!(row["upstream_status"], 404, "{row}");
    assert_eq!(row["provider"], "native", "{row}");
}

/// The provider cannot be reached at all: a `502`, no upstream status because
/// no status line came back, and the one attempt that was made.
#[tokio::test]
async fn a_provider_that_cannot_be_reached_is_logged_as_a_bad_gateway() {
    let up = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let config = config_over(rows.serve().await, ProviderKind::Openai, up);
    let gw = gateway(&config).await;
    create_response(&gw, &rows).await;

    // the provider moves to an address nothing listens on; the stored response
    // stays in the registry across the reload
    let dead = dead_address().await;
    let mut moved = config.clone();
    moved.providers[0].api_base = format!("http://{dead}");
    gw.state.reload(&moved, 2);

    let response = call(&gw, "retrieve", RESPONSE_ID, "unreachable").await;
    assert_eq!(response.status(), 502);

    let row = rows.row_for("unreachable").await;
    assert_lifecycle_row(&row, "retrieve");
    assert_eq!(row["status"], 502, "{row}");
    assert_eq!(row["upstream_status"], 0, "{row}");
    assert_eq!(row["attempts"], 1, "{row}");
    assert_eq!(row["provider"], "native", "{row}");
    assert!(!row["error"].as_str().unwrap().is_empty(), "{row}");
}

/// A call the gateway refuses before it reaches the provider still leaves a
/// row: the model it was about, no provider or target, no attempt.
#[tokio::test]
async fn a_lifecycle_call_the_gateway_refuses_leaves_a_row_with_no_upstream() {
    let up = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let config = config_over(rows.serve().await, ProviderKind::Openai, up);
    let gw = gateway(&config).await;
    create_response(&gw, &rows).await;

    // access revoked since the response was created
    let mut narrowed = config.clone();
    narrowed.db_virtual_keys[0].models = vec!["another-model".into()];
    gw.state.reload(&narrowed, 2);

    for operation in OPERATIONS {
        let request = format!("refused-{operation}");
        let response = call(&gw, operation, RESPONSE_ID, &request).await;
        assert_eq!(response.status(), 403, "{operation}");

        let row = rows.row_for(&request).await;
        assert_lifecycle_row(&row, operation);
        assert_eq!(row["status"], 403, "{row}");
        assert_eq!(row["provider"], "", "{row}");
        assert_eq!(row["target"], "", "{row}");
        assert_eq!(row["upstream_status"], 0, "{row}");
        assert_eq!(row["attempts"], 0, "{row}");
        assert!(
            row["error"].as_str().unwrap().contains("model not allowed"),
            "{row}"
        );
    }
}

/// A response created on a provider that has no lifecycle API answers `501`,
/// and the row says so.
#[tokio::test]
async fn an_operation_the_provider_does_not_support_leaves_a_row() {
    let up = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let config = config_over(rows.serve().await, ProviderKind::OpenaiCompatible, up);
    let gw = gateway(&config).await;
    // a compatible provider has no Responses API, so the gateway serves the
    // call over chat completions and mints the response's id itself
    let (status, created) = create(&gw, "create-translated").await;
    assert_eq!(status, 200);
    let id = created["id"].as_str().unwrap().to_string();
    rows.row_for("create-translated").await;

    let response = call(&gw, "retrieve", &id, "unsupported").await;
    assert_eq!(response.status(), 501);

    let row = rows.row_for("unsupported").await;
    assert_lifecycle_row(&row, "retrieve");
    assert_eq!(row["status"], 501, "{row}");
    assert_eq!(row["provider"], "", "{row}");
    assert_eq!(row["attempts"], 0, "{row}");
    assert!(
        row["error"]
            .as_str()
            .unwrap()
            .contains("not supported by the originating provider"),
        "{row}"
    );
}

/// An id the caller never created, or that has expired, or that belongs to
/// another tenant: no stored route, so no model, and the same row either way.
#[tokio::test]
async fn an_unknown_response_id_leaves_a_row_without_a_model() {
    let up = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let config = config_over(rows.serve().await, ProviderKind::Openai, up);
    let gw = gateway(&config).await;

    for operation in OPERATIONS {
        let request = format!("unknown-{operation}");
        let response = call(&gw, operation, "resp_nobody_made", &request).await;
        assert_eq!(response.status(), 404, "{operation}");

        let row = rows.row_for(&request).await;
        assert_eq!(row["virtual_key_id"], KEY_ID, "{row}");
        assert_eq!(row["org_id"], ORG, "{row}");
        assert_eq!(row["lifecycle_operation"], operation, "{row}");
        assert_eq!(row["status"], 404, "{row}");
        assert_eq!(row["model"], "", "{row}");
        assert_eq!(row["provider"], "", "{row}");
        assert_eq!(row["attempts"], 0, "{row}");
        assert_eq!(row["total_tokens"], 0, "{row}");
        assert_eq!(row["error"], "response not found", "{row}");
        // the caller's id is not echoed into the log
        assert!(
            !row.to_string().contains("resp_nobody_made"),
            "the id the caller sent reached the row: {row}"
        );
    }
}

/// The two lifecycle routes the gateway answers `501` for are requests from
/// an identified caller like any other.
#[tokio::test]
async fn the_routes_the_gateway_does_not_serve_leave_a_row() {
    let up = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let config = config_over(rows.serve().await, ProviderKind::Openai, up);
    let gw = gateway(&config).await;
    let client = reqwest::Client::new();

    let compact = client
        .post(format!("http://{}/v1/responses/resp_a/compact", gw.addr))
        .bearer_auth(KEY)
        .header("x-request-id", "compact")
        .send()
        .await
        .unwrap();
    assert_eq!(compact.status(), 501);
    let tokens = client
        .get(format!(
            "http://{}/v1/responses/resp_a/input_tokens",
            gw.addr
        ))
        .bearer_auth(KEY)
        .header("x-request-id", "input-tokens")
        .send()
        .await
        .unwrap();
    assert_eq!(tokens.status(), 501);

    for (request, operation) in [("compact", "compact"), ("input-tokens", "input_tokens")] {
        let row = rows.row_for(request).await;
        assert_eq!(row["virtual_key_id"], KEY_ID, "{row}");
        assert_eq!(row["lifecycle_operation"], operation, "{row}");
        assert_eq!(row["status"], 501, "{row}");
        assert_eq!(row["provider"], "", "{row}");
        assert_eq!(row["attempts"], 0, "{row}");
    }
}

/// Nobody owns a row for a caller the gateway cannot identify, and anonymous
/// traffic could otherwise write to the log without limit.
#[tokio::test]
async fn a_lifecycle_call_from_an_unknown_caller_leaves_no_row() {
    let up = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let config = config_over(rows.serve().await, ProviderKind::Openai, up);
    let gw = gateway(&config).await;
    let client = reqwest::Client::new();

    let missing = client
        .get(format!("http://{}/v1/responses/{RESPONSE_ID}", gw.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 401);
    let wrong = client
        .delete(format!("http://{}/v1/responses/{RESPONSE_ID}", gw.addr))
        .bearer_auth("sk-not-a-key")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);
    let compact = client
        .post(format!("http://{}/v1/responses/resp_a/compact", gw.addr))
        .bearer_auth("sk-not-a-key")
        .send()
        .await
        .unwrap();
    assert_eq!(compact.status(), 401);

    assert_eq!(rows.row_count_after_settling().await, 0);
}

/// A lifecycle call is observable in the log but is not a generation: it must
/// not land in the per-model latency histograms, whose percentiles describe
/// completions.
#[tokio::test]
async fn lifecycle_calls_stay_out_of_the_latency_histograms() {
    let up = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let config = config_over(rows.serve().await, ProviderKind::Openai, up);
    let gw = gateway(&config).await;
    create_response(&gw, &rows).await;

    let count = |text: &str| -> u64 {
        text.lines()
            .find(|line| {
                line.starts_with("rolter_request_latency_ms_count{")
                    && line.contains(&format!("model=\"{MODEL}\""))
            })
            .and_then(|line| line.rsplit(' ').next())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    };
    let scrape = || async {
        reqwest::get(format!("http://{}/metrics", gw.addr))
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    };
    let before = count(&scrape().await);
    assert_eq!(before, 1, "the create call is one observation");

    for operation in OPERATIONS {
        let request = format!("hist-{operation}");
        let response = call(&gw, operation, RESPONSE_ID, &request).await;
        assert_eq!(response.status(), 200);
        let _ = response.bytes().await.unwrap();
        rows.row_for(&request).await;
    }
    assert_eq!(count(&scrape().await), before);
}

// ── lifecycle calls are not billable ────────────────────────────────────────
//
// The rate-limit window lives in Redis, so this reads `ROLTER_TEST_REDIS_URL`
// and skips when it is unset, the same contract the other suites keep.

fn redis_url() -> Option<String> {
    let url = std::env::var("ROLTER_TEST_REDIS_URL").ok()?;
    (!url.is_empty()).then_some(url)
}

/// A key that may make two requests a minute makes two model requests however
/// many lifecycle calls came between them: the calls neither consume an
/// admission nor are refused when the window is full.
#[tokio::test]
async fn lifecycle_calls_do_not_consume_rate_limit_admissions() {
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let up = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let mut config = config_over(rows.serve().await, ProviderKind::Openai, up);
    // a key and org of this run alone, so the window is not shared with
    // another run on the same redis
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let key_id = format!("key-life-{}-{nanos}", std::process::id());
    config.db_virtual_keys[0].id = key_id.clone();
    config
        .rate_limits
        .push(serde_json::from_value(json!({"scope": "key", "id": key_id, "rpm": 2})).unwrap());
    let state = rolter_gateway::AppState::with_logging(&config, Some(&redis));
    let gw = Gateway {
        addr: serve(rolter_gateway::build_router(
            state.clone(),
            "/metrics",
            32 * 1024 * 1024,
        ))
        .await,
        state,
    };
    // the first of the two admissions
    assert_eq!(create(&gw, "first").await.0, 200);
    // five calls on its response: none of them takes the second, and none is
    // refused for the window being anything other than empty
    for round in 0..5 {
        let operation = OPERATIONS[round % 3];
        let response = call(&gw, operation, RESPONSE_ID, &format!("between-{round}")).await;
        assert_eq!(response.status(), 200, "{operation} #{round}");
    }
    // the second admission is still there, and the third is not
    assert_eq!(create(&gw, "second").await.0, 200);
    assert_eq!(create(&gw, "third").await.0, 429);
}

/// The create call spends against the org's budget and the lifecycle calls on
/// its response do not: a `GET` generates nothing, so it has nothing to price.
#[tokio::test]
async fn lifecycle_calls_do_not_charge_the_budget() {
    use redis::AsyncCommands;
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let up = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let mut config = config_over(rows.serve().await, ProviderKind::Openai, up);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let org = format!("org-life-{}-{nanos}", std::process::id());
    config.db_virtual_keys[0].org_id = org.clone();
    // a dollar a token, so the create call's seven tokens cost seven dollars
    for model in [MODEL, "upstream-model"] {
        config.model_prices.push(
            serde_json::from_value(json!({
                "model": model,
                "input_per_mtok": 1_000_000,
                "output_per_mtok": 1_000_000
            }))
            .unwrap(),
        );
    }
    config.budgets.push(
        serde_json::from_value(json!({
            "scope": "org", "id": org, "limit_usd": 100, "period": "monthly"
        }))
        .unwrap(),
    );
    let state = rolter_gateway::AppState::with_logging(&config, Some(&redis));
    let gw = Gateway {
        addr: serve(rolter_gateway::build_router(
            state.clone(),
            "/metrics",
            32 * 1024 * 1024,
        ))
        .await,
        state,
    };
    let client = redis::Client::open(redis.as_str()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let counter = format!(
        "rolter:budget:org:{org}:{}",
        chrono::Utc::now().format("%Y%m")
    );

    create_response(&gw, &rows).await;
    // spend is recorded off the request path, so wait for the create call's
    let mut spent = 0.0_f64;
    for _ in 0..200 {
        spent = conn
            .get::<_, Option<f64>>(&counter)
            .await
            .unwrap()
            .unwrap_or(0.0);
        if spent > 0.0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(spent > 0.0, "the create call must have been charged");

    for operation in OPERATIONS {
        let response = call(&gw, operation, RESPONSE_ID, &format!("budget-{operation}")).await;
        assert_eq!(response.status(), 200, "{operation}");
        let _ = response.bytes().await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after: f64 = conn
        .get::<_, Option<f64>>(&counter)
        .await
        .unwrap()
        .unwrap_or(0.0);
    assert_eq!(after, spent, "a lifecycle call must not move the counter");
}
