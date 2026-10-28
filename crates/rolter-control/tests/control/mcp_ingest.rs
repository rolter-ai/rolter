//! The control plane's MCP event ingest route applies the same payload
//! redaction policy as the gateway's own writer (#3277), against a stand-in
//! ClickHouse that records the row it is sent.

use crate::support::*;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn a_secret_in_an_ingested_tool_call_never_reaches_the_stored_row() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let inserted: Arc<Mutex<Vec<String>>> = Arc::default();
    let stub = {
        let inserted = inserted.clone();
        axum::Router::new().fallback(move |body: String| {
            let inserted = inserted.clone();
            async move {
                inserted.lock().await.push(body);
                axum::http::StatusCode::OK
            }
        })
    };
    let ch = format!("http://{}", serve(stub).await);
    let app = rolter_control::test_app_with_clickhouse_and_admin_token(
        pool.clone(),
        &ch,
        Some(admin_token().to_string()),
    )
    .await
    .unwrap();
    let base = format!("http://{}", serve(app).await);
    let client = reqwest::Client::new();
    // capture is off by default, so turn it on the way an operator does
    let saved = client
        .put(format!("{base}/api/v1/logging-settings"))
        .bearer_auth(admin_token())
        .json(&json!({"sample_rate": 1.0, "payload_capture_enabled": true, "payload_capture_max_bytes": 4096}))
        .send()
        .await
        .unwrap();
    assert!(saved.status().is_success(), "{}", saved.status());

    let key = format!("sk-ant-{}", "a1B2c3D4".repeat(4));
    let ingest = |event_id: &str| {
        client
            .post(format!("{base}/api/v1/mcp/events"))
            .bearer_auth(admin_token())
            .json(&json!({
                "event_id": event_id, "server": "docs", "tool": "search",
                "transport": "streamable_http", "status": "success", "latency_ms": 3,
                "arguments": {"query": format!("log in with {key}")},
                "result": {"content": [{"type": "text", "text": format!("found {key}")}]},
            }))
            .send()
    };
    // twice, so the cached redactor is exercised as well as the compiled one
    for id in ["e1", "e2"] {
        assert_eq!(ingest(id).await.unwrap().status(), 202);
    }
    let rows = inserted.lock().await;
    let rows: Vec<&String> = rows.iter().filter(|row| row.contains("\"e")).collect();
    assert_eq!(rows.len(), 2);
    for row in rows {
        let parsed: Value = serde_json::from_str(row).unwrap();
        for column in ["arguments", "result"] {
            let stored = parsed[column].as_str().unwrap();
            assert!(!stored.contains(&key), "{column}: {stored}");
            assert!(
                stored.contains("[redacted:anthropic_key]"),
                "{column}: {stored}"
            );
        }
    }
}
