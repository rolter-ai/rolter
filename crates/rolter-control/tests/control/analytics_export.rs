//! CSV export of the cost rollups and the request log (#1838), against a stub
//! ClickHouse that records what it was asked.
//!
//! What a caller may export is decided by the SQL ClickHouse evaluates, which a
//! stub cannot run: the real-engine proof that each role exports its own rows
//! and none else is `the_export_returns_only_the_rows_the_caller_can_list` in
//! `tests/analytics_scoping.rs`. What the stub can pin is the half that lives
//! in this crate: that the export binds exactly the scope the JSON read binds
//! for the same caller, selects no body, pages by keyset, and writes a file a
//! spreadsheet cannot turn into a formula.

use std::sync::Arc;

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::support::*;

/// One request the stub ClickHouse saw: its query-string pairs and its SQL.
#[derive(Clone, Debug)]
struct Seen {
    params: Vec<(String, String)>,
    sql: String,
}

impl Seen {
    fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The scope bindings of the caller: everything `AnalyticsAccess` binds.
    fn access(&self) -> Vec<(String, String)> {
        let mut access: Vec<_> = self
            .params
            .iter()
            .filter(|(k, _)| {
                k.starts_with("param_member_")
                    || k.starts_with("param_granted_")
                    || k.starts_with("param_unrestricted")
                    || k.starts_with("param_viewer_payload")
                    || k.starts_with("param_read_rank")
                    || k.starts_with("param_payload_rank")
            })
            .cloned()
            .collect();
        access.sort();
        access
    }
}

type Answer = Box<dyn Fn(usize, &Seen) -> (StatusCode, Value) + Send + Sync>;

struct Fake {
    seen: Mutex<Vec<Seen>>,
    answer: Answer,
}

async fn handle(
    State(fake): State<Arc<Fake>>,
    RawQuery(query): RawQuery,
    body: String,
) -> Response {
    let params: Vec<(String, String)> =
        reqwest::Url::parse(&format!("http://stub/?{}", query.unwrap_or_default()))
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
    let seen = Seen { params, sql: body };
    // the column probe the log reads make is not a page of data
    if seen.sql.contains("system.columns") {
        let names = [
            "ts",
            "upstream_status",
            "attempts",
            "lifecycle_operation",
            "cache_write_1h_tokens",
            "guardrail_events",
            "plugin_events",
            "seq",
        ];
        let rows: Vec<Value> = names.iter().map(|n| json!({"name": n})).collect();
        return (StatusCode::OK, axum::Json(json!({"data": rows}))).into_response();
    }
    let index = {
        let mut all = fake.seen.lock();
        all.push(seen.clone());
        all.len() - 1
    };
    let (status, body) = (fake.answer)(index, &seen);
    (status, axum::Json(body)).into_response()
}

async fn stub(answer: Answer) -> (String, Arc<Fake>) {
    let fake = Arc::new(Fake {
        seen: Mutex::new(Vec::new()),
        answer,
    });
    let app = Router::new().fallback(any(handle)).with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, fake)
}

struct App {
    base: String,
    pool: sqlx::PgPool,
    _db: rolter_store::postgres::test_schema::TestSchema,
}

const ADMIN: &str = "export-admin";

async fn start(clickhouse: &str) -> App {
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let router = rolter_control::test_app_with_clickhouse_and_admin_token(
        pool.clone(),
        clickhouse,
        Some(ADMIN.to_string()),
    )
    .await
    .unwrap();
    let addr = serve(router).await;
    App {
        base: format!("http://{addr}"),
        pool,
        _db: db,
    }
}

async fn project_viewer(app: &App, name: &str) -> (String, uuid::Uuid) {
    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
            .bind(name)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    let project: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'app') returning id")
            .bind(team)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    let user = seed_user(&app.pool, &format!("{name}@x.test"), false).await;
    seed_membership(&app.pool, user, None, None, Some(project), "viewer").await;
    (seed_session(&app.pool, user, name).await, project)
}

fn log_row(i: usize, model: &str) -> Value {
    json!({
        "ts": format!("2026-10-01T10:00:{:02}.{:03}Z", (i / 1000) % 60, 999 - (i % 1000)),
        "request_id": format!("req-{i:05}"), "trace_id": "t", "org_id": "o", "team_id": "tm",
        "project_id": "p", "virtual_key_id": "k", "business_unit_id": "b", "customer_id": "c",
        "model": model, "provider": "openai", "target": "x", "variant": "", "status": 200,
        "stream": 0, "cache_hit": 0, "cache_read_tokens": 0, "cache_write_tokens": 0,
        "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15, "cost_usd": 0.5,
        "unpriced": 0, "latency_ms": 120, "ttft_ms": 30, "sample_weight": 1,
        "seq": format!("{}", 5000 - i),
    })
}

fn all_rows(n: usize) -> Vec<Value> {
    (0..n).map(|i| log_row(i, "gpt-4o")).collect()
}

#[tokio::test]
async fn the_log_export_streams_every_page_under_the_scope_the_json_read_binds() {
    skip_without_db!();
    // 1,000 rows (a full page), then 2 more (the end), the second carrying a
    // spreadsheet formula in a model name
    let (url, fake) = stub(Box::new(|index, seen| {
        let rows = match seen.param("param_limit") {
            Some("1000") if index == 0 => all_rows(1000),
            _ => vec![log_row(2000, "=cmd|' /C calc'!A0"), log_row(2001, "ok")],
        };
        (StatusCode::OK, json!({"data": rows}))
    }))
    .await;
    let app = start(&url).await;
    let (token, project) = project_viewer(&app, "viewer-a").await;
    let client = reqwest::Client::new();

    // the JSON list first, so its scope bindings are the reference
    let list = client
        .get(format!("{}/api/v1/analytics/invocations?limit=5", app.base))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), 200);
    let reference = fake.seen.lock().last().cloned().unwrap().access();
    assert!(
        reference
            .iter()
            .any(|(_, v)| v.contains(&project.to_string())),
        "the viewer's project is bound: {reference:?}"
    );
    fake.seen.lock().clear();

    let response = client
        .get(format!(
            "{}/api/v1/analytics/invocations/export?since=2026-09-01&until=2026-10-01T00:00:00Z&status=success",
            app.base
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "text/csv; charset=utf-8"
    );
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"request-log_2026-09-01_to_2026-10-01_USD.csv\""
    );
    let csv = response.text().await.unwrap();

    // every page carried the very scope the list did, never wider
    let seen = fake.seen.lock().clone();
    assert_eq!(seen.len(), 2, "two pages");
    for page in &seen {
        assert_eq!(page.access(), reference);
        assert!(
            page.sql.contains("ts >= if("),
            "the visibility and window predicates"
        );
        // metadata only: no payload table, no body column, no error text
        assert!(
            !page.sql.contains("payload") && !page.sql.contains("error"),
            "{}",
            page.sql
        );
        assert!(page.sql.contains("status > 0 and status < 400"));
    }
    // the second page resumes after the last row of the first (keyset), so a row
    // written meanwhile can neither repeat nor be skipped
    assert_eq!(seen[0].param("param_cursor_id"), Some(""));
    assert_eq!(seen[1].param("param_cursor_id"), Some("req-00999"));
    // and on the start sequence too, so a tie inside a millisecond cannot repeat
    assert_eq!(seen[1].param("param_cursor_seq"), Some("4001"));
    assert_eq!(
        seen[1].param("param_cursor_ts"),
        Some("2026-10-01T10:00:00.000Z")
    );

    // one header, every row, once
    let lines: Vec<&str> = csv.split("\r\n").filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1 + 1000 + 2);
    assert!(lines[0].starts_with("ts,request_id,trace_id,"));
    assert!(!lines[0].contains("payload") && !lines[0].contains("error"));
    assert!(lines[1].contains("req-00000") && lines[1].contains(",USD,0,"));
    // the formula is defused, not written as the sheet would run it
    assert!(csv.contains("\"'=cmd|' /C calc'!A0\"") || csv.contains("'=cmd"));
    assert!(!csv.contains(",=cmd"));
}

#[tokio::test]
async fn an_unpriced_request_is_flagged_and_its_cost_left_blank() {
    skip_without_db!();
    let (url, _fake) = stub(Box::new(|_, _| {
        let mut row = log_row(1, "gpt-4o");
        row["unpriced"] = json!(1);
        row["cost_usd"] = json!(0.0);
        (StatusCode::OK, json!({"data": [row]}))
    }))
    .await;
    let app = start(&url).await;
    let csv = reqwest::Client::new()
        .get(format!("{}/api/v1/analytics/invocations/export", app.base))
        .bearer_auth(ADMIN)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let mut lines = csv.split("\r\n");
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    let row: Vec<&str> = lines.next().unwrap().split(',').collect();
    let at = |name: &str| header.iter().position(|c| *c == name).unwrap();
    assert_eq!(row[at("unpriced")], "1");
    assert_eq!(row[at("cost")], "");
}

#[tokio::test]
async fn the_rollup_exports_flag_a_partial_total_and_name_the_window_and_currency() {
    skip_without_db!();
    let unit: uuid::Uuid = uuid::Uuid::new_v4();
    let unit_text = unit.to_string();
    let (url, _fake) = stub(Box::new(move |_, seen| {
        let rows = if seen.sql.contains("group by id") {
            vec![
                json!({"id": unit_text, "requests": "10", "tokens": "100", "prompt_tokens": "60",
                       "completion_tokens": "40", "cost_usd": 1.5, "unpriced_requests": "3", "errors": "0"}),
                json!({"id": "", "requests": "2", "tokens": "5", "prompt_tokens": "3",
                       "completion_tokens": "2", "cost_usd": 0.25, "unpriced_requests": "0", "errors": "1"}),
            ]
        } else {
            vec![json!({"bucket": "2026-10-01", "requests": "12", "tokens": "105",
                        "cost_usd": 1.75, "unpriced_requests": "3"})]
        };
        (StatusCode::OK, json!({"data": rows}))
    }))
    .await;
    let app = start(&url).await;
    sqlx::query("insert into orgs (name, slug) values ('o', 'o')")
        .execute(&app.pool)
        .await
        .unwrap();
    let org: uuid::Uuid = sqlx::query_scalar("select id from orgs where slug = 'o'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    sqlx::query("insert into business_units (id, org_id, name, slug) values ($1, $2, '=Payments', 'payments')")
        .bind(unit)
        .bind(org)
        .execute(&app.pool)
        .await
        .unwrap();
    let client = reqwest::Client::new();

    let response = client
        .get(format!(
            "{}/api/v1/analytics/by-attribution/export?since=2026-09-01&until=2026-10-01&include_unattributed=true",
            app.base
        ))
        .bearer_auth(ADMIN)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"cost-attribution-business_unit_2026-09-01_to_2026-10-01_USD.csv\""
    );
    let csv = response.text().await.unwrap();
    let lines: Vec<&str> = csv.split("\r\n").filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].ends_with("cost,currency,unpriced_requests,cost_is_partial"));
    // the unit with unpriced requests is a floor, not a total, and says so; its
    // name (typed by someone else) cannot run as a formula
    assert!(
        lines[1].starts_with(&format!("business_unit,{unit},'=Payments,10,")),
        "{}",
        lines[1]
    );
    assert!(lines[1].ends_with(",1.5,USD,3,true"), "{}", lines[1]);
    assert!(
        lines[2].starts_with("business_unit,,(unattributed),"),
        "{}",
        lines[2]
    );
    assert!(lines[2].ends_with(",0.25,USD,0,false"), "{}", lines[2]);

    // a caller who cannot read the org's business units gets the id and no name
    let (viewer, _) = project_viewer(&app, "elsewhere").await;
    let csv = client
        .get(format!(
            "{}/api/v1/analytics/by-attribution/export?include_unattributed=true",
            app.base
        ))
        .bearer_auth(&viewer)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        csv.contains(&format!("business_unit,{unit},,10,")),
        "the name is withheld: {csv}"
    );
    assert!(!csv.contains("Payments"), "{csv}");

    let response = client
        .get(format!(
            "{}/api/v1/analytics/timeseries/export?bucket=day&since=2026-10-01",
            app.base
        ))
        .bearer_auth(ADMIN)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"spend-by-day_2026-10-01_to_now_USD.csv\""
    );
    let csv = response.text().await.unwrap();
    assert!(csv
        .starts_with("bucket,requests,tokens,cost,currency,unpriced_requests,cost_is_partial\r\n"));
    assert!(csv.contains("2026-10-01,12,105,1.75,USD,3,true"));

    // a bad dimension or bucket is refused before ClickHouse is asked
    for path in [
        "by-attribution/export?dimension=x",
        "timeseries/export?bucket=year",
    ] {
        let status = client
            .get(format!("{}/api/v1/analytics/{path}", app.base))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 400, "{path}");
    }
}

#[tokio::test]
async fn an_export_needs_a_session_and_a_failed_read_is_an_error_not_a_short_file() {
    skip_without_db!();
    let (url, _fake) = stub(Box::new(|_, _| {
        (StatusCode::INTERNAL_SERVER_ERROR, json!({}))
    }))
    .await;
    let app = start(&url).await;
    let client = reqwest::Client::new();
    for path in ["by-attribution", "timeseries", "invocations"] {
        let url = format!("{}/api/v1/analytics/{path}/export", app.base);
        assert_eq!(
            client.get(&url).send().await.unwrap().status(),
            401,
            "{path}"
        );
        // ClickHouse failing on the first page is a JSON error before any file
        let failed = client.get(&url).bearer_auth(ADMIN).send().await.unwrap();
        assert_eq!(failed.status(), 502, "{path}");
        assert_eq!(failed.headers()["content-type"], "application/json");
    }

    // and one failing part way ends the body with an error, so the download
    // does not pass for a complete file
    let (url, _fake) = stub(Box::new(|index, _| {
        if index == 0 {
            (StatusCode::OK, json!({"data": all_rows(1000)}))
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, json!({}))
        }
    }))
    .await;
    let app = start(&url).await;
    let response = client
        .get(format!("{}/api/v1/analytics/invocations/export", app.base))
        .bearer_auth(ADMIN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        response.bytes().await.is_err(),
        "the body must end in an error"
    );
}
