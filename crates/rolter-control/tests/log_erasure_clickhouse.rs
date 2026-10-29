//! Log erasure against a real ClickHouse (#3280).
//!
//! `tests/control/log_erasure.rs` drives the erasure flow against a stand-in
//! that records the statements it is sent and answers `system.mutations` as a
//! test directs. That proves the statements and the completion logic, not that
//! ClickHouse accepts the `alter table … delete where (…) and '<id>' != ''`
//! form, keeps the marker in `system.mutations.command`, or removes the rows.
//! That needs the real engine, so this seeds two organizations' logs and
//! payloads (including payload rows that predate
//! `clickhouse/020_request_payloads_key.sql`), erases one key and then one
//! request, polls until the erasure is `completed`, and reads the tables back
//! to see that only the intended rows are gone.
//!
//! Gated on the `postgres` feature and on both `ROLTER_TEST_DATABASE_URL` and
//! `ROLTER_TEST_CLICKHOUSE_URL`; unset either and the test self-skips. Every id
//! is random, so it can run against a ClickHouse other suites share.
#![cfg(feature = "postgres")]

#[path = "common/clickhouse_ddl.rs"]
mod clickhouse_ddl;

use std::time::Duration;

use rolter_store::postgres::test_database;
use rolter_store::postgres::test_schema::TestSchema;
use serde_json::{json, Value};
use uuid::Uuid;

const CLICKHOUSE_URL_ENV: &str = "ROLTER_TEST_CLICKHOUSE_URL";
const ADMIN_TOKEN: &str = "log-erasure-clickhouse-admin";

fn clickhouse_url() -> Option<String> {
    std::env::var(CLICKHOUSE_URL_ENV)
        .ok()
        .map(|url| url.trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
}

async fn insert_rows(client: &reqwest::Client, base: &str, table: &str, rows: &[Value]) {
    let body: String = rows.iter().map(|row| format!("{row}\n")).collect();
    let response = client
        .post(format!("{base}/"))
        .query(&[("query", format!("INSERT INTO {table} FORMAT JSONEachRow"))])
        .query(&[("date_time_input_format", "best_effort")])
        .body(body)
        .send()
        .await
        .expect("reach clickhouse");
    assert!(
        response.status().is_success(),
        "insert into {table}: {}",
        response.text().await.unwrap_or_default()
    );
}

/// `select count() from {table} where {predicate}`, sent as a GET: clickhouse
/// refuses a POST with no body and no `content-length` (`HTTP_LENGTH_REQUIRED`)
async fn count(client: &reqwest::Client, base: &str, table: &str, predicate: &str) -> u64 {
    let response = client
        .get(format!("{base}/"))
        .query(&[(
            "query",
            format!("select count() from {table} where {predicate}"),
        )])
        .send()
        .await
        .expect("reach clickhouse");
    let ok = response.status().is_success();
    let text = response.text().await.unwrap_or_default();
    assert!(ok, "count from {table}: {text}");
    text.trim().parse().expect("a count")
}

/// One organization's seeded traffic.
struct Seed {
    org: Uuid,
    key: Uuid,
    /// request ids of the rows written by the current schema
    current: String,
    /// the request whose payload row predates `020_request_payloads_key.sql`:
    /// no `virtual_key_id` on the payload, reachable only through its log row
    legacy: String,
}

impl Seed {
    fn new() -> Self {
        Self {
            org: Uuid::new_v4(),
            key: Uuid::new_v4(),
            current: format!("req-current-{}", Uuid::new_v4().simple()),
            legacy: format!("req-legacy-{}", Uuid::new_v4().simple()),
        }
    }

    fn rows(&self, ts: &str, shared: &str) -> (Vec<Value>, Vec<Value>) {
        let (org, key) = (self.org.to_string(), self.key.to_string());
        let mut logs = Vec::new();
        let mut payloads = Vec::new();
        for (request_id, legacy) in [
            (&self.current, false),
            (&self.legacy, true),
            (&shared.to_string(), false),
        ] {
            let log_id = Uuid::new_v4().to_string();
            logs.push(json!({
                "ts": ts, "request_id": request_id, "log_id": log_id,
                "org_id": org, "virtual_key_id": key,
                "model": "fake-llm", "status": 200,
            }));
            let mut payload = json!({
                "ts": ts, "request_id": request_id, "log_id": log_id, "org_id": org,
                "request_payload": format!("{{\"secret\":\"prompt-{request_id}\"}}"),
                "response_payload": format!("{{\"answer\":\"completion-{request_id}\"}}"),
            });
            if !legacy {
                payload["virtual_key_id"] = json!(key);
            }
            payloads.push(payload);
        }
        (logs, payloads)
    }
}

async fn serve(app: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Submit an erasure and wait for the control plane to report it `completed`.
async fn erase(
    client: &reqwest::Client,
    control: std::net::SocketAddr,
    scope: Value,
) -> (String, Value) {
    let created = client
        .post(format!("http://{control}/api/v1/log-erasures"))
        .bearer_auth(ADMIN_TOKEN)
        .json(&json!({"scope": scope, "reason": "erasure request DSR-3280"}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 202, "{}", created.text().await.unwrap());
    let created: Value = created.json().await.unwrap();
    assert_eq!(created["status"], "pending", "{created}");
    let id = created["id"].as_str().unwrap().to_string();

    for _ in 0..120 {
        let view: Value = client
            .get(format!("http://{control}/api/v1/log-erasures/{id}"))
            .bearer_auth(ADMIN_TOKEN)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        match view["status"].as_str() {
            Some("completed") => return (id, created),
            Some("pending") => tokio::time::sleep(Duration::from_millis(500)).await,
            other => panic!("the erasure ended as {other:?}: {view}"),
        }
    }
    panic!("the erasure never completed");
}

#[tokio::test]
async fn an_erasure_removes_exactly_its_rows_from_a_real_clickhouse() {
    if !test_database::is_configured() {
        eprintln!("skipping: {} not set", test_database::URL_ENV);
        return;
    }
    let Some(ch) = clickhouse_url() else {
        eprintln!("skipping: {CLICKHOUSE_URL_ENV} not set");
        return;
    };
    let http = reqwest::Client::new();
    clickhouse_ddl::apply_schema(&http, &ch).await;

    let db = TestSchema::create(&test_database::url().await.unwrap()).await;
    let app = rolter_control::test_app_with_clickhouse_and_admin_token(
        db.pool().clone(),
        &ch,
        Some(ADMIN_TOKEN.to_string()),
    )
    .await
    .unwrap();
    let control = serve(app).await;

    // two organizations, each with its own key; the same request id appears in
    // both, which is why a request erasure has to pin its organization
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let shared = format!("req-shared-{}", Uuid::new_v4().simple());
    let (acme, umbrella) = (Seed::new(), Seed::new());
    for seed in [&acme, &umbrella] {
        let (logs, payloads) = seed.rows(&ts, &shared);
        insert_rows(&http, &ch, "request_logs", &logs).await;
        insert_rows(&http, &ch, "request_payloads", &payloads).await;
    }

    let logs_of =
        |request_id: &str, org: Uuid| format!("request_id = '{request_id}' and org_id = '{org}'");
    // every (request id, organization) seeded
    let all = |seed: &Seed| {
        [seed.current.clone(), seed.legacy.clone(), shared.clone()]
            .map(|request_id| (request_id, seed.org))
    };
    for (request_id, org) in all(&acme).into_iter().chain(all(&umbrella)) {
        for table in ["request_logs", "request_payloads"] {
            assert_eq!(
                count(&http, &ch, table, &logs_of(&request_id, org)).await,
                1,
                "{table} before: {request_id}"
            );
        }
    }

    // erase acme's key: its current and its pre-020 payload rows, and its logs
    let (id, created) = erase(
        &http,
        control,
        json!({"kind": "virtual_key", "id": acme.key}),
    )
    .await;
    // payloads, logs and the MCP log, plus one more payload statement for the
    // rows that predate the column, found through their log row
    assert_eq!(created["statements"], 4, "{created}");

    // the marker stayed in the mutations' text, one per statement
    let marked = count(
        &http,
        &ch,
        "system.mutations",
        &format!("database = currentDatabase() and position(command, '{id}') > 0"),
    )
    .await;
    assert_eq!(marked, 4, "the erasure's marker in system.mutations");

    // acme's rows are gone, the one that predates the payload column included
    for request_id in [&acme.current, &acme.legacy, &shared] {
        for table in ["request_logs", "request_payloads"] {
            assert_eq!(
                count(&http, &ch, table, &logs_of(request_id, acme.org)).await,
                0,
                "{table} still holds acme's {request_id}"
            );
        }
    }
    // and umbrella's are not, the request id it shares with acme included
    for request_id in [&umbrella.current, &umbrella.legacy, &shared] {
        for table in ["request_logs", "request_payloads"] {
            assert_eq!(
                count(&http, &ch, table, &logs_of(request_id, umbrella.org)).await,
                1,
                "{table} lost umbrella's {request_id}"
            );
        }
    }

    // erase one request of umbrella: only that organization's copy of the id
    erase(
        &http,
        control,
        json!({"kind": "request", "request_id": shared, "org_id": umbrella.org}),
    )
    .await;
    for table in ["request_logs", "request_payloads"] {
        assert_eq!(
            count(&http, &ch, table, &logs_of(&shared, umbrella.org)).await,
            0,
            "{table} still holds umbrella's shared request"
        );
        for kept in [&umbrella.current, &umbrella.legacy] {
            assert_eq!(
                count(&http, &ch, table, &logs_of(kept, umbrella.org)).await,
                1,
                "{table} lost umbrella's {kept} to a request erasure"
            );
        }
    }
}
