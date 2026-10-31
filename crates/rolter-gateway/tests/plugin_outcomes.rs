//! What each plugin did to a request is on its log row (#2243).
//!
//! Request-log rows go to an in-process stand-in for the ClickHouse HTTP
//! interface, so these tests always run and need no external service.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, FailureMode, GatewayConfig, ModelRoute, PluginInstanceConfig, PluginStage,
    PluginsConfig, ProviderConfig, ProviderKind, Target, VirtualKeyRecord,
};
use serde_json::{json, Value};

const KEY: &str = "sk-plugin-outcomes";

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

async fn upstream() -> SocketAddr {
    serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            Json(json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "hello"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }))
        }),
    ))
    .await
}

/// A plugin endpoint answering every call with `answer`.
async fn hook(answer: Value) -> SocketAddr {
    serve(Router::new().route(
        "/hook",
        post(move || {
            let answer = answer.clone();
            async move { Json(answer) }
        }),
    ))
    .await
}

fn config(
    upstream: SocketAddr,
    clickhouse: SocketAddr,
    stage: PluginStage,
    failure_mode: FailureMode,
    endpoint: String,
) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
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
        id: "key-plugins".into(),
        org_id: "org-plugins".into(),
        ..Default::default()
    });
    config.plugins = PluginsConfig {
        instances: vec![PluginInstanceConfig {
            slug: "audit".to_string(),
            org_id: "org-plugins".to_string(),
            project_id: None,
            stage,
            position: 0,
            failure_mode,
            endpoint,
            auth: None,
        }],
    };
    config
}

async fn run(
    stage: PluginStage,
    failure_mode: FailureMode,
    endpoint: impl FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = String>>>,
) -> (u16, Value) {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let up = upstream().await;
    let config = config(up, clickhouse, stage, failure_mode, endpoint().await);
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .bearer_auth(KEY)
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let _ = response.bytes().await.unwrap();
    (status, rows.only_row().await)
}

fn answering(
    answer: Value,
) -> impl FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = String>>> {
    move || Box::pin(async move { format!("http://{}/hook", hook(answer).await) })
}

/// A port nothing listens on, so every call fails to connect.
fn dead_endpoint() -> impl FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = String>>>
{
    || {
        Box::pin(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            drop(listener);
            format!("http://{addr}/hook")
        })
    }
}

#[tokio::test]
async fn an_allowing_plugin_is_counted_as_a_call() {
    let (status, row) = run(
        PluginStage::PreUpstream,
        FailureMode::FailOpen,
        answering(json!({"action": "allow"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(row["plugin_events"], json!(["allowed:audit"]), "{row}");
}

#[tokio::test]
async fn a_block_leaves_a_row_naming_the_plugin_and_never_its_reason() {
    let (status, row) = run(
        PluginStage::PreUpstream,
        FailureMode::FailOpen,
        answering(json!({"action": "block", "reason": "secret-reason-text"})),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(row["plugin_events"], json!(["blocked:audit"]), "{row}");
    assert!(!row["plugin_events"].to_string().contains("secret-reason"));
}

#[tokio::test]
async fn a_pre_route_block_is_recorded_too() {
    let (status, row) = run(
        PluginStage::PreRoute,
        FailureMode::FailOpen,
        answering(json!({"action": "block"})),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(row["plugin_events"], json!(["blocked:audit"]), "{row}");
}

#[tokio::test]
async fn a_transform_is_recorded_on_the_delivered_request() {
    let (status, row) = run(
        PluginStage::PreUpstream,
        FailureMode::FailOpen,
        answering(json!({
            "action": "transform",
            "content": {"model": "test-model", "messages": [{"role": "user", "content": "changed"}]}
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(row["plugin_events"], json!(["transformed:audit"]), "{row}");
}

#[tokio::test]
async fn a_dead_fail_open_plugin_forwards_the_request_and_records_the_error() {
    let (status, row) = run(
        PluginStage::PreUpstream,
        FailureMode::FailOpen,
        dead_endpoint(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        row["plugin_events"],
        json!(["error:audit:connect"]),
        "{row}"
    );
}

#[tokio::test]
async fn a_dead_fail_closed_plugin_refuses_and_records_an_error_not_a_block() {
    let (status, row) = run(
        PluginStage::PreUpstream,
        FailureMode::FailClosed,
        dead_endpoint(),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        row["plugin_events"],
        json!(["error:audit:connect"]),
        "{row}"
    );
}

#[tokio::test]
async fn a_post_response_block_names_the_plugin_on_the_withheld_row() {
    let (status, row) = run(
        PluginStage::PostResponse,
        FailureMode::FailOpen,
        answering(json!({"action": "block"})),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(row["withheld"], 1, "{row}");
    assert_eq!(row["plugin_events"], json!(["blocked:audit"]), "{row}");
}

#[tokio::test]
async fn a_post_response_allow_is_recorded_on_the_delivered_row() {
    let (status, row) = run(
        PluginStage::PostResponse,
        FailureMode::FailOpen,
        answering(json!({"action": "allow"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(row["plugin_events"], json!(["allowed:audit"]), "{row}");
}

#[tokio::test]
async fn a_request_no_plugin_saw_carries_no_events_field() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let up = upstream().await;
    let mut config = config(
        up,
        clickhouse,
        PluginStage::PreUpstream,
        FailureMode::FailOpen,
        "http://127.0.0.1:1/hook".to_string(),
    );
    config.plugins = PluginsConfig::default();
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .bearer_auth(KEY)
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let _ = response.bytes().await.unwrap();
    assert!(rows.only_row().await.get("plugin_events").is_none());
}
