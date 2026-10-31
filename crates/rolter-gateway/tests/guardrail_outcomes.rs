//! What each guardrail rule did to a request is on its log row (#2158).
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
    BalancingStrategy, BuiltinRule, GatewayConfig, GuardAction, GuardStage, GuardrailRule,
    GuardrailsConfig, ModelRoute, ProviderConfig, ProviderKind, Target, VirtualKeyRecord,
};
use serde_json::{json, Value};

const KEY: &str = "sk-guardrail-outcomes";

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
                    "message": {"role": "assistant", "content": "write to ops@corp.com"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }))
        }),
    ))
    .await
}

fn config(
    upstream: SocketAddr,
    clickhouse: SocketAddr,
    stage: GuardStage,
    action: GuardAction,
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
        id: "key-guardrails".into(),
        org_id: "org-guardrails".into(),
        ..Default::default()
    });
    config.guardrails = GuardrailsConfig {
        enabled: true,
        max_scan_bytes: None,
        streaming_post_call: Default::default(),
        rules: vec![GuardrailRule {
            name: "email".to_string(),
            builtin: Some(BuiltinRule::Email),
            pattern: None,
            stage,
            action,
            replacement: None,
            include_system: false,
        }],
    };
    config
}

async fn ask(config: &GatewayConfig, prompt: &str) -> reqwest::Response {
    let state = rolter_gateway::AppState::with_logging(config, None);
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .bearer_auth(KEY)
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": prompt}]}))
        .send()
        .await
        .unwrap()
}

async fn run(stage: GuardStage, action: GuardAction, prompt: &str) -> (u16, Value) {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let up = upstream().await;
    let response = ask(&config(up, clickhouse, stage, action), prompt).await;
    let status = response.status().as_u16();
    let _ = response.bytes().await.unwrap();
    (status, rows.only_row().await)
}

#[tokio::test]
async fn a_pre_call_block_leaves_a_row_naming_the_rule_and_never_the_text() {
    let (status, row) = run(
        GuardStage::PreCall,
        GuardAction::Block,
        "mail zed@secret.example",
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(row["status"], 400, "{row}");
    assert_eq!(row["guardrail_events"], json!(["blocked:email"]), "{row}");
    // refused before routing, and named for why (#3307)
    assert_eq!(row["outcome"], "refused_guardrail", "{row}");
    assert!(!row.to_string().contains("secret.example"), "{row}");
}

#[tokio::test]
async fn a_pre_call_redaction_is_recorded_on_the_delivered_request() {
    let (status, row) = run(
        GuardStage::PreCall,
        GuardAction::Redact,
        "mail zed@secret.example",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(row["guardrail_events"], json!(["redacted:email"]), "{row}");
    assert_eq!(row["outcome"], "served", "{row}");
}

#[tokio::test]
async fn an_annotate_match_leaves_a_trace_and_forwards_unchanged() {
    let (status, row) = run(
        GuardStage::PreCall,
        GuardAction::Annotate,
        "mail zed@secret.example",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(row["guardrail_events"], json!(["annotated:email"]), "{row}");
}

#[tokio::test]
async fn a_request_no_rule_matched_carries_no_events_field() {
    let (status, row) = run(GuardStage::PreCall, GuardAction::Block, "hello").await;
    assert_eq!(status, 200);
    assert!(row.get("guardrail_events").is_none(), "{row}");
}

#[tokio::test]
async fn a_post_call_block_names_the_rule_on_the_withheld_row() {
    let (status, row) = run(GuardStage::PostCall, GuardAction::Block, "ping").await;
    assert_eq!(status, 403);
    assert_eq!(row["withheld"], 1, "{row}");
    assert_eq!(row["guardrail_events"], json!(["blocked:email"]), "{row}");
    assert_eq!(row["outcome"], "withheld", "{row}");
}

#[tokio::test]
async fn a_post_call_redaction_and_annotation_are_on_the_delivered_row() {
    // the upstream answers with an address, so the output rule matches (#3330)
    for (action, event) in [
        (GuardAction::Redact, "redacted:email"),
        (GuardAction::Annotate, "annotated:email"),
    ] {
        let (status, row) = run(GuardStage::PostCall, action, "ping").await;
        assert_eq!(status, 200, "{action:?}");
        assert_eq!(row["withheld"], 0, "{row}");
        assert_eq!(row["guardrail_events"], json!([event]), "{row}");
        assert!(!row.to_string().contains("ops@corp.com"), "{row}");
    }
}

/// Runs one request against `config` and returns the status and the one row.
async fn run_with(config: GatewayConfig, clickhouse: &Rows, prompt: &str) -> (u16, Value) {
    let response = ask(&config, prompt).await;
    let status = response.status().as_u16();
    let _ = response.bytes().await.unwrap();
    (status, clickhouse.only_row().await)
}

#[tokio::test]
async fn a_pre_call_block_keeps_the_redactions_applied_before_it() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let up = upstream().await;
    let mut config = config(up, clickhouse, GuardStage::PreCall, GuardAction::Redact);
    config.guardrails.rules.push(GuardrailRule {
        name: "card".to_string(),
        builtin: Some(BuiltinRule::PaymentCard),
        pattern: None,
        stage: GuardStage::PreCall,
        action: GuardAction::Block,
        replacement: None,
        include_system: false,
    });
    let (status, row) = run_with(
        config,
        &rows,
        "mail zed@secret.example card 4111 1111 1111 1111",
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        row["guardrail_events"],
        json!(["blocked:card", "redacted:email"]),
        "{row}"
    );
}

/// A response-stage block keeps the redactions of the output rules that ran
/// before it, as the request stage does (#3403).
#[tokio::test]
async fn a_post_call_block_keeps_the_redactions_applied_before_it() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let up = upstream().await;
    let mut config = config(up, clickhouse, GuardStage::PostCall, GuardAction::Redact);
    // the upstream answers "write to ops@corp.com": the address is redacted
    // first, then the verb withholds the masked answer
    config.guardrails.rules.push(GuardrailRule {
        name: "verb".to_string(),
        builtin: None,
        pattern: Some("write".to_string()),
        stage: GuardStage::PostCall,
        action: GuardAction::Block,
        replacement: None,
        include_system: false,
    });
    let (status, row) = run_with(config, &rows, "ping").await;
    assert_eq!(status, 403);
    assert_eq!(row["withheld"], 1, "{row}");
    assert_eq!(
        row["guardrail_events"],
        json!(["redacted:email", "blocked:verb"]),
        "{row}"
    );
    assert!(!row.to_string().contains("ops@corp.com"), "{row}");
}

async fn webhook(decision: Value) -> SocketAddr {
    serve(Router::new().route(
        "/check",
        post(move || {
            let decision = decision.clone();
            async move { Json(decision) }
        }),
    ))
    .await
}

fn named_webhook(url: String, name: Option<&str>) -> rolter_core::GuardrailWebhookConfig {
    let mut value = json!({"enabled": true, "url": url, "failure_mode": "fail_closed"});
    if let Some(name) = name {
        value["name"] = json!(name);
    }
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn a_provider_block_names_the_provider_and_keeps_earlier_redactions() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let up = upstream().await;
    let guard = webhook(json!({"action": "block"})).await;
    let mut config = config(up, clickhouse, GuardStage::PreCall, GuardAction::Redact);
    config.guardrail_webhook = named_webhook(format!("http://{guard}/check"), Some("Prod guard"));
    let (status, row) = run_with(config, &rows, "mail zed@secret.example").await;
    assert_eq!(status, 400);
    assert_eq!(
        row["guardrail_events"],
        json!(["redacted:email", "provider_blocked:Prod guard"]),
        "{row}"
    );
}

#[tokio::test]
async fn a_provider_transform_names_the_provider() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let up = upstream().await;
    let guard = webhook(json!({"action": "transform", "content": "hi"})).await;
    let mut config = config(up, clickhouse, GuardStage::PreCall, GuardAction::Redact);
    config.guardrail_webhook = named_webhook(format!("http://{guard}/check"), Some("Prod guard"));
    let (status, row) = run_with(config, &rows, "hello").await;
    assert_eq!(status, 200);
    assert_eq!(
        row["guardrail_events"],
        json!(["provider_transformed:Prod guard"]),
        "{row}"
    );
}

#[tokio::test]
async fn a_provider_without_a_name_records_the_bare_action() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let up = upstream().await;
    let guard = webhook(json!({"action": "block"})).await;
    let mut config = config(up, clickhouse, GuardStage::PreCall, GuardAction::Redact);
    config.guardrail_webhook = named_webhook(format!("http://{guard}/check"), None);
    let (status, row) = run_with(config, &rows, "hello").await;
    assert_eq!(status, 400);
    assert_eq!(
        row["guardrail_events"],
        json!(["provider_blocked"]),
        "{row}"
    );
}

/// An upstream that streams `text` whatever the request asked for.
async fn sse_upstream(text: &'static str) -> SocketAddr {
    serve(Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            let chunk = json!({
                "id": "c", "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": {"content": text}}]
            });
            let usage = json!({
                "id": "c", "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7}
            });
            (
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                format!("data: {chunk}\n\ndata: {usage}\n\ndata: [DONE]\n\n"),
            )
        }),
    ))
    .await
}

/// #3471: a request that did not ask for a stream keeps the whole-body
/// response guardrails, which cannot read an event stream. An upstream that
/// streams anyway has its answer withheld, still billed, rather than relayed
/// past the output rules; with no such rule the stream is served as it came.
#[tokio::test]
async fn an_unrequested_stream_is_withheld_under_output_rules() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let up = sse_upstream("write to ops@corp.com").await;
    let guarded = config(up, clickhouse, GuardStage::PostCall, GuardAction::Redact);
    let response = ask(&guarded, "ping").await;
    assert_eq!(response.status(), 403);
    let body = response.text().await.unwrap();
    assert!(body.contains("guardrail_blocked"), "{body}");
    assert!(
        !body.contains("ops@corp.com"),
        "the answer must not leak: {body}"
    );
    let row = rows.only_row().await;
    assert_eq!(row["withheld"], 1, "{row}");
    assert_eq!(
        row["total_tokens"], 7,
        "billed from the stream it sent: {row}"
    );
    assert_eq!(
        row["guardrail_events"],
        json!(["blocked:unrequested_stream"]),
        "{row}"
    );

    // the same answer on a route without response rules streams through
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let up = sse_upstream("hello").await;
    let mut open = config(up, clickhouse, GuardStage::PreCall, GuardAction::Redact);
    open.guardrails.rules.clear();
    let response = ask(&open, "ping").await;
    assert_eq!(response.status(), 200);
    assert!(response.text().await.unwrap().contains("hello"));
}
