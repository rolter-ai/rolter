//! Usage on a streamed chat completion from an OpenAI-style upstream (#3667).
//!
//! OpenAI, vLLM and most servers that speak its protocol send a stream's
//! token usage only when the request sets `stream_options.include_usage`.
//! Most callers do not, and their requests were logged with no tokens and no
//! cost. The gateway now asks on their behalf, logs the usage the upstream
//! then sends, and keeps that usage chunk from a caller that did not ask for
//! it, so the caller reads the stream it always read. A caller that asked
//! gets the chunk as before.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
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

/// A stand-in for the ClickHouse HTTP interface keeping every `request_logs` row.
#[derive(Clone, Default)]
struct Rows(Arc<Mutex<Vec<Value>>>);

impl Rows {
    async fn serve(&self) -> SocketAddr {
        async fn ingest(
            State(rows): State<Rows>,
            axum::extract::RawQuery(query): axum::extract::RawQuery,
            body: String,
        ) -> &'static str {
            if query.unwrap_or_default().contains("request_logs") {
                for line in body.lines().filter(|l| !l.trim().is_empty()) {
                    if let Ok(row) = serde_json::from_str::<Value>(line) {
                        rows.0.lock().push(row);
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

    async fn row_for(&self, request_id: &str) -> Value {
        for _ in 0..200 {
            if let Some(row) = self.0.lock().iter().find(|r| r["request_id"] == request_id) {
                return row.clone();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no request-log row arrived for {request_id}");
    }
}

const CONTENT: &str = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"pong\"},\"finish_reason\":\"stop\"}]}\n\n";
const USAGE: &str = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3,\"total_tokens\":10}}\n\n";
const DONE: &str = "data: [DONE]\n\n";

/// An upstream that streams usage only when the request asks for it, as
/// OpenAI does, and keeps every request body it was sent.
async fn upstream() -> (SocketAddr, Arc<Mutex<Vec<Value>>>) {
    async fn chat(
        State(seen): State<Arc<Mutex<Vec<Value>>>>,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        let asked = body["stream_options"]["include_usage"] == true;
        seen.lock().push(body);
        let stream = if asked {
            format!("{CONTENT}{USAGE}{DONE}")
        } else {
            format!("{CONTENT}{DONE}")
        };
        ([(header::CONTENT_TYPE, "text/event-stream")], stream)
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let addr = serve(
        Router::new()
            .route("/v1/chat/completions", post(chat))
            .with_state(seen.clone()),
    )
    .await;
    (addr, seen)
}

fn config(kind: ProviderKind, upstream: SocketAddr, clickhouse: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
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

async fn stream(config: &GatewayConfig, request_id: &str, body: Value) -> String {
    let state = rolter_gateway::AppState::with_logging(config, None);
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .header("x-request-id", request_id)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.text().await.unwrap()
}

fn request(extra: Value) -> Value {
    let mut body = json!({
        "model": "test-model",
        "stream": true,
        "messages": [{"role": "user", "content": "ping"}]
    });
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        body.extend(extra.clone());
    }
    body
}

#[tokio::test]
async fn usage_is_asked_for_logged_and_kept_from_a_caller_that_did_not_ask() {
    let rows = Rows::default();
    let (up, seen) = upstream().await;
    let config = config(ProviderKind::OpenaiCompatible, up, rows.serve().await);
    let text = stream(&config, "req-not-asked", request(json!({}))).await;
    // the upstream was asked for usage on the caller's behalf
    assert_eq!(seen.lock()[0]["stream_options"]["include_usage"], true);
    // the caller reads the stream it always read: content, then [DONE]
    assert_eq!(text, format!("{CONTENT}{DONE}"));
    // and the row carries the usage the upstream sent
    let row = rows.row_for("req-not-asked").await;
    assert_eq!(row["prompt_tokens"], 7, "{row}");
    assert_eq!(row["completion_tokens"], 3, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
}

#[tokio::test]
async fn a_caller_that_asked_for_usage_gets_the_chunk() {
    let rows = Rows::default();
    let (up, seen) = upstream().await;
    let config = config(ProviderKind::OpenaiCompatible, up, rows.serve().await);
    let text = stream(
        &config,
        "req-asked",
        request(json!({"stream_options": {"include_usage": true}})),
    )
    .await;
    assert_eq!(
        seen.lock()[0]["stream_options"],
        json!({"include_usage": true})
    );
    assert_eq!(text, format!("{CONTENT}{USAGE}{DONE}"));
    assert_eq!(rows.row_for("req-asked").await["total_tokens"], 10);
}

#[tokio::test]
async fn a_caller_that_said_no_is_not_overruled() {
    let rows = Rows::default();
    let (up, seen) = upstream().await;
    let config = config(ProviderKind::OpenaiCompatible, up, rows.serve().await);
    let text = stream(
        &config,
        "req-said-no",
        request(json!({"stream_options": {"include_usage": false}})),
    )
    .await;
    assert_eq!(seen.lock()[0]["stream_options"]["include_usage"], false);
    assert_eq!(text, format!("{CONTENT}{DONE}"));
}

#[tokio::test]
async fn a_request_that_does_not_stream_is_sent_as_it_came() {
    let rows = Rows::default();
    let (up, seen) = upstream().await;
    let config = config(ProviderKind::OpenaiCompatible, up, rows.serve().await);
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    let _ = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap()
        .text()
        .await;
    assert!(
        seen.lock()[0].get("stream_options").is_none(),
        "{:?}",
        seen.lock()[0]
    );
}
