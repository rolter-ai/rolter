//! The invocation list against a ClickHouse that lacks the hand-applied
//! columns, end to end against a real one (#2903).
//!
//! `clickhouse/015` to `017` add columns to `request_logs` that an existing
//! deployment applies by hand, possibly after it upgraded the control plane. A
//! select that names a column the table lacks fails the whole statement, which
//! the unit tests in `analytics.rs` can only show against a stand-in that
//! imitates the error. Whether `toUInt32(0) as cache_write_1h_tokens` is valid
//! beside the payload join, and whether `system.columns` is read the way the
//! probe reads it, is a question for the real engine.
//!
//! Each test gets a database of its own on the shared server, migrated only as
//! far as the deployment it stands for, and reads the list through a control
//! plane whose requests are forwarded into that database: the client has no
//! notion of a non-default database, and the SQL it sends is the production
//! text, unqualified.
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
const ADMIN_TOKEN: &str = "optional-columns-admin";

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

/// The row a request that wrote to the 1 hour cache after one retry leaves, as
/// a current gateway writes it. Inserted the way a gateway inserts, with
/// unknown fields skipped, so the same row goes into a table that has not been
/// migrated to carry every one of them.
fn full_row(request_id: &str) -> Value {
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    json!({
        "ts": ts, "request_id": request_id, "model": "claude-sonnet", "provider": "anthropic",
        "status": 200, "cache_write_tokens": 100, "cache_write_1h_tokens": 40,
        "upstream_status": 429, "attempts": 3, "lifecycle_operation": "retrieve",
    })
}

/// A deployment whose `request_logs` was migrated through `last` and nothing
/// after, holding one full row, read through the control plane. Returns the
/// row as the list answered it.
async fn list_after(last: u32) -> Option<Value> {
    let base = clickhouse_url().filter(|_| test_database::is_configured())?;
    let http = reqwest::Client::new();
    let scratch = ScratchDatabase::create(&http, &base).await;
    clickhouse_ddl::apply_schema_through(&http, &base, Some(&scratch.name), last).await;

    let request_id = format!("optional-columns-{}", Uuid::new_v4().simple());
    let insert = http
        .post(format!("{base}/"))
        .query(&[
            ("database", scratch.name.as_str()),
            ("query", "INSERT INTO request_logs FORMAT JSONEachRow"),
            ("date_time_input_format", "best_effort"),
            ("input_format_skip_unknown_fields", "1"),
        ])
        .body(format!("{}\n", full_row(&request_id)))
        .send()
        .await
        .expect("reach clickhouse");
    assert!(
        insert.status().is_success(),
        "insert: {}",
        insert.text().await.unwrap_or_default()
    );

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

    let response = http
        .get(format!(
            "http://{addr}/api/v1/analytics/invocations?request_id={request_id}"
        ))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        status, 200,
        "the list failed after migration {last}: {body}"
    );
    let rows = body["data"].as_array().expect("a data array");
    assert_eq!(rows.len(), 1, "{body}");
    drop(schema);
    drop(scratch);
    Some(rows[0].clone())
}

#[tokio::test]
async fn a_clickhouse_with_every_migration_returns_the_one_hour_share() {
    let _ = skip_without_stack!();
    let row = list_after(17).await.expect("a configured stack");

    assert_eq!(row["cache_write_tokens"], 100, "{row}");
    assert_eq!(row["cache_write_1h_tokens"], 40, "{row}");
    assert_eq!(row["upstream_status"], 429, "{row}");
    assert_eq!(row["attempts"], 3, "{row}");
    assert_eq!(row["lifecycle_operation"], "retrieve", "{row}");
}

#[tokio::test]
async fn a_clickhouse_from_before_015_lists_every_request_with_defaults() {
    let _ = skip_without_stack!();
    // 014 is the last file a deployment had before the optional columns
    let row = list_after(14).await.expect("a configured stack");

    // what the table holds still comes back
    assert_eq!(row["cache_write_tokens"], 100, "{row}");
    assert_eq!(row["model"], "claude-sonnet", "{row}");
    // and what it lacks reads as the default, with the type the column has:
    // numbers, not strings or nulls, so the dashboard needs no special case
    assert_eq!(row["cache_write_1h_tokens"], 0, "{row}");
    assert_eq!(row["upstream_status"], 0, "{row}");
    assert_eq!(row["attempts"], 0, "{row}");
    assert_eq!(row["lifecycle_operation"], "", "{row}");
}

#[tokio::test]
async fn a_clickhouse_with_only_015_returns_what_it_has_and_defaults_the_rest() {
    let _ = skip_without_stack!();
    let row = list_after(15).await.expect("a configured stack");

    assert_eq!(row["upstream_status"], 429, "{row}");
    assert_eq!(row["attempts"], 3, "{row}");
    assert_eq!(row["lifecycle_operation"], "", "{row}");
    assert_eq!(row["cache_write_1h_tokens"], 0, "{row}");
}

#[tokio::test]
async fn a_clickhouse_with_015_and_016_but_not_017_defaults_only_the_share() {
    let _ = skip_without_stack!();
    let row = list_after(16).await.expect("a configured stack");

    assert_eq!(row["upstream_status"], 429, "{row}");
    assert_eq!(row["lifecycle_operation"], "retrieve", "{row}");
    assert_eq!(row["cache_write_1h_tokens"], 0, "{row}");
}
