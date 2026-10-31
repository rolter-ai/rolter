//! Subject-scoped erasure of request logs and captured payloads (#1085), against
//! a stand-in ClickHouse that records what it is asked and answers
//! `system.mutations` as a test directs.

use crate::support::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Stub {
    seen: Arc<Mutex<Vec<String>>>,
    /// whether `system.mutations` reports the erasure's mutations done
    done: Arc<AtomicBool>,
    /// whether a DDL statement is refused
    refuse: Arc<AtomicBool>,
}

async fn clickhouse(stub: Stub) -> String {
    let app = axum::Router::new().fallback(move |body: String| {
        let stub = stub.clone();
        async move {
            stub.seen.lock().await.push(body.clone());
            if body.contains("system.mutations") {
                let done = u8::from(stub.done.load(Ordering::SeqCst));
                let row = json!({"is_done": done, "latest_fail_reason": ""});
                return (
                    axum::http::StatusCode::OK,
                    axum::Json(json!({"data": [row, row, row]})),
                )
                    .into_response_parts();
            }
            if body.contains("select distinct log_id") {
                return (axum::http::StatusCode::OK, axum::Json(json!({"data": []})))
                    .into_response_parts();
            }
            if stub.refuse.load(Ordering::SeqCst) {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(json!({"detail": "Code: 62. secret table layout"})),
                )
                    .into_response_parts();
            }
            (axum::http::StatusCode::OK, axum::Json(json!({}))).into_response_parts()
        }
    });
    format!("http://{}", serve(app).await)
}

trait Parts {
    fn into_response_parts(self) -> axum::response::Response;
}
impl Parts for (axum::http::StatusCode, axum::Json<Value>) {
    fn into_response_parts(self) -> axum::response::Response {
        axum::response::IntoResponse::into_response(self)
    }
}

async fn audit_actions(pool: &sqlx::PgPool) -> Vec<(String, Value)> {
    sqlx::query_as::<_, (String, Value)>(
        "select action, coalesce(detail, '{}'::jsonb) from audit_log \
         where action like 'log_erasure.%' order by at",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn an_erasure_is_pending_until_clickhouse_reports_every_mutation_done() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let stub = Stub::default();
    let ch = clickhouse(stub.clone()).await;
    let app = rolter_control::test_app_with_clickhouse_and_admin_token(
        pool.clone(),
        &ch,
        Some(admin_token().to_string()),
    )
    .await
    .unwrap();
    let base = format!("http://{}", serve(app).await);
    let client = reqwest::Client::new();
    let key = uuid::Uuid::new_v4();

    let created = client
        .post(format!("{base}/api/v1/log-erasures"))
        .bearer_auth(admin_token())
        .json(&json!({
            "scope": {"kind": "virtual_key", "id": key},
            "reason": "erasure request DSR-1"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 202);
    let created: Value = created.json().await.unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["status"], "pending");
    assert_eq!(created["statements"], 3);

    {
        let seen = stub.seen.lock().await;
        let ddl: Vec<_> = seen
            .iter()
            .filter(|s| s.starts_with("alter table"))
            .collect();
        assert_eq!(ddl.len(), 3, "{seen:?}");
        // payloads first, so the rows that point at the logs go before them
        assert!(ddl[0].starts_with("alter table request_payloads delete"));
        assert!(ddl[1].starts_with("alter table request_logs delete"));
        // the MCP tool-call log is part of the scope (#3278)
        assert!(ddl[2].starts_with("alter table mcp_tool_call_logs delete"));
        for statement in ddl {
            assert!(statement.contains(&key.to_string()));
            assert!(statement.contains(&format!("'{id}' != ''")));
        }
    }

    // ClickHouse has queued the mutations but not run them
    let pending: Value = client
        .get(format!("{base}/api/v1/log-erasures/{id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pending["status"], "pending");
    assert_eq!(pending["mutations_done"], 0);

    stub.done.store(true, Ordering::SeqCst);
    let settled: Value = client
        .get(format!("{base}/api/v1/log-erasures/{id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settled["status"], "completed");
    assert!(settled["completed_at"].is_string());

    // the entries name the scope and the reason, and are in Postgres, which the
    // erasure never touches
    let audit = audit_actions(&pool).await;
    assert_eq!(
        audit.iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(),
        ["log_erasure.request", "log_erasure.complete"]
    );
    assert_eq!(audit[0].1["scope"], "virtual_key");
    assert_eq!(audit[0].1["reason"], "erasure request DSR-1");

    let listed: Value = client
        .get(format!("{base}/api/v1/log-erasures"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["data"][0]["id"], id);
}

#[tokio::test]
async fn a_refused_statement_is_a_failed_record_that_does_not_echo_clickhouse() {
    skip_without_db!();
    let db = fresh_db().await;
    let stub = Stub::default();
    stub.refuse.store(true, Ordering::SeqCst);
    let ch = clickhouse(stub).await;
    let app = rolter_control::test_app_with_clickhouse_and_admin_token(
        db.pool().clone(),
        &ch,
        Some(admin_token().to_string()),
    )
    .await
    .unwrap();
    let base = format!("http://{}", serve(app).await);

    let response = reqwest::Client::new()
        .post(format!("{base}/api/v1/log-erasures"))
        .bearer_auth(admin_token())
        .json(&json!({
            "scope": {"kind": "org", "id": uuid::Uuid::new_v4()},
            "reason": "r"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["status"], "failed");
    assert!(!body.to_string().contains("secret table layout"), "{body}");
}

#[tokio::test]
async fn erasure_is_a_superadmins_and_validates_its_input() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let ch = clickhouse(Stub::default()).await;
    let app = rolter_control::test_app_with_clickhouse_and_admin_token(
        pool.clone(),
        &ch,
        Some(admin_token().to_string()),
    )
    .await
    .unwrap();
    let base = format!("http://{}", serve(app).await);
    let client = reqwest::Client::new();
    let url = format!("{base}/api/v1/log-erasures");
    let scope = json!({"kind": "org", "id": uuid::Uuid::new_v4()});

    assert_eq!(
        client
            .post(&url)
            .json(&json!({"scope": scope, "reason": "r"}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let member = seed_user(&pool, "member@example.test", false).await;
    let session = seed_session(&pool, member, "erasure-member").await;
    assert_eq!(
        client
            .post(&url)
            .bearer_auth(&session)
            .json(&json!({"scope": scope, "reason": "r"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    for body in [
        json!({"scope": scope, "reason": "  "}),
        json!({"scope": {"kind": "request", "request_id": "", "org_id": uuid::Uuid::new_v4()}, "reason": "r"}),
    ] {
        let response = client
            .post(&url)
            .bearer_auth(admin_token())
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{body}");
    }
    assert!(audit_actions(&pool).await.is_empty());
    // a user who created no key that still exists is not refused: the MCP log
    // carries their id itself, so there are rows to find (#3278)
    let response = client
        .post(&url)
        .bearer_auth(admin_token())
        .json(&json!({"scope": {"kind": "user", "id": member}, "reason": "r"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
}

#[tokio::test]
async fn a_user_who_holds_no_keys_is_recorded_as_leaving_the_request_logs_unsearched() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let stub = Stub::default();
    let ch = clickhouse(stub.clone()).await;
    let app = rolter_control::test_app_with_clickhouse_and_admin_token(
        pool.clone(),
        &ch,
        Some(admin_token().to_string()),
    )
    .await
    .unwrap();
    let base = format!("http://{}", serve(app).await);
    let client = reqwest::Client::new();

    let created = client
        .post(format!("{base}/api/v1/log-erasures"))
        .bearer_auth(admin_token())
        .json(&json!({
            "scope": {"kind": "user", "id": uuid::Uuid::new_v4()},
            "reason": "erasure request DSR-2"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 202);
    let created: Value = created.json().await.unwrap();
    let searched = |stores: &Value, name: &str| -> (bool, String) {
        let row = stores
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["store"] == name)
            .unwrap_or_else(|| panic!("{name} missing from {stores}"));
        (
            row["searched"].as_bool().unwrap(),
            row["reason"].as_str().unwrap_or_default().to_string(),
        )
    };
    // the response, the stored record and the audit entry all say it
    let id = created["id"].as_str().unwrap().to_string();
    let fetched: Value = client
        .get(format!("{base}/api/v1/log-erasures/{id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let audit = audit_actions(&pool).await;
    for stores in [
        &created["stores"],
        &fetched["stores"],
        &audit[0].1["stores"],
    ] {
        let (logs, reason) = searched(stores, "request_logs");
        assert!(!logs);
        assert!(reason.contains("no virtual keys"), "{reason}");
        assert!(!searched(stores, "request_payloads").0);
        assert!(searched(stores, "mcp_tool_call_logs").0);
    }
}

#[tokio::test]
async fn the_subject_picker_finds_named_keys_users_and_orgs_for_a_superadmin() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let ch = clickhouse(Stub::default()).await;
    let app = rolter_control::test_app_with_clickhouse_and_admin_token(
        pool.clone(),
        &ch,
        Some(admin_token().to_string()),
    )
    .await
    .unwrap();
    let base = format!("http://{}", serve(app).await);
    let client = reqwest::Client::new();

    let org: uuid::Uuid = sqlx::query_scalar(
        "insert into orgs (name, slug) values ('Acme Corp', 'acme') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'Platform') returning id")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    let project: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'prod') returning id")
            .bind(team)
            .fetch_one(&pool)
            .await
            .unwrap();
    let key: uuid::Uuid = sqlx::query_scalar(
        "insert into virtual_keys (project_id, key_hash, key_prefix, name) \
         values ($1, 'h-picker', 'pfx-picker', 'ci-deploy') returning id",
    )
    .bind(project)
    .fetch_one(&pool)
    .await
    .unwrap();
    let user = seed_user(&pool, "Grace.Hopper@example.test", false).await;

    let find = |query: String| {
        let client = client.clone();
        let url = format!("{base}/api/v1/log-erasures/subjects?{query}");
        async move {
            let response = client
                .get(url)
                .bearer_auth(admin_token())
                .send()
                .await
                .unwrap();
            let status = response.status().as_u16();
            (status, response.json::<Value>().await.unwrap())
        }
    };

    // by part of a name, case-insensitively; the place in the tenancy tells
    // two keys of one name apart
    let (status, body) = find("kind=virtual_key&q=DEPLOY".to_string()).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"][0]["id"], key.to_string());
    assert_eq!(body["data"][0]["name"], "ci-deploy");
    assert_eq!(body["data"][0]["detail"], "Acme Corp / Platform / prod");
    // by the key's prefix, too
    let (_, body) = find("kind=virtual_key&q=pfx-pic".to_string()).await;
    assert_eq!(body["data"][0]["id"], key.to_string());

    let (_, body) = find("kind=user&q=hopper".to_string()).await;
    assert_eq!(body["data"][0]["id"], user.to_string());
    assert_eq!(body["data"][0]["name"], "Grace.Hopper@example.test");

    let (_, body) = find("kind=org&q=acm".to_string()).await;
    assert_eq!(body["data"][0]["id"], org.to_string());
    assert_eq!(body["data"][0]["name"], "Acme Corp");

    // an id already in hand is named; an unknown one finds nothing
    let (_, body) = find(format!("kind=org&id={org}")).await;
    assert_eq!(body["data"][0]["name"], "Acme Corp");
    let (_, body) = find(format!("kind=org&id={}", uuid::Uuid::new_v4())).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 0);

    // `%` and `_` in the search match themselves, not everything
    let (_, body) = find("kind=org&q=%25".to_string()).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 0);

    // an unknown kind is refused, and so is a query that does not parse, in
    // the coded envelope rather than axum's plain text
    let (status, _) = find("kind=request".to_string()).await;
    assert_eq!(status, 400);
    for bad in ["kind=user&id=not-a-uuid", "q=acme"] {
        let (status, body) = find(bad.to_string()).await;
        assert_eq!(status, 400, "{bad}");
        assert_eq!(body["error"]["code"], "invalid_query", "{bad}: {body}");
    }

    // a list past the cap says it was cut, and `q` still reaches the rest
    assert!(!body_truncated(&find("kind=org".to_string()).await.1));
    sqlx::query(
        "insert into orgs (name, slug) select 'bulk ' || g, 'bulk-' || g \
         from generate_series(1, 520) g",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, body) = find("kind=org".to_string()).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 500);
    assert!(body_truncated(&body));
    let (_, body) = find("kind=org&q=bulk 520".to_string()).await;
    assert_eq!(body["data"][0]["name"], "bulk 520");
    assert!(!body_truncated(&body));

    // and a caller who may not erase may not look
    let member = seed_user(&pool, "member@example.test", false).await;
    let session = seed_session(&pool, member, "picker-member").await;
    let response = client
        .get(format!("{base}/api/v1/log-erasures/subjects?kind=org"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
}

fn body_truncated(body: &Value) -> bool {
    body["truncated"].as_bool().unwrap_or(false)
}
