//! Every request-log row names its outcome (#3307), and a served one the
//! shape of the request in either dialect (#3317).
//!
//! Request-log rows go to an in-process stand-in for the ClickHouse HTTP
//! interface, so these tests always run and need no external service.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
    VirtualKeyRecord,
};
use serde_json::{json, Value};

// split so the source holds no literal shaped like a key
const KEY: &str = concat!("sk-", "request-outcome");

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

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
                for line in body.lines().filter(|line| !line.trim().is_empty()) {
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

    async fn only_row(&self) -> Value {
        for _ in 0..200 {
            if !self.0.lock().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let rows = self.0.lock().clone();
        assert_eq!(rows.len(), 1, "expected one row, saw {rows:?}");
        rows[0].clone()
    }
}

/// An upstream that answers with `status`.
async fn upstream(status: StatusCode) -> SocketAddr {
    serve(Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            (
                status,
                Json(json!({
                    "id": "chatcmpl-mock",
                    "object": "chat.completion",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "pong"},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                })),
            )
        }),
    ))
    .await
}

fn config(upstream: SocketAddr, clickhouse: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
    config.retry.max_retries = 0;
    config.providers.push(ProviderConfig {
        name: "up".into(),
        kind: ProviderKind::OpenaiCompatible,
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
    config.db_virtual_keys.push(VirtualKeyRecord {
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), KEY),
        id: "key-outcome".into(),
        org_id: "org-outcome".into(),
        ..Default::default()
    });
    config
}

/// One request for `model` through a gateway whose only target answers with
/// `upstream_status`; the caller's status and the one row it left.
async fn run(upstream_status: StatusCode, model: &str) -> (u16, Value) {
    run_body(
        upstream_status,
        "/v1/chat/completions",
        json!({"model": model, "messages": [{"role": "user", "content": "hi"}]}),
    )
    .await
}

/// `body` sent to `path` through a gateway whose only target answers with
/// `upstream_status`; the caller's status and the one row it left.
async fn run_body(upstream_status: StatusCode, path: &str, body: Value) -> (u16, Value) {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let up = upstream(upstream_status).await;
    let state = rolter_gateway::AppState::with_logging(&config(up, clickhouse), None);
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    let response = reqwest::Client::new()
        .post(format!("http://{gw}{path}"))
        .bearer_auth(KEY)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let _ = response.bytes().await.unwrap();
    (status, rows.only_row().await)
}

#[tokio::test]
async fn a_successful_request_is_served() {
    let (status, row) = run(StatusCode::OK, "test-model").await;
    assert_eq!(status, 200);
    assert_eq!(row["outcome"], "served", "{row}");
}

#[tokio::test]
async fn an_upstream_failure_is_an_upstream_error() {
    let (status, row) = run(StatusCode::BAD_REQUEST, "test-model").await;
    assert_eq!(status, 400);
    assert_eq!(row["attempts"], 1, "{row}");
    assert_eq!(row["outcome"], "upstream_error", "{row}");
}

#[tokio::test]
async fn an_unknown_model_is_refused_for_access_with_no_routing_fields() {
    let (status, row) = run(StatusCode::OK, "no-such-model").await;
    assert_eq!(status, 404);
    assert_eq!(row["outcome"], "refused_access", "{row}");
    assert_eq!(row["provider"], "", "{row}");
    assert_eq!(row["attempts"], 0, "{row}");
}

#[tokio::test]
async fn a_plain_request_records_one_turn_and_no_shape_flags() {
    let (_, row) = run(StatusCode::OK, "test-model").await;
    assert_eq!(row["turns"], 1, "{row}");
    for flag in [
        "has_tools",
        "has_image_input",
        "has_audio_input",
        "has_response_schema",
    ] {
        // left out while zero, so an un-migrated clickhouse keeps every row
        assert!(row.get(flag).is_none(), "{flag}: {row}");
    }
}

#[tokio::test]
async fn an_openai_request_records_its_shape() {
    let (status, row) = run_body(
        StatusCode::OK,
        "/v1/chat/completions",
        json!({
            "model": "test-model",
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "what is this"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA"}}
                ]},
                {"role": "assistant", "content": "a cat"},
                {"role": "user", "content": "and this?"}
            ],
            "tools": [{"type": "function", "function": {"name": "lookup", "parameters": {"type": "object"}}}],
            "response_format": {"type": "json_schema", "json_schema": {"name": "x", "schema": {}}}
        }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(row["has_tools"], 1, "{row}");
    assert_eq!(row["has_image_input"], 1, "{row}");
    assert_eq!(row["has_response_schema"], 1, "{row}");
    assert_eq!(row["turns"], 2, "{row}");
    assert!(row.get("has_audio_input").is_none(), "{row}");
}

#[tokio::test]
async fn an_anthropic_request_records_its_shape() {
    // an anthropic client on an openai-compatible upstream: the shape is read
    // off the body the caller sent, in the caller's dialect
    let (status, row) = run_body(
        StatusCode::OK,
        "/v1/messages",
        json!({
            "model": "test-model",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AA"}},
                {"type": "text", "text": "what is this"}
            ]}],
            "tools": [{"name": "lookup", "input_schema": {"type": "object"}}]
        }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(row["has_tools"], 1, "{row}");
    assert_eq!(row["has_image_input"], 1, "{row}");
    assert_eq!(row["turns"], 1, "{row}");
}
