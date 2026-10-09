//! The prompt-cache share of a request that crosses dialects (#2863).
//!
//! The request log and the budgets read token usage from the body the client
//! receives. For a request translated between dialects that body is the
//! translator's, and it used to carry only the three headline counts, so a
//! cached prompt was priced as fresh input (a Messages or Responses client on a
//! Chat Completions provider) or its cache tokens were not billed at all (a
//! chat or Responses client on an Anthropic provider).
//!
//! Every test sends one request through a stand-in upstream that reports 2000
//! prompt tokens of which 1500 were read from the provider's cache, and asserts
//! two things: the row the gateway logs, priced against a model costing a
//! dollar per fresh input or output token and ten cents per cached one, and
//! what the client is shown in its own dialect. The dialects disagree about
//! whether the cache sits inside the prompt total (Chat Completions, Responses,
//! Gemini) or beside it (Anthropic), so the figures differ per client while the
//! row must not.
//!
//! These tests drive the gateway over HTTP against mock upstreams and an
//! in-process stand-in for the ClickHouse HTTP interface, like
//! `responses_stream_usage.rs`.

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

const KEY_ID: &str = "key-translated-cache";

/// The caller's key, made up per run rather than written out.
fn key() -> String {
    format!("sk-translated-cache-{:x}", std::process::id())
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

/// An upstream that answers every request with `body` as `content_type`,
/// whatever path the forwarder asks for (Gemini embeds the model in it).
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

/// A config with one `test-model` route over a single provider of `kind`, a
/// price of one dollar per fresh input or output token and ten cents per cached
/// input token, and request logs going to `clickhouse`.
fn config(kind: ProviderKind, upstream: SocketAddr, clickhouse: SocketAddr) -> GatewayConfig {
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
        // native gemini refuses to be called without one
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
        org_id: "org-translated-cache".into(),
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
    config
}

/// What a client of each dialect sends.
#[derive(Clone, Copy)]
enum Client {
    Chat,
    Messages,
    Responses,
}

impl Client {
    fn path(self) -> &'static str {
        match self {
            Client::Chat => "/v1/chat/completions",
            Client::Messages => "/v1/messages",
            Client::Responses => "/v1/responses",
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
            Client::Responses => json!({
                "model": "test-model", "stream": stream, "input": "ping"
            }),
        }
    }
}

// ── what each upstream says ─────────────────────────────────────────────────
// all of them: 2000 prompt tokens, 1500 of them read from the cache, 3 output

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
    let frame = |event: &str, data: String| format!("event: {event}\ndata: {data}\n\n");
    (
        "text/event-stream",
        [
            frame(
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_1","model":"test-model","usage":{"input_tokens":400,"cache_creation_input_tokens":100,"cache_read_input_tokens":1500,"output_tokens":1}}}"#
                    .into(),
            ),
            frame(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"pong"}}"#
                    .into(),
            ),
            frame(
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#
                    .into(),
            ),
            frame("message_stop", r#"{"type":"message_stop"}"#.into()),
        ]
        .concat(),
    )
}

/// Chat Completions counts the cached tokens inside `prompt_tokens`.
const CHAT_USAGE: &str = r#"{"prompt_tokens":2000,"completion_tokens":3,"total_tokens":2003,"prompt_tokens_details":{"cached_tokens":1500}}"#;

fn chat_completion() -> (&'static str, String) {
    (
        "application/json",
        format!(
            r#"{{"id":"chat_1","model":"test-model","choices":[{{"index":0,"message":{{"role":"assistant","content":"pong"}},"finish_reason":"stop"}}],"usage":{CHAT_USAGE}}}"#
        ),
    )
}

fn chat_stream() -> (&'static str, String) {
    let chunks = [
        r#"{"id":"chat_1","model":"test-model","choices":[{"index":0,"delta":{"content":"pong"}}]}"#
            .to_string(),
        r#"{"id":"chat_1","model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#
            .to_string(),
        format!(r#"{{"id":"chat_1","model":"test-model","choices":[],"usage":{CHAT_USAGE}}}"#),
    ];
    (
        "text/event-stream",
        chunks
            .iter()
            .map(|chunk| format!("data: {chunk}\n\n"))
            .chain(["data: [DONE]\n\n".to_string()])
            .collect(),
    )
}

/// Gemini counts the cached content inside `promptTokenCount` too.
const GEMINI_CHUNK: &str = r#"{"candidates":[{"content":{"parts":[{"text":"pong"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2000,"cachedContentTokenCount":1500,"candidatesTokenCount":3,"totalTokenCount":2003}}"#;

fn gemini_answer() -> (&'static str, String) {
    ("application/json", GEMINI_CHUNK.to_string())
}

fn gemini_stream() -> (&'static str, String) {
    ("text/event-stream", format!("data: {GEMINI_CHUNK}\n\n"))
}

// ── driving the gateway ─────────────────────────────────────────────────────

/// One request from a `client` to a provider of `kind` answering `answer`;
/// returns what the client was sent and the row that was logged.
async fn exchange(
    request_id: &str,
    kind: ProviderKind,
    client: Client,
    stream: bool,
    answer: (&'static str, String),
) -> (String, Value) {
    let upstream = upstream(answer.0, answer.1).await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let state = rolter_gateway::AppState::with_logging(&config(kind, upstream, clickhouse), None);
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
    let body = response.text().await.unwrap();
    let row = rows.row_for(request_id).await;
    (body, row)
}

/// The usage a client was shown: the body's own `usage`, or for a stream the
/// last event that carried one (top level, or under `response` for the
/// Responses API).
fn shown_usage(body: &str) -> Value {
    let whole: Option<Value> = serde_json::from_str(body).ok();
    let events = whole.into_iter().chain(
        body.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok()),
    );
    events
        .filter_map(|event| {
            event
                .get("usage")
                .or_else(|| event.pointer("/response/usage"))
                .filter(|usage| usage.is_object())
                .cloned()
        })
        .next_back()
        .unwrap_or_else(|| panic!("no usage in the answer: {body}"))
}

/// 2000 prompt tokens of which 1500 were cached, 3 output: 500 fresh input at
/// one dollar, 1500 cached at ten cents and 3 output at one dollar. Priced as
/// if the cache had not been hit it is 2003, and priced from the Anthropic
/// figures taken at face value (400 input, 1500 cached) it is 553.
fn assert_priced_with_the_cache(row: &Value) {
    assert_eq!(row["prompt_tokens"], 2000, "request-log row");
    assert_eq!(row["cache_read_tokens"], 1500, "request-log row");
    assert_eq!(row["completion_tokens"], 3, "request-log row");
    assert_eq!(row["cost_usd"], 653.0, "request-log row");
    assert_eq!(row["unpriced"], 0, "request-log row");
    assert_eq!(row["usage_unknown"], 0, "request-log row");
}

/// The 100 tokens written to the cache are in the prompt total (they are
/// charged as input) and counted in their own column.
fn assert_cache_write_counted(row: &Value) {
    assert_eq!(row["cache_write_tokens"], 100, "request-log row");
}

/// What a chat client reads: the cache inside `prompt_tokens`, named in
/// `prompt_tokens_details`.
fn assert_chat_usage(usage: &Value, cache_write: bool) {
    assert_eq!(usage["prompt_tokens"], 2000, "usage shown to the client");
    assert_eq!(usage["completion_tokens"], 3, "usage shown to the client");
    assert_eq!(usage["total_tokens"], 2003, "usage shown to the client");
    assert_eq!(
        usage["prompt_tokens_details"]["cached_tokens"], 1500,
        "usage shown to the client"
    );
    if cache_write {
        assert_eq!(
            usage["prompt_tokens_details"]["cache_write_tokens"], 100,
            "usage shown to the client"
        );
    }
}

/// What a Responses client reads: the same, under the Responses names.
fn assert_responses_usage(usage: &Value) {
    assert_eq!(usage["input_tokens"], 2000, "usage shown to the client");
    assert_eq!(usage["output_tokens"], 3, "usage shown to the client");
    assert_eq!(usage["total_tokens"], 2003, "usage shown to the client");
    assert_eq!(
        usage["input_tokens_details"]["cached_tokens"], 1500,
        "usage shown to the client"
    );
}

/// What a Messages client reads: Anthropic's `input_tokens` is the 500 that
/// were not read from the cache.
fn assert_messages_usage(usage: &Value) {
    assert_eq!(usage["input_tokens"], 500, "usage shown to the client");
    assert_eq!(usage["output_tokens"], 3, "usage shown to the client");
    assert_eq!(
        usage["cache_read_input_tokens"], 1500,
        "usage shown to the client"
    );
}

// ── an Anthropic provider, chat and Responses clients ───────────────────────
// the direction that billed nothing for the cache: Anthropic's `input_tokens`
// excludes the reads, and the translator dropped them

#[tokio::test]
async fn anthropic_answer_to_a_chat_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "anthropic-chat-buffered",
        ProviderKind::Anthropic,
        Client::Chat,
        false,
        anthropic_message(),
    )
    .await;
    assert_chat_usage(&shown_usage(&body), true);
    assert_priced_with_the_cache(&row);
    assert_cache_write_counted(&row);
}

#[tokio::test]
async fn anthropic_stream_to_a_chat_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "anthropic-chat-streamed",
        ProviderKind::Anthropic,
        Client::Chat,
        true,
        anthropic_stream(),
    )
    .await;
    assert_chat_usage(&shown_usage(&body), true);
    assert_priced_with_the_cache(&row);
    assert_cache_write_counted(&row);
}

#[tokio::test]
async fn anthropic_answer_to_a_responses_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "anthropic-responses-buffered",
        ProviderKind::Anthropic,
        Client::Responses,
        false,
        anthropic_message(),
    )
    .await;
    assert_responses_usage(&shown_usage(&body));
    assert_priced_with_the_cache(&row);
    assert_cache_write_counted(&row);
}

#[tokio::test]
async fn anthropic_stream_to_a_responses_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "anthropic-responses-streamed",
        ProviderKind::Anthropic,
        Client::Responses,
        true,
        anthropic_stream(),
    )
    .await;
    assert_responses_usage(&shown_usage(&body));
    assert_priced_with_the_cache(&row);
    assert_cache_write_counted(&row);
}

// ── a Chat Completions provider, Messages and Responses clients ─────────────
// the direction that over-billed the cache: `cached_tokens` was lost, so the
// whole prompt was priced as fresh input

#[tokio::test]
async fn chat_answer_to_a_messages_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "chat-messages-buffered",
        ProviderKind::OpenaiCompatible,
        Client::Messages,
        false,
        chat_completion(),
    )
    .await;
    assert_messages_usage(&shown_usage(&body));
    assert_priced_with_the_cache(&row);
}

#[tokio::test]
async fn chat_stream_to_a_messages_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "chat-messages-streamed",
        ProviderKind::OpenaiCompatible,
        Client::Messages,
        true,
        chat_stream(),
    )
    .await;
    assert_messages_usage(&shown_usage(&body));
    assert_priced_with_the_cache(&row);
}

#[tokio::test]
async fn chat_answer_to_a_responses_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "chat-responses-buffered",
        ProviderKind::OpenaiCompatible,
        Client::Responses,
        false,
        chat_completion(),
    )
    .await;
    assert_responses_usage(&shown_usage(&body));
    assert_priced_with_the_cache(&row);
}

#[tokio::test]
async fn chat_stream_to_a_responses_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "chat-responses-streamed",
        ProviderKind::OpenaiCompatible,
        Client::Responses,
        true,
        chat_stream(),
    )
    .await;
    assert_responses_usage(&shown_usage(&body));
    assert_priced_with_the_cache(&row);
}

// ── a Gemini provider ───────────────────────────────────────────────────────

#[tokio::test]
async fn gemini_answer_to_a_chat_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "gemini-chat-buffered",
        ProviderKind::GeminiNative,
        Client::Chat,
        false,
        gemini_answer(),
    )
    .await;
    assert_chat_usage(&shown_usage(&body), false);
    assert_priced_with_the_cache(&row);
}

#[tokio::test]
async fn gemini_stream_to_a_messages_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "gemini-messages-streamed",
        ProviderKind::GeminiNative,
        Client::Messages,
        true,
        gemini_stream(),
    )
    .await;
    assert_messages_usage(&shown_usage(&body));
    assert_priced_with_the_cache(&row);
}

// ── Anthropic to Anthropic: nothing is translated ───────────────────────────
// not a dialect hop, but it shares the figures' convention with the hop above:
// the log used to read `input_tokens` (400) as the whole prompt, so the 1500
// cached tokens were capped at it and the row priced 400 tokens at the cached
// rate and nothing else

#[tokio::test]
async fn an_anthropic_answer_to_a_messages_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "anthropic-messages-buffered",
        ProviderKind::Anthropic,
        Client::Messages,
        false,
        anthropic_message(),
    )
    .await;
    // the body is the provider's, untouched
    let usage = shown_usage(&body);
    assert_eq!(usage["input_tokens"], 400, "usage shown to the client");
    assert_priced_with_the_cache(&row);
    assert_cache_write_counted(&row);
}

#[tokio::test]
async fn an_anthropic_stream_to_a_messages_client_is_priced_with_its_cache() {
    let (_, row) = exchange(
        "anthropic-messages-streamed",
        ProviderKind::Anthropic,
        Client::Messages,
        true,
        anthropic_stream(),
    )
    .await;
    assert_priced_with_the_cache(&row);
    assert_cache_write_counted(&row);
}
