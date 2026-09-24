//! Who reads which request logs, end to end against a real ClickHouse (#1820).
//!
//! The analytics and health routes used to answer anyone, and nothing narrowed
//! what they answered to the caller's tenancy. The unit tests in
//! `analytics_access.rs` pin how memberships become a filter and the ones in
//! `analytics.rs` pin that every query binds it, but neither can show that the
//! predicate ClickHouse actually evaluates lets the right rows through. That
//! needs the real engine: `has`, `indexOf` and array parameters are exactly the
//! kind of thing that type-checks in Rust and still means something else in SQL.
//!
//! So this seeds two orgs' worth of request logs with captured bodies, signs
//! in as users holding different roles at different scopes, and reads the
//! invocation list back as each of them.
//!
//! Gated on the `postgres` feature and on both `ROLTER_TEST_DATABASE_URL` and
//! `ROLTER_TEST_CLICKHOUSE_URL`; unset either and the tests self-skip.
#![cfg(feature = "postgres")]

use std::net::SocketAddr;

use rolter_store::postgres::test_database;
use rolter_store::postgres::test_schema::TestSchema;
use serde_json::{json, Value};
use uuid::Uuid;

const CLICKHOUSE_URL_ENV: &str = "ROLTER_TEST_CLICKHOUSE_URL";
const ADMIN_TOKEN: &str = "analytics-scoping-admin";

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

/// Apply every shipped ClickHouse migration. All of them are idempotent — the
/// same property `ux-capture.sh apply-schema` relies on — so a shared server
/// that already has the tables is left as it was.
async fn ensure_schema(client: &reqwest::Client, base: &str) {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../clickhouse"));
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("read the clickhouse migration directory")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "sql").then_some(path)
        })
        .collect();
    files.sort();
    for path in files {
        let ddl = std::fs::read_to_string(&path).expect("read shipped DDL");
        // comments go first, exactly as `ux-capture.sh apply-schema` strips them:
        // several hold a `;` of their own. then one statement per request, since
        // the HTTP interface refuses more than one
        let stripped: String = ddl
            .lines()
            .map(|line| line.split("--").next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        for statement in stripped
            .split(';')
            .map(str::trim)
            .filter(|statement| !statement.is_empty())
            .map(str::to_string)
        {
            let response = client
                .post(format!("{base}/"))
                .body(statement)
                .send()
                .await
                .expect("reach clickhouse");
            assert!(
                response.status().is_success(),
                "{}: {}",
                path.display(),
                response.text().await.unwrap_or_default()
            );
        }
    }
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
    let user: Uuid = sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, null, false) \
         returning id",
    )
    .bind(format!("{role}-{}@scoping.test", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .unwrap();
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
    token
}

/// One request-log row per project with a captured body, plus one row the
/// gateway logged with no tenant at all. Returns the request ids by label.
async fn seed_logs(
    client: &reqwest::Client,
    base: &str,
    tenants: &[(&str, &Tenant)],
) -> Vec<(String, String)> {
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let mut logs = Vec::new();
    let mut payloads = Vec::new();
    let mut ids = Vec::new();
    for (tag, tenant) in tenants {
        for (label, project) in [("a", tenant.project_a), ("b", tenant.project_b)] {
            let request_id = format!("scoping-{tag}-{label}-{}", Uuid::new_v4().simple());
            logs.push(json!({
                "ts": ts, "request_id": request_id, "trace_id": format!("trace-{request_id}"),
                "org_id": tenant.org.to_string(), "team_id": tenant.team.to_string(),
                "project_id": project.to_string(),
                "model": "fake-llm", "status": 200,
            }));
            payloads.push(json!({
                "ts": ts, "request_id": request_id,
                "request_payload": format!("{{\"secret\":\"prompt-{tag}-{label}\"}}"),
                "response_payload": format!("{{\"answer\":\"completion-{tag}-{label}\"}}"),
            }));
            ids.push((format!("{tag}-{label}"), request_id));
        }
    }
    let orphan = format!("scoping-orphan-{}", Uuid::new_v4().simple());
    logs.push(json!({"ts": ts, "request_id": orphan, "model": "fake-llm", "status": 200}));
    ids.push(("orphan".to_string(), orphan));
    insert_rows(client, base, "request_logs", &logs).await;
    insert_rows(client, base, "request_payloads", &payloads).await;
    ids
}

/// The invocation rows `token` is shown among `ids`, keyed by label, each with
/// whether its request body came back and whether it was withheld.
async fn visible(
    client: &reqwest::Client,
    addr: SocketAddr,
    token: &str,
    ids: &[(String, String)],
) -> Vec<(String, bool, bool)> {
    let response = client
        .get(format!(
            "http://{addr}/api/v1/analytics/invocations?limit=200"
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "invocations as {token}");
    let body: Value = response.json().await.unwrap();
    let mut seen: Vec<(String, bool, bool)> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| {
            let request_id = row["request_id"].as_str()?;
            let (label, _) = ids.iter().find(|(_, id)| id == request_id)?;
            let has_body = !row["request_payload"]
                .as_str()
                .unwrap_or_default()
                .is_empty();
            let withheld = row["payload_withheld"].as_u64() == Some(1);
            Some((label.clone(), has_body, withheld))
        })
        .collect();
    seen.sort();
    seen
}

fn rows(expected: &[(&str, bool, bool)]) -> Vec<(String, bool, bool)> {
    let mut rows: Vec<(String, bool, bool)> = expected
        .iter()
        .map(|(label, body, withheld)| (label.to_string(), *body, *withheld))
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn each_role_reads_its_own_tenancy_and_bodies_only_where_it_may() {
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
    let ids = seed_logs(&http, &ch, &[("acme", &acme), ("umbrella", &umbrella)]).await;

    // anonymous and forged callers are refused before any query runs
    for bearer in [None, Some("not-a-session")] {
        let mut request = http.get(format!("http://{addr}/api/v1/analytics/invocations"));
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        assert_eq!(request.send().await.unwrap().status(), 401);
    }

    // the admin token reads everything, the tenantless row included
    assert_eq!(
        visible(&http, addr, ADMIN_TOKEN, &ids).await,
        rows(&[
            ("acme-a", true, false),
            ("acme-b", true, false),
            ("umbrella-a", true, false),
            ("umbrella-b", true, false),
            ("orphan", false, false),
        ])
    );

    // an org viewer sees the org's rows and none of their bodies
    let org_viewer = seed_user(&pool, Some(acme.org), None, None, "viewer").await;
    assert_eq!(
        visible(&http, addr, &org_viewer, &ids).await,
        rows(&[("acme-a", false, true), ("acme-b", false, true)])
    );

    // a project member sees that project, with its bodies, and nothing else
    let project_member = seed_user(&pool, None, None, Some(acme.project_a), "member").await;
    assert_eq!(
        visible(&http, addr, &project_member, &ids).await,
        rows(&[("acme-a", true, false)])
    );

    // a team member reaches both of the team's projects
    let team_member = seed_user(&pool, None, Some(umbrella.team), None, "member").await;
    assert_eq!(
        visible(&http, addr, &team_member, &ids).await,
        rows(&[("umbrella-a", true, false), ("umbrella-b", true, false)])
    );

    // a project admin lets its viewers read the bodies of that project alone
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
        visible(&http, addr, &org_viewer, &ids).await,
        rows(&[("acme-a", false, true), ("acme-b", true, false)])
    );

    // health rollups answer too, and refuse an anonymous caller the same way
    let health = http
        .get(format!("http://{addr}/api/v1/health/uptime"))
        .bearer_auth(&org_viewer)
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    let anonymous_health = http
        .get(format!("http://{addr}/api/v1/health/uptime"))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous_health.status(), 401);

    // a request looked up by the id its client was handed (#1849)
    let lookup = |token: String, query: String| {
        let http = http.clone();
        async move {
            let response = http
                .get(format!(
                    "http://{addr}/api/v1/analytics/invocations?{query}"
                ))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "lookup {query}");
            let body: Value = response.json().await.unwrap();
            body["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["request_id"].as_str().unwrap().to_string())
                .collect::<Vec<String>>()
        }
    };
    let id_of = |label: &str| {
        ids.iter()
            .find(|(seeded, _)| seeded == label)
            .unwrap()
            .1
            .clone()
    };
    let (acme_a, umbrella_a) = (id_of("acme-a"), id_of("umbrella-a"));
    let admin = ADMIN_TOKEN.to_string();
    assert_eq!(
        lookup(admin.clone(), format!("request_id={acme_a}")).await,
        vec![acme_a.clone()]
    );
    assert_eq!(
        lookup(admin.clone(), format!("trace_id=trace-{acme_a}")).await,
        vec![acme_a.clone()]
    );
    // naming another tenant's request exactly still finds nothing
    assert!(
        lookup(project_member.clone(), format!("request_id={umbrella_a}"))
            .await
            .is_empty()
    );
    assert_eq!(
        lookup(project_member.clone(), format!("request_id={acme_a}")).await,
        vec![acme_a.clone()]
    );
    // and an id older than the 7-day default window is found without `since`
    let old = format!("scoping-old-{}", Uuid::new_v4().simple());
    let long_ago = (chrono::Utc::now() - chrono::Duration::days(10))
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    insert_rows(
        &http,
        &ch,
        "request_logs",
        &[json!({
            "ts": long_ago, "request_id": old,
            "org_id": acme.org.to_string(), "team_id": acme.team.to_string(),
            "project_id": acme.project_a.to_string(),
            "model": "fake-llm", "status": 200,
        })],
    )
    .await;
    assert_eq!(lookup(admin, format!("request_id={old}")).await, vec![old]);
}
