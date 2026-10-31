//! Which scopes an MCP authorization request asks for (#1417): the resource's
//! own challenge first, the operator's configuration next, `scopes_supported`
//! only when nothing more specific exists, and a step-up that widens a live
//! grant instead of replacing it.

use axum::http::{header, StatusCode};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};

use crate::mcp::stub_authz;
use crate::support::*;

/// A protected resource at `/mcp` whose unauthenticated challenge carries
/// `challenge_scope` (when given) and whose metadata advertises `supported`.
async fn resource(authz: &str, challenge_scope: Option<&str>, supported: &[&str]) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/mcp");
    let metadata = json!({
        "resource": url,
        "authorization_servers": [authz],
        "scopes_supported": supported,
    });
    let mut challenge = format!(
        "Bearer resource_metadata=\"http://{addr}/.well-known/oauth-protected-resource/mcp\""
    );
    if let Some(scope) = challenge_scope {
        challenge.push_str(&format!(", scope=\"{scope}\""));
    }
    let app = Router::new()
        .route(
            "/mcp",
            get(move || {
                let challenge = challenge.clone();
                async move {
                    (
                        StatusCode::UNAUTHORIZED,
                        [(header::WWW_AUTHENTICATE, challenge)],
                    )
                }
            }),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(move || {
                let metadata = metadata.clone();
                async move { axum::Json(metadata) }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

struct Harness {
    client: reqwest::Client,
    base: String,
    token: String,
    org_id: String,
    user_id: uuid::Uuid,
    pool: sqlx::PgPool,
}

impl Harness {
    /// Register a server at `url` with the given scopes configuration and
    /// return its id.
    async fn server(&self, slug: &str, url: &str, required: &[&str], defaults: &[&str]) -> String {
        let server: Value = self
            .client
            .post(format!(
                "{}/api/v1/orgs/{}/mcp-servers",
                self.base, self.org_id
            ))
            .bearer_auth(admin_token())
            .json(&json!({
                "name": slug, "slug": slug, "url": url, "required_scopes": required,
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = server["id"].as_str().unwrap().to_string();
        let registered = self
            .client
            .put(format!(
                "{}/api/v1/mcp-servers/{id}/oauth-client",
                self.base
            ))
            .bearer_auth(admin_token())
            .json(&json!({"client_id": "rolter", "default_scopes": defaults}))
            .send()
            .await
            .unwrap();
        assert!(
            registered.status().is_success(),
            "{:?}",
            registered.text().await
        );
        id
    }

    async fn authorize(&self, id: &str, body: Value) -> (StatusCode, Value) {
        let response = self
            .client
            .post(format!(
                "{}/api/v1/mcp-servers/{id}/oauth/authorize",
                self.base
            ))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or_default())
    }

    /// The decoded `scope` parameter of the authorization URL a start returned.
    async fn requested(&self, id: &str, body: Value) -> Vec<String> {
        let (status, started) = self.authorize(id, body).await;
        assert_eq!(status, 200, "{started}");
        let url = started["authorization_url"].as_str().unwrap();
        url_param(url, "scope")
            .replace('+', " ")
            .replace("%20", " ")
            .replace("%3A", ":")
            .replace("%3a", ":")
            .split(' ')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }
}

async fn harness() -> (
    Harness,
    rolter_store::postgres::test_schema::TestSchema,
    String,
) {
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    let user_id = seed_local_user(&pool, "scopes@example.com", &password).await;
    let login = login_as(&client, &base, "scopes@example.com", &password).await;
    let token = login["token"].as_str().unwrap().to_string();
    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Scopes", "slug": "scopes"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let (authz, _stub) = stub_authz::serve_stub_with_metadata(true).await;
    (
        Harness {
            client,
            base,
            token,
            org_id,
            user_id,
            pool,
        },
        db,
        authz,
    )
}

#[tokio::test]
async fn the_resource_challenge_outranks_what_the_operator_configured() {
    skip_without_db!();
    let (h, _db, authz) = harness().await;
    let url = resource(
        &authz,
        Some("files:read files:write"),
        &["files:read", "admin"],
    )
    .await;
    let id = h
        .server("with-challenge", &url, &["audit"], &["configured"])
        .await;
    // the challenge's scopes, plus what the gateway needs the session to hold
    assert_eq!(
        h.requested(&id, json!({})).await,
        ["files:read", "files:write", "audit"]
    );
}

#[tokio::test]
async fn without_a_challenge_scope_the_configuration_beats_scopes_supported() {
    skip_without_db!();
    let (h, _db, authz) = harness().await;
    let url = resource(&authz, None, &["a", "b", "c"]).await;
    let id = h.server("configured", &url, &[], &["x", "y"]).await;
    assert_eq!(h.requested(&id, json!({})).await, ["x", "y"]);
}

#[tokio::test]
async fn scopes_supported_is_the_answer_only_when_nothing_is_configured() {
    skip_without_db!();
    let (h, _db, authz) = harness().await;
    let url = resource(&authz, None, &["a", "b"]).await;
    let id = h.server("supported", &url, &["req"], &[]).await;
    assert_eq!(h.requested(&id, json!({})).await, ["a", "b", "req"]);
}

#[tokio::test]
async fn an_explicit_list_is_used_exactly_and_a_bad_token_is_refused() {
    skip_without_db!();
    let (h, _db, authz) = harness().await;
    let url = resource(&authz, Some("from-challenge"), &["a"]).await;
    let id = h.server("explicit", &url, &["req"], &["cfg"]).await;
    assert_eq!(
        h.requested(&id, json!({"scopes": ["only"]})).await,
        ["only"]
    );
    let (status, _) = h.authorize(&id, json!({"scopes": ["bad\"quote"]})).await;
    assert_eq!(status, 400);
    let (status, _) = h
        .authorize(&id, json!({"step_up_scopes": ["has space"]}))
        .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_step_up_widens_the_live_grant_instead_of_replacing_it() {
    skip_without_db!();
    let (h, _db, authz) = harness().await;
    let url = resource(&authz, Some("ignored"), &[]).await;
    let id = h.server("step-up", &url, &["req"], &[]).await;
    let server_id: uuid::Uuid = id.parse().unwrap();
    rolter_store::postgres::repo::McpOAuthRepo(&h.pool)
        .upsert_grant(
            server_id,
            h.user_id,
            &["held".to_string(), "req".to_string()],
        )
        .await
        .unwrap();
    // what the grant holds, then the new scope, and the challenge plays no part
    assert_eq!(
        h.requested(&id, json!({"step_up_scopes": ["more"]})).await,
        ["held", "req", "more"]
    );
    // a scope already held is not asked for twice
    assert_eq!(
        h.requested(&id, json!({"step_up_scopes": ["held"]})).await,
        ["held", "req"]
    );
}
