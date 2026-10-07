//! Request-log rows for the built-in `fake-llm` model (#2802).
//!
//! `fake-llm` answers locally, before the pipeline that writes a row for every
//! provider request, so for a long time its traffic never reached LLM Logs or
//! any usage roll-up. The first request a new operator sends is a `fake-llm`
//! smoke test, so the dashboard answered it with an empty Logs screen.
//!
//! These tests drive every endpoint the built-in serves over HTTP, against an
//! in-process stand-in for the ClickHouse HTTP interface, with no provider or
//! route configured at all.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::post;
use axum::Router;
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
    VirtualKeyRecord,
};
use serde_json::{json, Value};

const KEY: &str = "sk-builtin-request-log";
const ORG: &str = "org-smoke";
const TEAM: &str = "team-smoke";
const PROJECT: &str = "project-smoke";
const KEY_ID: &str = "key-smoke";

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

    /// The row logged for `request_id`, waiting for it to arrive.
    async fn row_for(&self, request_id: &str) -> Value {
        for _ in 0..200 {
            if let Some(row) = self
                .logs
                .lock()
                .iter()
                .find(|row| row["request_id"] == request_id)
            {
                return row.clone();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "no request-log row for {request_id}; saw {:?}",
            self.logs.lock()
        );
    }
}

/// A gateway with no provider and no route, one virtual key attributed to an
/// org, team and project, and request logs going to `clickhouse`.
fn config(clickhouse: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
    config.db_virtual_keys.push(VirtualKeyRecord {
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
        business_unit_id: "bu-smoke".into(),
        customer_id: "customer-smoke".into(),
    });
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

/// Send `body` to `path` with the request id `id` and return the response.
async fn post_json(gw: SocketAddr, path: &str, id: &str, body: Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}{path}"))
        .bearer_auth(KEY)
        .header("x-request-id", id)
        .json(&body)
        .send()
        .await
        .unwrap()
}

/// What every row for the built-in has in common, whatever endpoint wrote it.
fn assert_builtin_row(row: &Value, status: u16, stream: u8) {
    assert_eq!(row["model"], "fake-llm", "{row}");
    assert_eq!(row["provider"], "builtin", "{row}");
    assert_eq!(row["status"], status, "{row}");
    assert_eq!(row["stream"], stream, "{row}");
    assert_eq!(row["virtual_key_id"], KEY_ID, "{row}");
    assert_eq!(row["org_id"], ORG, "{row}");
    assert_eq!(row["team_id"], TEAM, "{row}");
    assert_eq!(row["project_id"], PROJECT, "{row}");
    assert_eq!(row["business_unit_id"], "bu-smoke", "{row}");
    assert_eq!(row["customer_id"], "customer-smoke", "{row}");
    assert_eq!(row["cache_hit"], 0, "{row}");
    // the model costs nothing, so the zero is exact: neither unpriced nor an
    // unreported usage
    assert_eq!(row["cost_usd"], 0.0, "{row}");
    assert_eq!(row["unpriced"], 0, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
}

/// The usage a non-streamed body reports, under either dialect's names.
fn body_usage(body: &Value) -> (u64, u64) {
    let usage = &body["usage"];
    let prompt = usage["prompt_tokens"]
        .as_u64()
        .or_else(|| usage["input_tokens"].as_u64())
        .unwrap();
    let completion = usage["completion_tokens"]
        .as_u64()
        .or_else(|| usage["output_tokens"].as_u64())
        .unwrap_or(0);
    (prompt, completion)
}

fn assert_tokens(row: &Value, (prompt, completion): (u64, u64)) {
    assert!(prompt > 0, "the fake body reported no prompt tokens");
    assert_eq!(row["prompt_tokens"], prompt, "{row}");
    assert_eq!(row["completion_tokens"], completion, "{row}");
    assert_eq!(row["total_tokens"], prompt + completion, "{row}");
}

#[tokio::test]
async fn chat_completions_leave_a_row_with_the_fake_bodys_usage() {
    let rows = Rows::default();
    let gw = gateway(&config(rows.serve().await)).await;
    let request = json!({
        "model": "fake-llm",
        "messages": [{"role": "user", "content": "one two three four five"}]
    });

    let response = post_json(gw, "/v1/chat/completions", "chat-plain", request.clone()).await;
    assert_eq!(response.status(), 200);
    let usage = body_usage(&response.json().await.unwrap());

    let row = rows.row_for("chat-plain").await;
    assert_builtin_row(&row, 200, 0);
    assert_tokens(&row, usage);

    // a stream carries no usage object, yet the row has the same tokens
    let mut streamed = request;
    streamed["stream"] = json!(true);
    let response = post_json(gw, "/v1/chat/completions", "chat-stream", streamed).await;
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    assert!(text.trim_end().ends_with("data: [DONE]"));
    assert!(
        !text.contains("\"usage\""),
        "the stream is expected to carry no usage"
    );

    let row = rows.row_for("chat-stream").await;
    assert_builtin_row(&row, 200, 1);
    assert_tokens(&row, usage);
}

#[tokio::test]
async fn messages_and_responses_leave_rows_for_both_modes() {
    let rows = Rows::default();
    let gw = gateway(&config(rows.serve().await)).await;

    for (path, request) in [
        (
            "/v1/messages",
            json!({"model": "fake-llm", "max_tokens": 16,
                   "messages": [{"role": "user", "content": "one two three"}]}),
        ),
        (
            "/v1/responses",
            json!({"model": "fake-llm", "input": "one two three"}),
        ),
    ] {
        let plain_id = format!("plain{path}");
        let response = post_json(gw, path, &plain_id, request.clone()).await;
        assert_eq!(response.status(), 200, "{path}");
        let usage = body_usage(&response.json().await.unwrap());
        let row = rows.row_for(&plain_id).await;
        assert_builtin_row(&row, 200, 0);
        assert_tokens(&row, usage);

        let stream_id = format!("stream{path}");
        let mut streamed = request;
        streamed["stream"] = json!(true);
        let response = post_json(gw, path, &stream_id, streamed).await;
        assert_eq!(response.status(), 200, "{path}");
        let _ = response.bytes().await.unwrap();
        let row = rows.row_for(&stream_id).await;
        assert_builtin_row(&row, 200, 1);
        assert_tokens(&row, usage);
    }
}

#[tokio::test]
async fn embeddings_and_rerank_leave_rows_that_count_their_prompt() {
    let rows = Rows::default();
    let gw = gateway(&config(rows.serve().await)).await;

    // `stream` means nothing to either, and the row must not claim a stream
    let response = post_json(
        gw,
        "/v1/embeddings",
        "embed",
        json!({"model": "fake-llm", "input": ["alpha beta", "gamma"], "stream": true}),
    )
    .await;
    assert_eq!(response.status(), 200);
    let usage = body_usage(&response.json().await.unwrap());
    assert_eq!(usage, (3, 0));
    let row = rows.row_for("embed").await;
    assert_builtin_row(&row, 200, 0);
    assert_tokens(&row, usage);

    let response = post_json(
        gw,
        "/v1/rerank",
        "rerank",
        json!({"model": "fake-llm", "query": "one two", "documents": ["a b c", "d"]}),
    )
    .await;
    assert_eq!(response.status(), 200);
    let usage = body_usage(&response.json().await.unwrap());
    assert_eq!(usage, (6, 0));
    let row = rows.row_for("rerank").await;
    assert_builtin_row(&row, 200, 0);
    assert_tokens(&row, usage);
}

#[tokio::test]
async fn images_and_speech_leave_rows_without_tokens() {
    let rows = Rows::default();
    let gw = gateway(&config(rows.serve().await)).await;

    for (path, id, request) in [
        (
            "/v1/images/generations",
            "images",
            json!({"model": "fake-llm", "prompt": "a red square"}),
        ),
        (
            "/v1/audio/speech",
            "speech",
            json!({"model": "fake-llm", "input": "hello there"}),
        ),
    ] {
        let response = post_json(gw, path, id, request).await;
        assert_eq!(response.status(), 200, "{path}");
        let _ = response.bytes().await.unwrap();
        let row = rows.row_for(id).await;
        assert_builtin_row(&row, 200, 0);
        assert_tokens_absent(&row);
    }
}

fn assert_tokens_absent(row: &Value) {
    assert_eq!(row["prompt_tokens"], 0, "{row}");
    assert_eq!(row["completion_tokens"], 0, "{row}");
    assert_eq!(row["total_tokens"], 0, "{row}");
}

#[tokio::test]
async fn transcriptions_leave_a_row() {
    let rows = Rows::default();
    let gw = gateway(&config(rows.serve().await)).await;
    let boundary = "ROLTERBOUND";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nfake-llm\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\nRIFFxxxxWAVE\r\n--{b}--\r\n",
        b = boundary
    );
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/audio/transcriptions"))
        .bearer_auth(KEY)
        .header("x-request-id", "transcribe")
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let row = rows.row_for("transcribe").await;
    assert_builtin_row(&row, 200, 0);
    assert_tokens_absent(&row);
}

#[tokio::test]
async fn a_refused_request_is_logged_with_its_status() {
    let rows = Rows::default();
    let gw = gateway(&config(rows.serve().await)).await;

    // the endpoint exists but this request is not one the model can answer
    let response = post_json(
        gw,
        "/v1/embeddings",
        "embed-empty",
        json!({"model": "fake-llm"}),
    )
    .await;
    assert_eq!(response.status(), 400);
    let row = rows.row_for("embed-empty").await;
    assert_builtin_row(&row, 400, 0);
    assert_tokens_absent(&row);

    // and an endpoint the built-in does not serve at all
    let response = post_json(
        gw,
        "/v1/completions",
        "legacy",
        json!({"model": "fake-llm", "prompt": "hi"}),
    )
    .await;
    assert_eq!(response.status(), 404);
    let row = rows.row_for("legacy").await;
    assert_builtin_row(&row, 404, 0);
    assert_tokens_absent(&row);
}

#[tokio::test]
async fn a_configured_route_named_fake_llm_is_logged_as_its_provider() {
    // a route of that name shadows the built-in, so its traffic is a provider
    // request and must keep the provider label of the route's own target
    let rows = Rows::default();
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            axum::Json(json!({
                "id": "x", "object": "chat.completion", "model": "fake-llm",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"},
                             "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }))
        }),
    ))
    .await;
    let mut config = config(rows.serve().await);
    config.providers.push(ProviderConfig {
        name: "shadow".into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{upstream}"),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: "fake-llm".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "shadow".into(),
            model: Some("fake-llm".into()),
            weight: 1,
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    let gw = gateway(&config).await;

    let response = post_json(
        gw,
        "/v1/chat/completions",
        "shadowed",
        json!({"model": "fake-llm", "messages": []}),
    )
    .await;
    assert_eq!(response.status(), 200);
    let _ = response.bytes().await.unwrap();

    let row = rows.row_for("shadowed").await;
    assert_eq!(row["provider"], "shadow", "{row}");
}
