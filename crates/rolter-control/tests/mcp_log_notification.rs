//! The MCP log against a ClickHouse that has not applied `047` (#3466) or `051`
//! (#3529), and the summary rates (#3526), end to
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

/// A control plane over a scratch ClickHouse database migrated through `last`.
/// The guards drop the database and the Postgres schema when the test ends.
struct Stack {
    http: reqwest::Client,
    addr: SocketAddr,
    _schema: TestSchema,
    _scratch: ScratchDatabase,
}

impl Stack {
    async fn migrated_through(last: u32) -> Option<Self> {
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
        Some(Self {
            http,
            addr,
            _schema: schema,
            _scratch: scratch,
        })
    }

    /// Ingest one event with `fields` over a plain successful call; returns its
    /// id and the ingest status.
    async fn ingest(&self, fields: Value) -> (String, StatusCode) {
        let event_id = format!("evt-{}", Uuid::new_v4().simple());
        let mut event = json!({
            "event_id": event_id, "server": "docs", "tool": "search",
            "transport": "streamable_http", "status": "success", "latency_ms": 1,
        });
        for (key, value) in fields.as_object().into_iter().flatten() {
            event[key] = value.clone();
        }
        let response = self
            .http
            .post(format!("http://{}/api/v1/mcp/events", self.addr))
            .bearer_auth(ADMIN_TOKEN)
            .json(&event)
            .send()
            .await
            .unwrap();
        (event_id, response.status())
    }

    async fn get(&self, path: &str) -> Value {
        let response = self
            .http
            .get(format!("http://{}{path}", self.addr))
            .bearer_auth(ADMIN_TOKEN)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        assert_eq!(status, 200, "{path}: {body}");
        body
    }

    async fn detail(&self, event_id: &str) -> Value {
        self.get(&format!("/api/v1/mcp/logs/{event_id}")).await
    }

    /// the list row of `event_id`
    async fn listed(&self, event_id: &str) -> Value {
        let page = self.get("/api/v1/mcp/logs?limit=200").await;
        page["data"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["event_id"] == event_id)
            .cloned()
            .unwrap_or_else(|| panic!("{event_id} is not listed: {page}"))
    }

    async fn summary(&self) -> Value {
        self.get("/api/v1/mcp/logs/summary").await["data"][0].clone()
    }
}

/// An event ingested as a notification into a table migrated through `last`,
/// read back through the detail. Returns the ingest status and the detail row.
async fn ingest_then_read(last: u32) -> Option<(StatusCode, Value)> {
    let stack = Stack::migrated_through(last).await?;
    let (event_id, status) = stack.ingest(json!({"notification": true})).await;
    Some((status, stack.detail(&event_id).await))
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

/// #3526, #3529: the list carries both flags, and a table that has not applied
/// `051` stores an `accepted` call as a plain success and reads no outcome
#[tokio::test]
async fn the_list_and_detail_carry_the_notification_flag_and_the_outcome() {
    let _ = skip_without_stack!();
    for (last, outcome) in [(50, ""), (51, "accepted")] {
        let stack = Stack::migrated_through(last)
            .await
            .expect("a configured stack");
        let (accepted, status) = stack
            .ingest(json!({"transport": "sse", "outcome": "accepted"}))
            .await;
        assert_eq!(status, 202);
        let (note, _) = stack.ingest(json!({"notification": true})).await;
        for row in [stack.listed(&accepted).await, stack.detail(&accepted).await] {
            assert_eq!(
                (&row["status"], &row["outcome"], &row["notification"]),
                (&json!("success"), &json!(outcome), &json!(0)),
                "{last}: {row}"
            );
        }
        let row = stack.listed(&note).await;
        assert_eq!(
            (&row["notification"], &row["outcome"]),
            (&json!(1), &json!("")),
            "{row}"
        );
    }
}

/// Independently applied `051` must not make any read depend on `047`.
#[tokio::test]
async fn a_table_with_051_but_without_047_reads_outcomes_and_defaults_notifications() {
    let _ = skip_without_stack!();
    let stack = Stack::migrated_through(51)
        .await
        .expect("a configured stack");
    // model a deployment with `051` but no `047`, before its first read
    let response = stack
        .http
        .post(format!("{}/", stack._scratch.base))
        .query(&[("database", stack._scratch.name.as_str())])
        .body("alter table mcp_tool_call_logs drop column notification")
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "remove notification: {}",
        response.text().await.unwrap_or_default()
    );

    let empty = stack.summary().await;
    assert_eq!(empty["success_rate"], Value::Null, "{empty}");
    assert_eq!(empty["failure_rate"], Value::Null, "{empty}");
    for fields in [
        json!({"transport": "sse", "outcome": "accepted"}),
        json!({"outcome": "unread", "tool": ""}),
        json!({"notification": true}),
        json!({"status": "error"}),
    ] {
        let expected_outcome = fields["outcome"].as_str().unwrap_or("");
        let (event_id, status) = stack.ingest(fields.clone()).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        // repeated list/detail reads neither need the absent column nor change the row
        for _ in 0..2 {
            for row in [stack.listed(&event_id).await, stack.detail(&event_id).await] {
                assert_eq!(row["notification"], json!(0), "{row}");
                assert_eq!(row["outcome"], json!(expected_outcome), "{row}");
            }
        }
    }
    for _ in 0..2 {
        let summary = stack.summary().await;
        let count = |name: &str| summary[name].to_string().trim_matches('"').to_string();
        assert_eq!(count("calls"), "4", "{summary}");
        assert_eq!(count("not_awaited"), "2", "{summary}");
        assert_eq!(count("successes"), "1", "{summary}");
        assert_eq!(count("failures"), "1", "{summary}");
        assert_eq!(summary["success_rate"], json!(0.5), "{summary}");
        assert_eq!(summary["failure_rate"], json!(0.5), "{summary}");
    }
}

/// #3526: a notification the upstream accepted, an `accepted` call and an
/// `unread` body (#3528) say nothing about the tool, so the summary counts them
/// apart and leaves them out of both rates; a notification that failed is a
/// failure
#[tokio::test]
async fn the_summary_rates_leave_out_the_calls_no_reply_was_awaited_for() {
    let _ = skip_without_stack!();
    let stack = Stack::migrated_through(51)
        .await
        .expect("a configured stack");
    for fields in [
        json!({}),
        json!({}),
        json!({"status": "error"}),
        json!({"status": "timeout"}),
        json!({"notification": true}),
        json!({"notification": true}),
        // not delivered, so a failure like any other
        json!({"notification": true, "status": "transport_error"}),
        json!({"transport": "sse", "outcome": "accepted"}),
        // the one row of a body forwarded unread names no tool
        json!({"outcome": "unread", "tool": ""}),
    ] {
        assert_eq!(stack.ingest(fields).await.1, 202);
    }
    let summary = stack.summary().await;
    // clickhouse writes a UInt64 as a JSON string
    let count = |name: &str| summary[name].to_string().trim_matches('"').to_string();
    assert_eq!(count("calls"), "9", "{summary}");
    assert_eq!(count("not_awaited"), "4", "{summary}");
    assert_eq!(count("successes"), "2", "{summary}");
    assert_eq!(count("failures"), "3", "{summary}");
    assert_eq!(summary["success_rate"], json!(0.4), "{summary}");
    assert_eq!(summary["failure_rate"], json!(0.6), "{summary}");

    // a window with nothing awaited has no rate at all, not a rate of zero
    let only_notes = Stack::migrated_through(51)
        .await
        .expect("a configured stack");
    only_notes.ingest(json!({"notification": true})).await;
    let summary = only_notes.summary().await;
    assert_eq!(summary["success_rate"], Value::Null, "{summary}");
    assert_eq!(summary["failure_rate"], Value::Null, "{summary}");

    // a table without either column reads every call as awaited
    let bare = Stack::migrated_through(46)
        .await
        .expect("a configured stack");
    bare.ingest(json!({"notification": true})).await;
    bare.ingest(json!({"status": "error"})).await;
    let summary = bare.summary().await;
    let count = |name: &str| summary[name].to_string().trim_matches('"').to_string();
    assert_eq!(
        (count("not_awaited"), count("failures")),
        ("0".to_string(), "1".to_string()),
        "{summary}"
    );
    assert_eq!(summary["failure_rate"], json!(0.5), "{summary}");
}
