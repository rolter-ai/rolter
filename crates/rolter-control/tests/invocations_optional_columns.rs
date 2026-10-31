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
/// after, holding `rows` inserted the way a gateway inserts them, read through
/// the control plane at `path`. Returns the status and body of the answer.
async fn read_after(last: u32, rows: &[Value], path: &str) -> Option<(StatusCode, Value)> {
    read_without(last, &[], rows, path).await
}

/// As [`read_after`], with each of `dropped` then removed from `request_logs`:
/// a table that has every later migration but lacks one column, which no
/// prefix of the migrations produces.
async fn read_without(
    last: u32,
    dropped: &[&str],
    rows: &[Value],
    path: &str,
) -> Option<(StatusCode, Value)> {
    let base = clickhouse_url().filter(|_| test_database::is_configured())?;
    let http = reqwest::Client::new();
    let scratch = ScratchDatabase::create(&http, &base).await;
    clickhouse_ddl::apply_schema_through(&http, &base, Some(&scratch.name), last).await;
    for column in dropped {
        let response = http
            .post(format!("{base}/"))
            .query(&[("database", scratch.name.as_str())])
            .body(format!("alter table request_logs drop column {column}"))
            .send()
            .await
            .expect("reach clickhouse");
        assert!(
            response.status().is_success(),
            "drop {column}: {}",
            response.text().await.unwrap_or_default()
        );
    }

    let body: String = rows.iter().map(|row| format!("{row}\n")).collect();
    let insert = http
        .post(format!("{base}/"))
        .query(&[
            ("database", scratch.name.as_str()),
            ("query", "INSERT INTO request_logs FORMAT JSONEachRow"),
            ("date_time_input_format", "best_effort"),
            ("input_format_skip_unknown_fields", "1"),
        ])
        .body(body)
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
        .get(format!("http://{addr}{path}"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    drop(schema);
    drop(scratch);
    Some((status, body))
}

/// A deployment whose `request_logs` was migrated through `last` and nothing
/// after, holding one full row, read through the control plane. Returns the
/// row as the list answered it.
async fn list_after(last: u32) -> Option<Value> {
    let request_id = format!("optional-columns-{}", Uuid::new_v4().simple());
    let (status, body) = read_after(
        last,
        &[full_row(&request_id)],
        &format!("/api/v1/analytics/invocations?request_id={request_id}"),
    )
    .await?;
    assert_eq!(
        status, 200,
        "the list failed after migration {last}: {body}"
    );
    let rows = body["data"].as_array().expect("a data array");
    assert_eq!(rows.len(), 1, "{body}");
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

/// A figure ClickHouse sends as a number or, for a wide integer, as a string.
fn number(value: &Value) -> f64 {
    match value {
        Value::String(text) => text.parse().unwrap_or_else(|_| panic!("{text} parses")),
        other => other
            .as_f64()
            .unwrap_or_else(|| panic!("{other} is a number")),
    }
}

/// Two requests answered by one variant, one of them after a failover, as a
/// current gateway writes them (the second carries `fallback_reason`).
fn variant_rows(route: &str) -> Vec<Value> {
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let row = |reason: &str| {
        let mut row = json!({
            "ts": ts, "request_id": format!("variant-{}", Uuid::new_v4().simple()),
            "model": route, "variant": "canary", "status": 200, "latency_ms": 100,
        });
        if !reason.is_empty() {
            row["fallback_reason"] = json!(reason);
        }
        row
    };
    vec![row(""), row("timeout")]
}

/// #3409: a ClickHouse from before `023` cannot tell a first attempt from a
/// failover. The select must not name the column (it would fail), the answer
/// is still a `200` with the variant's requests, and `first_attempt_requests`
/// is `null` rather than a claim that every request was a first attempt.
#[tokio::test]
async fn by_variant_on_a_clickhouse_without_023_reports_no_first_attempt_count() {
    let _ = skip_without_stack!();
    let route = format!("variant-route-{}", Uuid::new_v4().simple());
    // 022 is the last file before the fallback reason
    let (status, body) = read_after(
        22,
        &variant_rows(&route),
        &format!("/api/v1/analytics/by-variant?model={route}"),
    )
    .await
    .expect("a configured stack");
    assert_eq!(status, 200, "{body}");
    let rows = body["data"].as_array().expect("a data array");
    assert_eq!(rows.len(), 1, "{body}");
    assert_eq!(rows[0]["variant"], "canary", "{body}");
    assert_eq!(number(&rows[0]["requests"]), 2.0, "{body}");
    assert!(rows[0]["first_attempt_requests"].is_null(), "{body}");
}

/// With `023` applied the same two requests split: one first attempt, one failover.
#[tokio::test]
async fn by_variant_on_a_clickhouse_with_023_counts_the_first_attempts() {
    let _ = skip_without_stack!();
    let route = format!("variant-route-{}", Uuid::new_v4().simple());
    let (status, body) = read_after(
        23,
        &variant_rows(&route),
        &format!("/api/v1/analytics/by-variant?model={route}"),
    )
    .await
    .expect("a configured stack");
    assert_eq!(status, 200, "{body}");
    let rows = body["data"].as_array().expect("a data array");
    assert_eq!(rows.len(), 1, "{body}");
    assert_eq!(number(&rows[0]["requests"]), 2.0, "{body}");
    assert_eq!(number(&rows[0]["first_attempt_requests"]), 1.0, "{body}");
}

/// Requests answered by two variants of one route, one of them failed over to,
/// as a current gateway writes them.
fn two_variant_rows(route: &str) -> Vec<Value> {
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let row = |variant: &str, reason: &str| {
        json!({
            "ts": ts, "request_id": format!("variant-{}", Uuid::new_v4().simple()),
            "model": route, "variant": variant, "status": 200, "latency_ms": 100,
            "fallback_reason": reason,
        })
    };
    vec![
        row("canary", ""),
        row("canary", "timeout"),
        row("stable", ""),
        row("stable", ""),
        row("stable", ""),
    ]
}

/// The variants a by-variant answer lists, with their request counts.
fn requests_by_variant(body: &Value) -> Vec<(String, f64)> {
    let mut rows: Vec<(String, f64)> = body["data"]
        .as_array()
        .expect("a data array")
        .iter()
        .map(|row| {
            (
                row["variant"].as_str().unwrap_or_default().to_string(),
                number(&row["requests"]),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// #3497: a table with every migration but without `fallback_reason` (dropped
/// by hand, or never added while later files were) still answers by-variant.
/// No prefix of the migrations is this table, so the 022 test above does not
/// cover it: the probe has to name the one column missing among the later ones.
#[tokio::test]
async fn by_variant_on_a_current_clickhouse_without_fallback_reason_reports_no_first_attempt_count()
{
    let _ = skip_without_stack!();
    let route = format!("variant-route-{}", Uuid::new_v4().simple());
    let path = format!("/api/v1/analytics/by-variant?model={route}");
    let rows = two_variant_rows(&route);

    let (status, full) = read_without(u32::MAX, &[], &rows, &path)
        .await
        .expect("a configured stack");
    assert_eq!(status, 200, "{full}");
    let (status, lacking) = read_without(u32::MAX, &["fallback_reason"], &rows, &path)
        .await
        .expect("a configured stack");
    assert_eq!(status, 200, "{lacking}");

    // every row says the split is unknown rather than reading as first attempts
    let data = lacking["data"].as_array().expect("a data array");
    assert_eq!(data.len(), 2, "{lacking}");
    for row in data {
        assert!(row["first_attempt_requests"].is_null(), "{lacking}");
    }
    // and the requests are the ones the full table counts
    assert_eq!(requests_by_variant(&lacking), requests_by_variant(&full));
    assert_eq!(
        requests_by_variant(&lacking),
        vec![("canary".to_string(), 2.0), ("stable".to_string(), 3.0)]
    );
}

/// A by-variant row's `(requests, first_attempt_requests, assigned_requests)`
/// per variant, sorted by variant.
fn split_by_variant(body: &Value) -> Vec<(String, f64, f64, Option<f64>)> {
    let mut rows: Vec<_> = body["data"]
        .as_array()
        .expect("a data array")
        .iter()
        .map(|row| {
            (
                row["variant"].as_str().unwrap_or_default().to_string(),
                number(&row["requests"]),
                number(&row["first_attempt_requests"]),
                (!row["assigned_requests"].is_null()).then(|| number(&row["assigned_requests"])),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// #3505: requests on one route as a gateway with `first_variant` writes them,
/// beside rows written before the column. `flaky` was dialled first by two
/// requests: one it answered and one that failed over to `stable`. `stable`
/// answered three requests of its own on the first attempt. Of the two old
/// rows (no `first_variant`), the one with no fallback reason counts for the
/// variant that answered it and the failed-over one counts for no variant.
fn assigned_rows(route: &str) -> Vec<Value> {
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let row = |variant: &str, first: &str, reason: &str| {
        let mut row = json!({
            "ts": ts, "request_id": format!("assigned-{}", Uuid::new_v4().simple()),
            "model": route, "variant": variant, "status": 200, "latency_ms": 100,
        });
        if !first.is_empty() {
            row["first_variant"] = json!(first);
        }
        if !reason.is_empty() {
            row["fallback_reason"] = json!(reason);
        }
        row
    };
    vec![
        row("flaky", "flaky", ""),
        // dialled to `flaky`, answered by `stable`
        row("stable", "flaky", "server_error"),
        row("stable", "stable", ""),
        row("stable", "stable", ""),
        row("stable", "stable", ""),
        // written before the column: one counted, one not
        row("stable", "", ""),
        row("stable", "", "timeout"),
    ]
}

#[tokio::test]
async fn by_variant_counts_a_failover_for_the_variant_first_dialled() {
    let _ = skip_without_stack!();
    let route = format!("variant-route-{}", Uuid::new_v4().simple());
    let (status, body) = read_after(
        u32::MAX,
        &assigned_rows(&route),
        &format!("/api/v1/analytics/by-variant?model={route}"),
    )
    .await
    .expect("a configured stack");
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        split_by_variant(&body),
        vec![
            // answered 1, first attempts 1, assigned 2 (its own and the failover)
            ("flaky".to_string(), 1.0, 1.0, Some(2.0)),
            // answered 6, first attempts 4 (three new, one old), assigned 4:
            // the failover is `flaky`'s and the old failed-over row is no one's
            ("stable".to_string(), 6.0, 4.0, Some(4.0)),
        ],
        "{body}"
    );
}

/// A variant that only ever failed over answered nothing, and still has a row:
/// its assigned share is what the dashboard compares with its weight.
#[tokio::test]
async fn by_variant_lists_a_variant_that_answered_nothing_by_its_assignments() {
    let _ = skip_without_stack!();
    let route = format!("variant-route-{}", Uuid::new_v4().simple());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let rows = vec![json!({
        "ts": ts, "request_id": format!("down-{}", Uuid::new_v4().simple()),
        "model": route, "variant": "stable", "first_variant": "down",
        "fallback_reason": "transport", "status": 200, "latency_ms": 100,
    })];
    let (status, body) = read_after(
        u32::MAX,
        &rows,
        &format!("/api/v1/analytics/by-variant?model={route}"),
    )
    .await
    .expect("a configured stack");
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        split_by_variant(&body),
        vec![
            ("down".to_string(), 0.0, 0.0, Some(1.0)),
            ("stable".to_string(), 1.0, 0.0, Some(0.0)),
        ],
        "{body}"
    );
}

/// On a ClickHouse without `048` by-variant still answers, and
/// `assigned_requests` is `null` rather than a guess; the other counts are the
/// ones the full table gives.
#[tokio::test]
async fn by_variant_on_a_clickhouse_without_048_reports_no_assigned_count() {
    let _ = skip_without_stack!();
    let route = format!("variant-route-{}", Uuid::new_v4().simple());
    let path = format!("/api/v1/analytics/by-variant?model={route}");
    let rows = assigned_rows(&route);
    // 047 is the last file before the column
    let (status, before) = read_after(47, &rows, &path)
        .await
        .expect("a configured stack");
    assert_eq!(status, 200, "{before}");
    let (status, dropped) = read_without(u32::MAX, &["first_variant"], &rows, &path)
        .await
        .expect("a configured stack");
    assert_eq!(status, 200, "{dropped}");
    for body in [&before, &dropped] {
        assert_eq!(
            split_by_variant(body),
            vec![
                // the rows land under the variant that answered, as before
                ("flaky".to_string(), 1.0, 1.0, None),
                ("stable".to_string(), 6.0, 4.0, None),
            ],
            "{body}"
        );
    }
}

/// The invocation list returns `first_variant` beside `variant`, and an empty
/// string on a table without the column.
#[tokio::test]
async fn the_invocation_list_returns_the_first_variant() {
    let _ = skip_without_stack!();
    let request_id = format!("first-variant-{}", Uuid::new_v4().simple());
    let mut row = full_row(&request_id);
    row["variant"] = json!("stable");
    row["first_variant"] = json!("flaky");
    let path = format!("/api/v1/analytics/invocations?request_id={request_id}");
    for (last, expected) in [(u32::MAX, "flaky"), (47, "")] {
        let (status, body) = read_after(last, std::slice::from_ref(&row), &path)
            .await
            .expect("a configured stack");
        assert_eq!(status, 200, "{body}");
        let listed = &body["data"][0];
        assert_eq!(listed["variant"], "stable", "{body}");
        assert_eq!(listed["first_variant"], expected, "{body}");
    }
}

/// A by-variant row's `(requests, assigned_requests, sampled_requests)` per
/// variant, sorted by variant.
fn sampled_by_variant(body: &Value) -> Vec<(String, f64, f64, Option<f64>)> {
    let mut rows: Vec<_> = body["data"]
        .as_array()
        .expect("a data array")
        .iter()
        .map(|row| {
            (
                row["variant"].as_str().unwrap_or_default().to_string(),
                number(&row["requests"]),
                number(&row["assigned_requests"]),
                (!row["sampled_requests"].is_null()).then(|| number(&row["sampled_requests"])),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// #3547: requests on one route as a gateway with `sampled_variant` writes
/// them. The split sampled `parked` three times, but its breaker was open for
/// two of them, which the gateway dialled to `spare` first; `spare` was
/// sampled once and answered it. A classic row counts under the empty string,
/// and a variant row written before the column (no `sampled_variant`) is
/// left out of the sampled count.
fn sampled_rows(route: &str) -> Vec<Value> {
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let row = |sampled: &str, first: &str, variant: &str| {
        let mut row = json!({
            "ts": ts, "request_id": format!("sampled-{}", Uuid::new_v4().simple()),
            "model": route, "variant": variant, "status": 200, "latency_ms": 100,
        });
        if !first.is_empty() {
            row["first_variant"] = json!(first);
        }
        if !sampled.is_empty() {
            row["sampled_variant"] = json!(sampled);
        }
        row
    };
    vec![
        row("parked", "parked", "parked"),
        // sampled `parked`, skipped while its breaker was open
        row("parked", "spare", "spare"),
        row("parked", "spare", "spare"),
        row("spare", "spare", "spare"),
        // the classic pool
        row("", "", ""),
        // written before the column
        row("", "spare", "spare"),
    ]
}

#[tokio::test]
async fn by_variant_counts_the_sampled_primary_beside_the_variant_first_dialled() {
    let _ = skip_without_stack!();
    let route = format!("variant-route-{}", Uuid::new_v4().simple());
    let (status, body) = read_after(
        u32::MAX,
        &sampled_rows(&route),
        &format!("/api/v1/analytics/by-variant?model={route}"),
    )
    .await
    .expect("a configured stack");
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        sampled_by_variant(&body),
        vec![
            ("".to_string(), 1.0, 1.0, Some(1.0)),
            // answered 1, dialled first 1, sampled 3
            ("parked".to_string(), 1.0, 1.0, Some(3.0)),
            // answered 4, dialled first 4, sampled 1: the old row has no sample
            ("spare".to_string(), 4.0, 4.0, Some(1.0)),
        ],
        "{body}"
    );
}

/// On a ClickHouse without `050` by-variant still answers, `sampled_requests`
/// is `null` rather than a guess, and the other counts are the full table's.
#[tokio::test]
async fn by_variant_on_a_clickhouse_without_050_reports_no_sampled_count() {
    let _ = skip_without_stack!();
    let route = format!("variant-route-{}", Uuid::new_v4().simple());
    let path = format!("/api/v1/analytics/by-variant?model={route}");
    let rows = sampled_rows(&route);
    // 048 is the last file this count does not need
    let (status, before) = read_after(48, &rows, &path)
        .await
        .expect("a configured stack");
    assert_eq!(status, 200, "{before}");
    let (status, dropped) = read_without(u32::MAX, &["sampled_variant"], &rows, &path)
        .await
        .expect("a configured stack");
    assert_eq!(status, 200, "{dropped}");
    for body in [&before, &dropped] {
        assert_eq!(
            sampled_by_variant(body),
            vec![
                ("".to_string(), 1.0, 1.0, None),
                ("parked".to_string(), 1.0, 1.0, None),
                ("spare".to_string(), 4.0, 4.0, None),
            ],
            "{body}"
        );
    }
    // and a table with `050` but not `048` counts the samples, not the assignments
    let (status, body) = read_without(u32::MAX, &["first_variant"], &rows, &path)
        .await
        .expect("a configured stack");
    assert_eq!(status, 200, "{body}");
    let mut unassigned: Vec<_> = body["data"]
        .as_array()
        .expect("a data array")
        .iter()
        .map(|row| {
            (
                row["variant"].as_str().unwrap_or_default().to_string(),
                row["assigned_requests"].is_null(),
                number(&row["sampled_requests"]) as u32,
            )
        })
        .collect();
    unassigned.sort();
    assert_eq!(
        unassigned,
        vec![
            ("".to_string(), true, 1),
            ("parked".to_string(), true, 3),
            ("spare".to_string(), true, 1),
        ],
        "{body}"
    );
}

/// The invocation list returns `sampled_variant` beside `first_variant`, and
/// an empty string on a table without the column.
#[tokio::test]
async fn the_invocation_list_returns_the_sampled_variant() {
    let _ = skip_without_stack!();
    let request_id = format!("sampled-variant-{}", Uuid::new_v4().simple());
    let mut row = full_row(&request_id);
    row["variant"] = json!("spare");
    row["first_variant"] = json!("spare");
    row["sampled_variant"] = json!("parked");
    let path = format!("/api/v1/analytics/invocations?request_id={request_id}");
    for (last, expected) in [(u32::MAX, "parked"), (48, "")] {
        let (status, body) = read_after(last, std::slice::from_ref(&row), &path)
            .await
            .expect("a configured stack");
        assert_eq!(status, 200, "{body}");
        let listed = &body["data"][0];
        assert_eq!(listed["first_variant"], "spare", "{body}");
        assert_eq!(listed["sampled_variant"], expected, "{body}");
    }
}
