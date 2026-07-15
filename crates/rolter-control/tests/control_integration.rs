//! End-to-end control-plane integration tests against a real Postgres.
//!
//! Gated on the `postgres` feature and the `ROLTER_TEST_DATABASE_URL` env var:
//! when the var is unset (local runs without a database) the tests self-skip.
//! CI provides a Postgres service and the var, so they run there.
#![cfg(feature = "postgres")]

use std::net::SocketAddr;

use serde_json::{json, Value};

fn database_url() -> Option<String> {
    std::env::var("ROLTER_TEST_DATABASE_URL").ok()
}

/// Connect, reset `public` to a clean slate, and return a control-plane app
/// router (migrations applied by `test_app`).
async fn fresh_app() -> axum::Router {
    let url = database_url().expect("ROLTER_TEST_DATABASE_URL checked by caller");
    let pool = rolter_store::postgres::connect(&url)
        .await
        .expect("connect");
    // wipe the schema (including sqlx bookkeeping) so every run migrates fresh
    sqlx::query("drop schema public cascade")
        .execute(&pool)
        .await
        .expect("reset schema");
    sqlx::query("create schema public")
        .execute(&pool)
        .await
        .expect("recreate schema");
    rolter_control::test_app(pool).await.expect("build app")
}

/// Serve `app` on an ephemeral port and return its address.
async fn serve(app: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

macro_rules! skip_without_db {
    () => {
        if database_url().is_none() {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        }
    };
}

#[tokio::test]
async fn ping_and_healthz_respond() {
    skip_without_db!();
    let addr = serve(fresh_app().await).await;
    let client = reqwest::Client::new();

    let health = client
        .get(format!("http://{addr}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);

    let ping: Value = client
        .get(format!("http://{addr}/api/v1/ping"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ping["pong"], true);
}

#[tokio::test]
async fn snapshot_served_on_empty_store() {
    skip_without_db!();
    let addr = serve(fresh_app().await).await;

    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/internal/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["version"].is_number(),
        "snapshot missing version: {body}"
    );
    assert!(
        body["config"].is_object(),
        "snapshot missing config: {body}"
    );
}

#[tokio::test]
async fn crud_create_round_trip_reflects_in_snapshot() {
    skip_without_db!();
    let addr = serve(fresh_app().await).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // helper: POST json, assert 2xx, return the parsed body
    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    // org → team → project hierarchy
    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id");

    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().expect("team id");

    let project = post(
        &client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().expect("project id");

    // provider under the org
    let provider = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/providers"),
        json!({"name": "openai", "kind": "openai", "api_base": "https://api.openai.com"}),
    )
    .await;
    let provider_id = provider["id"].as_str().expect("provider id");

    // route under the project, plus a target pointing at the provider
    let route = post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    let route_id = route["id"].as_str().expect("route id");

    post(
        &client,
        format!("{base}/api/v1/routes/{route_id}/targets"),
        json!({"provider_id": provider_id, "weight": 1}),
    )
    .await;

    // the snapshot the gateway polls must now reflect the new provider + route
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(
        snap["version"].as_u64().unwrap_or(0) > 0,
        "version should bump after writes: {snap}"
    );
    let providers = snap["config"]["providers"].as_array().expect("providers");
    assert!(
        providers.iter().any(|p| p["name"] == "openai"),
        "provider missing from snapshot: {snap}"
    );
    let routes = snap["config"]["routes"].as_array().expect("routes");
    assert!(
        routes.iter().any(|r| r["model"] == "gpt-4o"),
        "route missing from snapshot: {snap}"
    );
}

/// Provider credentials posted to the API must be sealed at rest, decrypted
/// into the gateway snapshot, and never leak through the dashboard config
/// endpoint. Runs in its own process (nextest), so setting the KEK env var
/// here cannot race other tests.
#[tokio::test]
async fn provider_api_key_seals_at_rest_and_decrypts_into_snapshot() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", "integration-test-kek");

    let url = database_url().unwrap();
    let pool = rolter_store::postgres::connect(&url).await.unwrap();
    sqlx::query("drop schema public cascade")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("create schema public")
        .execute(&pool)
        .await
        .unwrap();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().expect("org id");

    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
        .json(&json!({
            "name": "openai",
            "kind": "openai",
            "api_base": "https://api.openai.com",
            "api_key": "sk-live-secret",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = provider["id"].as_str().expect("provider id");

    // at rest: sealed, not plaintext
    let ciphertext: Vec<u8> =
        sqlx::query_scalar("select ciphertext from provider_keys where provider_id = $1::uuid")
            .bind(provider_id)
            .fetch_one(&pool)
            .await
            .expect("provider_keys row must exist");
    assert!(
        !String::from_utf8_lossy(&ciphertext).contains("sk-live-secret"),
        "credential must not be stored in plaintext"
    );

    // gateway snapshot: decrypted and usable
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        snap["config"]["providers"][0]["api_key"], "sk-live-secret",
        "snapshot must carry the decrypted key: {snap}"
    );

    // dashboard config: redacted
    let config_body = client
        .get(format!("{base}/api/v1/config"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !config_body.contains("sk-live-secret"),
        "config endpoint must redact provider keys"
    );

    // rotate via PUT, then clear with an empty string
    let updated = client
        .put(format!("{base}/api/v1/providers/{provider_id}"))
        .json(&json!({"api_key": "sk-rotated", "api_base": "https://eu.api.openai.com"}))
        .send()
        .await
        .unwrap();
    assert!(updated.status().is_success(), "{}", updated.status());

    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snap["config"]["providers"][0]["api_key"], "sk-rotated");
    assert_eq!(
        snap["config"]["providers"][0]["api_base"],
        "https://eu.api.openai.com"
    );

    let cleared = client
        .put(format!("{base}/api/v1/providers/{provider_id}"))
        .json(&json!({"api_key": ""}))
        .send()
        .await
        .unwrap();
    assert!(cleared.status().is_success());

    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        snap["config"]["providers"][0]["api_key"].is_null(),
        "cleared key must drop from the snapshot: {snap}"
    );
}

/// With an admin token configured, the CRUD API and snapshot endpoint reject
/// unauthenticated calls and accept the bearer token.
#[tokio::test]
async fn admin_token_guards_crud_and_snapshot() {
    skip_without_db!();
    let url = database_url().unwrap();
    let pool = rolter_store::postgres::connect(&url).await.unwrap();
    sqlx::query("drop schema public cascade")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("create schema public")
        .execute(&pool)
        .await
        .unwrap();
    let app = rolter_control::test_app_with_admin_token(pool, Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let denied = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let denied_snapshot = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied_snapshot.status(), 401);

    let allowed = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("sekrit")
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap();
    assert!(allowed.status().is_success(), "{}", allowed.status());
}

/// End-to-end local-account login (ROL-32): seed a user with an argon2id hash
/// directly (no signup flow exists yet), then exercise login → `/auth/me` →
/// logout → the now-revoked token is rejected.
#[tokio::test]
async fn login_me_logout_round_trip() {
    skip_without_db!();
    let url = database_url().unwrap();
    let pool = rolter_store::postgres::connect(&url).await.unwrap();
    sqlx::query("drop schema public cascade")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("create schema public")
        .execute(&pool)
        .await
        .unwrap();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // seed a user the way `rolter-seed` does (same argon2id hashing call shape)
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::{PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon2::Argon2::default()
        .hash_password(b"correct horse battery staple", &salt)
        .unwrap()
        .to_string();
    sqlx::query("insert into users (email, password_hash, is_superadmin) values ($1, $2, true)")
        .bind("admin@example.com")
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();

    // wrong password is rejected
    let denied = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "admin@example.com", "password": "wrong"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    // /auth/me without a token is rejected
    let no_token = client
        .get(format!("{base}/api/v1/auth/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(no_token.status(), 401);

    // a made-up token is rejected too (never matches a stored session hash)
    let bad_token = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth("rolter_sess_not-a-real-token")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_token.status(), 401);

    // correct credentials issue a session token
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "admin@example.com", "password": "correct horse battery staple"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().expect("token").to_string();
    assert_eq!(login["user"]["email"], "admin@example.com");

    // the token resolves the current user via the CurrentUser extractor
    let me: Value = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["user"]["email"], "admin@example.com");
    assert!(me["memberships"].is_array());

    // logout revokes the session; a repeat logout is a no-op (idempotent)
    let logout = client
        .post(format!("{base}/api/v1/auth/logout"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 204);
    let logout_again = client
        .post(format!("{base}/api/v1/auth/logout"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(logout_again.status(), 204);

    // the revoked token no longer resolves a session
    let after_logout = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(after_logout.status(), 401);
}

/// An expired session row is rejected even though the token digest matches,
/// proving `find_active_by_hash`'s `expires_at > now()` bound is doing its job.
#[tokio::test]
async fn expired_session_is_rejected() {
    skip_without_db!();
    let url = database_url().unwrap();
    let pool = rolter_store::postgres::connect(&url).await.unwrap();
    sqlx::query("drop schema public cascade")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("create schema public")
        .execute(&pool)
        .await
        .unwrap();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let user_id: uuid::Uuid = sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, $2, true) returning id",
    )
    .bind("expired@example.com")
    .bind("unused-hash")
    .fetch_one(&pool)
    .await
    .unwrap();

    // insert an already-expired session directly, bypassing login, with a
    // digest matching what the extractor computes for an empty pepper
    let token = "rolter_sess_deadbeef";
    let token_hash = rolter_auth::hash_key("", token);
    sqlx::query(
        "insert into sessions (user_id, token_hash, expires_at) values ($1, $2, now() - interval '1 hour')",
    )
    .bind(user_id)
    .bind(&token_hash)
    .execute(&pool)
    .await
    .unwrap();

    let resp = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

/// Seed a local user directly (no signup flow) and return its id.
async fn seed_user(pool: &sqlx::PgPool, email: &str, is_superadmin: bool) -> uuid::Uuid {
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
async fn seed_membership(
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
async fn seed_session(pool: &sqlx::PgPool, user_id: uuid::Uuid, suffix: &str) -> String {
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

/// With an admin token configured (RBAC enforcement active), every control
/// mutation is checked against the caller's role at the resource's scope:
/// viewers are denied, scoped admins are allowed only within their scope,
/// cross-scope admins are denied, superadmins bypass, and the machine admin
/// token bypasses. Covers ROL-33 (resolver) + ROL-34 (enforcement).
#[tokio::test]
async fn rbac_enforced_on_every_mutation() {
    skip_without_db!();
    let url = database_url().unwrap();
    let pool = rolter_store::postgres::connect(&url).await.unwrap();
    sqlx::query("drop schema public cascade")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("create schema public")
        .execute(&pool)
        .await
        .unwrap();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // bootstrap the org hierarchy as the machine admin token (superadmin)
    async fn post_as(client: &reqwest::Client, url: &str, token: &str, body: Value) -> Value {
        let resp = client
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org_a = post_as(
        &client,
        &format!("{base}/api/v1/orgs"),
        "admintok",
        json!({"name": "OrgA", "slug": "org-a"}),
    )
    .await;
    let org_a_id = org_a["id"].as_str().unwrap().to_string();
    let org_b = post_as(
        &client,
        &format!("{base}/api/v1/orgs"),
        "admintok",
        json!({"name": "OrgB", "slug": "org-b"}),
    )
    .await;
    let org_b_id = org_b["id"].as_str().unwrap().to_string();

    let org_a_uuid: uuid::Uuid = org_a_id.parse().unwrap();
    let org_b_uuid: uuid::Uuid = org_b_id.parse().unwrap();

    // seed principals: a viewer and an admin on org A, an admin on org B, and a
    // superadmin with no memberships at all
    let viewer = seed_user(&pool, "viewer@example.com", false).await;
    seed_membership(&pool, viewer, Some(org_a_uuid), None, None, "viewer").await;
    let viewer_token = seed_session(&pool, viewer, "viewer").await;

    let admin_a = seed_user(&pool, "admin-a@example.com", false).await;
    seed_membership(&pool, admin_a, Some(org_a_uuid), None, None, "admin").await;
    let admin_a_token = seed_session(&pool, admin_a, "admina").await;

    let admin_b = seed_user(&pool, "admin-b@example.com", false).await;
    seed_membership(&pool, admin_b, Some(org_b_uuid), None, None, "admin").await;
    let admin_b_token = seed_session(&pool, admin_b, "adminb").await;

    let super_user = seed_user(&pool, "super@example.com", true).await;
    let super_token = seed_session(&pool, super_user, "super").await;

    let create_team_url = format!("{base}/api/v1/orgs/{org_a_id}/teams");

    // unauthenticated → 401 (RBAC enforcement is active)
    let unauth = client
        .post(&create_team_url)
        .json(&json!({"name": "T"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401, "no credentials must be rejected");

    // viewer on org A → 403 creating a team (mutation needs admin)
    let viewer_denied = client
        .post(&create_team_url)
        .bearer_auth(&viewer_token)
        .json(&json!({"name": "T-viewer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(viewer_denied.status(), 403, "viewer must not create");

    // admin on org A → allowed on org A
    let admin_ok = client
        .post(&create_team_url)
        .bearer_auth(&admin_a_token)
        .json(&json!({"name": "T-admin"}))
        .send()
        .await
        .unwrap();
    assert!(
        admin_ok.status().is_success(),
        "org-A admin must create under org A: {}",
        admin_ok.status()
    );

    // admin on org B → 403 creating a provider under org A (cross-scope)
    let cross_scope = client
        .post(format!("{base}/api/v1/orgs/{org_a_id}/providers"))
        .bearer_auth(&admin_b_token)
        .json(&json!({"name": "p1", "kind": "openai", "api_base": "https://api.openai.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross_scope.status(),
        403,
        "org-B admin must not mutate org-A resources"
    );

    // superadmin session → bypass, allowed even with no membership
    let super_ok = client
        .post(&create_team_url)
        .bearer_auth(&super_token)
        .json(&json!({"name": "T-super"}))
        .send()
        .await
        .unwrap();
    assert!(
        super_ok.status().is_success(),
        "superadmin must bypass: {}",
        super_ok.status()
    );

    // global model-price catalog is superadmin-only: the org-A admin is denied
    let price_denied = client
        .put(format!("{base}/api/v1/model-prices"))
        .bearer_auth(&admin_a_token)
        .json(&json!({"model": "gpt-4o", "input_per_mtok": "1", "output_per_mtok": "2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        price_denied.status(),
        403,
        "model-price mutation is superadmin-only"
    );
    let price_ok = client
        .put(format!("{base}/api/v1/model-prices"))
        .bearer_auth("admintok")
        .json(&json!({"model": "gpt-4o", "input_per_mtok": "1", "output_per_mtok": "2"}))
        .send()
        .await
        .unwrap();
    assert!(price_ok.status().is_success(), "{}", price_ok.status());
}

/// Open mode (no admin token) must keep the CRUD API fully open: an
/// unauthenticated mutation still succeeds, preserving zero-cred local dev.
#[tokio::test]
async fn open_mode_allows_unauthenticated_mutations() {
    skip_without_db!();
    let addr = serve(fresh_app().await).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/orgs"))
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "open mode must allow unauthenticated mutations: {}",
        resp.status()
    );
}

/// Full user lifecycle (ROL-223): invite an account into an org, list it, grant
/// a team-scoped role, then deactivate it and confirm login is blocked while the
/// row and its memberships survive.
#[tokio::test]
async fn user_and_membership_lifecycle() {
    skip_without_db!();
    let addr = serve(fresh_app().await).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    // org → team scaffold
    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().unwrap().to_string();

    // invite a user into the org with an initial password + role
    let created = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/users"),
        json!({"email": "dev@example.com", "password": "hunter2!!", "role": "member"}),
    )
    .await;
    let user_id = created["user"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["user"]["email"], "dev@example.com");
    assert_eq!(created["user"]["is_superadmin"], false);
    // the password hash must never be serialized back
    assert!(created["user"].get("password_hash").is_none());
    assert_eq!(created["membership"]["role"], "member");

    // the account shows up in the org's user list
    let users: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/users"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        users.as_array().unwrap().iter().any(|u| u["id"] == user_id),
        "invited user missing from org list: {users}"
    );

    // duplicate email is a conflict
    let dup = client
        .post(format!("{base}/api/v1/orgs/{org_id}/users"))
        .json(&json!({"email": "dev@example.com", "password": "hunter2!!"}))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), 409);

    // grant a team-scoped admin role
    let membership = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/memberships"),
        json!({"user_id": user_id, "scope_type": "team", "scope_id": team_id, "role": "admin"}),
    )
    .await;
    let membership_id = membership["id"].as_str().unwrap().to_string();
    assert_eq!(membership["team_id"], team_id);

    // both memberships (org member + team admin) are listed for the org
    let memberships: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/memberships"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        memberships.as_array().unwrap().len(),
        2,
        "expected org + team memberships: {memberships}"
    );

    // the account can log in before deactivation
    let ok = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "dev@example.com", "password": "hunter2!!"}))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200, "login should succeed before deactivation");

    // deactivate the account
    let deact = client
        .put(format!("{base}/api/v1/users/{user_id}"))
        .json(&json!({"deactivated": true}))
        .send()
        .await
        .unwrap();
    assert!(deact.status().is_success());
    let deact_body: Value = deact.json().await.unwrap();
    assert!(deact_body["deactivated_at"].is_string());

    // login is now blocked, but the user + memberships still exist
    let blocked = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "dev@example.com", "password": "hunter2!!"}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 401, "deactivated account must not log in");
    let still: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/memberships"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(still.as_array().unwrap().len(), 2);

    // revoke the team membership, then delete the account
    let del_m = client
        .delete(format!("{base}/api/v1/memberships/{membership_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(del_m.status(), 204);
    let del_u = client
        .delete(format!("{base}/api/v1/users/{user_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(del_u.status(), 204);

    // the org user list is empty again (cascade removed the org membership too)
    let after: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/users"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        after.as_array().unwrap().is_empty(),
        "user should be gone after delete: {after}"
    );
}

/// Self-service key lifecycle (ROL-224): a logged-in member mints, lists,
/// rotates and deletes their own virtual keys, and usage 503s without
/// ClickHouse. Runs in open mode; `/me/*` still requires a real session.
#[tokio::test]
async fn self_service_key_lifecycle() {
    skip_without_db!();
    let addr = serve(fresh_app().await).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    // org → team → project
    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().unwrap().to_string();
    let project = post(
        &client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().unwrap().to_string();

    // invite a member into the org (org membership authorizes the project too)
    post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/users"),
        json!({"email": "member@example.com", "password": "hunter2!!", "role": "member"}),
    )
    .await;

    // log in to get a session token
    let login = post(
        &client,
        format!("{base}/api/v1/auth/login"),
        json!({"email": "member@example.com", "password": "hunter2!!"}),
    )
    .await;
    let token = login["token"].as_str().unwrap().to_string();

    // /me/* requires a session: unauthenticated is rejected
    let anon = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    // mint a key I own in the project I belong to
    let minted = client
        .post(format!(
            "{base}/api/v1/me/projects/{project_id}/virtual-keys"
        ))
        .bearer_auth(&token)
        .json(&json!({"name": "laptop", "models": ["gpt-4o"]}))
        .send()
        .await
        .unwrap();
    assert!(minted.status().is_success());
    let minted: Value = minted.json().await.unwrap();
    assert!(minted["key"].as_str().unwrap().starts_with("sk-rolter-"));
    let key_id = minted["id"].as_str().unwrap().to_string();

    // it shows up in my key list, enriched with project/org names
    let keys: Value = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = keys.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["project_name"], "Gateway");
    assert_eq!(arr[0]["org_name"], "Acme");
    // the key hash is never exposed on the self-service surface
    assert!(arr[0].get("key_hash").is_none());

    // rotate: a new secret, old key disabled, both still owned/listed
    let rotated = client
        .post(format!("{base}/api/v1/me/virtual-keys/{key_id}/rotate"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert!(rotated.status().is_success());
    let rotated: Value = rotated.json().await.unwrap();
    let new_id = rotated["id"].as_str().unwrap().to_string();
    assert_ne!(new_id, key_id);

    let after: Value = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let after = after.as_array().unwrap();
    assert_eq!(after.len(), 2);
    let old = after.iter().find(|k| k["id"] == key_id).unwrap();
    assert_eq!(old["disabled"], true, "rotated-out key must be disabled");

    // usage 503s without ClickHouse configured
    let usage = client
        .get(format!("{base}/api/v1/me/usage"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(usage.status(), 503);

    // delete the new key
    let del = client
        .delete(format!("{base}/api/v1/me/virtual-keys/{new_id}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 204);
    let remaining: Value = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(remaining.as_array().unwrap().len(), 1);
}
