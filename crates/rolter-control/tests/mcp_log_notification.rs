//! The MCP log against a ClickHouse that has not applied `047` (#3466), end to
//! end against a real one.
//!
//! `047_mcp_log_notification.sql` adds `notification` to `mcp_tool_call_logs`,
//! applied by hand on an existing deployment. An event ingested through
//! `POST /api/v1/mcp/events` with `notification: true` must still be stored by a
//! table without the column (the insert skips unknown fields), and the detail
//! must still be read (the probe selects a default in its place). Each test gets
//! a database of its own, migrated only as far as the deployment it stands for.
//!
//! Gated on the `postgres` feature and on both `ROLTER_TEST_DATABASE_URL` and
//! `ROLTER_TEST_CLICKHOUSE_URL`; unset either and the tests self-skip.
#![cfg(feature = "postgres")]

#[path = "common/clickhouse_ddl.rs"]
mod clickhouse_ddl;

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Router;
use rolter_store::postgres::test_database;
use rolter_store::postgres::test_schema::TestSchema;
use serde_json::{json, Value};
use uuid::Uuid;

const CLICKHOUSE_URL_ENV: &str = "ROLTER_TEST_CLICKHOUSE_URL";
const ADMIN_TOKEN: &str = "mcp-notification-admin";

fn clickhouse_url() -> Option<String> {
    std::env::var(CLICKHOUSE_URL_ENV)
        .ok()
        .map(|url| url.trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
}

macro_rules! skip_without_stack {
    () => {{
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        match clickhouse_url() {
            Some(url) => url,
            None => {
                eprintln!("skipping: {CLICKHOUSE_URL_ENV} not set");
                return;
            }
        }
    }};
}

/// A database of its own on the shared server, dropped when the test ends,
/// panic included, so a failed run leaks nothing.
struct ScratchDatabase {
    base: String,
    name: String,
}

impl ScratchDatabase {
    async fn create(client: &reqwest::Client, base: &str) -> Self {
        let name = format!("rolter_test_cols_{}", Uuid::new_v4().simple());
        let response = client
            .post(format!("{base}/"))
            .body(format!("create database {name}"))
            .send()
            .await
            .expect("reach clickhouse");
        assert!(
            response.status().is_success(),
            "create database: {}",
            response.text().await.unwrap_or_default()
        );
        Self {
            base: base.to_string(),
            name,
        }
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        let (base, name) = (self.base.clone(), self.name.clone());
        // a runtime of its own on its own thread: this may run while the
        // test's runtime is unwinding, where blocking on it would deadlock
        let _ = std::thread::spawn(move || {
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                runtime.block_on(async move {
                    let _ = reqwest::Client::new()
                        .post(format!("{base}/"))
                        .body(format!("drop database if exists {name}"))
                        .send()
                        .await;
                });
            }
        })
        .join();
    }
}

/// Forward every request to `base` with `database` set, so the control plane's
/// unqualified `from request_logs` reads the scratch database.
#[derive(Clone)]
struct Target {
    base: String,
    database: String,
    client: reqwest::Client,
}

/// The incoming query pairs are re-attached as query parameters of a request to
/// the fixed server url, never spliced into its text: nothing the control plane
/// sent decides where this request goes.
async fn forward(
    State(target): State<Target>,
    Query(pairs): Query<Vec<(String, String)>>,
    body: Bytes,
) -> Response {
    let upstream = match target
        .client
        .post(format!("{}/", target.base))
        .query(&pairs)
        .query(&[("database", target.database.as_str())])
        .body(body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => return (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
    };
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let text = upstream.text().await.unwrap_or_default();
    (status, text).into_response()
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// An event ingested as a notification into a table migrated through `last`,
/// read back through the detail. Returns the ingest status and the detail row.
async fn ingest_then_read(last: u32) -> Option<(StatusCode, Value)> {
    let base = clickhouse_url().filter(|_| test_database::is_configured())?;
    let http = reqwest::Client::new();
    let scratch = ScratchDatabase::create(&http, &base).await;
    clickhouse_ddl::apply_schema_through(&http, &base, Some(&scratch.name), last).await;
    let proxy = serve(
        Router::new()
            .fallback(axum::routing::post(forward))
            .with_state(Target {
                base: base.clone(),
                database: scratch.name.clone(),
                client: http.clone(),
            }),
    )
    .await;
    let schema = TestSchema::create(&test_database::url().await?).await;
    let app = rolter_control::test_app_with_clickhouse_and_admin_token(
        schema.pool().clone(),
        &format!("http://{proxy}"),
        Some(ADMIN_TOKEN.to_string()),
    )
    .await
    .unwrap();
    let addr = serve(app).await;

    let event_id = format!("notification-{}", Uuid::new_v4().simple());
    let ingest = http
        .post(format!("http://{addr}/api/v1/mcp/events"))
        .bearer_auth(ADMIN_TOKEN)
        .json(&json!({
            "event_id": event_id, "server": "docs", "tool": "search",
            "transport": "streamable_http", "status": "success", "latency_ms": 1,
            "notification": true,
        }))
        .send()
        .await
        .unwrap();
    let status = ingest.status();
    let detail = http
        .get(format!("http://{addr}/api/v1/mcp/logs/{event_id}"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    let read = detail.status();
    let row: Value = detail.json().await.unwrap_or(Value::Null);
    drop(schema);
    drop(scratch);
    assert_eq!(read, 200, "the detail failed after migration {last}: {row}");
    Some((status, row))
}

#[tokio::test]
async fn a_table_without_047_stores_a_notification_and_reads_the_default() {
    let _ = skip_without_stack!();
    let (status, row) = ingest_then_read(46).await.expect("a configured stack");
    assert_eq!(status, 202);
    assert_eq!(row["notification"], 0, "{row}");
    assert_eq!(row["status"], "success", "{row}");
}

#[tokio::test]
async fn a_table_with_047_keeps_the_notification_flag() {
    let _ = skip_without_stack!();
    let (status, row) = ingest_then_read(47).await.expect("a configured stack");
    assert_eq!(status, 202);
    assert_eq!(row["notification"], 1, "{row}");
}
