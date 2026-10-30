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

#[path = "common/clickhouse_ddl.rs"]
mod clickhouse_ddl;

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

    // an API token reads the chain it holds by the rules of a person with that
    // role, the body floor included (#3291): a project member token sees the
    // project with its bodies, an org viewer token the org's rows without them
    let mint = |scope: Value| {
        let http = http.clone();
        async move {
            let minted: Value = http
                .post(format!("http://{addr}/api/v1/orgs/{}/api-tokens", acme.org))
                .bearer_auth(ADMIN_TOKEN)
                .json(&scope)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            minted["secret"]
                .as_str()
                .expect("a minted secret")
                .to_string()
        }
    };
    let project_token = mint(json!({
        "name": "reporting",
        "project_id": acme.project_a,
        "role": "member",
    }))
    .await;
    assert_eq!(
        visible(&http, addr, &project_token, &ids).await,
        rows(&[("acme-a", true, false)])
    );
    let org_token = mint(json!({"name": "org reporting", "role": "viewer"})).await;
    assert_eq!(
        visible(&http, addr, &org_token, &ids).await,
        rows(&[("acme-a", false, true), ("acme-b", false, true)])
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
    // a project role has no org-level read, but reaches the providers its
    // project's routes target (#1833)
    let acme_project_admin = seed_user(&pool, None, None, Some(acme.project_a), "admin").await;
    let acme_other_project = seed_user(&pool, None, None, Some(acme.project_b), "viewer").await;
    let route_id: Uuid =
        sqlx::query_scalar("insert into routes (project_id, model) values ($1, $2) returning id")
            .bind(acme.project_a)
            .bind(format!("hp-model-{suffix}"))
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "insert into route_targets (route_id, provider_id) \
         select $1, id from providers where org_id = $2 and name = $3",
    )
    .bind(route_id)
    .bind(acme.org)
    .bind(&acme_provider)
    .execute(&pool)
    .await
    .unwrap();

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
            vec![acme_provider.clone()],
            "{route} as a member of the project whose route targets the provider"
        );
        // a project that does not route to it, and another org's provider,
        // stay closed
        assert_eq!(
            health_providers(&http, addr, route, &acme_other_project, &names).await,
            Vec::<String>::new(),
            "{route} as a member of a project with no route to the provider"
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

/// #3441: two orgs each have a provider of one name. Every health rollup
/// answers a series per org, with `org_id`, rather than merging them, and a
/// probe row a gateway wrote with the provider's runtime id (`name@org`, once
/// the name became shared) stays in the same series as the rows written
/// before, as a `provider`-grain row named by the display name. The
/// parameter-adjustment rollup keys a provider by `provider_org_id` the same way.
#[tokio::test]
async fn two_orgs_same_named_providers_are_two_series_that_survive_the_name_becoming_shared() {
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
    let org_a = seed_tenant(&pool, "twin-a").await;
    let org_b = seed_tenant(&pool, "twin-b").await;
    let suffix = &Uuid::new_v4().simple().to_string()[..12];
    let name = format!("hp-twin-{suffix}");
    seed_provider(&pool, org_a.org, &name).await;
    seed_provider(&pool, org_b.org, &name).await;

    let at = |ago_ms: i64| {
        (chrono::Utc::now() - chrono::Duration::milliseconds(ago_ms))
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string()
    };
    let (a, b) = (org_a.org.to_string(), org_b.org.to_string());
    let qualified = |org: &str| format!("{name}@{org}");
    // org a's probe before the name was shared, then after, under its runtime
    // id; org b's only after. each has one outage and its recovery
    let mut events = Vec::new();
    for (ago_ms, outcome, org, target) in [
        (9000, "ok", &a, name.clone()),
        (7000, "error", &a, name.clone()),
        (5000, "ok", &a, qualified(&a)),
        (8000, "ok", &b, qualified(&b)),
        (6000, "error", &b, qualified(&b)),
        (4000, "ok", &b, qualified(&b)),
    ] {
        events.push(json!({
            "ts": at(ago_ms), "target_id": target, "provider": name,
            "org_id": org, "source": "probe", "outcome": outcome, "latency_ms": 10,
        }));
    }
    insert_rows(&http, &ch, "provider_health_events", &events).await;

    for route in ["uptime", "mttr", "timeline"] {
        let body: Value = http
            .get(format!("http://{addr}/api/v1/health/{route}"))
            .bearer_auth(ADMIN_TOKEN)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut series: Vec<(String, String, String)> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["provider"] == name.as_str())
            .map(|row| {
                (
                    row["org_id"].as_str().unwrap().to_string(),
                    row["target_id"].as_str().unwrap().to_string(),
                    row["grain"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        series.sort();
        series.dedup();
        let mut expected = vec![
            (a.clone(), name.clone(), "provider".to_string()),
            (b.clone(), name.clone(), "provider".to_string()),
        ];
        expected.sort();
        assert_eq!(series, expected, "{route}: {body}");
        if route == "uptime" {
            let events_of = |org: &str| {
                body["data"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["provider"] == name.as_str() && row["org_id"] == org)
                    // a 64-bit count comes back quoted or bare, per the
                    // server's json settings
                    .and_then(|row| {
                        row["events"]
                            .as_u64()
                            .or_else(|| row["events"].as_str()?.parse().ok())
                    })
            };
            // org a's series spans the moment its runtime id changed
            assert_eq!(events_of(&a), Some(3), "{body}");
            assert_eq!(events_of(&b), Some(3), "{body}");
        }
    }

    // each org's request carried a dropped parameter through its own provider
    let ts = at(3000);
    let mut logs = Vec::new();
    for tenant in [&org_a, &org_b] {
        logs.push(json!({
            "ts": ts, "request_id": format!("twin-{}", Uuid::new_v4().simple()),
            "org_id": tenant.org.to_string(), "team_id": tenant.team.to_string(),
            "project_id": tenant.project_a.to_string(),
            "model": format!("twin-model-{suffix}"), "provider": name,
            "provider_org_id": tenant.org.to_string(), "status": 200,
            "param_adjustments": ["dropped:top_k"],
        }));
    }
    insert_rows(&http, &ch, "request_logs", &logs).await;
    let body: Value = http
        .get(format!("http://{addr}/api/v1/analytics/param-adjustments"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut rows: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["provider"] == name.as_str())
        .map(|row| row["provider_org_id"].as_str().unwrap().to_string())
        .collect();
    rows.sort();
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(rows, expected, "{body}");
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

/// A number from a ClickHouse `FORMAT JSON` row, which quotes 64-bit integers
/// and leaves floats bare.
fn number(value: &Value) -> f64 {
    match value {
        Value::String(text) => text.parse().unwrap_or_else(|_| panic!("{text} parses")),
        other => other
            .as_f64()
            .unwrap_or_else(|| panic!("{other} is a number")),
    }
}

/// The dashboard's summary, timeseries and by-model reads take the invocation
/// list's `model`, `key`, `business_unit` and `customer` filters (#2453). A
/// filter narrows what the caller already reads and never reaches past it:
/// rows of another project or org that match it exactly still stay out.
#[tokio::test]
async fn dashboard_filters_narrow_the_rollups_inside_the_callers_visibility() {
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
    let acme = seed_tenant(&pool, "dash-acme").await;
    let umbrella = seed_tenant(&pool, "dash-umbrella").await;

    // fresh names and ids, so rows other tests left in a shared clickhouse
    // can never match a filter below
    let tag = Uuid::new_v4().simple().to_string();
    let (model_one, model_two) = (format!("dash-one-{tag}"), format!("dash-two-{tag}"));
    let (key_one, key_two) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    let (unit_one, unit_two) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    let (customer_one, customer_two) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let row = |tenant: &Tenant,
               project: Uuid,
               model: &str,
               key: &str,
               unit: &str,
               customer: &str,
               cost: f64| {
        json!({
            "ts": ts, "request_id": format!("dash-{}", Uuid::new_v4().simple()),
            "org_id": tenant.org.to_string(), "team_id": tenant.team.to_string(),
            "project_id": project.to_string(), "virtual_key_id": key,
            "business_unit_id": unit, "customer_id": customer,
            "model": model, "status": 200, "total_tokens": 10, "cost_usd": cost,
        })
    };
    // the "one" attribution appears in every project and both orgs; the "two"
    // attribution only in acme's project a
    insert_rows(
        &http,
        &ch,
        "request_logs",
        &[
            row(
                &acme,
                acme.project_a,
                &model_one,
                &key_one,
                &unit_one,
                &customer_one,
                1.0,
            ),
            row(
                &acme,
                acme.project_a,
                &model_two,
                &key_two,
                &unit_two,
                &customer_two,
                2.0,
            ),
            row(
                &acme,
                acme.project_b,
                &model_one,
                &key_one,
                &unit_one,
                &customer_one,
                4.0,
            ),
            row(
                &umbrella,
                umbrella.project_a,
                &model_one,
                &key_one,
                &unit_one,
                &customer_one,
                8.0,
            ),
        ],
    )
    .await;

    let get = |token: String, path: &'static str, query: String| {
        let http = http.clone();
        async move {
            let response = http
                .get(format!("http://{addr}/api/v1/analytics/{path}?{query}"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{path}?{query}");
            let body: Value = response.json().await.unwrap();
            body["data"].as_array().unwrap().clone()
        }
    };
    // (requests, cost) of the summary
    let summary = |token: String, query: String| async move {
        let data = get(token, "summary", query).await;
        (number(&data[0]["requests"]), number(&data[0]["cost_usd"]))
    };
    let models = format!("model={model_one}");

    // a project member reads project a only; each filter narrows inside it
    let member = seed_user(&pool, None, None, Some(acme.project_a), "member").await;
    assert_eq!(summary(member.clone(), models.clone()).await, (1.0, 1.0));
    assert_eq!(
        summary(member.clone(), format!("model={model_two}")).await,
        (1.0, 2.0)
    );
    assert_eq!(
        summary(member.clone(), format!("key={key_one}")).await,
        (1.0, 1.0)
    );
    assert_eq!(
        summary(member.clone(), format!("business_unit={unit_one}")).await,
        (1.0, 1.0)
    );
    // a set is any of its ids
    assert_eq!(
        summary(
            member.clone(),
            format!("business_unit={unit_one},{unit_two}")
        )
        .await,
        (2.0, 3.0)
    );
    assert_eq!(
        summary(member.clone(), format!("customer={customer_two}")).await,
        (1.0, 2.0)
    );
    // filters combine with and: one model under the other unit is nothing
    assert_eq!(
        summary(member.clone(), format!("{models}&business_unit={unit_two}")).await,
        (0.0, 0.0)
    );

    // an org viewer reaches both of acme's projects but never umbrella's
    let org_viewer = seed_user(&pool, Some(acme.org), None, None, "viewer").await;
    assert_eq!(
        summary(org_viewer.clone(), format!("customer={customer_one}")).await,
        (2.0, 5.0)
    );
    // the admin token reads every org, so the same filter now finds all three
    assert_eq!(
        summary(ADMIN_TOKEN.to_string(), format!("customer={customer_one}")).await,
        (3.0, 13.0)
    );

    // the timeseries applies the same filters
    let buckets = get(
        org_viewer.clone(),
        "timeseries",
        format!("bucket=hour&key={key_one}"),
    )
    .await;
    let requests: f64 = buckets.iter().map(|b| number(&b["requests"])).sum();
    let cost: f64 = buckets.iter().map(|b| number(&b["cost_usd"])).sum();
    assert_eq!((requests, cost), (2.0, 5.0), "{buckets:?}");

    // and so does by-model: a customer filter leaves only that customer's model
    let by_model = get(
        member.clone(),
        "by-model",
        format!("customer={customer_one},{customer_two}&business_unit={unit_one}"),
    )
    .await;
    let rows: Vec<(String, f64)> = by_model
        .iter()
        .map(|row| {
            (
                row["model"].as_str().unwrap().to_string(),
                number(&row["requests"]),
            )
        })
        .collect();
    assert_eq!(rows, vec![(model_one.clone(), 1.0)]);
}

/// Request-log sampling (#2239): kept rows weigh the requests they stand for,
/// so counts and sums scale by it and the latency percentiles are weighted by it
/// (#3355). The weights differ per row, as they do when the rate changed inside
/// the window, which is what an unweighted percentile gets wrong.
#[tokio::test]
async fn sampled_rows_scale_counts_and_sums_and_weight_percentiles() {
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
    let rows: Vec<Value> = [(200, 100, 1.0), (200, 200, 1.0), (500, 300, 8.0)]
        .iter()
        .enumerate()
        .map(|(i, (status, latency, weight))| {
            json!({
                "ts": ts, "request_id": format!("sampled-{i}-{}", Uuid::new_v4().simple()),
                "model": model, "status": status, "latency_ms": latency,
                "total_tokens": 10, "prompt_tokens": 4, "completion_tokens": 6,
                "cost_usd": 0.5, "sample_weight": weight,
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
    assert_eq!(row["requests"].as_f64(), Some(10.0));
    assert_eq!(row["tokens"].as_f64(), Some(100.0));
    assert_eq!(row["cost_usd"].as_f64(), Some(5.0));
    assert_eq!(row["errors"].as_f64(), Some(8.0));
    // eight of the ten requests took 300 ms: the unweighted median of the three
    // stored rows would be 200
    assert_eq!(row["p50_latency_ms"].as_f64(), Some(300.0));
    assert_eq!(row["p95_latency_ms"].as_f64(), Some(300.0));
}

/// The summary's cache-hit count is weighted like the other counts and its p95
/// is read from the weighted rows (#1978, #3355). Both sit beside aliased aggregates in one select,
/// which a fake ClickHouse cannot tell from a real one that nests them.
#[tokio::test]
async fn summary_reports_weighted_cache_hits_and_an_overall_p95() {
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

    let model = format!("cache-{}", Uuid::new_v4().simple());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    // two kept rows weigh two requests each: one hit, one miss
    let rows: Vec<Value> = [(1, 100), (0, 300)]
        .iter()
        .enumerate()
        .map(|(i, (hit, latency))| {
            json!({
                "ts": ts, "request_id": format!("cache-{i}-{}", Uuid::new_v4().simple()),
                "model": model, "status": 200, "latency_ms": latency, "cache_hit": hit,
                "sample_weight": 2.0,
            })
        })
        .collect();
    insert_rows(&http, &ch, "request_logs", &rows).await;

    let response = http
        .get(format!(
            "http://{addr}/api/v1/analytics/summary?model={model}"
        ))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let row = &body["data"][0];
    assert_eq!(row["requests"].as_f64(), Some(4.0));
    assert_eq!(row["cache_hit_requests"].as_f64(), Some(2.0));
    // two requests at 100 ms and two at 300: the 95th percentile is the slow one
    assert_eq!(row["p95_latency_ms"].as_f64(), Some(300.0));
}

/// #3410: the prompt-cache read share is over the prompt tokens of the
/// requests whose provider stated its cache reads, so a provider that says
/// nothing is counted apart, as unmeasured, and does not dilute the share.
#[tokio::test]
async fn summary_measures_the_prompt_cache_over_the_requests_that_reported_it() {
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

    let model = format!("cache-share-{}", Uuid::new_v4().simple());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    // weight 2 each: a reporting provider read 40 of its 100 prompt tokens,
    // one that says nothing sent 300, and a request with no prompt is neither
    let rows: Vec<Value> = [(100, 40, 1), (300, 0, 0), (0, 0, 0)]
        .iter()
        .enumerate()
        .map(|(i, (prompt, read, reported))| {
            json!({
                "ts": ts, "request_id": format!("pc-{i}-{}", Uuid::new_v4().simple()),
                "model": model, "status": 200, "latency_ms": 10,
                "prompt_tokens": prompt, "cache_read_tokens": read,
                "cache_usage_reported": reported, "sample_weight": 2.0,
            })
        })
        .collect();
    insert_rows(&http, &ch, "request_logs", &rows).await;

    let response = http
        .get(format!(
            "http://{addr}/api/v1/analytics/summary?model={model}"
        ))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let row = &body["data"][0];
    assert_eq!(row["prompt_tokens"].as_f64(), Some(800.0), "{row}");
    assert_eq!(row["cache_read_tokens"].as_f64(), Some(80.0), "{row}");
    assert_eq!(
        row["cache_reported_prompt_tokens"].as_f64(),
        Some(200.0),
        "{row}"
    );
    assert_eq!(
        row["cache_unmeasured_requests"].as_f64(),
        Some(2.0),
        "{row}"
    );
}

/// A request that never got a status (`status = 0`) is an error in the totals
/// and in the LLM Logs `error` filter alike (#3294): the two screens must agree
/// about how many requests failed.
#[tokio::test]
async fn a_status_zero_request_counts_as_an_error_in_totals_and_in_the_error_filter() {
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

    let model = format!("status0-{}", Uuid::new_v4().simple());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    // a success, a transport failure, a 4xx and a 5xx
    let rows: Vec<Value> = [200, 0, 429, 502]
        .iter()
        .enumerate()
        .map(|(i, status)| {
            json!({
                "ts": ts, "request_id": format!("status0-{i}-{}", Uuid::new_v4().simple()),
                "model": model, "status": status, "latency_ms": 10,
            })
        })
        .collect();
    insert_rows(&http, &ch, "request_logs", &rows).await;

    let summary: Value = http
        .get(format!(
            "http://{addr}/api/v1/analytics/summary?model={model}"
        ))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(summary["data"][0]["requests"].as_f64(), Some(4.0));
    assert_eq!(summary["data"][0]["errors"].as_f64(), Some(3.0));

    let by_model: Value = http
        .get(format!("http://{addr}/api/v1/analytics/by-model"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = by_model["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["model"] == model.as_str())
        .expect("the model has a row");
    assert_eq!(row["errors"].as_f64(), Some(3.0));

    let listed: Value = http
        .get(format!(
            "http://{addr}/api/v1/analytics/invocations?model={model}&status=error&limit=100"
        ))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["data"].as_array().map(Vec::len), Some(3));
}

/// The invocation list's request ids for `token`, among `request_ids`, for one
/// query string, sorted.
async fn invocation_ids(
    client: &reqwest::Client,
    addr: SocketAddr,
    token: &str,
    query: &str,
    request_ids: &[&str],
) -> Vec<String> {
    let response = client
        .get(format!(
            "http://{addr}/api/v1/analytics/invocations?limit=200&{query}"
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "invocations {query}");
    let body: Value = response.json().await.unwrap();
    let mut ids: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["request_id"].as_str())
        .filter(|id| request_ids.contains(id))
        .map(str::to_string)
        .collect();
    ids.sort();
    ids
}

/// `withheld=true` and `usage_unknown=true` narrow the list on the flags the
/// gateway recorded (#3302), inside the caller's row scope: a member of one
/// project never sees another tenant's withheld row through the filter.
#[tokio::test]
async fn withheld_and_unknown_usage_filters_narrow_inside_the_callers_scope() {
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
    let mine = seed_tenant(&pool, "withheld-mine").await;
    let other = seed_tenant(&pool, "withheld-other").await;
    let member = seed_user(
        &pool,
        Some(mine.org),
        Some(mine.team),
        Some(mine.project_a),
        "member",
    )
    .await;

    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let tag = Uuid::new_v4().simple().to_string();
    let log = |label: &str, tenant: &Tenant, withheld: u8, usage_unknown: u8| {
        json!({
            "ts": ts, "request_id": format!("flag-{label}-{tag}"),
            "org_id": tenant.org.to_string(), "team_id": tenant.team.to_string(),
            "project_id": tenant.project_a.to_string(),
            "model": "fake-llm", "status": if withheld == 1 { 403 } else { 200 },
            "withheld": withheld, "usage_unknown": usage_unknown,
        })
    };
    insert_rows(
        &http,
        &ch,
        "request_logs",
        &[
            log("plain", &mine, 0, 0),
            log("withheld", &mine, 1, 0),
            log("unknown", &mine, 0, 1),
            log("theirs", &other, 1, 1),
        ],
    )
    .await;
    let id = |label: &str| format!("flag-{label}-{tag}");
    let all: Vec<String> = ["plain", "withheld", "unknown", "theirs"]
        .iter()
        .map(|l| id(l))
        .collect();
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    let ids = |labels: &[&str]| {
        let mut v: Vec<String> = labels.iter().map(|l| id(l)).collect();
        v.sort();
        v
    };

    // no filter: the member's three rows, never the other tenant's
    assert_eq!(
        invocation_ids(&http, addr, &member, "", &all).await,
        ids(&["plain", "unknown", "withheld"])
    );
    assert_eq!(
        invocation_ids(&http, addr, &member, "withheld=true", &all).await,
        ids(&["withheld"])
    );
    assert_eq!(
        invocation_ids(&http, addr, &member, "usage_unknown=true", &all).await,
        ids(&["unknown"])
    );
    // `false` is the filter left off
    assert_eq!(
        invocation_ids(&http, addr, &member, "withheld=false", &all).await,
        ids(&["plain", "unknown", "withheld"])
    );
    // the admin token sees the other tenant's row through the same filter
    assert_eq!(
        invocation_ids(&http, addr, ADMIN_TOKEN, "withheld=true", &all).await,
        ids(&["theirs", "withheld"])
    );
}

/// The request ids a caller's CSV export of the log carries, among `ids`.
async fn exported(
    client: &reqwest::Client,
    addr: SocketAddr,
    token: &str,
    ids: &[(String, String)],
) -> Vec<String> {
    let response = client
        .get(format!("http://{addr}/api/v1/analytics/invocations/export"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "export");
    let csv = response.text().await.unwrap();
    // metadata only: whatever the caller's role, no captured body is in the file
    assert!(
        !csv.contains("prompt-") && !csv.contains("completion-"),
        "{csv}"
    );
    let mut labels: Vec<String> = ids
        .iter()
        .filter(|(_, id)| csv.contains(id.as_str()))
        .map(|(label, _)| label.clone())
        .collect();
    labels.sort();
    labels
}

/// A caller exports exactly the rows they can list, never one outside their
/// scope (#1838): the export binds the same predicate as the JSON read.
#[tokio::test]
async fn the_export_returns_only_the_rows_the_caller_can_list() {
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

    let everything = exported(&http, addr, ADMIN_TOKEN, &ids).await;
    assert_eq!(everything.len(), 5, "{everything:?}");

    let project_member = seed_user(&pool, None, None, Some(acme.project_a), "member").await;
    assert_eq!(
        exported(&http, addr, &project_member, &ids).await,
        ["acme-a"]
    );
    let org_viewer = seed_user(&pool, Some(acme.org), None, None, "viewer").await;
    assert_eq!(
        exported(&http, addr, &org_viewer, &ids).await,
        ["acme-a", "acme-b"]
    );
    let team_member = seed_user(&pool, None, Some(umbrella.team), None, "member").await;
    assert_eq!(
        exported(&http, addr, &team_member, &ids).await,
        ["umbrella-a", "umbrella-b"]
    );
    // the rollup exports are scoped the same way: another org's spend is not in
    // a caller's chargeback file
    let rollup = http
        .get(format!(
            "http://{addr}/api/v1/analytics/by-attribution/export?include_unattributed=true"
        ))
        .bearer_auth(&project_member)
        .send()
        .await
        .unwrap();
    assert_eq!(rollup.status(), 200);
}

/// Requests that began inside one millisecond are listed in the order they
/// began, and paging through them neither repeats nor skips one (#1392). The
/// `ts` of all three is identical, their request ids sort the opposite way to
/// their start sequence, and a page holds one row, so every step of the walk
/// is decided by `seq` alone.
#[tokio::test]
async fn requests_in_one_millisecond_list_by_their_start_sequence() {
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
    let tenant = seed_tenant(&pool, "seq").await;
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let tag = Uuid::new_v4().simple().to_string();
    // started first to last: first (seq 10), second (seq 20), third (seq 30)
    let started = [("zzz-first", 10u64), ("mmm-second", 20), ("aaa-third", 30)];
    let rows: Vec<Value> = started
        .iter()
        .map(|(name, seq)| {
            json!({
                "ts": ts, "seq": seq, "request_id": format!("{name}-{tag}"),
                "org_id": tenant.org.to_string(), "team_id": tenant.team.to_string(),
                "project_id": tenant.project_a.to_string(),
                "model": "fake-llm", "status": 200,
            })
        })
        .collect();
    insert_rows(&http, &ch, "request_logs", &rows).await;

    let mut seen = Vec::new();
    let mut cursor = String::new();
    for _ in 0..5 {
        let response = http
            .get(format!("http://{addr}/api/v1/analytics/invocations"))
            .query(&[
                ("limit", "1"),
                ("model", "fake-llm"),
                ("cursor", cursor.as_str()),
            ])
            .bearer_auth(ADMIN_TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        let page = body["data"].as_array().unwrap();
        let Some(row) = page.first() else { break };
        if row["request_id"].as_str().unwrap().ends_with(&tag) {
            seen.push(
                row["request_id"]
                    .as_str()
                    .unwrap()
                    .replace(&format!("-{tag}"), ""),
            );
        }
        cursor = body["next_cursor"].as_str().unwrap().to_string();
    }
    // newest start first, each exactly once
    assert_eq!(seen, ["aaa-third", "mmm-second", "zzz-first"]);
}

/// The savings advisor's queries against the real engine (#1581). Its unit
/// tests answer them from a fake, which cannot show that ClickHouse resolves a
/// bare column name to a select alias first: a summed column read inside the
/// grouping key or inside another sum only fails on a real server.
#[tokio::test]
async fn the_savings_advisor_reads_real_request_logs() {
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
    let acme = seed_tenant(&pool, "savings").await;

    // a fresh route name, so rows other tests left in a shared clickhouse are
    // never counted
    let route = format!("savings-{}", Uuid::new_v4().simple());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let row = |status: u16, prompt: u32, completion: u32, cost: f64| {
        json!({
            "ts": ts, "request_id": format!("savings-{}", Uuid::new_v4().simple()),
            "org_id": acme.org.to_string(), "team_id": acme.team.to_string(),
            "project_id": acme.project_a.to_string(), "model": route, "target": "big-model",
            "status": status, "prompt_tokens": prompt, "completion_tokens": completion,
            "total_tokens": prompt + completion, "cost_usd": cost,
        })
    };
    // one small eligible request, one whose prompt is over the threshold, and
    // one that failed: three reasons in the grouping key
    insert_rows(
        &http,
        &ch,
        "request_logs",
        &[
            row(200, 100, 20, 1.0),
            row(200, 9000, 20, 2.0),
            row(500, 100, 0, 0.5),
        ],
    )
    .await;

    let response = http
        .get(format!("http://{addr}/api/v1/analytics/savings"))
        .query(&[("route", route.as_str())])
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let report: Value = response.json().await.unwrap_or(Value::Null);
    assert_eq!(status, 200, "{report}");
    assert_eq!(report["spend"]["requests"], 3.0, "{report}");
    assert_eq!(report["spend"]["cost_usd"], 3.5, "{report}");
}

/// The per-variant rollup (#3342) groups one route's requests by the variant
/// that served them, weighted by the sampling weight, and reads only what the
/// caller's roles reach: a project member sees their own project's share of a
/// route that two orgs both call by the same name.
#[tokio::test]
async fn by_variant_groups_one_route_inside_the_callers_scope() {
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
    let acme = seed_tenant(&pool, "variant-acme").await;
    let umbrella = seed_tenant(&pool, "variant-umbrella").await;

    let route = format!("variant-route-{}", Uuid::new_v4().simple());
    let other_route = format!("variant-other-{}", Uuid::new_v4().simple());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let row = |tenant: &Tenant,
               project: Uuid,
               model: &str,
               variant: &str,
               status: u32,
               latency: u32,
               cost: f64,
               weight: f64| {
        json!({
            "ts": ts, "request_id": format!("variant-{}", Uuid::new_v4().simple()),
            "org_id": tenant.org.to_string(), "team_id": tenant.team.to_string(),
            "project_id": project.to_string(),
            "model": model, "variant": variant, "status": status,
            "latency_ms": latency, "total_tokens": 10, "cost_usd": cost,
            "sample_weight": weight,
        })
    };
    let failed_over = |mut row: Value, reason: &str| {
        row["fallback_reason"] = json!(reason);
        row
    };
    insert_rows(
        &http,
        &ch,
        "request_logs",
        &[
            // acme project a: variant "canary" is sampled one in two, one of its two
            // kept rows failed; "stable" and the classic path ("") are unsampled
            row(&acme, acme.project_a, &route, "stable", 200, 100, 1.0, 1.0),
            // answered after another target failed over to it (#3409): counted in
            // `requests` and left out of the first attempts
            failed_over(
                row(&acme, acme.project_a, &route, "stable", 200, 300, 1.0, 1.0),
                "timeout",
            ),
            row(&acme, acme.project_a, &route, "canary", 200, 200, 0.5, 2.0),
            failed_over(
                row(&acme, acme.project_a, &route, "canary", 500, 200, 0.0, 2.0),
                "refusal:content_filter",
            ),
            row(&acme, acme.project_a, &route, "", 200, 100, 2.0, 1.0),
            // another project of the same org, and another org, call the route too
            row(&acme, acme.project_b, &route, "stable", 200, 100, 4.0, 1.0),
            row(
                &umbrella,
                umbrella.project_a,
                &route,
                "stable",
                200,
                100,
                8.0,
                1.0,
            ),
            // a different route's variant never joins this route's rows
            row(
                &acme,
                acme.project_a,
                &other_route,
                "stable",
                200,
                100,
                16.0,
                1.0,
            ),
        ],
    )
    .await;

    let read = |token: &'static str, query: String| {
        let http = http.clone();
        async move {
            let response = http
                .get(format!("http://{addr}/api/v1/analytics/by-variant?{query}"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            (response.status(), response.json::<Value>().await.unwrap())
        }
    };
    // rows are keyed on the org too (#3548), so a name is `org/variant` here
    let org_name = |r: &Value| {
        let org = r["org_id"].as_str().unwrap_or_default();
        if org == acme.org.to_string() {
            "acme"
        } else if org == umbrella.org.to_string() {
            "umbrella"
        } else {
            panic!("a row of an org this test did not seed: {r}")
        }
    };
    let by_variant = |body: &Value| -> Vec<(String, f64, f64, f64)> {
        let mut out: Vec<_> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    format!("{}/{}", org_name(r), r["variant"].as_str().unwrap()),
                    number(&r["requests"]),
                    number(&r["errors"]),
                    number(&r["cost_usd"]),
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    };

    // the admin token reads every org: requests and cost are weighted, errors count
    // the weight of the failed row, and the empty variant is the classic pool.
    // umbrella's route of the same name is its own row (#3548)
    let (status, body) = read(ADMIN_TOKEN, format!("model={route}")).await;
    assert_eq!(status, 200);
    assert_eq!(
        by_variant(&body),
        vec![
            ("acme/".to_string(), 1.0, 0.0, 2.0),
            ("acme/canary".to_string(), 4.0, 2.0, 1.0),
            ("acme/stable".to_string(), 3.0, 0.0, 6.0),
            ("umbrella/stable".to_string(), 1.0, 0.0, 8.0),
        ]
    );
    // the first attempts leave out the requests another target failed over to
    // the variant, weighted like every count (#3409): canary's failed row stands
    // for 2 of its 4, and one of stable's four rows is a failover
    let first_attempts = |body: &Value| -> Vec<(String, f64)> {
        let mut out: Vec<_> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    format!("{}/{}", org_name(r), r["variant"].as_str().unwrap()),
                    number(&r["first_attempt_requests"]),
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    };
    assert_eq!(
        first_attempts(&body),
        vec![
            ("acme/".to_string(), 1.0),
            ("acme/canary".to_string(), 2.0),
            ("acme/stable".to_string(), 2.0),
            ("umbrella/stable".to_string(), 1.0),
        ]
    );
    // the percentiles are read off the kept rows, not scaled
    let stable = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["variant"] == "stable" && org_name(r) == "acme")
        .unwrap();
    assert!(number(&stable["p50_latency_ms"]) >= 100.0);
    assert!(number(&stable["p95_latency_ms"]) <= 300.0);

    // a project member reads that project only, however the route is shared
    let member: &'static str = Box::leak(
        seed_user(&pool, None, None, Some(acme.project_a), "member")
            .await
            .into_boxed_str(),
    );
    let (status, body) = read(member, format!("model={route}")).await;
    assert_eq!(status, 200);
    assert_eq!(
        by_variant(&body),
        vec![
            ("acme/".to_string(), 1.0, 0.0, 2.0),
            ("acme/canary".to_string(), 4.0, 2.0, 1.0),
            ("acme/stable".to_string(), 2.0, 0.0, 2.0),
        ]
    );
    // the member's first attempts are their own project's, not the route's
    assert_eq!(
        first_attempts(&body),
        vec![
            ("acme/".to_string(), 1.0),
            ("acme/canary".to_string(), 2.0),
            ("acme/stable".to_string(), 1.0),
        ]
    );
    // and a project filter cannot reach past what the member already reads
    let (_, body) = read(member, format!("model={route}&project={}", acme.project_b)).await;
    assert!(body["data"].as_array().unwrap().is_empty());

    // without a route the rows carry their route, so no two routes' variants merge
    let (status, body) = read(ADMIN_TOKEN, String::new()).await;
    assert_eq!(status, 200);
    let both: std::collections::BTreeSet<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["variant"] == "stable")
        .filter_map(|r| r["model"].as_str())
        .filter(|m| *m == route || *m == other_route)
        .collect();
    assert_eq!(both.len(), 2, "{body}");

    // anonymous callers are refused before any query runs
    let anonymous = http
        .get(format!(
            "http://{addr}/api/v1/analytics/by-variant?model={route}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
}

/// The summary's mean latency is weighted by the row's sample weight like its
/// percentiles (#3420): a kept row counts as the requests it stands for, so a
/// slow request that was sampled one in four pulls the mean four times as hard
/// as an unsampled one, and a window with no rows reads 0 and not nan.
#[tokio::test]
async fn summary_mean_latency_is_weighted_by_the_row_weight() {
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

    let model = format!("mean-latency-{}", Uuid::new_v4().simple());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let row = |latency: u32, weight: f64| {
        json!({
            "ts": ts, "request_id": format!("mean-{}", Uuid::new_v4().simple()),
            "org_id": Uuid::new_v4().to_string(),
            "model": model, "status": 200, "latency_ms": latency,
            "total_tokens": 10, "cost_usd": 0.0, "sample_weight": weight,
        })
    };
    // 100 ms once and 400 ms three times over: the weighted mean is
    // (100 + 3 * 400) / 4 = 325, where the mean of the two kept rows is 250
    insert_rows(&http, &ch, "request_logs", &[row(100, 1.0), row(400, 3.0)]).await;

    let read = |model: String| {
        let http = http.clone();
        async move {
            let response = http
                .get(format!(
                    "http://{addr}/api/v1/analytics/summary?model={model}"
                ))
                .bearer_auth(ADMIN_TOKEN)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let body: Value = response.json().await.unwrap();
            body["data"][0].clone()
        }
    };
    let summary = read(model.clone()).await;
    assert_eq!(number(&summary["requests"]), 4.0, "{summary}");
    assert_eq!(number(&summary["avg_latency_ms"]), 325.0, "{summary}");

    // equal weights are the plain mean
    let even = format!("mean-even-{}", Uuid::new_v4().simple());
    let even_row = |latency: u32| {
        let mut r = row(latency, 2.0);
        r["model"] = json!(even);
        r
    };
    insert_rows(&http, &ch, "request_logs", &[even_row(100), even_row(300)]).await;
    assert_eq!(number(&read(even).await["avg_latency_ms"]), 200.0);

    // no rows at all: a zero, not nan
    let none = read(format!("mean-none-{}", Uuid::new_v4().simple())).await;
    assert_eq!(number(&none["avg_latency_ms"]), 0.0, "{none}");
}

/// The summary carries the weighted prompt-cache read tokens beside the prompt
/// tokens (#3356), so a read share is one division, and a row logged before the
/// provider reported any reads as zero.
#[tokio::test]
async fn summary_reports_weighted_prompt_cache_reads_beside_prompt_tokens() {
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

    let model = format!("promptcache-{}", Uuid::new_v4().simple());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    // each kept row stands for three requests: 600 of 1000 prompt tokens were read
    // from the cache on one, none on the other
    let rows: Vec<Value> = [(1000, 600), (1000, 0)]
        .iter()
        .enumerate()
        .map(|(i, (prompt, read))| {
            json!({
                "ts": ts, "request_id": format!("pc-{i}-{}", Uuid::new_v4().simple()),
                "model": model, "status": 200, "prompt_tokens": prompt,
                "cache_read_tokens": read, "sample_weight": 3.0,
            })
        })
        .collect();
    insert_rows(&http, &ch, "request_logs", &rows).await;

    let response = http
        .get(format!(
            "http://{addr}/api/v1/analytics/summary?model={model}"
        ))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let row = &body["data"][0];
    assert_eq!(number(&row["prompt_tokens"]), 6000.0);
    assert_eq!(number(&row["cache_read_tokens"]), 1800.0);
}

/// The by-model rows `token` reads for `model`, as `(org_id, requests)`, sorted.
async fn by_model_rows(
    client: &reqwest::Client,
    addr: SocketAddr,
    token: &str,
    model: &str,
) -> Vec<(String, f64)> {
    let response = client
        .get(format!(
            "http://{addr}/api/v1/analytics/by-model?model={model}"
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "by-model as {token}");
    let body: Value = response.json().await.unwrap();
    let mut rows: Vec<(String, f64)> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            assert_eq!(row["model"], model, "{body}");
            let requests = match &row["requests"] {
                Value::String(text) => text.parse().unwrap(),
                other => other.as_f64().unwrap(),
            };
            (
                row["org_id"].as_str().unwrap_or_default().to_string(),
                requests,
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// #3486: two orgs may each have a route named `gpt-4o` (#1857). Grouped by the
/// bare name, a superadmin read them as one row whose figures were summed
/// across tenants. Grouped by `(org_id, model)` they are two rows, each naming
/// its org, and a member of one org still reads only its own.
#[tokio::test]
async fn by_model_keeps_two_orgs_same_named_routes_as_two_rows() {
    let ch = skip_without_stack!();
    let http = reqwest::Client::new();
    ensure_schema(&http, &ch).await;

    let db = TestSchema::create(&test_database::url().await.unwrap()).await;
    let pool = db.pool().clone();
    // the app builder migrates the schema, so it comes before the seeding
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

    // one name, unique to this run, served in both orgs: twice in acme, three
    // times in umbrella
    let model = format!("shared-{}", Uuid::new_v4().simple());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let rows: Vec<Value> = [(&acme, 2), (&umbrella, 3)]
        .iter()
        .flat_map(|(tenant, count)| {
            (0..*count).map(|i| {
                json!({
                    "ts": ts,
                    "request_id": format!("shared-{i}-{}", Uuid::new_v4().simple()),
                    "org_id": tenant.org.to_string(), "team_id": tenant.team.to_string(),
                    "project_id": tenant.project_a.to_string(),
                    "model": model, "status": 200, "latency_ms": 10,
                })
            })
        })
        .collect();
    insert_rows(&http, &ch, "request_logs", &rows).await;

    let mut both = vec![(acme.org.to_string(), 2.0), (umbrella.org.to_string(), 3.0)];
    both.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        by_model_rows(&http, addr, ADMIN_TOKEN, &model).await,
        both,
        "the deployment admin reads each org's route as its own row"
    );

    let acme_viewer = seed_user(&pool, Some(acme.org), None, None, "viewer").await;
    assert_eq!(
        by_model_rows(&http, addr, &acme_viewer, &model).await,
        vec![(acme.org.to_string(), 2.0)],
        "a member of acme never reads umbrella's row"
    );
    let umbrella_member = seed_user(&pool, None, None, Some(umbrella.project_a), "member").await;
    assert_eq!(
        by_model_rows(&http, addr, &umbrella_member, &model).await,
        vec![(umbrella.org.to_string(), 3.0)],
        "nor a member of umbrella acme's"
    );
}

/// The by-variant rows `token` reads for `model`, as `(org_id, variant,
/// requests, assigned_requests)`, sorted.
async fn by_variant_rows(
    client: &reqwest::Client,
    addr: SocketAddr,
    token: &str,
    model: &str,
) -> Vec<(String, String, f64, f64)> {
    let response = client
        .get(format!(
            "http://{addr}/api/v1/analytics/by-variant?model={model}"
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "by-variant as {token}");
    let body: Value = response.json().await.unwrap();
    let mut rows: Vec<_> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            assert_eq!(row["model"], model, "{body}");
            (
                row["org_id"].as_str().unwrap_or_default().to_string(),
                row["variant"].as_str().unwrap_or_default().to_string(),
                number(&row["requests"]),
                number(&row["assigned_requests"]),
            )
        })
        .collect();
    rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    rows
}

/// #3548: as by-model since #3486, two orgs' routes of one name, with variants
/// of one name, are two sets of by-variant rows for a caller who reads both,
/// in the answered and the assigned halves alike, and a member of one org
/// reads only its own.
#[tokio::test]
async fn by_variant_keeps_two_orgs_same_named_routes_as_two_sets_of_rows() {
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
    let acme = seed_tenant(&pool, "variant-org-acme").await;
    let umbrella = seed_tenant(&pool, "variant-org-umbrella").await;

    let model = format!("shared-variants-{}", Uuid::new_v4().simple());
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let row = |tenant: &Tenant, first: &str, answered: &str| {
        let mut row = json!({
            "ts": ts, "request_id": format!("shared-variant-{}", Uuid::new_v4().simple()),
            "org_id": tenant.org.to_string(), "team_id": tenant.team.to_string(),
            "project_id": tenant.project_a.to_string(),
            "model": model, "variant": answered, "first_variant": first,
            "status": 200, "latency_ms": 10,
        });
        if first != answered {
            row["fallback_reason"] = json!("timeout");
        }
        row
    };
    insert_rows(
        &http,
        &ch,
        "request_logs",
        &[
            // acme: two requests for "a", one dialled to "b" that failed over to "a"
            row(&acme, "a", "a"),
            row(&acme, "a", "a"),
            row(&acme, "b", "a"),
            // umbrella: one request for "a", one for "b"
            row(&umbrella, "a", "a"),
            row(&umbrella, "b", "b"),
        ],
    )
    .await;

    let (acme_org, umbrella_org) = (acme.org.to_string(), umbrella.org.to_string());
    let acme_rows = vec![
        (acme_org.clone(), "a".to_string(), 3.0, 2.0),
        (acme_org.clone(), "b".to_string(), 0.0, 1.0),
    ];
    let umbrella_rows = vec![
        (umbrella_org.clone(), "a".to_string(), 1.0, 1.0),
        (umbrella_org.clone(), "b".to_string(), 1.0, 1.0),
    ];
    let mut both: Vec<_> = acme_rows.iter().chain(&umbrella_rows).cloned().collect();
    both.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    assert_eq!(
        by_variant_rows(&http, addr, ADMIN_TOKEN, &model).await,
        both,
        "the deployment admin reads each org's variants as their own rows"
    );

    let acme_viewer = seed_user(&pool, Some(acme.org), None, None, "viewer").await;
    assert_eq!(
        by_variant_rows(&http, addr, &acme_viewer, &model).await,
        acme_rows,
        "a member of acme never reads umbrella's rows"
    );
    let umbrella_member = seed_user(&pool, None, None, Some(umbrella.project_a), "member").await;
    assert_eq!(
        by_variant_rows(&http, addr, &umbrella_member, &model).await,
        umbrella_rows,
        "nor a member of umbrella acme's"
    );
}

#[tokio::test]
async fn semantic_truncation_outcomes_are_counted_per_provider() {
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

    let tag = Uuid::new_v4().simple().to_string();
    let first = format!("injection-{tag}");
    let second = format!("toxicity-{tag}");
    let ts = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();
    let row = |suffix: &str, provider: &str| {
        json!({
            "ts": ts,
            "request_id": format!("semantic-truncation-{tag}-{suffix}"),
            "model": "fake-llm",
            "status": 200,
            "guardrail_events": [format!("semantic_truncated:{provider}")],
        })
    };
    insert_rows(
        &http,
        &ch,
        "request_logs",
        &[row("a", &first), row("b", &first), row("c", &second)],
    )
    .await;

    let response = http
        .get(format!("http://{addr}/api/v1/analytics/guardrails"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["recorded"], true);
    let rows = body["data"].as_array().expect("outcome rows");
    let count = |provider: &str| {
        rows.iter()
            .find(|row| row["action"] == "semantic_truncated" && row["rule"] == provider)
            .unwrap_or_else(|| panic!("no semantic_truncated row for {provider}: {body}"))
            ["requests"]
            .clone()
    };
    assert_eq!(count(&first), json!(2));
    assert_eq!(count(&second), json!(1));
}
