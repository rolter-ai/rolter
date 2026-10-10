//! Request-log rows for requests that fail after upstream errors, and for
//! requests the gateway refuses before they reach one (#2807).
//!
//! Found against a route with one target on OpenRouter's free tier: calling
//! the provider directly answered `429 … temporarily rate-limited upstream`,
//! while through the gateway the caller got `503 no target selected` and LLM
//! Logs held no row for the request at all. "No target selected" points at
//! routing config when the cause was an upstream that said no.
//!
//! These tests drive the gateway over HTTP against mock upstreams and an
//! in-process stand-in for the ClickHouse HTTP interface.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, GatewayConfig, GuardAction, ModelRoute, ProviderConfig, ProviderKind,
    Target, VirtualKeyRecord,
};
use serde_json::{json, Value};

const KEY: &str = "sk-upstream-failure-log";
const KEY_ID: &str = "key-failure";
const ORG: &str = "org-failure";
const TEAM: &str = "team-failure";
const PROJECT: &str = "project-failure";
/// A key that may only address `allowed-only`, for the 403 refusal.
const RESTRICTED_KEY: &str = "sk-upstream-failure-restricted";

/// What OpenRouter answers when the model behind it is rate limited: the
/// provider's own words are one level down, in `metadata.raw`.
fn rate_limited_body() -> Value {
    json!({"error": {
        "message": "Provider returned error",
        "code": 429,
        "metadata": {
            "raw": "google/gemma-4-31b-it:free is temporarily rate-limited upstream",
            "provider_name": "Google AI Studio"
        }
    }})
}

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

    /// Every row logged for `request_id`, once one has arrived.
    async fn rows_for(&self, request_id: &str) -> Vec<Value> {
        for _ in 0..200 {
            let found: Vec<Value> = self
                .logs
                .lock()
                .iter()
                .filter(|row| row["request_id"] == request_id)
                .cloned()
                .collect();
            if !found.is_empty() {
                // one more flush interval, so a second row that should not
                // exist has had its chance to arrive
                tokio::time::sleep(Duration::from_millis(100)).await;
                return self
                    .logs
                    .lock()
                    .iter()
                    .filter(|row| row["request_id"] == request_id)
                    .cloned()
                    .collect();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "no request-log row for {request_id}; saw {:?}",
            self.logs.lock()
        );
    }

    /// The one row logged for `request_id`.
    async fn row_for(&self, request_id: &str) -> Value {
        let mut rows = self.rows_for(request_id).await;
        assert_eq!(rows.len(), 1, "{request_id} must leave exactly one row");
        rows.remove(0)
    }

    /// Wait out a flush interval and say how many rows have arrived, for a
    /// request that must leave none.
    async fn row_count_after_settling(&self) -> usize {
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.logs.lock().len()
    }
}

/// A mock upstream that answers every call with `status` and `body`, counting
/// the calls it gets.
struct Upstream {
    addr: SocketAddr,
    calls: Arc<AtomicU32>,
}

#[derive(Clone)]
struct Canned {
    status: StatusCode,
    body: Value,
    retry_after: Option<&'static str>,
    calls: Arc<AtomicU32>,
}

async fn canned(State(canned): State<Canned>) -> axum::response::Response {
    canned.calls.fetch_add(1, Ordering::SeqCst);
    let mut response = (canned.status, Json(canned.body)).into_response();
    if let Some(after) = canned.retry_after {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, after.parse().unwrap());
    }
    response
}

/// An upstream that answers `path` with `status` and `body`.
async fn upstream(
    path: &str,
    status: StatusCode,
    body: Value,
    retry_after: Option<&'static str>,
) -> Upstream {
    let calls = Arc::new(AtomicU32::new(0));
    let addr = serve(Router::new().route(path, post(canned)).with_state(Canned {
        status,
        body,
        retry_after,
        calls: calls.clone(),
    }))
    .await;
    Upstream { addr, calls }
}

fn provider(name: &str, kind: ProviderKind, addr: SocketAddr) -> ProviderConfig {
    ProviderConfig {
        name: name.into(),
        kind,
        api_base: format!("http://{addr}"),
        ..Default::default()
    }
}

fn target(provider: &str) -> Target {
    Target {
        provider: provider.into(),
        model: Some("test-model".into()),
        weight: 1,
    }
}

fn route(model: &str, targets: Vec<Target>) -> ModelRoute {
    ModelRoute {
        model: model.into(),
        strategy: BalancingStrategy::RoundRobin,
        targets,
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    }
}

fn key(config: &GatewayConfig, secret: &str, id: &str, models: Vec<String>) -> VirtualKeyRecord {
    VirtualKeyRecord {
        access_policy: None,
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), secret),
        id: id.into(),
        org_id: ORG.into(),
        team_id: TEAM.into(),
        project_id: PROJECT.into(),
        user_id: String::new(),
        models,
        providers: vec![],
        disabled: false,
        expires_at: None,
        cache: None,
        business_unit_id: String::new(),
        customer_id: String::new(),
    }
}

/// A gateway config with request logs going to `clickhouse`, no backoff, and
/// the two keys the tests use.
fn base_config(clickhouse: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
    config.retry.base_backoff_ms = 0;
    config.retry.max_backoff_ms = 0;
    let all = key(&config, KEY, KEY_ID, vec![]);
    let restricted = key(
        &config,
        RESTRICTED_KEY,
        "key-restricted",
        vec!["allowed-only".into()],
    );
    config.db_virtual_keys.push(all);
    config.db_virtual_keys.push(restricted);
    config
}

/// One route, `test-model`, over `providers` in order.
fn config_over(clickhouse: SocketAddr, providers: Vec<ProviderConfig>) -> GatewayConfig {
    let mut config = base_config(clickhouse);
    let targets = providers.iter().map(|p| target(&p.name)).collect();
    config.providers = providers;
    config.routes.push(route("test-model", targets));
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

/// POST a chat completion for `model` with the request id `id`.
async fn chat(gw: SocketAddr, id: &str, model: &str) -> reqwest::Response {
    chat_as(gw, KEY, id, model).await
}

async fn chat_as(gw: SocketAddr, secret: &str, id: &str, model: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .bearer_auth(secret)
        .header("x-request-id", id)
        .json(&json!({"model": model, "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap()
}

/// POST an Anthropic Messages request for `model` with the request id `id`.
async fn message(gw: SocketAddr, id: &str, model: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}/v1/messages"))
        .bearer_auth(KEY)
        .header("x-request-id", id)
        .json(&json!({
            "model": model,
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap()
}

async fn error_body(response: reqwest::Response) -> Value {
    response.json::<Value>().await.unwrap()["error"].clone()
}

/// What every row for a request from `KEY` has in common.
fn assert_attributed(row: &Value) {
    assert_eq!(row["virtual_key_id"], KEY_ID, "{row}");
    assert_eq!(row["org_id"], ORG, "{row}");
    assert_eq!(row["team_id"], TEAM, "{row}");
    assert_eq!(row["project_id"], PROJECT, "{row}");
}

// ── requests that fail after reaching an upstream ───────────────────────────

/// The case from the livetest: one target, and the upstream says 429.
#[tokio::test]
async fn a_single_target_route_whose_upstream_answers_429_logs_it_and_stays_a_rate_limit() {
    let up = upstream(
        "/v1/chat/completions",
        StatusCode::TOO_MANY_REQUESTS,
        rate_limited_body(),
        Some("1"),
    )
    .await;
    let rows = Rows::default();
    let config = config_over(
        rows.serve().await,
        vec![provider("up", ProviderKind::OpenaiCompatible, up.addr)],
    );
    let gw = gateway(&config).await;

    let response = chat(gw, "rate-limited", "test-model").await;

    // the caller is told what happened, in the status a client backs off on
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers()[header::RETRY_AFTER], "1");
    let error = error_body(response).await;
    assert_eq!(error["type"], "rate_limit_error");
    assert_eq!(error["code"], "upstream_rate_limited");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("429"), "{message}");
    assert!(!message.contains("no target selected"), "{message}");
    assert_eq!(up.calls.load(Ordering::SeqCst), 1, "one target, one try");

    // and the operator finds the request, with the reason, in LLM Logs
    let row = rows.row_for("rate-limited").await;
    assert_attributed(&row);
    assert_eq!(row["model"], "test-model", "{row}");
    assert_eq!(row["status"], 429, "{row}");
    assert_eq!(row["upstream_status"], 429, "{row}");
    assert_eq!(row["attempts"], 1, "{row}");
    assert_eq!(row["provider"], "up", "{row}");
    assert_eq!(row["target"], "test-model", "{row}");
    let error = row["error"].as_str().unwrap();
    assert!(error.contains("upstream returned 429"), "{error}");
    assert!(error.contains("Provider returned error"), "{error}");
    assert!(
        error.contains("temporarily rate-limited upstream"),
        "the provider's own words were lost: {error}"
    );
    assert!(row["latency_ms"].is_number(), "{row}");
}

/// An Anthropic-dialect caller gets the same answer: the gateway has one error
/// envelope for every dialect, so the status, type and code agree.
#[tokio::test]
async fn an_anthropic_caller_gets_the_same_rate_limit_answer() {
    let up = upstream(
        "/v1/messages",
        StatusCode::TOO_MANY_REQUESTS,
        json!({"type": "error", "error": {"type": "rate_limit_error", "message": "Number of requests has exceeded your rate limit"}}),
        None,
    )
    .await;
    let rows = Rows::default();
    let config = config_over(
        rows.serve().await,
        vec![provider("claude", ProviderKind::Anthropic, up.addr)],
    );
    let gw = gateway(&config).await;

    let response = message(gw, "anthropic-rate-limited", "test-model").await;

    assert_eq!(response.status(), 429);
    let error = error_body(response).await;
    assert_eq!(error["type"], "rate_limit_error");
    assert_eq!(error["code"], "upstream_rate_limited");

    let row = rows.row_for("anthropic-rate-limited").await;
    assert_eq!(row["status"], 429, "{row}");
    assert_eq!(row["upstream_status"], 429, "{row}");
    assert_eq!(row["provider"], "claude", "{row}");
    assert!(
        row["error"]
            .as_str()
            .unwrap()
            .contains("exceeded your rate limit"),
        "{row}"
    );
}

/// Failover exhaustion: every target failed, and the row is the last one's.
#[tokio::test]
async fn failover_exhaustion_logs_the_last_attempt_and_counts_them_all() {
    let first = upstream(
        "/v1/chat/completions",
        StatusCode::BAD_GATEWAY,
        json!({"error": {"message": "first is down"}}),
        None,
    )
    .await;
    let second = upstream(
        "/v1/chat/completions",
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error": {"message": "second is overloaded"}}),
        None,
    )
    .await;
    let rows = Rows::default();
    let config = config_over(
        rows.serve().await,
        vec![
            provider("first", ProviderKind::OpenaiCompatible, first.addr),
            provider("second", ProviderKind::OpenaiCompatible, second.addr),
        ],
    );
    let gw = gateway(&config).await;

    let response = chat(gw, "exhausted", "test-model").await;

    assert_eq!(response.status(), 503);
    let error = error_body(response).await;
    assert_eq!(error["type"], "overloaded_error");
    assert_eq!(error["code"], "upstream_unavailable");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("cooling down"), "{message}");
    assert!(message.contains("503"), "{message}");
    assert_eq!(first.calls.load(Ordering::SeqCst), 1);
    assert_eq!(second.calls.load(Ordering::SeqCst), 1);

    let row = rows.row_for("exhausted").await;
    assert_attributed(&row);
    assert_eq!(row["status"], 503, "{row}");
    assert_eq!(row["upstream_status"], 503, "{row}");
    assert_eq!(row["attempts"], 2, "{row}");
    // the last attempt, not the first: it is the one that ended the request
    assert_eq!(row["provider"], "second", "{row}");
    let error = row["error"].as_str().unwrap();
    assert!(error.contains("second is overloaded"), "{error}");
    assert!(!error.contains("first is down"), "{error}");
}

/// A route whose targets are variants fails the same way.
#[tokio::test]
async fn a_variant_route_that_runs_out_of_targets_logs_it_with_its_variant() {
    let up = upstream(
        "/v1/chat/completions",
        StatusCode::TOO_MANY_REQUESTS,
        rate_limited_body(),
        None,
    )
    .await;
    let rows = Rows::default();
    let mut config = config_over(
        rows.serve().await,
        vec![provider("up", ProviderKind::OpenaiCompatible, up.addr)],
    );
    let route = &mut config.routes[0];
    route.variants = vec![rolter_core::Variant {
        name: "canary".into(),
        weight: 1,
        targets: std::mem::take(&mut route.targets),
        params: Default::default(),
    }];
    let gw = gateway(&config).await;

    let response = chat(gw, "variant-rate-limited", "test-model").await;

    assert_eq!(response.status(), 429);
    let row = rows.row_for("variant-rate-limited").await;
    assert_eq!(row["variant"], "canary", "{row}");
    assert_eq!(row["status"], 429, "{row}");
    assert_eq!(row["upstream_status"], 429, "{row}");
    assert_eq!(row["attempts"], 1, "{row}");
    assert!(
        row["error"]
            .as_str()
            .unwrap()
            .contains("temporarily rate-limited upstream"),
        "{row}"
    );
}

/// With no retry budget the upstream's own answer is what the caller gets, as
/// before, and its row now carries the reason the body gave.
#[tokio::test]
async fn an_upstream_error_handed_back_as_it_was_keeps_its_reason_in_the_row() {
    let up = upstream(
        "/v1/chat/completions",
        StatusCode::TOO_MANY_REQUESTS,
        rate_limited_body(),
        None,
    )
    .await;
    let rows = Rows::default();
    let mut config = config_over(
        rows.serve().await,
        vec![provider("up", ProviderKind::OpenaiCompatible, up.addr)],
    );
    config.retry.max_retries = 0;
    let gw = gateway(&config).await;

    let response = chat(gw, "passthrough", "test-model").await;

    // the upstream's body, untouched
    assert_eq!(response.status(), 429);
    assert_eq!(response.json::<Value>().await.unwrap(), rate_limited_body());

    let row = rows.row_for("passthrough").await;
    assert_eq!(row["status"], 429, "{row}");
    assert_eq!(row["upstream_status"], 429, "{row}");
    assert_eq!(row["attempts"], 1, "{row}");
    assert_eq!(row["provider"], "up", "{row}");
    let error = row["error"].as_str().unwrap();
    assert!(error.starts_with("upstream returned 429: "), "{error}");
    assert!(
        error.contains("temporarily rate-limited upstream"),
        "{error}"
    );
}

/// A request an upstream answered successfully keeps an empty `error` and says
/// how it got there.
#[tokio::test]
async fn a_request_that_succeeded_after_a_failover_records_its_attempts() {
    let down = upstream(
        "/v1/chat/completions",
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error": {"message": "down"}}),
        None,
    )
    .await;
    let up = upstream(
        "/v1/chat/completions",
        StatusCode::OK,
        json!({"id": "x", "choices": [], "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}),
        None,
    )
    .await;
    let rows = Rows::default();
    let config = config_over(
        rows.serve().await,
        vec![
            provider("down", ProviderKind::OpenaiCompatible, down.addr),
            provider("up", ProviderKind::OpenaiCompatible, up.addr),
        ],
    );
    let gw = gateway(&config).await;

    let response = chat(gw, "recovered", "test-model").await;
    assert_eq!(response.status(), 200);
    let _ = response.bytes().await.unwrap();

    let row = rows.row_for("recovered").await;
    assert_eq!(row["status"], 200, "{row}");
    assert_eq!(row["upstream_status"], 200, "{row}");
    assert_eq!(row["attempts"], 2, "{row}");
    assert_eq!(row["provider"], "up", "{row}");
    assert_eq!(row["error"], "", "{row}");
}

/// A connection that fails before any status line has no upstream status.
#[tokio::test]
async fn a_connection_that_fails_is_logged_without_an_upstream_status() {
    let dead = dead_address().await;
    let rows = Rows::default();
    let config = config_over(
        rows.serve().await,
        vec![provider("gone", ProviderKind::OpenaiCompatible, dead)],
    );
    let gw = gateway(&config).await;

    let response = chat(gw, "refused-connection", "test-model").await;
    assert_eq!(response.status(), 502);

    let row = rows.row_for("refused-connection").await;
    assert_eq!(row["status"], 502, "{row}");
    assert_eq!(row["upstream_status"], 0, "{row}");
    assert_eq!(row["attempts"], 1, "{row}");
    assert_eq!(row["provider"], "gone", "{row}");
    assert!(!row["error"].as_str().unwrap().is_empty(), "{row}");
}

/// #2919: the transport error quotes the address the provider is configured
/// with, and an operator's address may carry credentials. Neither the caller's
/// 502 nor the request-log row the dashboard shows may repeat them.
#[tokio::test]
async fn a_connection_that_fails_does_not_repeat_the_provider_credentials() {
    let dead = dead_address().await;
    let secret = format!("pw-{}", uuid::Uuid::new_v4().simple());
    let mut gone = provider("gone", ProviderKind::OpenaiCompatible, dead);
    gone.api_base = format!("http://svc:{secret}@{dead}/v1?api_key={secret}");
    let rows = Rows::default();
    let config = config_over(rows.serve().await, vec![gone]);
    let gw = gateway(&config).await;

    let response = chat(gw, "refused-with-credentials", "test-model").await;
    assert_eq!(response.status(), 502);
    let error = error_body(response).await;
    let message = error["message"].as_str().unwrap();
    assert!(!message.contains(&secret), "the caller was told: {message}");
    assert!(
        message.contains("error sending request"),
        "the failure itself is still named: {message}"
    );

    let row = rows.row_for("refused-with-credentials").await;
    let logged = row["error"].as_str().unwrap();
    assert!(!logged.contains(&secret), "the log row says: {logged}");
}

/// Uploads go through their own loop and fail the same way.
#[tokio::test]
async fn a_rate_limited_upload_logs_the_upstream_reason() {
    let up = upstream(
        "/v1/audio/transcriptions",
        StatusCode::TOO_MANY_REQUESTS,
        rate_limited_body(),
        None,
    )
    .await;
    let rows = Rows::default();
    let config = config_over(
        rows.serve().await,
        vec![provider("up", ProviderKind::OpenaiCompatible, up.addr)],
    );
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
        .bearer_auth(KEY)
        .header("x-request-id", "upload-rate-limited")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 429);
    assert_eq!(error_body(response).await["code"], "upstream_rate_limited");
    let row = rows.row_for("upload-rate-limited").await;
    assert_eq!(row["status"], 429, "{row}");
    assert_eq!(row["upstream_status"], 429, "{row}");
    assert_eq!(row["attempts"], 1, "{row}");
    assert!(
        row["error"]
            .as_str()
            .unwrap()
            .contains("temporarily rate-limited upstream"),
        "{row}"
    );
}

// ── requests refused before they reach an upstream ──────────────────────────

/// A refusal of a known caller leaves a row that says what the caller was told
/// and names no upstream, because none was involved.
fn assert_refusal_row(row: &Value, status: u16, error_contains: &str) {
    assert_attributed(row);
    assert_eq!(row["status"], status, "{row}");
    assert_eq!(row["provider"], "", "{row}");
    assert_eq!(row["target"], "", "{row}");
    assert_eq!(row["upstream_status"], 0, "{row}");
    assert_eq!(row["attempts"], 0, "{row}");
    assert_eq!(row["prompt_tokens"], 0, "{row}");
    assert_eq!(row["cost_usd"], 0.0, "{row}");
    let error = row["error"].as_str().unwrap();
    assert!(error.contains(error_contains), "{error}");
}

#[tokio::test]
async fn an_unknown_model_leaves_a_row() {
    let up = upstream(
        "/v1/chat/completions",
        StatusCode::OK,
        json!({"choices": []}),
        None,
    )
    .await;
    let rows = Rows::default();
    let config = config_over(
        rows.serve().await,
        vec![provider("up", ProviderKind::OpenaiCompatible, up.addr)],
    );
    let gw = gateway(&config).await;

    let response = chat(gw, "unknown-model", "no-such-model").await;
    assert_eq!(response.status(), 404);

    let row = rows.row_for("unknown-model").await;
    assert_refusal_row(&row, 404, "no route for model 'no-such-model'");
    assert_eq!(row["model"], "no-such-model", "{row}");
    assert_eq!(up.calls.load(Ordering::SeqCst), 0);
}

/// The model is whatever the caller typed, so the row keeps a bounded copy.
#[tokio::test]
async fn a_refusal_row_bounds_what_it_quotes_from_the_caller() {
    let rows = Rows::default();
    let config = base_config(rows.serve().await);
    let gw = gateway(&config).await;
    let long = "m".repeat(10_000);

    let response = chat(gw, "long-model", &long).await;
    assert_eq!(response.status(), 404);

    let row = rows.row_for("long-model").await;
    assert_eq!(row["model"].as_str().unwrap().len(), 256, "{row}");
    assert!(row["error"].as_str().unwrap().len() <= 512, "{row}");
}

#[tokio::test]
async fn a_model_the_key_may_not_use_leaves_a_row() {
    let up = upstream(
        "/v1/chat/completions",
        StatusCode::OK,
        json!({"choices": []}),
        None,
    )
    .await;
    let rows = Rows::default();
    let config = config_over(
        rows.serve().await,
        vec![provider("up", ProviderKind::OpenaiCompatible, up.addr)],
    );
    let gw = gateway(&config).await;

    let response = chat_as(gw, RESTRICTED_KEY, "not-allowed", "test-model").await;
    assert_eq!(response.status(), 403);

    let row = rows.row_for("not-allowed").await;
    assert_eq!(row["virtual_key_id"], "key-restricted", "{row}");
    assert_eq!(row["status"], 403, "{row}");
    assert_eq!(row["attempts"], 0, "{row}");
    assert!(
        row["error"]
            .as_str()
            .unwrap()
            .contains("model not allowed for this key"),
        "{row}"
    );
}

#[tokio::test]
async fn a_request_a_guardrail_blocks_leaves_a_row() {
    let up = upstream(
        "/v1/chat/completions",
        StatusCode::OK,
        json!({"choices": []}),
        None,
    )
    .await;
    let rows = Rows::default();
    let mut config = config_over(
        rows.serve().await,
        vec![provider("up", ProviderKind::OpenaiCompatible, up.addr)],
    );
    config.guardrails = rolter_core::GuardrailsConfig {
        enabled: true,
        max_scan_bytes: None,
        streaming_post_call: Default::default(),
        rules: vec![rolter_core::GuardrailRule {
            name: "no-hi".to_string(),
            builtin: None,
            pattern: Some("hi".to_string()),
            stage: rolter_core::GuardStage::PreCall,
            action: GuardAction::Block,
            replacement: None,
            include_system: false,
        }],
    };
    let gw = gateway(&config).await;

    let response = chat(gw, "blocked", "test-model").await;
    assert_eq!(response.status(), 400);

    let row = rows.row_for("blocked").await;
    assert_refusal_row(&row, 400, "request blocked by guardrail 'no-hi'");
    assert_eq!(up.calls.load(Ordering::SeqCst), 0, "nothing was spent");
}

#[tokio::test]
async fn a_model_with_no_price_the_deployment_refuses_leaves_a_row() {
    let up = upstream(
        "/v1/chat/completions",
        StatusCode::OK,
        json!({"choices": []}),
        None,
    )
    .await;
    let rows = Rows::default();
    let mut config = config_over(
        rows.serve().await,
        vec![provider("up", ProviderKind::OpenaiCompatible, up.addr)],
    );
    config.unpriced_policy = rolter_core::UnpricedPolicy::Block;
    let gw = gateway(&config).await;

    let response = chat(gw, "unpriced", "test-model").await;
    assert_eq!(response.status(), 402);

    let row = rows.row_for("unpriced").await;
    assert_refusal_row(&row, 402, "has no price");
}

/// The same refusals on the upload pipeline, which has its own copy of each gate.
#[tokio::test]
async fn an_upload_for_an_unknown_model_leaves_a_row() {
    let rows = Rows::default();
    let config = base_config(rows.serve().await);
    let gw = gateway(&config).await;
    let boundary = "ROLTERBOUND";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nno-such-model\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\nRIFFxxxxWAVE\r\n--{b}--\r\n",
        b = boundary
    );

    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/audio/transcriptions"))
        .bearer_auth(KEY)
        .header("x-request-id", "upload-unknown-model")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);

    let row = rows.row_for("upload-unknown-model").await;
    assert_refusal_row(&row, 404, "no route for model 'no-such-model'");
}

#[tokio::test]
async fn a_route_with_no_targets_and_a_spent_output_cap_leave_rows() {
    let up = upstream(
        "/v1/chat/completions",
        StatusCode::OK,
        json!({"choices": []}),
        None,
    )
    .await;
    let rows = Rows::default();
    let mut config = config_over(
        rows.serve().await,
        vec![provider("up", ProviderKind::OpenaiCompatible, up.addr)],
    );
    config.routes.push(route("empty", vec![]));
    config.routes[0].advanced.limits.output_tokens = Some(10);
    let gw = gateway(&config).await;

    let response = chat(gw, "no-targets", "empty").await;
    assert_eq!(response.status(), 503);
    let row = rows.row_for("no-targets").await;
    assert_refusal_row(&row, 503, "route has no targets");

    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .bearer_auth(KEY)
        .header("x-request-id", "too-long")
        .json(&json!({"model": "test-model", "max_tokens": 100,
                      "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let row = rows.row_for("too-long").await;
    assert_refusal_row(&row, 400, "max_tokens exceeds");
    assert_eq!(up.calls.load(Ordering::SeqCst), 0);
}

/// A refusal is written without observing the per-model latency histogram:
/// that series is keyed by a string the caller chose.
#[tokio::test]
async fn a_refusal_does_not_mint_a_metric_series_for_the_model_it_named() {
    let rows = Rows::default();
    let config = base_config(rows.serve().await);
    let gw = gateway(&config).await;

    let response = chat(gw, "no-series", "a-model-nobody-configured").await;
    assert_eq!(response.status(), 404);
    let _ = rows.row_for("no-series").await;

    let metrics = reqwest::get(format!("http://{gw}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !metrics.contains("a-model-nobody-configured"),
        "a refusal must not create a series: {metrics}"
    );
}

/// What comes before the caller is identified is counted, not logged: no
/// tenant owns the row, and anonymous traffic could otherwise write without
/// limit.
#[tokio::test]
async fn requests_refused_before_the_caller_is_known_leave_no_row() {
    let rows = Rows::default();
    let config = base_config(rows.serve().await);
    let gw = gateway(&config).await;
    let client = reqwest::Client::new();
    let url = format!("http://{gw}/v1/chat/completions");

    // not JSON
    let response = client
        .post(&url)
        .bearer_auth(KEY)
        .header("x-request-id", "not-json")
        .body("{")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    // no model
    let response = client
        .post(&url)
        .bearer_auth(KEY)
        .header("x-request-id", "no-model")
        .json(&json!({"messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    // no key, and a key nobody issued
    let response = client
        .post(&url)
        .header("x-request-id", "no-key")
        .json(&json!({"model": "test-model", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let response = chat_as(gw, "sk-nobody-issued-this", "wrong-key", "test-model").await;
    assert_eq!(response.status(), 401);

    assert_eq!(rows.row_count_after_settling().await, 0);
}

// ── refusals that need redis: budgets and rate limits ───────────────────────
//
// Both counters live in Redis, so these read `ROLTER_TEST_REDIS_URL` and skip
// when it is unset, the same contract the other suites keep.

fn redis_url() -> Option<String> {
    let url = std::env::var("ROLTER_TEST_REDIS_URL").ok()?;
    (!url.is_empty()).then_some(url)
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

/// `config` with its first key renamed and moved to a fresh org, so a counter
/// keyed by either belongs to this run alone.
fn with_unique_scope(mut config: GatewayConfig) -> (GatewayConfig, String, String) {
    let (key_id, org) = (unique("key"), unique("org"));
    config.db_virtual_keys[0].id = key_id.clone();
    config.db_virtual_keys[0].org_id = org.clone();
    (config, key_id, org)
}

#[tokio::test]
async fn a_rate_limited_caller_leaves_a_row() {
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let up = upstream(
        "/v1/chat/completions",
        StatusCode::OK,
        json!({"choices": [], "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}),
        None,
    )
    .await;
    let rows = Rows::default();
    let (mut config, key_id, _) = with_unique_scope(config_over(
        rows.serve().await,
        vec![provider("up", ProviderKind::OpenaiCompatible, up.addr)],
    ));
    config
        .rate_limits
        .push(serde_json::from_value(json!({"scope": "key", "id": key_id, "rpm": 1})).unwrap());
    let state = rolter_gateway::AppState::with_logging(&config, Some(&redis));
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;

    let admitted = chat(gw, "within-limit", "test-model").await;
    assert_eq!(admitted.status(), 200);
    let _ = admitted.bytes().await.unwrap();
    let refused = chat(gw, "over-limit", "test-model").await;
    assert_eq!(refused.status(), 429);

    let row = rows.row_for("over-limit").await;
    assert_eq!(row["virtual_key_id"], key_id, "{row}");
    assert_eq!(row["status"], 429, "{row}");
    assert_eq!(row["provider"], "", "{row}");
    assert_eq!(row["attempts"], 0, "{row}");
    assert!(
        row["error"]
            .as_str()
            .unwrap()
            .contains("rate limit exceeded"),
        "{row}"
    );
    assert_eq!(
        up.calls.load(Ordering::SeqCst),
        1,
        "only the first got through"
    );
}

#[tokio::test]
async fn a_caller_over_budget_leaves_a_row() {
    use redis::AsyncCommands;
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let up = upstream(
        "/v1/chat/completions",
        StatusCode::OK,
        json!({"choices": []}),
        None,
    )
    .await;
    let rows = Rows::default();
    let (mut config, _, org) = with_unique_scope(config_over(
        rows.serve().await,
        vec![provider("up", ProviderKind::OpenaiCompatible, up.addr)],
    ));
    config.budgets.push(
        serde_json::from_value(json!({
            "scope": "org", "id": org, "limit_usd": 1, "period": "monthly"
        }))
        .unwrap(),
    );
    // the org has already spent more than it may
    let client = redis::Client::open(redis.as_str()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let counter = format!(
        "rolter:budget:org:{org}:{}",
        chrono::Utc::now().format("%Y%m")
    );
    let _: f64 = conn.incr(&counter, 5.0_f64).await.unwrap();
    let state = rolter_gateway::AppState::with_logging(&config, Some(&redis));
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;

    let response = chat(gw, "over-budget", "test-model").await;
    assert_eq!(response.status(), 402);

    let row = rows.row_for("over-budget").await;
    assert_eq!(row["org_id"], org, "{row}");
    assert_eq!(row["status"], 402, "{row}");
    assert_eq!(row["provider"], "", "{row}");
    assert_eq!(row["attempts"], 0, "{row}");
    assert!(
        row["error"].as_str().unwrap().contains("budget exceeded"),
        "{row}"
    );
    assert_eq!(up.calls.load(Ordering::SeqCst), 0, "nothing was spent");
}
