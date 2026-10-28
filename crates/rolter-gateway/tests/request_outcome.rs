//! Every request-log row names its outcome (#3307).
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
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), KEY),
        id: "key-outcome".into(),
        org_id: "org-outcome".into(),
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
    config
}

/// One request for `model` through a gateway whose only target answers with
/// `upstream_status`; the caller's status and the one row it left.
async fn run(upstream_status: StatusCode, model: &str) -> (u16, Value) {
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
        .post(format!("http://{gw}/v1/chat/completions"))
        .bearer_auth(KEY)
        .json(&json!({"model": model, "messages": [{"role": "user", "content": "hi"}]}))
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
