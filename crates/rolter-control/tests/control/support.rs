//! Helpers shared by more than one area: the schema and app fixtures, seeding,
//! and the stub servers.

use std::net::SocketAddr;

use rolter_store::postgres::test_schema::TestSchema;
use serde_json::{json, Value};

use rolter_store::postgres::test_database;

/// The one KEK every test in this file installs (#1351).
///
/// `std::env::set_var` is process-wide. The main test job runs `cargo nextest`,
/// which gives each test its own process, but the `coverage` job runs plain
/// `cargo test` — one binary, tests as threads, one environment — and
/// `Kek::from_env()` is read at *request* time. So a test setting a value of
/// its own is read by another test's in-flight request, and a sealed value
/// written under one key fails to open under the next. The symptom is never
/// local: it lands on whichever unrelated seal-then-open test was mid-flight.
///
/// Nothing here needs a *distinct* key, only *a* key — the one test that needs
/// a non-matching one builds it directly with `Kek::from_secret`, never through
/// the environment. So every call site installs this same value and the race
/// has nothing to observe.
pub const TEST_KEK: &str = "integration-test-kek";

/// The database this worktree owns. `ROLTER_TEST_DATABASE_URL` names the
/// server; the database itself is derived from the workspace path, so parallel
/// worktrees do not share one (#1430).
pub async fn database_url() -> Option<String> {
    test_database::url().await
}

/// Create a fresh isolated schema and return the guard owning it, so a test
/// gets its schema back when it finishes — including when it panics (#1364).
///
/// Bind the guard for the whole test: the schema goes with it. Take the pool
/// out with `db.pool().clone()`; tests that only need a router use
/// [`fresh_app`].
pub async fn fresh_db() -> TestSchema {
    let url = database_url()
        .await
        .expect("ROLTER_TEST_DATABASE_URL checked by caller");
    TestSchema::create(&url).await
}

/// Isolated schema + control-plane app router (migrations applied by
/// `test_app`). The schema guard comes back with the router and has to outlive
/// it, so bind it rather than dropping it on the floor.
pub async fn fresh_app() -> (axum::Router, TestSchema) {
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone())
        .await
        .expect("build app");
    (app, db)
}

/// Serve `app` on an ephemeral port and return its address.
pub async fn serve(app: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Serve a control-plane app that knows its own ephemeral address as the
/// deployment's public base URL (#1418).
///
/// The SSO and MCP OAuth flows derive their redirect URI from that URL, and it
/// used to arrive through `ROLTER_PUBLIC_URL`. The environment is process-wide:
/// under `cargo nextest` each test owns its process and that is harmless, but
/// the coverage job runs plain `cargo test`, where every test in this binary is
/// a thread sharing one environment — so one test's listener address became
/// another test's redirect URI, and the failure landed on whichever flow
/// happened to be mid-exchange. Binding the listener first and passing the
/// address into the app keeps each test's value its own.
pub async fn serve_with_public_url(pool: sqlx::PgPool, admin_token: Option<String>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app =
        rolter_control::test_app_with_public_url(pool, admin_token, &format!("http://{addr}"))
            .await
            .expect("build app");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// `POST /api/v1/auth/password` with `token`.
pub async fn post_password_change(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    current: &str,
    new: &str,
) -> reqwest::Response {
    client
        .post(format!("{base}/api/v1/auth/password"))
        .bearer_auth(token)
        .json(&json!({"current_password": current, "new_password": new}))
        .send()
        .await
        .unwrap()
}

/// The session token a password sign-in handed out.
pub async fn session_token(
    client: &reqwest::Client,
    base: &str,
    email: &str,
    password: &str,
) -> String {
    login_as(client, base, email, password).await["token"]
        .as_str()
        .expect("a session token")
        .to_string()
}

/// Seed a local user directly (no signup flow) and return its id.
pub async fn seed_user(pool: &sqlx::PgPool, email: &str, is_superadmin: bool) -> uuid::Uuid {
    sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, null, $2) returning id",
    )
    .bind(email)
    .bind(is_superadmin)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Grant `user` a role membership at an org/team/project scope (pass the ids
/// that apply; `None` for the levels that don't).
pub async fn seed_membership(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
    org: Option<uuid::Uuid>,
    team: Option<uuid::Uuid>,
    project: Option<uuid::Uuid>,
    role: &str,
) {
    sqlx::query(
        "insert into memberships (user_id, org_id, team_id, project_id, role)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(org)
    .bind(team)
    .bind(project)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
}

/// Mint a live session for `user` and return the opaque bearer token. The
/// digest is computed with the empty pepper the extractor uses when
/// `ROLTER_SESSION_PEPPER` is unset in tests.
pub async fn seed_session(pool: &sqlx::PgPool, user_id: uuid::Uuid, suffix: &str) -> String {
    let token = format!("rolter_sess_{suffix}");
    let token_hash = rolter_auth::hash_key("", &token);
    sqlx::query(
        "insert into sessions (user_id, token_hash, expires_at)
         values ($1, $2, now() + interval '1 hour')",
    )
    .bind(user_id)
    .bind(&token_hash)
    .execute(pool)
    .await
    .unwrap();
    token
}

/// Pull a query parameter out of a redirect URL.
pub fn url_param(url: &str, key: &str) -> String {
    url.split(['?', '&'])
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// who an audit row and a `created_by` column name (#2844)
// ---------------------------------------------------------------------------

/// The callers whose recorded identity differs: a superadmin session that holds
/// no membership anywhere (the operator `rolter-seed` creates), and an org
/// admin. The admin token is the third kind and needs no account.
pub struct Actors {
    pub operator: uuid::Uuid,
    pub operator_bearer: String,
    pub manager: uuid::Uuid,
    pub manager_bearer: String,
}

/// Seed the two accounts. The sessions are generated per run, so no test reuses
/// another's bearer string.
pub async fn seed_actors(pool: &sqlx::PgPool, org: uuid::Uuid) -> Actors {
    let operator = seed_user(pool, "operator@example.com", true).await;
    let manager = seed_user(pool, "manager@example.com", false).await;
    seed_membership(pool, manager, Some(org), None, None, "admin").await;
    // the premise of the bug: nothing on the membership list names the operator
    let operator_grants: i64 =
        sqlx::query_scalar("select count(*) from memberships where user_id = $1")
            .bind(operator)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(operator_grants, 0);
    let operator_bearer =
        seed_session(pool, operator, &uuid::Uuid::new_v4().simple().to_string()).await;
    let manager_bearer =
        seed_session(pool, manager, &uuid::Uuid::new_v4().simple().to_string()).await;
    Actors {
        operator,
        operator_bearer,
        manager,
        manager_bearer,
    }
}

pub async fn config_version(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("select version from config_version where id = 1")
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// TOTP second factor (#1078)
// ---------------------------------------------------------------------------

/// A password for one test account, generated per run rather than written out.
/// A literal here is a hard-coded credential to every scanner that reads this
/// file, and the tests need only that the password round-trips -- not that it
/// is any particular string.
pub fn random_password() -> String {
    format!("pw-{}", uuid::Uuid::new_v4())
}

/// The configured admin bearer token, generated once per test process rather
/// than written out; tests need only that the server and the client agree on it.
pub fn admin_token() -> &'static str {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(|| format!("tok-{}", uuid::Uuid::new_v4()))
}

/// Seed a local superadmin with a known password and return its id.
pub async fn seed_local_user(pool: &sqlx::PgPool, email: &str, password: &str) -> uuid::Uuid {
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, $2, true)
         returning id",
    )
    .bind(email)
    .bind(&hash)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Seed a non-superadmin local account as an admin of a fresh org whose
/// `mfa_policy` is `policy`, with the grace window `enforce_after` (a SQL
/// expression, so a test can say `now() + interval '7 days'` without a clock
/// of its own). Returns `(user_id, org_id)`.
pub async fn seed_bound_member(
    pool: &sqlx::PgPool,
    email: &str,
    password: &str,
    policy: &str,
    enforce_after: &str,
) -> (uuid::Uuid, uuid::Uuid) {
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, $2, false)
         returning id",
    )
    .bind(email)
    .bind(&hash)
    .fetch_one(pool)
    .await
    .unwrap();
    let org_id: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
            .bind(format!("org-{}", uuid::Uuid::new_v4().simple()))
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'admin')")
        .bind(user_id)
        .bind(org_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "insert into org_auth_policies (org_id, mfa_policy, mfa_enforce_after)
         values ($1, $2, {enforce_after})"
    )))
    .bind(org_id)
    .bind(policy)
    .execute(pool)
    .await
    .unwrap();
    (user_id, org_id)
}

/// Sign in with a password and return whatever the exchange answered.
pub async fn login_as(client: &reqwest::Client, base: &str, email: &str, password: &str) -> Value {
    let response = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "password sign-in");
    response.json().await.unwrap()
}

/// PUT a model price and return the status with the parsed body.
pub async fn put_model_price(
    client: &reqwest::Client,
    base: &str,
    body: Value,
) -> (reqwest::StatusCode, Value) {
    let resp = client
        .put(format!("{base}/api/v1/model-prices"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    (status, resp.json().await.unwrap())
}

/// The price of `model` as the gateway's snapshot carries it.
pub async fn snapshot_price(client: &reqwest::Client, base: &str, model: &str) -> Value {
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    snap["config"]["model_prices"]
        .as_array()
        .expect("model_prices")
        .iter()
        .find(|price| price["model"] == model)
        .unwrap_or_else(|| panic!("{model} is not priced in the snapshot: {snap}"))
        .clone()
}

pub async fn user_row(pool: &sqlx::PgPool, id: uuid::Uuid) -> Option<(bool, bool)> {
    sqlx::query_as("select is_superadmin, deactivated_at is not null from users where id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// An app with the admin token configured and one org, `acme`.
pub async fn people_fixture(pool: &sqlx::PgPool) -> (String, uuid::Uuid) {
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let base = format!("http://{addr}");
    let org: Value = reqwest::Client::new()
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (base, org["id"].as_str().unwrap().parse().unwrap())
}

pub async fn grant_id(pool: &sqlx::PgPool, holder: uuid::Uuid, org: uuid::Uuid) -> uuid::Uuid {
    sqlx::query_scalar("select id from memberships where user_id = $1 and org_id = $2")
        .bind(holder)
        .bind(org)
        .fetch_one(pool)
        .await
        .unwrap()
}

pub async fn org_admin_rows(
    pool: &sqlx::PgPool,
    holder: uuid::Uuid,
    org: uuid::Uuid,
) -> Vec<String> {
    sqlx::query_scalar(
        "select source from memberships where user_id = $1 and org_id = $2 and role = 'admin' \
         order by source",
    )
    .bind(holder)
    .bind(org)
    .fetch_all(pool)
    .await
    .unwrap()
}

pub async fn last_admin_kept_audits(pool: &sqlx::PgPool, org: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "select count(*) from audit_log where org_id = $1 and action = 'membership.last_admin_kept'",
    )
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap()
}
