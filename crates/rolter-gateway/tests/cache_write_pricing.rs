//! Tokens written to the prompt cache are priced at the model's cache-write
//! rate (#2876).
//!
//! Anthropic bills a cache write at a premium over the input rate. Since #2863
//! the write count sits inside the prompt total on every dialect, so before the
//! price row could name a rate the write tokens were charged as ordinary input
//! and write-heavy traffic was under-stated by the premium.
//!
//! Every test sends one request through a stand-in upstream that reports 2000
//! prompt tokens, of which 1500 were read from the cache and 100 written to it,
//! and 3 output tokens, then asserts the `cost_usd` of the row the gateway logs.
//! The model costs a dollar per fresh input or output token, ten cents per
//! cached one and (when the row sets it) a dollar twenty-five per written one:
//!
//! - 400 fresh input at 1.00 = 400
//! - 1500 cache reads at 0.10 = 150
//! - 100 cache writes at 1.25 = 125
//! - 3 output at 1.00 = 3
//!
//! which is 678. A row with no write rate charges the 100 as input, as every
//! row did before the column existed, and costs 653.
//!
//! These tests drive the gateway over HTTP against mock upstreams and an
//! in-process stand-in for the ClickHouse HTTP interface, like
//! `translated_cache_usage.rs`.

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

const KEY_ID: &str = "key-cache-write-pricing";

/// The caller's key, made up per run rather than written out.
fn key() -> String {
    format!("sk-cache-write-{:x}", std::process::id())
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
            if !self.matching(request_id).is_empty() {
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

/// An upstream that answers every request with `body` as `content_type`.
async fn upstream(content_type: &'static str, body: String) -> SocketAddr {
    async fn handler(
        State((content_type, body)): State<(&'static str, String)>,
    ) -> impl IntoResponse {
        ([(header::CONTENT_TYPE, content_type)], body)
    }
    serve(
        Router::new()
            .fallback(handler)
            .with_state((content_type, body)),
    )
    .await
}

/// A config with one `test-model` route over a single provider of `kind`, and
/// request logs going to `clickhouse`. The price is a dollar per fresh input or
/// output token and ten cents per cached input token, and `cache_write_per_mtok`
/// when given: the field is left out of the row otherwise, the way a price
/// written before the column existed is.
fn config(
    kind: ProviderKind,
    upstream: SocketAddr,
    clickhouse: SocketAddr,
    cache_write_per_mtok: Option<u64>,
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
        api_key: Some(format!("up-{:x}", std::process::id())),
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
        org_id: "org-cache-write-pricing".into(),
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
    let mut price = json!({
        "model": "test-model",
        "input_per_mtok": 1_000_000,
        "output_per_mtok": 1_000_000,
        "cached_input_per_mtok": 100_000
    });
    if let Some(rate) = cache_write_per_mtok {
        price["cache_write_per_mtok"] = json!(rate);
    }
    config
        .model_prices
        .push(serde_json::from_value(price).unwrap());
    config
}

/// A dollar twenty-five per written token.
const WRITE_RATE: u64 = 1_250_000;

/// What a client of each dialect sends.
#[derive(Clone, Copy)]
enum Client {
    Chat,
    Messages,
}

impl Client {
    fn path(self) -> &'static str {
        match self {
            Client::Chat => "/v1/chat/completions",
            Client::Messages => "/v1/messages",
        }
    }

    fn request(self, stream: bool) -> Value {
        match self {
            Client::Chat => json!({
                "model": "test-model", "stream": stream,
                "messages": [{"role": "user", "content": "ping"}]
            }),
            Client::Messages => json!({
                "model": "test-model", "stream": stream, "max_tokens": 16,
                "messages": [{"role": "user", "content": "ping"}]
            }),
        }
    }
}

/// Anthropic's `input_tokens` leaves the cache out: 400 fresh, 100 written to
/// the cache and 1500 read from it make the 2000.
const ANTHROPIC_USAGE: &str = r#"{"input_tokens":400,"cache_creation_input_tokens":100,"cache_read_input_tokens":1500,"output_tokens":3}"#;

fn anthropic_message() -> (&'static str, String) {
    (
        "application/json",
        format!(
            r#"{{"id":"msg_1","type":"message","role":"assistant","model":"test-model","content":[{{"type":"text","text":"pong"}}],"stop_reason":"end_turn","usage":{ANTHROPIC_USAGE}}}"#
        ),
    )
}

fn anthropic_stream() -> (&'static str, String) {
    let frame = |event: &str, data: &str| format!("event: {event}\ndata: {data}\n\n");
    (
        "text/event-stream",
        [
            frame(
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_1","model":"test-model","usage":{"input_tokens":400,"cache_creation_input_tokens":100,"cache_read_input_tokens":1500,"output_tokens":1}}}"#,
            ),
            frame(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"pong"}}"#,
            ),
            frame(
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
            ),
            frame("message_stop", r#"{"type":"message_stop"}"#),
        ]
        .concat(),
    )
}

/// A Chat Completions provider that names the write count beside the read
/// count, as OpenRouter does; the prompt total holds both.
fn chat_completion_with_writes() -> (&'static str, String) {
    (
        "application/json",
        r#"{"id":"chat_1","model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2000,"completion_tokens":3,"total_tokens":2003,"prompt_tokens_details":{"cached_tokens":1500,"cache_write_tokens":100}}}"#
            .to_string(),
    )
}

/// One request from a `client` to a provider of `kind` answering `answer`;
/// returns the row that was logged.
async fn exchange(
    request_id: &str,
    kind: ProviderKind,
    client: Client,
    stream: bool,
    answer: (&'static str, String),
    cache_write_per_mtok: Option<u64>,
) -> Value {
    let upstream = upstream(answer.0, answer.1).await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let state = rolter_gateway::AppState::with_logging(
        &config(kind, upstream, clickhouse, cache_write_per_mtok),
        None,
    );
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;

    let response = reqwest::Client::new()
        .post(format!("http://{gw}{}", client.path()))
        .bearer_auth(key())
        .header("x-api-key", key())
        .header("anthropic-version", "2023-06-01")
        .header("x-request-id", request_id)
        .json(&client.request(stream))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{request_id}");
    // read to the end so a stream has finished and its row has been emitted
    let _ = response.text().await.unwrap();
    rows.row_for(request_id).await
}

/// The row of a request whose 2000-token prompt held 1500 cache reads and 100
/// cache writes, priced at `cost`.
fn assert_row(row: &Value, cost: f64) {
    assert_eq!(row["prompt_tokens"], 2000, "request-log row");
    assert_eq!(row["cache_read_tokens"], 1500, "request-log row");
    assert_eq!(row["cache_write_tokens"], 100, "request-log row");
    assert_eq!(row["completion_tokens"], 3, "request-log row");
    assert_eq!(row["cost_usd"], cost, "request-log row");
    assert_eq!(row["unpriced"], 0, "request-log row");
    assert_eq!(row["usage_unknown"], 0, "request-log row");
}

/// The 678 of the module comment: each token class at its own rate.
const WITH_WRITE_RATE: f64 = 678.0;
/// The same request on a row with no write rate: the 100 written tokens are
/// input, so 25 less than above.
const WITHOUT_WRITE_RATE: f64 = 653.0;

#[tokio::test]
async fn an_anthropic_answer_prices_its_cache_write_at_the_write_rate() {
    let row = exchange(
        "cwp-messages-buffered",
        ProviderKind::Anthropic,
        Client::Messages,
        false,
        anthropic_message(),
        Some(WRITE_RATE),
    )
    .await;
    assert_row(&row, WITH_WRITE_RATE);
}

#[tokio::test]
async fn a_streamed_anthropic_answer_prices_its_cache_write_at_the_write_rate() {
    let row = exchange(
        "cwp-messages-streamed",
        ProviderKind::Anthropic,
        Client::Messages,
        true,
        anthropic_stream(),
        Some(WRITE_RATE),
    )
    .await;
    assert_row(&row, WITH_WRITE_RATE);
}

/// A chat client on an Anthropic provider is served a translated body, and the
/// row is built from that body, so the write count has to survive the hop.
#[tokio::test]
async fn a_translated_answer_prices_its_cache_write_at_the_write_rate() {
    for (request_id, stream, answer) in [
        ("cwp-chat-buffered", false, anthropic_message()),
        ("cwp-chat-streamed", true, anthropic_stream()),
    ] {
        let row = exchange(
            request_id,
            ProviderKind::Anthropic,
            Client::Chat,
            stream,
            answer,
            Some(WRITE_RATE),
        )
        .await;
        assert_row(&row, WITH_WRITE_RATE);
    }
}

/// OpenRouter and the gateways that copy it name the write count inside
/// `prompt_tokens_details`, with the prompt total holding it.
#[tokio::test]
async fn a_chat_completions_write_count_is_priced_at_the_write_rate() {
    let row = exchange(
        "cwp-openrouter",
        ProviderKind::Openai,
        Client::Chat,
        false,
        chat_completion_with_writes(),
        Some(WRITE_RATE),
    )
    .await;
    assert_row(&row, WITH_WRITE_RATE);
}

/// A row that predates the column, or whose operator never set a rate, is
/// charged exactly what it was: the written tokens are input.
#[tokio::test]
async fn a_price_without_a_write_rate_charges_the_write_as_input() {
    let row = exchange(
        "cwp-no-rate",
        ProviderKind::Anthropic,
        Client::Messages,
        false,
        anthropic_message(),
        None,
    )
    .await;
    assert_row(&row, WITHOUT_WRITE_RATE);
}
