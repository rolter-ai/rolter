//! Who reads which MCP tool-call logs, end to end against a real ClickHouse
//! (#1831).
//!
//! `mcp_log:read` used to be superadmin-only, so the engineer whose agent's tool
//! call failed could not see the one row that said why. It now follows the
//! request-log model of #1820: a caller reads the rows of the orgs, teams and
//! projects they hold a role in, plus every row of their own OAuth sessions,
//! and the tool arguments and results need the `request_payload` floor at the
//! row's own scope. As in `analytics_scoping.rs`, the predicate has to be
//! evaluated by the real engine to be believed, so this seeds rows straight into
//! ClickHouse and reads them back through the control plane as different users.
//!
//! Gated on the `postgres` feature and on both `ROLTER_TEST_DATABASE_URL` and
//! `ROLTER_TEST_CLICKHOUSE_URL`; unset either and the tests self-skip.
#![cfg(feature = "postgres")]

#[path = "common/clickhouse_ddl.rs"]
mod clickhouse_ddl;

use std::net::SocketAddr;

use rolter_store::postgres::test_database;
use rolter_store::postgres::test_schema::TestSchema;
use serde_json::{json, Value};
use uuid::Uuid;

const CLICKHOUSE_URL_ENV: &str = "ROLTER_TEST_CLICKHOUSE_URL";
const ADMIN_TOKEN: &str = "mcp-log-scoping-admin";

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

async fn ensure_schema(client: &reqwest::Client, base: &str) {
    clickhouse_ddl::apply_schema(client, base).await;
}

async fn insert_rows(client: &reqwest::Client, base: &str, table: &str, rows: &[Value]) {
    let body: String = rows
        .iter()
        .map(|row| format!("{row}\n"))
        .collect::<Vec<_>>()
        .concat();
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

async fn serve(app: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// One org with a team and two projects, created straight in the store.
struct Tenant {
    org: Uuid,
    team: Uuid,
    project_a: Uuid,
    project_b: Uuid,
}

async fn seed_tenant(pool: &sqlx::PgPool, tag: &str) -> Tenant {
    let org: Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
            .bind(format!("scoping-{tag}-{}", Uuid::new_v4().simple()))
            .fetch_one(pool)
            .await
            .unwrap();
    let team: Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(pool)
            .await
            .unwrap();
    let project = |name: &'static str| {
        sqlx::query_scalar::<_, Uuid>(
            "insert into projects (team_id, name) values ($1, $2) returning id",
        )
        .bind(team)
        .bind(name)
        .fetch_one(pool)
    };
    let project_a = project("alpha").await.unwrap();
    let project_b = project("beta").await.unwrap();
    Tenant {
        org,
        team,
        project_a,
        project_b,
    }
}

/// A signed-in local user holding `role` at exactly one scope, returned as the
/// bearer token the dashboard would send.
async fn seed_user(
    pool: &sqlx::PgPool,
    org: Option<Uuid>,
    team: Option<Uuid>,
    project: Option<Uuid>,
    role: &str,
) -> String {
    let (user, token) = seed_account(pool, role).await;
    add_membership(pool, user, org, team, project, role).await;
    token
}

async fn add_membership(
    pool: &sqlx::PgPool,
    user: Uuid,
    org: Option<Uuid>,
    team: Option<Uuid>,
    project: Option<Uuid>,
    role: &str,
) {
    sqlx::query(
        "insert into memberships (user_id, org_id, team_id, project_id, role) \
         values ($1, $2, $3, $4, $5)",
    )
    .bind(user)
    .bind(org)
    .bind(team)
    .bind(project)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
}

/// A signed-in local user with no role anywhere yet, as its id and the bearer
/// token the dashboard would send.
async fn seed_account(pool: &sqlx::PgPool, label: &str) -> (Uuid, String) {
    let user: Uuid = sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, null, false) \
         returning id",
    )
    .bind(format!("{label}-{}@scoping.test", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .unwrap();
    let token = format!("rolter_sess_scoping_{}", Uuid::new_v4().simple());
    sqlx::query(
        "insert into sessions (user_id, token_hash, expires_at) \
         values ($1, $2, now() + interval '1 hour')",
    )
    .bind(user)
    .bind(rolter_auth::hash_key("", &token))
    .execute(pool)
    .await
    .unwrap();
    (user, token)
}

/// One event per project of each tenant, one row a user's own OAuth session
/// wrote with no tenancy, and one legacy row with neither tenancy nor user.
/// Returns the event ids by label.
async fn seed_events(
    client: &reqwest::Client,
    base: &str,
    tenants: &[(&str, &Tenant)],
    own_user: Uuid,
) -> Vec<(String, String)> {
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let mut rows = Vec::new();
    let mut ids = Vec::new();
    let mut push = |label: String, org: String, team: String, project: String, user: String| {
        let event_id = format!("mcp-scoping-{label}-{}", Uuid::new_v4().simple());
        rows.push(json!({
            "ts": ts, "event_id": event_id, "server": "docs", "tool": "search",
            "transport": "streamable_http", "status": "transport_error", "latency_ms": 5,
            "org_id": org, "team_id": team, "project_id": project,
            "virtual_key_id": "", "user_id": user, "request_id": "", "trace_id": "",
            "arguments": format!("{{\"q\":\"arg-{label}\"}}"),
            "result": format!("{{\"r\":\"res-{label}\"}}"),
            "error": "transport failure",
        }));
        ids.push((label, event_id));
    };
    for (tag, tenant) in tenants {
        for (label, project) in [("a", tenant.project_a), ("b", tenant.project_b)] {
            push(
                format!("{tag}-{label}"),
                tenant.org.to_string(),
                tenant.team.to_string(),
                project.to_string(),
                Uuid::new_v4().to_string(),
            );
        }
    }
    push(
        "own".to_string(),
        String::new(),
        String::new(),
        String::new(),
        own_user.to_string(),
    );
    push(
        "legacy".to_string(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    insert_rows(client, base, "mcp_tool_call_logs", &rows).await;
    ids
}

/// The event labels `token` is shown in the list, sorted.
async fn listed(
    client: &reqwest::Client,
    addr: SocketAddr,
    token: &str,
    ids: &[(String, String)],
) -> Vec<String> {
    let response = client
        .get(format!("http://{addr}/api/v1/mcp/logs?limit=200"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "mcp log list");
    let body: Value = response.json().await.unwrap();
    let mut seen: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| {
            let event_id = row["event_id"].as_str()?;
            ids.iter()
                .find(|(_, id)| id == event_id)
                .map(|(label, _)| label.clone())
        })
        .collect();
    seen.sort();
    seen
}

fn labels(expected: &[&str]) -> Vec<String> {
    let mut labels: Vec<String> = expected.iter().map(|label| label.to_string()).collect();
    labels.sort();
    labels
}

/// The detail of one event as `(status, has arguments, has result, withheld)`.
async fn detail(
    client: &reqwest::Client,
    addr: SocketAddr,
    token: &str,
    ids: &[(String, String)],
    label: &str,
) -> (u16, bool, bool, bool) {
    let (_, event_id) = ids.iter().find(|(l, _)| l == label).unwrap();
    let response = client
        .get(format!("http://{addr}/api/v1/mcp/logs/{event_id}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    if status != 200 {
        return (status, false, false, false);
    }
    let body: Value = response.json().await.unwrap();
    let filled = |field: &str| !body[field].as_str().unwrap_or_default().is_empty();
    (
        status,
        filled("arguments"),
        filled("result"),
        body["payload_withheld"].as_u64() == Some(1),
    )
}

async fn summary_calls(client: &reqwest::Client, addr: SocketAddr, token: &str) -> u64 {
    let response = client
        .get(format!("http://{addr}/api/v1/mcp/logs/summary"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "mcp log summary");
    let body: Value = response.json().await.unwrap();
    // clickhouse renders UInt64 as a string in JSON output
    let calls = &body["data"][0]["calls"];
    calls
        .as_u64()
        .or_else(|| calls.as_str().and_then(|calls| calls.parse().ok()))
        .unwrap()
}

#[tokio::test]
async fn each_role_reads_its_own_tenancy_and_arguments_only_where_it_may() {
    let ch = skip_without_stack!();
    let http = reqwest::Client::new();
    ensure_schema(&http, &ch).await;

    let db = TestSchema::create(&test_database::url().await.unwrap()).await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_clickhouse_and_admin_token(
        pool.clone(),
        &ch,
        Some(ADMIN_TOKEN.to_string()),
    )
    .await
    .unwrap();
    let addr = serve(app).await;

    let acme = seed_tenant(&pool, "acme").await;
    let umbrella = seed_tenant(&pool, "umbrella").await;
    // the member's own OAuth session wrote a row attributed to no project
    let (member, member_token) = seed_account(&pool, "member").await;
    add_membership(&pool, member, None, None, Some(acme.project_a), "member").await;
    let ids = seed_events(
        &http,
        &ch,
        &[("acme", &acme), ("umbrella", &umbrella)],
        member,
    )
    .await;

    // anonymous and forged callers are refused before any query runs
    for bearer in [None, Some("not-a-session")] {
        for path in ["/api/v1/mcp/logs", "/api/v1/mcp/logs/summary"] {
            let mut request = http.get(format!("http://{addr}{path}"));
            if let Some(bearer) = bearer {
                request = request.bearer_auth(bearer);
            }
            assert_eq!(request.send().await.unwrap().status(), 401, "{path}");
        }
    }

    // the admin token reads everything, the legacy row and all bodies included
    assert_eq!(
        listed(&http, addr, ADMIN_TOKEN, &ids).await,
        labels(&[
            "acme-a",
            "acme-b",
            "umbrella-a",
            "umbrella-b",
            "own",
            "legacy"
        ])
    );
    assert_eq!(
        detail(&http, addr, ADMIN_TOKEN, &ids, "legacy").await,
        (200, true, true, false)
    );

    // a member of project a sees a's rows and their own session's row, not b's,
    // not another tenant's and not the legacy row
    assert_eq!(
        listed(&http, addr, &member_token, &ids).await,
        labels(&["acme-a", "own"])
    );
    // ... with the arguments and results of both, since a member meets the floor
    // on a and the own row is read at no scope the member holds a role in
    assert_eq!(
        detail(&http, addr, &member_token, &ids, "acme-a").await,
        (200, true, true, false)
    );
    // the summary counts the same rows, not the deployment
    assert_eq!(summary_calls(&http, addr, &member_token).await, 2);
    // a shared server may hold rows of other runs, so only a floor is stable
    assert!(summary_calls(&http, addr, ADMIN_TOKEN).await >= 6);

    // a row outside the caller's reach is a 404, exactly like a missing one
    for hidden in ["acme-b", "umbrella-a", "legacy"] {
        assert_eq!(
            detail(&http, addr, &member_token, &ids, hidden).await.0,
            404,
            "{hidden}"
        );
    }

    // a viewer lists the org's rows but is not given arguments or results
    let org_viewer = seed_user(&pool, Some(acme.org), None, None, "viewer").await;
    assert_eq!(
        listed(&http, addr, &org_viewer, &ids).await,
        labels(&["acme-a", "acme-b"])
    );
    assert_eq!(
        detail(&http, addr, &org_viewer, &ids, "acme-a").await,
        (200, false, false, true)
    );

    // the most specific role decides the bodies: an org admin who is only a
    // viewer on project a reads b's arguments but not a's
    let (narrowed, narrowed_token) = seed_account(&pool, "narrowed").await;
    add_membership(&pool, narrowed, Some(acme.org), None, None, "admin").await;
    add_membership(&pool, narrowed, None, None, Some(acme.project_a), "viewer").await;
    assert_eq!(
        detail(&http, addr, &narrowed_token, &ids, "acme-a").await,
        (200, false, false, true)
    );
    assert_eq!(
        detail(&http, addr, &narrowed_token, &ids, "acme-b").await,
        (200, true, true, false)
    );

    // a project admin lets its viewers read that project's bodies, and only its
    let admin_of_b = seed_user(&pool, None, None, Some(acme.project_b), "admin").await;
    let set = http
        .put(format!(
            "http://{addr}/api/v1/projects/{}/settings",
            acme.project_b
        ))
        .bearer_auth(&admin_of_b)
        .json(&json!({"payload_min_role": "viewer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(set.status(), 200);
    assert_eq!(
        detail(&http, addr, &org_viewer, &ids, "acme-b").await,
        (200, true, true, false)
    );
    assert_eq!(
        detail(&http, addr, &org_viewer, &ids, "acme-a").await,
        (200, false, false, true)
    );

    // a user with no role anywhere sees an empty list, a 404 and a zero count
    let (_, outsider) = seed_account(&pool, "outsider").await;
    assert!(listed(&http, addr, &outsider, &ids).await.is_empty());
    assert_eq!(detail(&http, addr, &outsider, &ids, "acme-a").await.0, 404);
    assert_eq!(summary_calls(&http, addr, &outsider).await, 0);

    // ingestion stays with the gateway and the superadmin: a member is refused
    let ingest = http
        .post(format!("http://{addr}/api/v1/mcp/events"))
        .bearer_auth(&member_token)
        .json(&json!({
            "event_id": "forged", "server": "docs", "tool": "search",
            "transport": "streamable_http", "status": "success", "latency_ms": 1,
            "project_id": acme.project_a.to_string(),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(ingest.status(), 403);
}
