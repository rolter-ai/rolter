//! The readers of `request_logs` that report counts, spend and rates, against a
//! sampled log and a real ClickHouse (#2278).
//!
//! `/api/v1/analytics/*` scales every count and sum by the row's
//! `sample_weight` (#2239). `GET /api/v1/me/usage` and the `request_volume`,
//! `spend_velocity` and `error_rate` alert signals read the same table, and a
//! unit test on their SQL text cannot show that the numbers come out right. So
//! each test seeds rows of different weights into a database of its own and
//! reads the figure back through the control plane.
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
const ADMIN_TOKEN: &str = "sampled-readers-admin";

fn clickhouse_url() -> Option<String> {
    std::env::var(CLICKHOUSE_URL_ENV)
        .ok()
        .map(|url| url.trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
}

/// A database of its own on the shared server, dropped when the test ends,
/// panic included, so a failed run leaks nothing.
struct ScratchDatabase {
    base: String,
    name: String,
}

impl ScratchDatabase {
    async fn create(client: &reqwest::Client, base: &str) -> Self {
        let name = format!("rolter_test_sampled_{}", Uuid::new_v4().simple());
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

/// A control plane over a scratch `request_logs` and a fresh schema.
struct Stack {
    http: reqwest::Client,
    base: String,
    scratch: ScratchDatabase,
    schema: TestSchema,
    addr: SocketAddr,
}

async fn stack() -> Option<Stack> {
    let base = clickhouse_url().filter(|_| test_database::is_configured())?;
    let http = reqwest::Client::new();
    let scratch = ScratchDatabase::create(&http, &base).await;
    clickhouse_ddl::apply_schema(&http, &base).await;
    clickhouse_ddl::apply_schema_through(&http, &base, Some(&scratch.name), u32::MAX).await;
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
    Some(Stack {
        http,
        base,
        scratch,
        schema,
        addr,
    })
}

impl Stack {
    async fn insert(&self, rows: &[Value]) {
        let body: String = rows.iter().map(|row| format!("{row}\n")).collect();
        let response = self
            .http
            .post(format!("{}/", self.base))
            .query(&[
                ("database", self.scratch.name.as_str()),
                ("query", "INSERT INTO request_logs FORMAT JSONEachRow"),
                ("date_time_input_format", "best_effort"),
            ])
            .body(body)
            .send()
            .await
            .expect("reach clickhouse");
        assert!(
            response.status().is_success(),
            "insert: {}",
            response.text().await.unwrap_or_default()
        );
    }
}

fn now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

/// One kept row standing for `weight` requests, `cost` each.
fn row(key: &str, status: u16, weight: f64) -> Value {
    json!({
        "ts": now(), "request_id": format!("sampled-{}", Uuid::new_v4().simple()),
        "virtual_key_id": key, "model": "fake-llm", "status": status, "latency_ms": 100,
        "total_tokens": 10, "prompt_tokens": 4, "completion_tokens": 6,
        "cost_usd": 0.5, "sample_weight": weight,
    })
}

#[tokio::test]
async fn me_usage_scales_requests_tokens_cost_and_errors_by_the_row_weight() {
    let Some(stack) = stack().await else {
        eprintln!("skipping: the postgres and clickhouse test servers are not both set");
        return;
    };
    let pool = stack.schema.pool();
    let org: Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
            .bind(format!("sampled-{}", Uuid::new_v4().simple()))
            .fetch_one(pool)
            .await
            .unwrap();
    let team: Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(pool)
            .await
            .unwrap();
    let project: Uuid = sqlx::query_scalar(
        "insert into projects (team_id, name) values ($1, 'alpha') returning id",
    )
    .bind(team)
    .fetch_one(pool)
    .await
    .unwrap();
    let user: Uuid = sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, null, false) \
         returning id",
    )
    .bind(format!("sampled-{}@sampled.test", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .unwrap();
    let token = format!("rolter_sess_sampled_{}", Uuid::new_v4().simple());
    sqlx::query(
        "insert into sessions (user_id, token_hash, expires_at) \
         values ($1, $2, now() + interval '1 hour')",
    )
    .bind(user)
    .bind(rolter_auth::hash_key("", &token))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("insert into memberships (user_id, project_id, role) values ($1, $2, 'member')")
        .bind(user)
        .bind(project)
        .execute(pool)
        .await
        .unwrap();

    let minted: Value = stack
        .http
        .post(format!(
            "http://{}/api/v1/me/projects/{project}/virtual-keys",
            stack.addr
        ))
        .bearer_auth(&token)
        .json(&json!({"name": "laptop", "models": ["fake-llm"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = minted["id"].as_str().expect("a key id").to_string();

    // rows of different weights: a rate change mid-window must still add up
    stack
        .insert(&[row(&key, 200, 2.0), row(&key, 500, 4.0)])
        .await;

    let usage: Value = stack
        .http
        .get(format!("http://{}/api/v1/me/usage", stack.addr))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = usage["data"].as_array().expect("a data array");
    assert_eq!(rows.len(), 1, "{usage}");
    assert_eq!(rows[0]["requests"].as_f64(), Some(6.0), "{usage}");
    assert_eq!(rows[0]["tokens"].as_f64(), Some(60.0), "{usage}");
    assert_eq!(rows[0]["cost_usd"].as_f64(), Some(3.0), "{usage}");
    assert_eq!(rows[0]["errors"].as_f64(), Some(4.0), "{usage}");
}

/// The value an alert rule on `signal` reads over an hour, through the same
/// path **Evaluate now** takes.
async fn read_alert(stack: &Stack, signal: &str) -> Option<f64> {
    let created: Value = stack
        .http
        .post(format!("http://{}/api/v1/alert-rules", stack.addr))
        .bearer_auth(ADMIN_TOKEN)
        .json(&json!({
            "name": format!("{signal}-{}", Uuid::new_v4().simple()),
            "signal": signal, "threshold": 1.0, "window_secs": 3600, "enabled": false,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"));
    let evaluated: Value = stack
        .http
        .post(format!(
            "http://{}/api/v1/alert-rules/{id}/evaluate",
            stack.addr
        ))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(evaluated["rule"]["last_error"].is_null(), "{evaluated}");
    evaluated["rule"]["last_value"].as_f64()
}

fn near(got: Option<f64>, want: f64) {
    let got = got.expect("a reading");
    assert!((got - want).abs() < 1e-9, "read {got}, wanted {want}");
}

#[tokio::test]
async fn alert_signals_scale_by_the_row_weight_and_the_error_rate_stays_a_ratio_of_weighted_counts()
{
    let Some(stack) = stack().await else {
        eprintln!("skipping: the postgres and clickhouse test servers are not both set");
        return;
    };
    // 1 request kept at weight 1 (ok) and 1 at weight 3 (an error): four real
    // requests, 2.0 of spend, and three quarters of them failing. the unweighted
    // reading would be 2 requests, 1.0 and one half
    stack.insert(&[row("", 200, 1.0), row("", 500, 3.0)]).await;

    near(read_alert(&stack, "request_volume").await, 4.0);
    // spend per hour over a one hour window is the window's spend
    near(read_alert(&stack, "spend_velocity").await, 2.0);
    near(read_alert(&stack, "error_rate").await, 0.75);
}

/// #3294: the error rate counts server failures and a request that never got
/// a status (0), and leaves out a 4xx, which is the caller's own mistake.
#[tokio::test]
async fn the_error_rate_alert_counts_status_zero_and_leaves_out_a_4xx() {
    let Some(stack) = stack().await else {
        eprintln!("skipping: the postgres and clickhouse test servers are not both set");
        return;
    };
    stack
        .insert(&[
            row("", 200, 1.0),
            row("", 0, 1.0),
            row("", 429, 1.0),
            row("", 502, 1.0),
        ])
        .await;
    near(read_alert(&stack, "error_rate").await, 0.5);
}

/// The p95 latency alert weights each kept row by the requests it stands for
/// (#3355): one slow request among fifty-odd fast ones is not the 95th percentile,
/// though it is one of three stored rows.
#[tokio::test]
async fn the_p95_latency_alert_weights_each_row_by_the_requests_it_stands_for() {
    let Some(stack) = stack().await else {
        eprintln!("skipping: the postgres and clickhouse test servers are not both set");
        return;
    };
    let mut rows = vec![row("", 200, 50.0), row("", 200, 1.0), row("", 200, 1.0)];
    for (stored, latency) in rows.iter_mut().zip([100, 200, 1000]) {
        stored["latency_ms"] = json!(latency);
    }
    stack.insert(&rows).await;

    // an unweighted reading of the three rows is the slow one, near 1000
    near(read_alert(&stack, "p95_latency_ms").await, 100.0);
}

/// One kept row of `weight` requests whose plugins did `events` (#2243).
fn plugin_row(org: &str, weight: f64, ts: &str, events: &[&str]) -> Value {
    let mut row = row("key-plugins", 200, weight);
    row["ts"] = json!(ts);
    row["org_id"] = json!(org);
    row["plugin_events"] = json!(events);
    row
}

#[tokio::test]
async fn the_plugin_rollup_counts_each_outcome_by_weight_and_names_the_newest_error() {
    let Some(stack) = stack().await else {
        eprintln!("skipping: the postgres and clickhouse test servers are not both set");
        return;
    };
    let newest = now();
    let older = (chrono::Utc::now() - chrono::Duration::minutes(10))
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    stack
        .insert(&[
            plugin_row(
                "org-a",
                2.0,
                &older,
                &["error:audit:connect", "allowed:redact"],
            ),
            plugin_row("org-a", 4.0, &newest, &["error:audit:status_503"]),
            plugin_row(
                "org-a",
                1.0,
                &newest,
                &["blocked:redact", "transformed:audit"],
            ),
            // the same slug in another organization is a plugin of its own
            plugin_row("org-b", 3.0, &newest, &["allowed:audit"]),
            // a row no plugin saw adds nothing
            row("key-plugins", 200, 5.0),
        ])
        .await;

    let body: Value = stack
        .http
        .get(format!("http://{}/api/v1/analytics/plugins", stack.addr))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["recorded"], true, "{body}");
    let find = |org: &str, plugin: &str| -> Value {
        body["data"]
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|r| r["org_id"] == org && r["plugin"] == plugin)
            })
            .cloned()
            .unwrap_or_else(|| panic!("no {org}/{plugin} in {body}"))
    };

    let audit = find("org-a", "audit");
    // 2 + 4 + 1 calls by weight, 6 of them failures, 1 transformed
    assert_eq!(audit["calls"].as_f64(), Some(7.0), "{body}");
    assert_eq!(audit["errors"].as_f64(), Some(6.0), "{body}");
    assert_eq!(audit["transformed"].as_f64(), Some(1.0), "{body}");
    assert_eq!(audit["blocked"].as_f64(), Some(0.0), "{body}");
    // the newest failure wins, not the one inserted first
    assert_eq!(audit["last_error_kind"], "status_503", "{body}");
    assert!(audit["last_error_at"].is_string(), "{body}");

    let redact = find("org-a", "redact");
    assert_eq!(redact["calls"].as_f64(), Some(3.0), "{body}");
    assert_eq!(redact["blocked"].as_f64(), Some(1.0), "{body}");
    assert!(redact["last_error_at"].is_null(), "{body}");
    assert_eq!(redact["last_error_kind"], "", "{body}");

    let other = find("org-b", "audit");
    assert_eq!(other["calls"].as_f64(), Some(3.0), "{body}");
    assert_eq!(other["errors"].as_f64(), Some(0.0), "{body}");
}

/// `row`, with `extra` merged over it.
fn row_with(key: &str, status: u16, weight: f64, extra: Value) -> Value {
    let mut row = row(key, status, weight);
    if let (Some(row), Some(extra)) = (row.as_object_mut(), extra.as_object()) {
        for (name, value) in extra {
            row.insert(name.clone(), value.clone());
        }
    }
    row
}

async fn get_json(stack: &Stack, path: &str) -> Value {
    let response = stack
        .http
        .get(format!("http://{}{path}", stack.addr))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    assert!(status.is_success(), "{path}: {status} {body}");
    body
}

/// #3323: the rollup's single scan (`sumMap` over the entries, then an `array
/// join`) yields each parameter's weighted requests, the route's weighted
/// total to that provider and their share, against the real column type.
#[tokio::test]
async fn the_param_adjustment_rollup_weighs_each_entry_against_the_route() {
    let Some(stack) = stack().await else {
        eprintln!("skipping: the postgres and clickhouse test servers are not both set");
        return;
    };
    let adjusted = json!({"provider": "anthropic", "param_adjustments": ["dropped:top_k"]});
    stack
        .insert(&[
            row_with("k", 200, 2.0, adjusted.clone()),
            row_with("k", 200, 3.0, json!({"provider": "anthropic"})),
            // another provider of the same route is its own total
            row_with("k", 200, 5.0, json!({"provider": "openai"})),
        ])
        .await;

    let body = get_json(&stack, "/api/v1/analytics/param-adjustments").await;
    assert_eq!(body["recorded"], true, "{body}");
    let rows = body["data"].as_array().expect("a data array");
    assert_eq!(rows.len(), 1, "{body}");
    let row = &rows[0];
    assert_eq!(row["route"], "fake-llm", "{body}");
    assert_eq!(row["provider"], "anthropic", "{body}");
    assert_eq!(row["disposition"], "dropped", "{body}");
    assert_eq!(row["param"], "top_k", "{body}");
    let number = |name: &str| {
        row[name]
            .as_f64()
            .or_else(|| row[name].as_str().and_then(|s| s.parse().ok()))
    };
    assert_eq!(number("requests"), Some(2.0), "{body}");
    assert_eq!(number("route_requests"), Some(5.0), "{body}");
    assert_eq!(number("share"), Some(0.4), "{body}");
}

/// #3307: `outcome=` narrows the list to rows recorded with it, and `unknown`
/// matches a row written without one, which the column's default fills in.
#[tokio::test]
async fn the_invocation_list_filters_on_the_recorded_outcome() {
    let Some(stack) = stack().await else {
        eprintln!("skipping: the postgres and clickhouse test servers are not both set");
        return;
    };
    stack
        .insert(&[
            row_with(
                "k",
                402,
                1.0,
                json!({"request_id": "refused-one", "outcome": "refused_budget"}),
            ),
            row_with(
                "k",
                200,
                1.0,
                json!({"request_id": "served-one", "outcome": "served"}),
            ),
            row_with("k", 200, 1.0, json!({"request_id": "older-one"})),
        ])
        .await;

    for (outcome, want) in [
        ("refused_budget", "refused-one"),
        ("served", "served-one"),
        ("unknown", "older-one"),
    ] {
        let body = get_json(
            &stack,
            &format!("/api/v1/analytics/invocations?outcome={outcome}"),
        )
        .await;
        let ids: Vec<_> = body["data"]
            .as_array()
            .expect("a data array")
            .iter()
            .map(|row| row["request_id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(ids, [want], "{outcome}: {body}");
        assert_eq!(body["data"][0]["outcome"], outcome, "{body}");
    }
}
