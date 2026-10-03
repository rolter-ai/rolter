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

/// Give `user` a custom role of `org` whose base is viewer and which grants
/// `resource:read` outright, conferred at `project` through an access profile.
async fn grant_custom_role(
    pool: &sqlx::PgPool,
    user: Uuid,
    org: Uuid,
    project: Uuid,
    resource: &str,
) {
    let role: Uuid = sqlx::query_scalar(
        "insert into custom_roles (org_id, slug, name, base_role) \
         values ($1, 'payload-reader', 'Payload reader', 'viewer') returning id",
    )
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into custom_role_grants (role_id, resource, action) values ($1, $2, 'read')",
    )
    .bind(role)
    .bind(resource)
    .execute(pool)
    .await
    .unwrap();
    let profile: Uuid = sqlx::query_scalar(
        "insert into access_profiles (org_id, slug, name) \
         values ($1, 'auditors', 'Auditors') returning id",
    )
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into access_profile_roles (profile_id, role_id, project_id) values ($1, $2, $3)",
    )
    .bind(profile)
    .bind(role)
    .bind(project)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("insert into access_profile_assignments (profile_id, user_id) values ($1, $2)")
        .bind(profile)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
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
    assert_eq!(response.status(), 200, "invocations");
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

    // the most specific role decides the bodies: an org admin who is only a
    // viewer on project a reads the org's rows but not project a's prompts
    let (narrowed, narrowed_token) = seed_account(&pool, "narrowed").await;
    add_membership(&pool, narrowed, Some(acme.org), None, None, "admin").await;
    add_membership(&pool, narrowed, None, None, Some(acme.project_a), "viewer").await;
    assert_eq!(
        visible(&http, addr, &narrowed_token, &ids).await,
        rows(&[("acme-a", false, true), ("acme-b", true, false)])
    );

    // a custom role granting request_payload:read at one project, and no
    // membership at all, reads exactly that project with its bodies
    let (auditor, auditor_token) = seed_account(&pool, "auditor").await;
    grant_custom_role(&pool, auditor, acme.org, acme.project_a, "request_payload").await;
    assert_eq!(
        visible(&http, addr, &auditor_token, &ids).await,
        rows(&[("acme-a", true, false)])
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

/// Every invocation row `token` is shown whose request id is in `request_ids`,
/// as `(request_id, project_id, request_payload, payload_withheld)`, sorted.
async fn invocation_rows(
    client: &reqwest::Client,
    addr: SocketAddr,
    token: &str,
    request_ids: &[&str],
) -> Vec<(String, String, String, bool)> {
    let response = client
        .get(format!(
            "http://{addr}/api/v1/analytics/invocations?limit=200"
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "invocations");
    let body: Value = response.json().await.unwrap();
    let mut seen: Vec<(String, String, String, bool)> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| request_ids.contains(&row["request_id"].as_str().unwrap_or_default()))
        .map(|row| {
            let text = |key: &str| row[key].as_str().unwrap_or_default().to_string();
            (
                text("request_id"),
                text("project_id"),
                text("request_payload"),
                row["payload_withheld"].as_u64() == Some(1),
            )
        })
        .collect();
    seen.sort();
    seen
}

#[tokio::test]
async fn a_shared_request_id_never_carries_another_projects_body() {
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
    let acme = seed_tenant(&pool, "shared").await;
    let (victim, intruder) = (acme.project_a.to_string(), acme.project_b.to_string());

    // the gateway keeps whatever x-request-id the caller sent, so a member of
    // project b can log a request under an id they saw on project a's rows.
    // `bodiless` is the attack: b's request stored no body of its own, so an
    // id-only join handed it a's. `both` is the reverse: b's own later body
    // must not replace a's on a's row either
    let bodiless = format!("shared-bodiless-{}", Uuid::new_v4().simple());
    let both = format!("shared-both-{}", Uuid::new_v4().simple());
    let at = |ago_ms: i64| {
        (chrono::Utc::now() - chrono::Duration::milliseconds(ago_ms))
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string()
    };
    let (earlier, later) = (at(4000), at(2000));
    let log = |request_id: &str, project: &str, ts: &str| {
        json!({
            "ts": ts, "request_id": request_id,
            "org_id": acme.org.to_string(), "team_id": acme.team.to_string(),
            "project_id": project, "model": "fake-llm", "status": 200,
        })
    };
    let payload = |request_id: &str, ts: &str, body: &str| {
        json!({
            "ts": ts, "request_id": request_id,
            "request_payload": body, "response_payload": body,
        })
    };
    insert_rows(
        &http,
        &ch,
        "request_logs",
        &[
            log(&bodiless, &victim, &earlier),
            log(&bodiless, &intruder, &later),
            log(&both, &victim, &earlier),
            log(&both, &intruder, &later),
        ],
    )
    .await;
    insert_rows(
        &http,
        &ch,
        "request_payloads",
        &[
            payload(&bodiless, &earlier, "victim-bodiless"),
            payload(&both, &earlier, "victim-both"),
            payload(&both, &later, "intruder-both"),
        ],
    )
    .await;
    let ids = [bodiless.as_str(), both.as_str()];

    // the intruder's own rows carry nothing of the victim's, and nothing is
    // "withheld" on the bodiless one because there never was a body there
    let intruder_member = seed_user(&pool, None, None, Some(acme.project_b), "member").await;
    assert_eq!(
        invocation_rows(&http, addr, &intruder_member, &ids).await,
        vec![
            (bodiless.clone(), intruder.clone(), String::new(), false),
            (
                both.clone(),
                intruder.clone(),
                "intruder-both".into(),
                false
            ),
        ]
    );

    // the victim's members still read their own bodies, never the intruder's
    let victim_member = seed_user(&pool, None, None, Some(acme.project_a), "member").await;
    assert_eq!(
        invocation_rows(&http, addr, &victim_member, &ids).await,
        vec![
            (
                bodiless.clone(),
                victim.clone(),
                "victim-bodiless".into(),
                false
            ),
            (both.clone(), victim.clone(), "victim-both".into(), false),
        ]
    );

    // and a caller who reads both projects gets each row exactly once, with
    // its own body. project ids are random, so the expectation is sorted the
    // same way the rows are
    let mut expected = vec![
        (bodiless.clone(), intruder.clone(), String::new(), false),
        (
            bodiless.clone(),
            victim.clone(),
            "victim-bodiless".into(),
            false,
        ),
        (
            both.clone(),
            intruder.clone(),
            "intruder-both".into(),
            false,
        ),
        (both.clone(), victim.clone(), "victim-both".into(), false),
    ];
    expected.sort();
    assert_eq!(
        invocation_rows(&http, addr, ADMIN_TOKEN, &ids).await,
        expected
    );
}

/// A provider named `name` in `org`, created straight in the store.
async fn seed_provider(pool: &sqlx::PgPool, org: Uuid, name: &str) {
    sqlx::query(
        "insert into providers (org_id, name, slug, kind, api_base) \
         values ($1, $2, $2, 'openai', 'http://127.0.0.1:9')",
    )
    .bind(org)
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

/// The provider names among `names` that a health route answers `token` with.
async fn health_providers(
    client: &reqwest::Client,
    addr: SocketAddr,
    route: &str,
    token: &str,
    names: &[&str],
) -> Vec<String> {
    let response = client
        .get(format!("http://{addr}/api/v1/health/{route}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "health/{route}");
    let body: Value = response.json().await.unwrap();
    let mut seen: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["provider"].as_str())
        .filter(|provider| names.contains(provider))
        .map(str::to_string)
        .collect();
    seen.sort();
    seen.dedup();
    seen
}

#[tokio::test]
async fn provider_health_answers_only_the_providers_of_orgs_the_caller_reads() {
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
    let acme = seed_tenant(&pool, "health-acme").await;
    let umbrella = seed_tenant(&pool, "health-umbrella").await;

    // names are unique to this run: the clickhouse server is shared, and the
    // rows this test writes outlive it
    let suffix = &Uuid::new_v4().simple().to_string()[..12];
    let acme_provider = format!("hp-acme-{suffix}");
    let umbrella_provider = format!("hp-umbrella-{suffix}");
    seed_provider(&pool, acme.org, &acme_provider).await;
    seed_provider(&pool, umbrella.org, &umbrella_provider).await;

    // one outage and its recovery per provider, so mttr has an incident to
    // report as well as uptime and the timeline having events
    let at = |ago_ms: i64| {
        (chrono::Utc::now() - chrono::Duration::milliseconds(ago_ms))
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string()
    };
    let mut events = Vec::new();
    for (provider, org) in [
        (&acme_provider, acme.org),
        (&umbrella_provider, umbrella.org),
    ] {
        for (ago_ms, outcome) in [(6000, "ok"), (4000, "error"), (2000, "ok")] {
            events.push(json!({
                "ts": at(ago_ms), "target_id": provider, "provider": provider,
                "org_id": org.to_string(),
                "source": "probe", "outcome": outcome, "latency_ms": 10,
            }));
        }
    }
    insert_rows(&http, &ch, "provider_health_events", &events).await;
    let names = [acme_provider.as_str(), umbrella_provider.as_str()];

    let acme_viewer = seed_user(&pool, Some(acme.org), None, None, "viewer").await;
    let umbrella_viewer = seed_user(&pool, Some(umbrella.org), None, None, "viewer").await;
    // provider health is org-level: a project role does not reach it
    let acme_project_admin = seed_user(&pool, None, None, Some(acme.project_a), "admin").await;

    for route in ["uptime", "mttr", "timeline"] {
        assert_eq!(
            health_providers(&http, addr, route, ADMIN_TOKEN, &names).await,
            vec![acme_provider.clone(), umbrella_provider.clone()],
            "{route} as the admin token"
        );
        assert_eq!(
            health_providers(&http, addr, route, &acme_viewer, &names).await,
            vec![acme_provider.clone()],
            "{route} as an acme org viewer"
        );
        assert_eq!(
            health_providers(&http, addr, route, &umbrella_viewer, &names).await,
            vec![umbrella_provider.clone()],
            "{route} as an umbrella org viewer"
        );
        assert_eq!(
            health_providers(&http, addr, route, &acme_project_admin, &names).await,
            Vec::<String>::new(),
            "{route} as an acme project admin"
        );
    }
}

/// The `target_id`s among `targets` that a health route answers `token` with.
async fn health_targets(
    client: &reqwest::Client,
    addr: SocketAddr,
    route: &str,
    token: &str,
    targets: &[&str],
) -> Vec<String> {
    let response = client
        .get(format!("http://{addr}/api/v1/health/{route}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "health/{route}");
    let body: Value = response.json().await.unwrap();
    let mut seen: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["target_id"].as_str())
        .filter(|target| targets.contains(target))
        .map(str::to_string)
        .collect();
    seen.sort();
    seen.dedup();
    seen
}

/// Names are unique per org only, so a name an org deleted and another org
/// created again must not carry the first org's history into the second (#1908).
#[tokio::test]
async fn a_recreated_provider_name_does_not_inherit_another_orgs_health_history() {
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
    let org_a = seed_tenant(&pool, "recreate-a").await;
    let org_b = seed_tenant(&pool, "recreate-b").await;

    let suffix = &Uuid::new_v4().simple().to_string()[..12];
    let shared_name = format!("hp-shared-{suffix}");
    // org a owned the name and deleted it; org b created it afterwards
    seed_provider(&pool, org_a.org, &shared_name).await;
    sqlx::query("delete from providers where org_id = $1 and name = $2")
        .bind(org_a.org)
        .bind(&shared_name)
        .execute(&pool)
        .await
        .unwrap();
    seed_provider(&pool, org_b.org, &shared_name).await;

    let target_a = format!("hp-old-a-{suffix}");
    let target_b = format!("hp-new-b-{suffix}");
    let target_unowned = format!("hp-none-{suffix}");
    let at = |ago_ms: i64| {
        (chrono::Utc::now() - chrono::Duration::milliseconds(ago_ms))
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string()
    };
    let mut events = Vec::new();
    // an empty org is a config-file provider, or a row from before the column
    for (target, org) in [
        (&target_a, org_a.org.to_string()),
        (&target_b, org_b.org.to_string()),
        (&target_unowned, String::new()),
    ] {
        for (ago_ms, outcome) in [(6000, "ok"), (4000, "error"), (2000, "ok")] {
            events.push(json!({
                "ts": at(ago_ms), "target_id": target, "provider": shared_name,
                "org_id": org, "source": "passive", "outcome": outcome, "latency_ms": 10,
            }));
        }
    }
    insert_rows(&http, &ch, "provider_health_events", &events).await;
    let targets = [
        target_a.as_str(),
        target_b.as_str(),
        target_unowned.as_str(),
    ];

    let viewer_b = seed_user(&pool, Some(org_b.org), None, None, "viewer").await;
    let viewer_a = seed_user(&pool, Some(org_a.org), None, None, "viewer").await;
    for route in ["uptime", "mttr", "timeline"] {
        assert_eq!(
            health_targets(&http, addr, route, &viewer_b, &targets).await,
            vec![target_b.clone()],
            "{route}: org b must see only its own rows for the recreated name"
        );
        // the old org keeps its own history, and nobody's viewer sees the unowned rows
        assert_eq!(
            health_targets(&http, addr, route, &viewer_a, &targets).await,
            vec![target_a.clone()],
            "{route}: org a keeps its history"
        );
    }
    // the unrestricted callers still see every row
    assert_eq!(
        health_targets(&http, addr, "timeline", ADMIN_TOKEN, &targets).await,
        {
            let mut all = vec![target_a, target_b, target_unowned];
            all.sort();
            all
        },
    );
}

/// Bodies are keyed on the gateway's own `log_id`, which no caller chooses, so
/// requests that share both the caller's `x-request-id` and the millisecond
/// still read their own bodies and nobody else's (#1937).
#[tokio::test]
async fn requests_sharing_an_id_and_a_millisecond_keep_their_own_bodies() {
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
    let acme = seed_tenant(&pool, "keyed").await;
    let (victim, intruder) = (acme.project_a.to_string(), acme.project_b.to_string());

    // one id and one ts for every row below, which is the worst case: the old
    // (request_id, ts) join could not tell any of them apart
    let shared = format!("keyed-shared-{}", Uuid::new_v4().simple());
    let ts = (chrono::Utc::now() - chrono::Duration::seconds(3))
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let (victim_key, intruder_key) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    let (twin_one, twin_two) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    let log = |log_id: &str, project: &str| {
        json!({
            "ts": ts, "request_id": shared, "log_id": log_id,
            "org_id": acme.org.to_string(), "team_id": acme.team.to_string(),
            "project_id": project, "model": "fake-llm", "status": 200,
        })
    };
    let payload = |log_id: &str, project: &str, body: &str| {
        json!({
            "ts": ts, "request_id": shared, "log_id": log_id,
            "org_id": acme.org.to_string(), "project_id": project,
            "request_payload": body, "response_payload": body,
        })
    };
    // the intruder's request stored no body; the victim's did. an old payload
    // row with no key carries the same id and ts, as one written before the
    // key existed would, and must not attach to a keyed row either
    insert_rows(
        &http,
        &ch,
        "request_logs",
        &[
            log(&victim_key, &victim),
            log(&intruder_key, &intruder),
            log(&twin_one, &intruder),
            log(&twin_two, &intruder),
        ],
    )
    .await;
    insert_rows(
        &http,
        &ch,
        "request_payloads",
        &[
            payload(&victim_key, &victim, "victim-body"),
            payload(&twin_one, &intruder, "twin-one"),
            payload(&twin_two, &intruder, "twin-two"),
            json!({
                "ts": ts, "request_id": shared,
                "request_payload": "legacy-body", "response_payload": "legacy-body",
            }),
        ],
    )
    .await;
    let ids = [shared.as_str()];

    // cross-project: the intruder reads nothing of the victim's, and no
    // body from the unkeyed row. same-project: two requests of one project
    // under one id and ts each read their own body
    let intruder_member = seed_user(&pool, None, None, Some(acme.project_b), "member").await;
    let mut expected = vec![
        (shared.clone(), intruder.clone(), String::new(), false),
        (shared.clone(), intruder.clone(), "twin-one".into(), false),
        (shared.clone(), intruder.clone(), "twin-two".into(), false),
    ];
    expected.sort();
    assert_eq!(
        invocation_rows(&http, addr, &intruder_member, &ids).await,
        expected
    );

    let victim_member = seed_user(&pool, None, None, Some(acme.project_a), "member").await;
    assert_eq!(
        invocation_rows(&http, addr, &victim_member, &ids).await,
        vec![(shared.clone(), victim.clone(), "victim-body".into(), false)]
    );
}

/// Request-log sampling at 50 % (#2239): two kept rows weigh two requests
/// each, so counts and sums double while latency percentiles do not.
#[tokio::test]
async fn sampled_rows_scale_counts_and_sums_but_not_percentiles() {
    let ch = skip_without_stack!();
    let http = reqwest::Client::new();
    ensure_schema(&http, &ch).await;

    let db = TestSchema::create(&test_database::url().await.unwrap()).await;
    let app = rolter_control::test_app_with_clickhouse_and_admin_token(
        db.pool().clone(),
        &ch,
        Some(ADMIN_TOKEN.to_string()),
    )
    .await
    .unwrap();
    let addr = serve(app).await;

    // a model name no other test writes, so a shared server cannot skew it
    let model = format!("sampled-{}", Uuid::new_v4().simple());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let rows: Vec<Value> = [(200, 100), (500, 300)]
        .iter()
        .enumerate()
        .map(|(i, (status, latency))| {
            json!({
                "ts": ts, "request_id": format!("sampled-{i}-{}", Uuid::new_v4().simple()),
                "model": model, "status": status, "latency_ms": latency,
                "total_tokens": 10, "prompt_tokens": 4, "completion_tokens": 6,
                "cost_usd": 0.5, "sample_weight": 2.0,
            })
        })
        .collect();
    insert_rows(&http, &ch, "request_logs", &rows).await;

    let response = http
        .get(format!("http://{addr}/api/v1/analytics/by-model"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let row = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["model"] == model.as_str())
        .expect("the sampled model has a row");
    assert_eq!(row["requests"].as_f64(), Some(4.0));
    assert_eq!(row["tokens"].as_f64(), Some(40.0));
    assert_eq!(row["cost_usd"].as_f64(), Some(2.0));
    assert_eq!(row["errors"].as_f64(), Some(2.0));
    // a percentile of a uniform sample is already an estimate: never scaled
    let p50 = row["p50_latency_ms"].as_f64().unwrap();
    assert!((100.0..=300.0).contains(&p50), "{p50}");
}
