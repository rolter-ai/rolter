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
//! Two sibling cases share the harness. Gemini reports its thinking tokens
//! beside `candidatesTokenCount`, and its tool-use prompt tokens beside
//! `promptTokenCount`; both are billed, so a translated answer must carry them
//! in the completion and the prompt, and the row must price them (#2875).
//! Providers that cache but spell the hit differently from Chat Completions
//! (DeepSeek, Kimi, GigaChat) must be priced at the cached rate on the way
//! through and across a dialect hop (#2877).
//!
//! Two more follow-ups share it. Qwen's explicit cache reports the tokens it
//! wrote under `prompt_tokens_details.cache_creation_input_tokens`, inside the
//! prompt, and the row and the translators must read the count (#2879). The
//! thinking share of the completion is named in the OpenAI dialects' details
//! blocks (`completion_tokens_details.reasoning_tokens`,
//! `output_tokens_details.reasoning_tokens`) but is inside the completion
//! already, so a body carries it across without the row or the client counting
//! it twice (#2881).
//!
//! Gemini's OpenAI-compatible endpoint (`ProviderKind::Gemini`) reports its
//! thinking tokens in `total_tokens` alone, and Vertex AI's names them beside a
//! `completion_tokens` that leaves them out. Both are billed as output, so the
//! row, and a translated answer, must carry them in the completion (#2880).
//!
//! There is no Gemini-dialect client in the gateway: the only client dialects
//! are Chat Completions, Messages and Responses, so a native Gemini body is
//! always translated before the client sees it or the row is built, and the two
//! can never disagree.
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
    chat_completion_with(CHAT_USAGE)
}

/// A Chat Completions answer reporting `usage` as given, whatever it spells.
fn chat_completion_with(usage: &str) -> (&'static str, String) {
    (
        "application/json",
        format!(
            r#"{{"id":"chat_1","model":"test-model","choices":[{{"index":0,"message":{{"role":"assistant","content":"pong"}},"finish_reason":"stop"}}],"usage":{usage}}}"#
        ),
    )
}

fn chat_stream() -> (&'static str, String) {
    chat_stream_with(CHAT_USAGE)
}

fn chat_stream_with(usage: &str) -> (&'static str, String) {
    let chunks = [
        r#"{"id":"chat_1","model":"test-model","choices":[{"index":0,"delta":{"content":"pong"}}]}"#
            .to_string(),
        r#"{"id":"chat_1","model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#
            .to_string(),
        format!(r#"{{"id":"chat_1","model":"test-model","choices":[],"usage":{usage}}}"#),
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

/// A thinking model: 2000 prompt tokens, 3 answer tokens and 20 thinking
/// tokens, which `candidatesTokenCount` leaves out and `totalTokenCount`
/// includes.
const GEMINI_THINKING_CHUNK: &str = r#"{"candidates":[{"content":{"parts":[{"text":"pong"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2000,"candidatesTokenCount":3,"thoughtsTokenCount":20,"totalTokenCount":2023}}"#;

fn gemini_thinking_answer() -> (&'static str, String) {
    ("application/json", GEMINI_THINKING_CHUNK.to_string())
}

fn gemini_thinking_stream() -> (&'static str, String) {
    (
        "text/event-stream",
        format!("data: {GEMINI_THINKING_CHUNK}\n\n"),
    )
}

/// The same request through the Interactions API: 1800 input tokens and 200
/// that a built-in tool fed back, 3 output and 20 thinking tokens.
const INTERACTIONS_USAGE: &str = r#"{"total_input_tokens":1800,"total_tool_use_tokens":200,"total_output_tokens":3,"total_thought_tokens":20,"total_tokens":2023}"#;

fn interactions_answer() -> (&'static str, String) {
    (
        "application/json",
        format!(
            r#"{{"id":"int_1","status":"completed","model":"test-model","steps":[{{"type":"model_output","content":[{{"type":"text","text":"pong"}}]}}],"usage":{INTERACTIONS_USAGE}}}"#
        ),
    )
}

fn interactions_stream() -> (&'static str, String) {
    (
        "text/event-stream",
        format!(
            "data: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"status\":\"completed\",\"usage\":{INTERACTIONS_USAGE}}}}}\n\n"
        ),
    )
}

// ── providers that spell the cache hit differently ──────────────────────────
// 2000 prompt tokens, 1500 of them hit, 3 output, as above

/// DeepSeek: the prompt is documented as hits plus misses.
const DEEPSEEK_USAGE: &str = r#"{"prompt_tokens":2000,"completion_tokens":3,"total_tokens":2003,"prompt_cache_hit_tokens":1500,"prompt_cache_miss_tokens":500}"#;

/// Kimi: the hit at the top of `usage`.
const KIMI_USAGE: &str =
    r#"{"prompt_tokens":2000,"completion_tokens":3,"total_tokens":2003,"cached_tokens":1500}"#;

/// GigaChat: `precached_prompt_tokens`.
const GIGACHAT_USAGE: &str = r#"{"prompt_tokens":2000,"completion_tokens":3,"total_tokens":2003,"precached_prompt_tokens":1500}"#;

/// Qwen on an explicit cache (#2879): the 100 tokens written to the cache are
/// named in the details block beside the hit, and are inside `prompt_tokens`.
const QWEN_USAGE: &str = r#"{"prompt_tokens":2000,"completion_tokens":3,"total_tokens":2003,"prompt_tokens_details":{"cached_tokens":1500,"cache_creation_input_tokens":100}}"#;

/// A Chat Completions provider that breaks out its reasoning (#2881): 2000
/// prompt tokens, 23 completion tokens of which 20 were thinking. The 23 are
/// the whole completion, as OpenAI states it.
const CHAT_REASONING_USAGE: &str = r#"{"prompt_tokens":2000,"completion_tokens":23,"total_tokens":2023,"completion_tokens_details":{"reasoning_tokens":20}}"#;

/// xAI (#2888): the same 2000 prompt tokens, 3 answer tokens and 20 thinking
/// ones, but the thinking is stated beside `completion_tokens` and only the
/// total (2000 + 3 + 20) adds it up.
const XAI_USAGE: &str = r#"{"prompt_tokens":2000,"completion_tokens":3,"total_tokens":2023,"completion_tokens_details":{"reasoning_tokens":20}}"#;

/// Gemini's OpenAI-compatible endpoint (#2880): the same 2000 prompt tokens, 3
/// answer tokens and 20 thinking ones, but the thinking is in the total and
/// nowhere else, as every response captured from the endpoint shows.
const GEMINI_COMPAT_USAGE: &str =
    r#"{"prompt_tokens":2000,"completion_tokens":3,"total_tokens":2023}"#;

/// Vertex AI's OpenAI-compatible endpoint: the thinking is named, beside a
/// completion that leaves it out, and the total adds the three up.
const VERTEX_COMPAT_USAGE: &str = r#"{"completion_tokens":3,"completion_tokens_details":{"reasoning_tokens":20},"extra_properties":{"google":{"traffic_type":"ON_DEMAND"}},"prompt_tokens":2000,"total_tokens":2023}"#;

/// The usage Gemini's endpoint sends on every chunk of a stream, the thinking
/// already in each total (the completion grows from 1 to 3 beside a total that
/// is 20 more than the prompt and the completion).
fn gemini_compat_stream() -> (&'static str, String) {
    let chunks = [
        r#"{"id":"chat_1","model":"test-model","choices":[{"index":0,"delta":{"content":"po"}}],"usage":{"prompt_tokens":2000,"completion_tokens":1,"total_tokens":2021}}"#,
        r#"{"id":"chat_1","model":"test-model","choices":[{"index":0,"delta":{"content":"ng"}}],"usage":{"prompt_tokens":2000,"completion_tokens":3,"total_tokens":2023}}"#,
        r#"{"id":"chat_1","model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":2000,"completion_tokens":3,"total_tokens":2023}}"#,
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

/// 2000 prompt tokens at one dollar, and 3 answer plus 20 thinking tokens at
/// one dollar: 2023. Read for `candidatesTokenCount` alone it is 2003.
fn assert_priced_with_the_thoughts(row: &Value) {
    assert_eq!(row["prompt_tokens"], 2000, "request-log row");
    assert_eq!(row["completion_tokens"], 23, "request-log row");
    assert_eq!(row["cost_usd"], 2023.0, "request-log row");
    assert_eq!(row["unpriced"], 0, "request-log row");
    assert_eq!(row["usage_unknown"], 0, "request-log row");
}

/// What a chat client reads of a thinking answer: the thinking counted as
/// completion, and the total the sum of the two counts beside it.
fn assert_thinking_chat_usage(usage: &Value) {
    assert_eq!(usage["prompt_tokens"], 2000, "usage shown to the client");
    assert_eq!(usage["completion_tokens"], 23, "usage shown to the client");
    assert_eq!(usage["total_tokens"], 2023, "usage shown to the client");
    assert_reasoning_named_chat(usage);
}

/// The 20 thinking tokens are named in the chat details block and are part of
/// the 23, not on top of them (#2881).
fn assert_reasoning_named_chat(usage: &Value) {
    assert_eq!(
        usage["completion_tokens_details"]["reasoning_tokens"], 20,
        "usage shown to the client"
    );
}

/// The same under the Responses names.
fn assert_reasoning_named_responses(usage: &Value) {
    assert_eq!(usage["output_tokens"], 23, "usage shown to the client");
    assert_eq!(
        usage["output_tokens_details"]["reasoning_tokens"], 20,
        "usage shown to the client"
    );
}

/// Messages has no field for the thinking share.
fn assert_no_reasoning_named(usage: &Value) {
    let text = usage.to_string();
    assert!(
        !text.contains("reasoning"),
        "usage shown to the client: {text}"
    );
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

// ── a Gemini thinking model (#2875) ─────────────────────────────────────────
// the thinking tokens are billed as output but sit beside the answer's count,
// not inside it. before they were left out of the completion, so the row and
// the budget were charged 2003 where the provider charged 2023, and a buffered
// body's total (the provider's 2023) did not add up to the counts beside it

#[tokio::test]
async fn gemini_thinking_answer_to_a_chat_client_is_priced_for_its_thoughts() {
    let (body, row) = exchange(
        "gemini-thinking-chat-buffered",
        ProviderKind::GeminiNative,
        Client::Chat,
        false,
        gemini_thinking_answer(),
    )
    .await;
    assert_thinking_chat_usage(&shown_usage(&body));
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn gemini_thinking_stream_to_a_chat_client_is_priced_for_its_thoughts() {
    let (body, row) = exchange(
        "gemini-thinking-chat-streamed",
        ProviderKind::GeminiNative,
        Client::Chat,
        true,
        gemini_thinking_stream(),
    )
    .await;
    assert_thinking_chat_usage(&shown_usage(&body));
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn gemini_thinking_answer_to_a_messages_client_is_priced_for_its_thoughts() {
    let (body, row) = exchange(
        "gemini-thinking-messages-buffered",
        ProviderKind::GeminiNative,
        Client::Messages,
        false,
        gemini_thinking_answer(),
    )
    .await;
    let usage = shown_usage(&body);
    assert_eq!(usage["input_tokens"], 2000, "usage shown to the client");
    assert_eq!(usage["output_tokens"], 23, "usage shown to the client");
    assert_no_reasoning_named(&usage);
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn gemini_thinking_stream_to_a_responses_client_is_priced_for_its_thoughts() {
    let (body, row) = exchange(
        "gemini-thinking-responses-streamed",
        ProviderKind::GeminiNative,
        Client::Responses,
        true,
        gemini_thinking_stream(),
    )
    .await;
    let usage = shown_usage(&body);
    assert_eq!(usage["input_tokens"], 2000, "usage shown to the client");
    assert_eq!(usage["output_tokens"], 23, "usage shown to the client");
    assert_eq!(usage["total_tokens"], 2023, "usage shown to the client");
    assert_reasoning_named_responses(&usage);
    assert_priced_with_the_thoughts(&row);
}

// the Interactions API names the counts `total_thought_tokens` and
// `total_tool_use_tokens`; the tool-use tokens a built-in tool fed back are
// charged as input, so 1800 + 200 is the 2000 prompt tokens

#[tokio::test]
async fn interactions_answer_to_a_chat_client_is_priced_for_its_thoughts_and_tool_use() {
    let (body, row) = exchange(
        "interactions-thinking-chat-buffered",
        ProviderKind::GeminiInteractions,
        Client::Chat,
        false,
        interactions_answer(),
    )
    .await;
    assert_thinking_chat_usage(&shown_usage(&body));
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn interactions_stream_to_a_responses_client_is_priced_for_its_thoughts_and_tool_use() {
    let (body, row) = exchange(
        "interactions-thinking-responses-streamed",
        ProviderKind::GeminiInteractions,
        Client::Responses,
        true,
        interactions_stream(),
    )
    .await;
    let usage = shown_usage(&body);
    assert_eq!(usage["input_tokens"], 2000, "usage shown to the client");
    assert_reasoning_named_responses(&usage);
    assert_priced_with_the_thoughts(&row);
}

// ── a Chat Completions provider that breaks out its reasoning (#2881) ───────
// the 20 thinking tokens are inside the 23 the provider states. the row must
// log 23, not 43, and a Responses client is shown the same 23 with the share
// named; before there was no field for it, so it was dropped on the way

#[tokio::test]
async fn chat_reasoning_answer_to_a_chat_client_is_logged_once() {
    let (body, row) = exchange(
        "reasoning-chat-buffered",
        ProviderKind::OpenaiCompatible,
        Client::Chat,
        false,
        chat_completion_with(CHAT_REASONING_USAGE),
    )
    .await;
    // nothing is translated: the provider's body is the client's
    assert_thinking_chat_usage(&shown_usage(&body));
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn chat_reasoning_answer_to_a_responses_client_names_the_share() {
    let (body, row) = exchange(
        "reasoning-responses-buffered",
        ProviderKind::OpenaiCompatible,
        Client::Responses,
        false,
        chat_completion_with(CHAT_REASONING_USAGE),
    )
    .await;
    let usage = shown_usage(&body);
    assert_eq!(usage["input_tokens"], 2000, "usage shown to the client");
    assert_eq!(usage["total_tokens"], 2023, "usage shown to the client");
    assert_reasoning_named_responses(&usage);
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn chat_reasoning_stream_to_a_responses_client_names_the_share() {
    let (body, row) = exchange(
        "reasoning-responses-streamed",
        ProviderKind::OpenaiCompatible,
        Client::Responses,
        true,
        chat_stream_with(CHAT_REASONING_USAGE),
    )
    .await;
    let usage = shown_usage(&body);
    assert_eq!(usage["total_tokens"], 2023, "usage shown to the client");
    assert_reasoning_named_responses(&usage);
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn chat_reasoning_answer_to_a_messages_client_has_nowhere_to_name_it() {
    let (body, row) = exchange(
        "reasoning-messages-buffered",
        ProviderKind::OpenaiCompatible,
        Client::Messages,
        false,
        chat_completion_with(CHAT_REASONING_USAGE),
    )
    .await;
    let usage = shown_usage(&body);
    assert_eq!(usage["output_tokens"], 23, "usage shown to the client");
    assert_no_reasoning_named(&usage);
    assert_priced_with_the_thoughts(&row);
}

// ── providers that spell the cache hit differently (#2877) ──────────────────
// a request that goes straight through to a Chat Completions client is logged
// from the provider's own body, so the log has to read the spelling itself; a
// Messages or Responses client is served a translated body, so the translator
// has to carry it. both must price the 1500 cached tokens at the cached rate

#[tokio::test]
async fn deepseek_answer_to_a_chat_client_is_priced_with_its_cache() {
    let (_, row) = exchange(
        "deepseek-chat-buffered",
        ProviderKind::Deepseek,
        Client::Chat,
        false,
        chat_completion_with(DEEPSEEK_USAGE),
    )
    .await;
    assert_priced_with_the_cache(&row);
}

#[tokio::test]
async fn deepseek_stream_to_a_chat_client_is_priced_with_its_cache() {
    let (_, row) = exchange(
        "deepseek-chat-streamed",
        ProviderKind::Deepseek,
        Client::Chat,
        true,
        chat_stream_with(DEEPSEEK_USAGE),
    )
    .await;
    assert_priced_with_the_cache(&row);
}

#[tokio::test]
async fn deepseek_answer_to_a_messages_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "deepseek-messages-buffered",
        ProviderKind::Deepseek,
        Client::Messages,
        false,
        chat_completion_with(DEEPSEEK_USAGE),
    )
    .await;
    assert_messages_usage(&shown_usage(&body));
    assert_priced_with_the_cache(&row);
}

#[tokio::test]
async fn deepseek_stream_to_a_responses_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "deepseek-responses-streamed",
        ProviderKind::Deepseek,
        Client::Responses,
        true,
        chat_stream_with(DEEPSEEK_USAGE),
    )
    .await;
    assert_responses_usage(&shown_usage(&body));
    assert_priced_with_the_cache(&row);
}

#[tokio::test]
async fn kimi_answer_to_a_chat_client_is_priced_with_its_cache() {
    let (_, row) = exchange(
        "kimi-chat-buffered",
        ProviderKind::Kimi,
        Client::Chat,
        false,
        chat_completion_with(KIMI_USAGE),
    )
    .await;
    assert_priced_with_the_cache(&row);
}

#[tokio::test]
async fn kimi_stream_to_a_messages_client_is_priced_with_its_cache() {
    let (body, row) = exchange(
        "kimi-messages-streamed",
        ProviderKind::Kimi,
        Client::Messages,
        true,
        chat_stream_with(KIMI_USAGE),
    )
    .await;
    assert_messages_usage(&shown_usage(&body));
    assert_priced_with_the_cache(&row);
}

#[tokio::test]
async fn gigachat_answer_to_a_chat_client_is_priced_with_its_cache() {
    let (_, row) = exchange(
        "gigachat-chat-buffered",
        ProviderKind::Gigachat,
        Client::Chat,
        false,
        chat_completion_with(GIGACHAT_USAGE),
    )
    .await;
    assert_priced_with_the_cache(&row);
}

// ── a cache write spelled by Qwen (#2879) ───────────────────────────────────
// Qwen's explicit cache names the tokens it wrote under
// `prompt_tokens_details.cache_creation_input_tokens`, inside `prompt_tokens`.
// a request that created a cache entry used to be logged with no write

#[tokio::test]
async fn qwen_answer_to_a_chat_client_logs_its_cache_write() {
    let (_, row) = exchange(
        "qwen-chat-buffered",
        ProviderKind::Qwen,
        Client::Chat,
        false,
        chat_completion_with(QWEN_USAGE),
    )
    .await;
    assert_priced_with_the_cache(&row);
    assert_cache_write_counted(&row);
}

#[tokio::test]
async fn qwen_stream_to_a_chat_client_logs_its_cache_write() {
    let (_, row) = exchange(
        "qwen-chat-streamed",
        ProviderKind::Qwen,
        Client::Chat,
        true,
        chat_stream_with(QWEN_USAGE),
    )
    .await;
    assert_priced_with_the_cache(&row);
    assert_cache_write_counted(&row);
}

#[tokio::test]
async fn qwen_answer_to_a_messages_client_carries_its_cache_write() {
    let (body, row) = exchange(
        "qwen-messages-buffered",
        ProviderKind::Qwen,
        Client::Messages,
        false,
        chat_completion_with(QWEN_USAGE),
    )
    .await;
    // anthropic's `input_tokens` leaves the reads and the writes out
    let usage = shown_usage(&body);
    assert_eq!(usage["input_tokens"], 400, "usage shown to the client");
    assert_eq!(
        usage["cache_read_input_tokens"], 1500,
        "usage shown to the client"
    );
    assert_eq!(
        usage["cache_creation_input_tokens"], 100,
        "usage shown to the client"
    );
    assert_priced_with_the_cache(&row);
    assert_cache_write_counted(&row);
}

#[tokio::test]
async fn qwen_stream_to_a_responses_client_carries_its_cache_write() {
    let (body, row) = exchange(
        "qwen-responses-streamed",
        ProviderKind::Qwen,
        Client::Responses,
        true,
        chat_stream_with(QWEN_USAGE),
    )
    .await;
    let usage = shown_usage(&body);
    assert_responses_usage(&usage);
    assert_eq!(
        usage["input_tokens_details"]["cache_write_tokens"], 100,
        "usage shown to the client"
    );
    assert_priced_with_the_cache(&row);
    assert_cache_write_counted(&row);
}

// ── a provider that states reasoning beside the completion (#2888) ──────────
// xAI bills the 20 thinking tokens as output but does not count them in
// `completion_tokens`, so the row used to log 3 and be charged 2003 where the
// provider charged 2023

#[tokio::test]
async fn xai_answer_to_a_chat_client_is_logged_with_its_reasoning() {
    let (_, row) = exchange(
        "xai-chat-buffered",
        ProviderKind::Xai,
        Client::Chat,
        false,
        chat_completion_with(XAI_USAGE),
    )
    .await;
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn xai_stream_to_a_chat_client_is_logged_with_its_reasoning() {
    let (_, row) = exchange(
        "xai-chat-streamed",
        ProviderKind::Xai,
        Client::Chat,
        true,
        chat_stream_with(XAI_USAGE),
    )
    .await;
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn xai_answer_to_a_responses_client_counts_its_reasoning_once() {
    let (body, row) = exchange(
        "xai-responses-buffered",
        ProviderKind::Xai,
        Client::Responses,
        false,
        chat_completion_with(XAI_USAGE),
    )
    .await;
    let usage = shown_usage(&body);
    assert_eq!(usage["total_tokens"], 2023, "usage shown to the client");
    assert_reasoning_named_responses(&usage);
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn xai_stream_to_a_messages_client_counts_its_reasoning_once() {
    let (body, row) = exchange(
        "xai-messages-streamed",
        ProviderKind::Xai,
        Client::Messages,
        true,
        chat_stream_with(XAI_USAGE),
    )
    .await;
    let usage = shown_usage(&body);
    assert_eq!(usage["output_tokens"], 23, "usage shown to the client");
    assert_no_reasoning_named(&usage);
    assert_priced_with_the_thoughts(&row);
}

// ── thinking that only the total states (#2880) ─────────────────────────────
// gemini's openai-compatible endpoint bills the 20 thinking tokens as output
// but states them nowhere except `total_tokens`, so the row used to log 3 and be
// charged 2003 where the provider charged 2023. the chat client is served the
// provider's bytes as they are, so only the row can be put right there

#[tokio::test]
async fn gemini_compat_answer_to_a_chat_client_is_logged_with_its_thinking() {
    let (body, row) = exchange(
        "gemini-compat-chat-buffered",
        ProviderKind::Gemini,
        Client::Chat,
        false,
        chat_completion_with(GEMINI_COMPAT_USAGE),
    )
    .await;
    // a passthrough body is the provider's own
    assert_eq!(shown_usage(&body)["completion_tokens"], 3);
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn gemini_compat_stream_to_a_chat_client_is_logged_with_its_thinking() {
    let (_, row) = exchange(
        "gemini-compat-chat-streamed",
        ProviderKind::Gemini,
        Client::Chat,
        true,
        gemini_compat_stream(),
    )
    .await;
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn gemini_compat_answer_to_a_responses_client_counts_its_thinking_once() {
    let (body, row) = exchange(
        "gemini-compat-responses-buffered",
        ProviderKind::Gemini,
        Client::Responses,
        false,
        chat_completion_with(GEMINI_COMPAT_USAGE),
    )
    .await;
    let usage = shown_usage(&body);
    assert_eq!(usage["total_tokens"], 2023, "usage shown to the client");
    assert_reasoning_named_responses(&usage);
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn gemini_compat_stream_to_a_messages_client_counts_its_thinking_once() {
    let (body, row) = exchange(
        "gemini-compat-messages-streamed",
        ProviderKind::Gemini,
        Client::Messages,
        true,
        gemini_compat_stream(),
    )
    .await;
    let usage = shown_usage(&body);
    assert_eq!(usage["output_tokens"], 23, "usage shown to the client");
    assert_no_reasoning_named(&usage);
    assert_priced_with_the_thoughts(&row);
}

/// the same body from any other kind is read as stated: a total that is more
/// than its parts is not a claim about thinking there
#[tokio::test]
async fn a_total_beyond_its_parts_is_left_alone_for_other_providers() {
    let (_, row) = exchange(
        "mistral-chat-buffered",
        ProviderKind::Mistral,
        Client::Chat,
        false,
        chat_completion_with(GEMINI_COMPAT_USAGE),
    )
    .await;
    assert_eq!(row["completion_tokens"], 3, "request-log row");
    assert_eq!(row["cost_usd"], 2003.0, "request-log row");
}

#[tokio::test]
async fn vertex_answer_to_a_chat_client_is_logged_with_its_thinking() {
    let (_, row) = exchange(
        "vertex-chat-buffered",
        ProviderKind::Vertex,
        Client::Chat,
        false,
        chat_completion_with(VERTEX_COMPAT_USAGE),
    )
    .await;
    assert_priced_with_the_thoughts(&row);
}

#[tokio::test]
async fn vertex_stream_to_a_messages_client_counts_its_thinking_once() {
    let (body, row) = exchange(
        "vertex-messages-streamed",
        ProviderKind::Vertex,
        Client::Messages,
        true,
        chat_stream_with(VERTEX_COMPAT_USAGE),
    )
    .await;
    let usage = shown_usage(&body);
    assert_eq!(usage["output_tokens"], 23, "usage shown to the client");
    assert_priced_with_the_thoughts(&row);
}
