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
