//! MCP OAuth grants, consent, discovery and static credentials.

use crate::support::*;
use serde_json::{json, Value};

/// MCP OAuth grants and sessions (#541): admins see the whole org, a member
/// sees only what they own, revoking a grant kills its sessions, and no token
/// material ever appears in a response.
#[tokio::test]
async fn mcp_oauth_grants_and_sessions_are_owner_scoped_and_revocable() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "McpOrg", "slug": "mcp-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();

    // a bad transport or a non-http url is refused before anything is stored
    for bad in [
        json!({"name": "S", "slug": "s", "url": "https://mcp.example.com", "transport": "carrier-pigeon"}),
        json!({"name": "S", "slug": "s", "url": "file:///etc/passwd"}),
    ] {
        let resp = client
            .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
            .bearer_auth(admin_token())
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "accepted {bad}");
    }

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Docs",
            "slug": "docs",
            "url": "https://mcp.example.com"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(server["transport"], "streamable_http");
    let server_uuid: uuid::Uuid = server["id"].as_str().unwrap().parse().unwrap();
    let server: Value = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_uuid}"))
        .bearer_auth(admin_token())
        .json(&json!({"required_scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(server["required_scopes"], json!(["tools:read"]));

    // two members of the same org, each with their own grant and session
    let alice = seed_user(&pool, "alice@example.com", false).await;
    seed_membership(&pool, alice, Some(org_uuid), None, None, "member").await;
    let alice_token = seed_session(&pool, alice, "alice").await;
    let bob = seed_user(&pool, "bob@example.com", false).await;
    seed_membership(&pool, bob, Some(org_uuid), None, None, "member").await;
    let bob_token = seed_session(&pool, bob, "bob").await;

    use rolter_store::postgres::repo::McpSessionMaterial;

    let kek = rolter_store::postgres::crypto::Kek::from_secret("test-kek");
    let repo = rolter_store::postgres::repo::McpOAuthRepo(&pool);
    let alice_grant = repo
        .upsert_grant(server_uuid, alice, &["tools:read".to_string()])
        .await
        .unwrap();
    let bob_grant = repo
        .upsert_grant(server_uuid, bob, &["tools:read".to_string()])
        .await
        .unwrap();
    let excessive = repo
        .store_session(
            &kek,
            alice_grant.id,
            McpSessionMaterial {
                access_token: "must-not-store",
                refresh_token: None,
                scopes: &["tools:write".to_string()],
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                refresh_expires_at: None,
            },
        )
        .await;
    assert!(excessive.is_err(), "session exceeded the consent grant");
    let alice_session = repo
        .store_session(
            &kek,
            alice_grant.id,
            McpSessionMaterial {
                access_token: "at-alice-secret",
                refresh_token: Some("rt-alice-secret"),
                scopes: &["tools:read".to_string()],
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                refresh_expires_at: Some(chrono::Utc::now() + chrono::Duration::days(30)),
            },
        )
        .await
        .unwrap();
    repo.store_session(
        &kek,
        bob_grant.id,
        McpSessionMaterial {
            access_token: "at-bob-secret",
            refresh_token: None,
            scopes: &["tools:read".to_string()],
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            refresh_expires_at: None,
        },
    )
    .await
    .unwrap();

    // the protected snapshot carries only the newest live, scope-valid session
    // for each user/server pair and opens access tokens with the deployment KEK
    let snapshot_store =
        rolter_store::PostgresConfigStore::with_kek(pool.clone(), Some(kek.clone()));
    let snapshot = rolter_store::ConfigStore::load(&snapshot_store)
        .await
        .unwrap();
    assert_eq!(snapshot.mcp_servers.len(), 1);
    assert_eq!(snapshot.mcp_servers[0].required_scopes, ["tools:read"]);
    assert_eq!(snapshot.mcp_oauth_sessions.len(), 2);
    assert!(snapshot
        .mcp_oauth_sessions
        .iter()
        .any(|session| session.user_id == alice.to_string()
            && session.access_token == "at-alice-secret"));

    // the admin token sees both owners
    let all: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(all.as_array().unwrap().len(), 2);

    // a member sees only their own grant and session
    let mine: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mine = mine.as_array().unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["user_id"], alice.to_string());
    assert_eq!(mine[0]["active"], true);

    let sessions: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/sessions"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sessions_text = sessions.to_string();
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["has_refresh_token"], true);
    // the response describes the session without ever carrying its tokens
    assert!(
        !sessions_text.contains("at-alice-secret") && !sessions_text.contains("rt-alice-secret"),
        "token material leaked into the sessions response: {sessions_text}"
    );

    // one member may not revoke another's session
    let denied = client
        .delete(format!("{base}/api/v1/mcp/sessions/{}", alice_session.id))
        .bearer_auth(&bob_token)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);

    // a member of a different org cannot even see it
    let other_org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "OtherMcpOrg", "slug": "other-mcp-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_uuid: uuid::Uuid = other_org["id"].as_str().unwrap().parse().unwrap();
    let outsider = seed_user(&pool, "outsider@example.com", false).await;
    seed_membership(&pool, outsider, Some(other_uuid), None, None, "admin").await;
    let outsider_token = seed_session(&pool, outsider, "outsider").await;
    let cross_tenant = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth(&outsider_token)
        .send()
        .await
        .unwrap();
    assert_eq!(cross_tenant.status(), 403);
    let cross_revoke = client
        .delete(format!("{base}/api/v1/mcp/grants/{}", alice_grant.id))
        .bearer_auth(&outsider_token)
        .send()
        .await
        .unwrap();
    assert_eq!(cross_revoke.status(), 403);

    // the sealed tokens open while the grant is live
    let opened = repo
        .open_session(&kek, alice_session.id, chrono::Utc::now())
        .await
        .unwrap()
        .expect("live session opens");
    assert_eq!(opened.access_token, "at-alice-secret");
    assert_eq!(opened.refresh_token.as_deref(), Some("rt-alice-secret"));

    // revoking the grant revokes its sessions in the same breath
    let revoked: Value = client
        .delete(format!("{base}/api/v1/mcp/grants/{}", alice_grant.id))
        .bearer_auth(&alice_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(revoked["active"], false);
    let after: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/sessions"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(after[0]["revoked_at"].is_string());
    // and the tokens stop opening, so withdrawn consent cannot be spent
    assert!(repo
        .open_session(&kek, alice_session.id, chrono::Utc::now())
        .await
        .unwrap()
        .is_none());

    // an expired session is dead even though nobody revoked it
    let expired = repo
        .store_session(
            &kek,
            bob_grant.id,
            McpSessionMaterial {
                access_token: "at-bob-expired",
                refresh_token: None,
                scopes: &["tools:read".to_string()],
                expires_at: chrono::Utc::now() - chrono::Duration::minutes(1),
                refresh_expires_at: None,
            },
        )
        .await
        .unwrap();
    assert!(repo
        .open_session(&kek, expired.id, chrono::Utc::now())
        .await
        .unwrap()
        .is_none());

    // and a KEK that did not seal the token cannot open it
    let wrong = rolter_store::postgres::crypto::Kek::from_secret("not-the-kek");
    let bob_live = repo
        .list_sessions(org_uuid, Some(bob))
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id != expired.id)
        .unwrap();
    assert!(repo
        .open_session(&wrong, bob_live.id, chrono::Utc::now())
        .await
        .is_err());

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'mcp_oauth_grant.revoke' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("mcp_oauth_grant.revoke"));
}

/// Revoking an MCP consent records who revoked it in `revoked_by` and in the
/// audit row (#2844). A superadmin revokes a member's grant without holding a
/// membership, and the column used to stay empty for them.
#[tokio::test]
async fn an_mcp_grant_revocation_records_the_superadmin_session_that_made_it() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let acme: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let actors = seed_actors(&pool, acme).await;
    let owner = seed_user(&pool, "owner@example.com", false).await;
    seed_membership(&pool, owner, Some(acme), None, None, "member").await;
    let server: uuid::Uuid = sqlx::query_scalar(
        "insert into mcp_servers (org_id, name, slug, url) \
         values ($1, 'Docs', 'docs', 'https://mcp.example.com') returning id",
    )
    .bind(acme)
    .fetch_one(&pool)
    .await
    .unwrap();

    let callers = [
        (
            "operator",
            actors.operator_bearer.clone(),
            Some(actors.operator),
        ),
        (
            "manager",
            actors.manager_bearer.clone(),
            Some(actors.manager),
        ),
        ("admin-token", admin_token().to_string(), None),
    ];
    for (who, bearer, expected) in &callers {
        // a live grant per round: the partial unique index allows one at a time
        let grant: uuid::Uuid = sqlx::query_scalar(
            "insert into mcp_oauth_grants (server_id, user_id, scopes) \
             values ($1, $2, '{tools:read}') returning id",
        )
        .bind(server)
        .bind(owner)
        .fetch_one(&pool)
        .await
        .unwrap();
        let response = client
            .delete(format!("{base}/api/v1/mcp/grants/{grant}"))
            .bearer_auth(bearer)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "revoke as {who}");
        let revoked_by: Option<uuid::Uuid> =
            sqlx::query_scalar("select revoked_by from mcp_oauth_grants where id = $1")
                .bind(grant)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(&revoked_by, expected, "revoked_by for {who}");
        let audited: Option<uuid::Uuid> = sqlx::query_scalar(
            "select actor_user_id from audit_log \
             where action = 'mcp_oauth_grant.revoke' and target_id = $1",
        )
        .bind(grant)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(&audited, expected, "audit row for {who}");
    }
}

/// A stub OAuth 2.0 authorization server for the MCP consent flow (#707).
/// It records the last form it was posted so PKCE, scope and grant type can be
/// asserted, and its next answer is set by the test.
/// A server registered through the API is asked for its tools in the
/// background (#1283), which writes the row and so bumps `config_version`. A
/// test that counts bumps waits for that to finish first, or it counts it.
pub(crate) async fn wait_for_discovery(pool: &sqlx::PgPool, id: uuid::Uuid) {
    for _ in 0..200 {
        let attempted: bool = sqlx::query_scalar(
            "select tools_discovery_attempted_at is not null from mcp_servers where id = $1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
        if attempted {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("background discovery never finished");
}

/// The config version once background discovery (#1283) has stopped writing:
/// consent starts one, and it bumps the version whenever it lands. A test that
/// counts the bumps one of its own edits causes must not start counting while
/// that is still in flight.
async fn settled_config_version(pool: &sqlx::PgPool) -> i64 {
    let mut seen = config_version(pool).await;
    let mut quiet = 0;
    while quiet < 6 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let now = config_version(pool).await;
        if now == seen {
            quiet += 1;
        } else {
            seen = now;
            quiet = 0;
        }
    }
    seen
}

pub(crate) mod stub_authz {
    use super::*;
    use axum::Router;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    pub struct Stub {
        /// json body (and status) the next token request receives
        pub next: Arc<Mutex<(u16, Value)>>,
        /// form body of the last token request
        pub last_form: Arc<Mutex<String>>,
        /// how many token requests have been served
        pub calls: Arc<Mutex<u32>>,
    }

    impl Default for Stub {
        fn default() -> Self {
            Self {
                next: Arc::new(Mutex::new((200, json!({})))),
                last_form: Arc::new(Mutex::new(String::new())),
                calls: Arc::new(Mutex::new(0)),
            }
        }
    }

    impl Stub {
        pub fn answer(&self, status: u16, body: Value) {
            *self.next.lock().unwrap() = (status, body);
        }
        pub fn form(&self) -> String {
            self.last_form.lock().unwrap().clone()
        }
        pub fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    pub async fn serve_stub() -> (String, Stub) {
        serve_stub_with_metadata(true).await
    }

    /// The same stub, plus the RFC 8414 metadata document discovery reads
    /// (#1347). `iss_supported` is what the metadata advertises, which is the
    /// row of the RFC 9207 §2.4 table a response with no `iss` is judged by.
    pub async fn serve_stub_with_metadata(iss_supported: bool) -> (String, Stub) {
        let stub = Stub::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let issuer = format!("http://{addr}");
        let metadata = json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "authorization_response_iss_parameter_supported": iss_supported,
            "response_types_supported": ["code"],
        });
        let app = Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                axum::routing::get(move || {
                    let metadata = metadata.clone();
                    async move { axum::Json(metadata) }
                }),
            )
            .route(
                "/token",
                axum::routing::post({
                    let stub = stub.clone();
                    move |body: String| {
                        let stub = stub.clone();
                        async move {
                            *stub.last_form.lock().unwrap() = body;
                            *stub.calls.lock().unwrap() += 1;
                            let (status, payload) = stub.next.lock().unwrap().clone();
                            (
                                axum::http::StatusCode::from_u16(status).unwrap(),
                                axum::Json(payload),
                            )
                        }
                    }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (issuer, stub)
    }
}

/// A stub MCP server that publishes RFC 9728 protected resource metadata
/// (#1347): an unauthenticated request is challenged with a
/// `resource_metadata` URL, and that document names the authorization server.
mod stub_resource {
    use super::*;
    use axum::Router;

    /// Serve a protected resource at `/mcp` whose metadata points at
    /// `authorization_server`. Returns the resource's canonical URI.
    pub async fn serve_stub(authorization_server: &str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let resource = format!("http://{addr}/mcp");
        let metadata = json!({
            "resource": resource,
            "authorization_servers": [authorization_server],
            "scopes_supported": ["tools:read", "tools:write"],
            "bearer_methods_supported": ["header"],
        });
        let challenge = format!(
            "Bearer resource_metadata=\"http://{addr}/.well-known/oauth-protected-resource/mcp\", \
             scope=\"tools:read\""
        );
        let app = Router::new()
            .route(
                "/mcp",
                axum::routing::get(move || {
                    let challenge = challenge.clone();
                    async move {
                        (
                            axum::http::StatusCode::UNAUTHORIZED,
                            [(axum::http::header::WWW_AUTHENTICATE, challenge)],
                        )
                    }
                }),
            )
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                axum::routing::get(move || {
                    let metadata = metadata.clone();
                    async move { axum::Json(metadata) }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        resource
    }
}

/// The MCP OAuth token-acquisition path (#707), end to end against a stub
/// authorization server: consent mints a grant plus a sealed session, a refresh
/// rotates it, a refused refresh revokes rather than loops, an on-behalf-of
/// exchange cannot widen the consent, and no token material ever appears in a
/// response.
#[tokio::test]
async fn mcp_oauth_consent_refresh_and_exchange() {
    skip_without_db!();
    let password = random_password();
    let client_secret = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (authz, stub) = stub_authz::serve_stub().await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "McpOrg", "slug": "mcp-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    // a member who will do the consenting
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("insert into users (email, password_hash) values ($1, $2) returning id")
            .bind("ada@example.com")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
        .bind(user_id)
        .bind(uuid::Uuid::parse_str(&org_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "ada@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Docs", "slug": "docs", "url": "https://mcp.example.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();

    // consent is impossible before an oauth client is registered
    let unregistered = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(unregistered.status(), 400);

    // a plaintext token endpoint on a non-loopback host is refused outright
    let plaintext = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(admin_token())
        .json(&json!({
            "authorize_url": "http://mcp.example.com/authorize",
            "token_url": "http://mcp.example.com/token",
            "client_id": "rolter"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(plaintext.status(), 400);

    // a member may not register the client; that is an admin decision
    let forbidden = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(&token)
        .json(&json!({
            "authorize_url": format!("{authz}/authorize"),
            "token_url": format!("{authz}/token"),
            "client_id": "rolter"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);

    let registered: Value = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(admin_token())
        .json(&json!({
            "authorize_url": format!("{authz}/authorize"),
            "token_url": format!("{authz}/token"),
            "client_id": "rolter",
            "client_secret": client_secret,
            "default_scopes": ["tools:read", "tools:write"],
            // this server publishes no metadata, so it is pinned to the
            // hand-configured endpoints and nothing is probed (#1347)
            "discovery": "manual"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(registered["has_client_secret"], true);
    assert_eq!(
        registered["redirect_uri"],
        format!("{base}/auth/mcp/callback")
    );
    let registered_text = registered.to_string();
    assert!(
        !registered_text.contains(&client_secret),
        "the client secret leaked into the api response: {registered_text}"
    );
    // and listing the servers must not carry it either
    let servers: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!servers.to_string().contains(&client_secret));

    // -- consent ------------------------------------------------------------

    let started: Value = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read", "tools:write"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    assert!(auth_url.starts_with(&format!("{authz}/authorize?response_type=code")));
    assert!(auth_url.contains("code_challenge_method=S256"));
    let state = url_param(&auth_url, "state");
    assert!(!state.is_empty());

    // the authorization server grants only the read scope of the two asked for
    stub.answer(
        200,
        json!({
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "tools:read"
        }),
    );
    let consented: Value = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-1&state={state}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session_id = consented["session_id"].as_str().unwrap().to_string();
    let grant_id = consented["grant_id"].as_str().unwrap().to_string();
    assert_eq!(consented["scopes"], json!(["tools:read"]));
    assert_eq!(consented["has_refresh_token"], true);
    let consented_text = consented.to_string();
    assert!(
        !consented_text.contains("access-1") && !consented_text.contains("refresh-1"),
        "token material leaked into the callback response: {consented_text}"
    );
    // the exchange used PKCE and the deployment-owned redirect uri
    let form = stub.form();
    assert!(form.contains("grant_type=authorization_code"));
    assert!(form.contains("code_verifier="));
    assert!(form.contains(&format!("client_secret={client_secret}")));

    // the same state cannot be redeemed twice
    let replayed = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-1&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        replayed.status(),
        400,
        "a redeemed state must not work twice"
    );

    // the sealed columns really are sealed
    let (access_ct, access_pt): (Vec<u8>, Option<String>) = sqlx::query_as(
        "select access_ciphertext, encode(access_ciphertext, 'escape') from mcp_oauth_sessions \
         where id = $1",
    )
    .bind(uuid::Uuid::parse_str(&session_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!access_ct.is_empty());
    assert!(
        !access_pt.unwrap_or_default().contains("access-1"),
        "the access token is stored in the clear"
    );

    // the session shows up on the sessions screen, without token material
    let sessions: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/sessions"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert!(!sessions.to_string().contains("access-1"));
    let grants: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(grants[0]["id"], grant_id);
    assert_eq!(grants[0]["scopes"], json!(["tools:read"]));

    // -- scope ceiling ------------------------------------------------------

    // an exchange may not ask for more than the grant carries
    let escalated = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/exchange"))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read", "tools:write"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        escalated.status(),
        400,
        "an exchange must not widen the consent"
    );
    let before_exchange = stub.calls();

    // an in-bounds exchange mints a second, independently revocable session
    stub.answer(
        200,
        json!({
            "access_token": "obo-access",
            "token_type": "Bearer",
            "expires_in": 600,
            "scope": "tools:read"
        }),
    );
    let exchanged: Value = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/exchange"))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read"], "audience": "https://downstream.example.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(exchanged["id"], json!(session_id));
    assert_eq!(exchanged["grant_id"], grant_id);
    assert_eq!(exchanged["scopes"], json!(["tools:read"]));
    assert!(!exchanged.to_string().contains("obo-access"));
    assert_eq!(
        stub.calls(),
        before_exchange + 1,
        "the refused exchange must not have reached the authorization server"
    );
    let form = stub.form();
    assert!(form.contains("token-exchange"));
    assert!(form.contains("subject_token"));
    assert!(form.contains("audience"));

    // -- refresh ------------------------------------------------------------

    // a rotated refresh token replaces the old one
    stub.answer(
        200,
        json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "token_type": "Bearer",
            "expires_in": 7200
        }),
    );
    let refreshed: Value = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        refreshed["id"],
        json!(session_id),
        "a refresh keeps the row"
    );
    assert_eq!(refreshed["revoked_at"], Value::Null);
    assert!(!refreshed.to_string().contains("refresh-2"));
    let form = stub.form();
    assert!(form.contains("grant_type=refresh_token"));
    assert!(form.contains("refresh_token=refresh-1"));

    // a transient failure leaves the session alone to be retried
    stub.answer(503, json!({"error": "temporarily_unavailable"}));
    let transient = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    // the authorization server having a bad minute is not the caller's mistake
    // (#2919): a 502 with a fixed sentence, not a 400
    assert_eq!(transient.status(), 502);
    let transient: Value = transient.json().await.unwrap();
    assert_eq!(
        transient["error"]["code"],
        "authorization_server_unavailable"
    );
    let still_live: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select revoked_at from mcp_oauth_sessions where id = $1")
            .bind(uuid::Uuid::parse_str(&session_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        still_live.is_none(),
        "a 503 is not a verdict on the grant; the session must survive it"
    );

    // an authorization server that cannot be reached at all (#2919): the error
    // `reqwest` raises quotes the token endpoint, which an operator may have
    // registered with a credential in its query, and the caller of a refresh
    // is not that operator
    let endpoint_secret = random_password();
    let unreachable_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let register_token_url = |token_url: String| {
        let client = client.clone();
        let url = format!("{base}/api/v1/mcp-servers/{server_id}/oauth-client");
        let authorize_url = format!("{authz}/authorize");
        async move {
            let updated = client
                .put(url)
                .bearer_auth(admin_token())
                .json(&json!({
                    "authorize_url": authorize_url,
                    "token_url": token_url,
                    "client_id": "rolter",
                    "discovery": "manual"
                }))
                .send()
                .await
                .unwrap();
            assert_eq!(updated.status(), 200, "{}", updated.text().await.unwrap());
        }
    };
    register_token_url(format!(
        "http://127.0.0.1:{unreachable_port}/token?client_secret={endpoint_secret}\
         &tenant={endpoint_secret}"
    ))
    .await;
    let unreachable = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(unreachable.status(), 502);
    let unreachable = unreachable.text().await.unwrap();
    assert!(
        !unreachable.contains(&endpoint_secret) && !unreachable.contains("127.0.0.1"),
        "the token endpoint reached the caller: {unreachable}"
    );
    assert!(unreachable.contains("authorization_server_unavailable"));
    register_token_url(format!("{authz}/token")).await;

    // and a fault in rolter's own store is rolter's, not a 400 quoting the
    // driver: here the sealed refresh token's nonce no longer opens it
    let session_uuid = uuid::Uuid::parse_str(&session_id).unwrap();
    let sealed_nonce: Vec<u8> =
        sqlx::query_scalar("select refresh_nonce from mcp_oauth_sessions where id = $1")
            .bind(session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("update mcp_oauth_sessions set refresh_nonce = $2 where id = $1")
        .bind(session_uuid)
        .bind(vec![1u8, 2, 3])
        .execute(&pool)
        .await
        .unwrap();
    let faulted = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(faulted.status(), 500);
    let faulted: Value = faulted.json().await.unwrap();
    assert_eq!(faulted["error"]["message"], "internal server error");
    for fragment in ["nonce", "decrypt", "provider key", "ROLTER_KEK"] {
        assert!(
            !faulted.to_string().contains(fragment),
            "{fragment} reached the caller: {faulted}"
        );
    }
    sqlx::query("update mcp_oauth_sessions set refresh_nonce = $2 where id = $1")
        .bind(session_uuid)
        .bind(sealed_nonce)
        .execute(&pool)
        .await
        .unwrap();

    // a refusal is final: the session is revoked rather than retried forever
    stub.answer(400, json!({"error": "invalid_grant"}));
    let refused = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let revoked_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select revoked_at from mcp_oauth_sessions where id = $1")
            .bind(uuid::Uuid::parse_str(&session_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        revoked_at.is_some(),
        "a refused refresh must revoke the session, not loop on it"
    );

    // and a revoked session is not renewable again
    let after_revoke = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(after_revoke.status(), 400);

    // the whole lifecycle is on the audit trail
    let actions: Vec<String> = sqlx::query_scalar(
        "select action from audit_log where action like 'mcp_oauth%' order by at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for expected in [
        "mcp_oauth_client.update",
        "mcp_oauth_grant.consent",
        "mcp_oauth_session.exchange",
        "mcp_oauth_session.refresh_refused",
    ] {
        assert!(
            actions.iter().any(|a| a == expected),
            "missing audit event {expected} in {actions:?}"
        );
    }
}

/// #2859: a signed-in superadmin who holds no membership anywhere is a person
/// and can consent for themselves; the grant belongs to their account. The
/// admin token and open mode are nobody, so they are still refused, and the
/// refusal names both.
#[tokio::test]
async fn a_superadmin_session_grants_mcp_consent_as_its_own_account() {
    skip_without_db!();
    let password = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (authz, stub) = stub_authz::serve_stub().await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "ConsentOrg", "slug": "consent-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Docs", "slug": "docs", "url": "https://mcp.example.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    let registered = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(admin_token())
        .json(&json!({
            "authorize_url": format!("{authz}/authorize"),
            "token_url": format!("{authz}/token"),
            "client_id": "rolter",
            "default_scopes": ["tools:read"],
            "discovery": "manual"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), 200);

    // the admin token is nobody: no account to own the grant
    let by_token = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(by_token.status(), 400);
    let refusal = by_token.text().await.unwrap();
    assert!(
        refusal.contains("admin token") && refusal.contains("open mode"),
        "the refusal names the two callers it turns away: {refusal}"
    );

    // a superadmin with a session and no membership anywhere
    let superadmin_id = seed_local_user(&pool, "root@example.com", &password).await;
    let memberships: i64 =
        sqlx::query_scalar("select count(*) from memberships where user_id = $1")
            .bind(superadmin_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(memberships, 0, "the account holds no role anywhere");
    let token = session_token(&client, &base, "root@example.com", &password).await;

    let started = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), 200, "a superadmin session may consent");
    let started: Value = started.json().await.unwrap();
    let state = url_param(started["authorization_url"].as_str().unwrap(), "state");

    stub.answer(
        200,
        json!({
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "tools:read"
        }),
    );
    let consented = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-1&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(consented.status(), 200);
    let consented: Value = consented.json().await.unwrap();
    let grant_id: uuid::Uuid = consented["grant_id"].as_str().unwrap().parse().unwrap();

    // the grant is owned by the signed-in account, not by nobody
    let owner: uuid::Uuid =
        sqlx::query_scalar("select user_id from mcp_oauth_grants where id = $1")
            .bind(grant_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(owner, superadmin_id);
    let started_by: Option<String> = sqlx::query_scalar(
        "select detail->>'user_id' from audit_log where action = 'mcp_oauth_grant.authorize'
         order by at desc limit 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(started_by, Some(superadmin_id.to_string()));

    // open mode has no account either
    let open_addr = serve_with_public_url(pool.clone(), None).await;
    let open = client
        .post(format!(
            "http://{open_addr}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(open.status(), 400);
    let refusal = open.text().await.unwrap();
    assert!(
        refusal.contains("open mode"),
        "open mode is refused by name: {refusal}"
    );
}

/// The three client-side MUSTs of the current MCP specification (#1347), end to
/// end: the authorization server is discovered from what the MCP server
/// publishes rather than typed in, both requests carry the RFC 8707 `resource`,
/// and the callback applies RFC 9207 §2.4 — including the two failures that
/// would otherwise be silent, a wrong `iss` and an absent one from a server
/// that advertises it.
#[tokio::test]
async fn mcp_oauth_discovers_its_authorization_server_and_validates_the_issuer() {
    skip_without_db!();
    let password = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    // deliberately *not* setting ROLTER_PUBLIC_URL: it is process-wide, and
    // under plain `cargo test` (the coverage job) one test's value is read by
    // another test's in-flight request. nothing here asserts on the redirect
    // uri, so whatever the deployment default is will do
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    // an authorization server that publishes metadata and says it returns `iss`
    let (authz, stub) = stub_authz::serve_stub_with_metadata(true).await;
    let resource = stub_resource::serve_stub(&authz).await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "DiscoOrg", "slug": "disco-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("insert into users (email, password_hash) values ($1, $2) returning id")
            .bind("grace@example.com")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
        .bind(user_id)
        .bind(uuid::Uuid::parse_str(&org_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "grace@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Disco", "slug": "disco", "url": resource}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();

    // a client id and nothing else: no endpoint is typed in, because the
    // server publishes where its authorization server is
    let registered: Value = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(admin_token())
        .json(&json!({"client_id": "rolter", "default_scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(registered["authorize_url"], Value::Null);
    assert_eq!(registered["token_url"], Value::Null);
    assert_eq!(registered["resource"], json!(resource));

    // one consent start, used three times over: each callback consumes its own
    // login state, so every case below asks for a fresh one
    let start = || async {
        let started: Value = client
            .post(format!(
                "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
            ))
            .bearer_auth(&token)
            .json(&json!({"scopes": ["tools:read"]}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        started
    };

    let started = start().await;
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    // the endpoint came from the authorization server's metadata, not from a
    // column an operator filled in
    assert!(
        auth_url.starts_with(&format!("{authz}/authorize?")),
        "authorization url must come from discovery: {auth_url}"
    );
    // RFC 8707 on the authorization request
    assert_eq!(
        url_param(&auth_url, "resource"),
        resource.replace(':', "%3A").replace('/', "%2F"),
        "the authorization request must carry the canonical resource: {auth_url}"
    );

    // -- RFC 9207: a wrong issuer ------------------------------------------
    let wrong = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-1&state={}&iss=https%3A%2F%2Fevil.example.com",
            url_param(&auth_url, "state")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 400, "a mismatched iss must be rejected");
    assert_eq!(
        stub.calls(),
        0,
        "the authorization code must never reach a token endpoint after an iss mismatch"
    );

    // -- RFC 9207: an absent issuer from a server that advertises one -------
    let started = start().await;
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    let absent = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-2&state={}",
            url_param(&auth_url, "state")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        absent.status(),
        400,
        "an absent iss must be rejected when the metadata advertises it"
    );
    assert_eq!(stub.calls(), 0);

    // -- and an error response whose issuer does not check out --------------
    let started = start().await;
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    let refused = client
        .get(format!(
            "{base}/auth/mcp/callback?error=access_denied&error_description=go-here-instead\
             &state={}&iss=https%3A%2F%2Fevil.example.com",
            url_param(&auth_url, "state")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let body = refused.text().await.unwrap();
    assert!(
        !body.contains("go-here-instead") && !body.contains("access_denied"),
        "an unvalidated error response must not be displayed: {body}"
    );

    // nothing above created a grant
    let grants: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(grants.as_array().map(Vec::len), Some(0), "{grants}");

    // -- the happy path -----------------------------------------------------
    let started = start().await;
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    stub.answer(
        200,
        json!({
            "access_token": "access-1",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "tools:read"
        }),
    );
    let consented = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-3&state={}&iss={}",
            url_param(&auth_url, "state"),
            authz.replace(':', "%3A").replace('/', "%2F")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        consented.status(),
        200,
        "{}",
        consented.text().await.unwrap()
    );
    // RFC 8707 on the token request, carrying the very same identifier
    let form = stub.form();
    assert!(
        form.contains(&format!(
            "resource={}",
            resource.replace(':', "%3A").replace('/', "%2F")
        )),
        "the token request must carry the same canonical resource: {form}"
    );

    // -- the hand-configured fallback still works ---------------------------
    // a server that publishes nothing at all: discovery fails against a dead
    // port and the operator's endpoints are used instead
    let quiet: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Quiet", "slug": "quiet", "url": "http://127.0.0.1:1/mcp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let quiet_id = quiet["id"].as_str().unwrap().to_string();
    let quiet_client = client
        .put(format!("{base}/api/v1/mcp-servers/{quiet_id}/oauth-client"))
        .bearer_auth(admin_token())
        .json(&json!({
            "authorize_url": format!("{authz}/authorize"),
            "token_url": format!("{authz}/token"),
            "client_id": "rolter"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(quiet_client.status(), 200);
    let started: Value = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{quiet_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    assert!(
        auth_url.starts_with(&format!("{authz}/authorize?")),
        "the configured endpoint must be the fallback: {auth_url}"
    );
    // the resource parameter is not conditional on discovery having worked
    assert_eq!(
        url_param(&auth_url, "resource"),
        "http%3A%2F%2F127.0.0.1%3A1%2Fmcp"
    );
    // nothing was discovered and no issuer was pinned, so a returned `iss` has
    // nothing authentic to be checked against and the exchange is refused
    let unverifiable = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-4&state={}&iss={}",
            url_param(&auth_url, "state"),
            authz.replace(':', "%3A").replace('/', "%2F")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(unverifiable.status(), 400);

    // while a response with no `iss` at all is row four of the table, and
    // proceeds exactly as it did before this flow knew about RFC 9207
    let started: Value = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{quiet_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    stub.answer(
        200,
        json!({"access_token": "access-2", "token_type": "Bearer", "expires_in": 3600}),
    );
    let legacy = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-5&state={}",
            url_param(&auth_url, "state")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(legacy.status(), 200, "{}", legacy.text().await.unwrap());
}

/// #2166: the authorization server sends the user's browser back to the
/// callback, and a browser is redirected to the dashboard rather than left on
/// a JSON body. Every outcome lands on Auth Sessions with non-secret
/// identifiers only — the session and server ids, or a failure code — and a
/// client that does not ask for HTML still gets the JSON the other tests read.
#[tokio::test]
async fn mcp_oauth_callback_sends_a_browser_to_the_dashboard() {
    skip_without_db!();
    let password = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    // what a browser does, minus following the redirect: the Location is the
    // assertion
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");
    let (authz, stub) = stub_authz::serve_stub().await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "LandOrg", "slug": "land-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("insert into users (email, password_hash) values ($1, $2) returning id")
            .bind("lin@example.com")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
        .bind(user_id)
        .bind(uuid::Uuid::parse_str(&org_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "lin@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();
    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Land", "slug": "land", "url": "https://mcp.example.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    let registered = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(admin_token())
        .json(&json!({
            "authorize_url": format!("{authz}/authorize"),
            "token_url": format!("{authz}/token"),
            "client_id": "rolter",
            "default_scopes": ["tools:read"],
            "discovery": "manual"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), 200);

    // a fresh login state per case: each callback consumes the one it names
    let start = || async {
        let started: Value = client
            .post(format!(
                "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
            ))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        url_param(started["authorization_url"].as_str().unwrap(), "state")
    };
    // the Accept header a browser navigates with
    let navigate = |query: String| {
        let request = browser
            .get(format!("{base}/auth/mcp/callback?{query}"))
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            );
        let landing = format!("{base}/auth-sessions?consent=");
        async move {
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), 303, "a browser is sent onwards");
            let location = response.headers()[reqwest::header::LOCATION]
                .to_str()
                .unwrap()
                .to_string();
            assert!(
                location.starts_with(&landing),
                "a browser lands on Auth Sessions: {location}"
            );
            location
        }
    };

    // -- success ------------------------------------------------------------
    let state = start().await;
    stub.answer(
        200,
        json!({
            "access_token": "access-land",
            "refresh_token": "refresh-land",
            "token_type": "Bearer",
            "expires_in": 3600
        }),
    );
    let landed = navigate(format!("code=code-land&state={state}")).await;
    let sessions: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/sessions"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session_id = sessions[0]["id"].as_str().unwrap();
    assert_eq!(
        landed,
        format!("{base}/auth-sessions?consent=completed&session={session_id}&server={server_id}")
    );
    // names the kind of value, never the value, so a failure cannot print one
    for (what, secret) in [
        ("authorization code", "code-land"),
        ("access token", "access-land"),
        ("refresh token", "refresh-land"),
        ("state", state.as_str()),
    ] {
        assert!(
            !landed.contains(secret),
            "the {what} leaked into the landing url"
        );
    }

    // -- a replay, and a callback with no state at all ----------------------
    let replayed = navigate(format!("code=code-land&state={state}")).await;
    assert_eq!(
        replayed,
        format!("{base}/auth-sessions?consent=failed&reason=state_invalid"),
        "before the state resolves there is no server to name"
    );
    let stateless = navigate("code=code-land".to_string()).await;
    assert!(stateless.ends_with("consent=failed&reason=state_invalid"));

    // -- the authorization server says no -----------------------------------
    let denied = navigate(format!(
        "error=access_denied&error_description=go-here-instead&state={}",
        start().await
    ))
    .await;
    assert_eq!(
        denied,
        format!("{base}/auth-sessions?consent=failed&reason=access_denied&server={server_id}")
    );
    assert!(
        !denied.contains("go-here-instead"),
        "the upstream's words stay upstream: {denied}"
    );
    let broken = navigate(format!("error=server_error&state={}", start().await)).await;
    assert!(broken.contains("reason=authorization_failed&server="));

    // -- an issuer nothing was pinned to check against ----------------------
    let calls = stub.calls();
    let mismatched = navigate(format!(
        "code=code-iss&state={}&iss=https%3A%2F%2Fevil.example.com",
        start().await
    ))
    .await;
    assert!(mismatched.contains("reason=issuer_mismatch&server="));
    assert_eq!(
        stub.calls(),
        calls,
        "the code never reached a token endpoint"
    );

    // -- the token endpoint refuses the code --------------------------------
    stub.answer(400, json!({"error": "invalid_grant"}));
    let exchange = navigate(format!("code=code-bad&state={}", start().await)).await;
    assert!(exchange.contains("reason=token_exchange_failed&server="));

    // -- and a client that does not ask for html keeps the json -------------
    let state = start().await;
    let json_refusal = browser
        .get(format!(
            "{base}/auth/mcp/callback?error=access_denied&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(json_refusal.status(), 400);
    assert_eq!(json_refusal.headers()[reqwest::header::VARY], "accept");
    let body: Value = json_refusal.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .is_some_and(|m| m.contains("access_denied")));

    // exactly one consent happened in all of that
    let grants: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(grants.as_array().map(Vec::len), Some(1), "{grants}");
}

/// The `oauth_discovered_*` cache of one server, plus whether
/// `oauth_discovered_at` is set — read as a boolean so the tuple needs no
/// timestamp type.
type DiscoveryCache = (Option<String>, Option<String>, Option<String>, bool, bool);

async fn discovery_cache(pool: &sqlx::PgPool, id: uuid::Uuid) -> DiscoveryCache {
    sqlx::query_as(
        "select oauth_discovered_issuer, oauth_discovered_authorize_url, \
                oauth_discovered_token_url, oauth_discovered_iss_supported, \
                oauth_discovered_at is not null \
         from mcp_servers where id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// #1416: the discovery cache is keyed by the server's URL — it holds the
/// endpoints of whatever authorization server that URL's protected-resource
/// metadata named. Pointing the row somewhere else must therefore drop it, or a
/// refresh landing before the next interactive authorize would post to the old
/// server's token endpoint while naming the new canonical URI in `resource`.
///
/// The second half is the constraint that makes the fix non-obvious:
/// `mcp_servers` has a statement-level `bump_config_version()` trigger, so the
/// clearing has to ride on the update that writes the URL rather than follow
/// it, and an edit that leaves the URL alone must not clear anything.
#[tokio::test]
async fn moving_an_mcp_server_url_invalidates_its_discovery_cache() {
    skip_without_db!();
    let password = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let (authz, _stub) = stub_authz::serve_stub_with_metadata(true).await;
    let resource = stub_resource::serve_stub(&authz).await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "MoveOrg", "slug": "move-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    // consent is a user-facing act, so the authorize below needs a member
    // session rather than the admin token
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("insert into users (email, password_hash) values ($1, $2) returning id")
            .bind("mallory@example.com")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
        .bind(user_id)
        .bind(uuid::Uuid::parse_str(&org_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mallory@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Movable", "slug": "movable", "url": resource}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    let server_uuid = uuid::Uuid::parse_str(&server_id).unwrap();
    wait_for_discovery(&pool, server_uuid).await;

    let registered = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(admin_token())
        .json(&json!({"client_id": "rolter", "default_scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), 200);

    // one interactive authorize is what fills the cache
    let started = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), 200, "{}", started.text().await.unwrap());
    let cached = discovery_cache(&pool, server_uuid).await;
    assert_eq!(
        cached,
        (
            Some(authz.clone()),
            Some(format!("{authz}/authorize")),
            Some(format!("{authz}/token")),
            true,
            true
        ),
        "discovery should have cached the stub authorization server"
    );

    // #1569: the row now holds `oauth_discovered_iss_supported = true` (the
    // fourth element above), so this is the point where the read endpoint can
    // be checked against it rather than against itself. an api client that
    // reads only `/oauth-client` needs the flag to explain why a callback
    // carrying no `iss` was rejected — it selects the RFC 9207 §2.4 row
    let view: Value = client
        .get(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        view["discovered_iss_supported"],
        json!(true),
        "the oauth-client view must report the discovered iss support: {view}"
    );
    assert_eq!(
        view["discovered_issuer"],
        json!(authz),
        "and the rest of the discovery cache alongside it: {view}"
    );

    // an edit that leaves the url where it is keeps the cache: re-discovering
    // on every rename would put an upstream probe on a path that has no reason
    // to touch one
    let before = settled_config_version(&pool).await;
    let renamed = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Renamed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status(), 200, "{}", renamed.text().await.unwrap());
    assert_eq!(
        discovery_cache(&pool, server_uuid).await,
        cached,
        "an edit that did not move the url must not invalidate the cache"
    );
    assert_eq!(
        config_version(&pool).await - before,
        1,
        "one edit must bump config_version exactly once"
    );

    // moving the url drops every discovered column, so the next refresh
    // re-discovers instead of posting to the previous server's token endpoint
    let before = settled_config_version(&pool).await;
    let moved = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .bearer_auth(admin_token())
        .json(&json!({"url": "http://127.0.0.1:1/mcp"}))
        .send()
        .await
        .unwrap();
    assert_eq!(moved.status(), 200, "{}", moved.text().await.unwrap());
    assert_eq!(
        discovery_cache(&pool, server_uuid).await,
        (None, None, None, false, false),
        "moving the url must clear the endpoints discovered for the old one"
    );
    // the clearing rides on the update that wrote the url; a second statement
    // would bump twice for one logical edit
    assert_eq!(
        config_version(&pool).await - before,
        1,
        "clearing the cache must not cost a second config_version bump"
    );

    // and with the cache gone the fallback is what an operator configured,
    // rather than an endpoint belonging to a server this row no longer names
    let quiet = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        quiet.status(),
        400,
        "nothing is discoverable at the new url and no endpoint was configured"
    );
}

/// #1432: re-pinning `oauth_issuer` (or flipping `oauth_discovery`) while
/// leaving the server's `url` alone must invalidate the discovery cache the
/// same way moving the url does — otherwise the cached triple from the old
/// authorization server shadows the correction on every path that never
/// probes (the background refresher here, chosen because it is exactly the
/// path #1432 names).
#[tokio::test]
async fn repinning_the_oauth_issuer_invalidates_its_discovery_cache() {
    skip_without_db!();
    let password = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // the discovered server, and a second one standing in for the
    // authorization server an operator rotates to
    let (authz, stub) = stub_authz::serve_stub_with_metadata(true).await;
    let (authz2, stub2) = stub_authz::serve_stub_with_metadata(true).await;
    let resource = stub_resource::serve_stub(&authz).await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "RepinOrg", "slug": "repin-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("insert into users (email, password_hash) values ($1, $2) returning id")
            .bind("repin@example.com")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
        .bind(user_id)
        .bind(uuid::Uuid::parse_str(&org_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "repin@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Repinnable", "slug": "repinnable", "url": resource}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    let server_uuid = uuid::Uuid::parse_str(&server_id).unwrap();
    wait_for_discovery(&pool, server_uuid).await;

    let registered = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(admin_token())
        .json(&json!({"client_id": "rolter", "default_scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), 200);

    // one interactive authorize discovers and caches the first server
    let started: Value = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    let state = url_param(&auth_url, "state");
    let cached_first = discovery_cache(&pool, server_uuid).await;
    assert_eq!(
        cached_first,
        (
            Some(authz.clone()),
            Some(format!("{authz}/authorize")),
            Some(format!("{authz}/token")),
            true,
            true
        ),
        "discovery should have cached the first stub authorization server"
    );

    // complete the login so there is a session with a refresh token to renew
    stub.answer(
        200,
        json!({
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "tools:read"
        }),
    );
    let consented_resp = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-1&state={state}&iss={}",
            authz.replace(':', "%3A").replace('/', "%2F")
        ))
        .send()
        .await
        .unwrap();
    let consented_status = consented_resp.status();
    let consented_text = consented_resp.text().await.unwrap();
    assert_eq!(consented_status, 200, "{consented_text}");
    let consented: Value = serde_json::from_str(&consented_text).unwrap();
    let session_id = consented["session_id"].as_str().unwrap().to_string();

    // an edit that touches neither the issuer nor the discovery mode must not
    // invalidate the cache — re-registering the same client with a rotated
    // secret is exactly this kind of edit
    let before = settled_config_version(&pool).await;
    let resecreted = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(admin_token())
        .json(&json!({
            "client_id": "rolter",
            "default_scopes": ["tools:read"],
            "client_secret": "rotated-secret"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resecreted.status(), 200);
    assert_eq!(
        discovery_cache(&pool, server_uuid).await,
        cached_first,
        "an edit that left the issuer and discovery mode alone must not invalidate the cache"
    );
    // an OAuth client is not something the gateway reads, so since 0115 this
    // edit bumps nothing; what must never happen is more than once
    assert!(
        config_version(&pool).await - before <= 1,
        "one edit must not bump config_version more than once"
    );

    // the operator rotates to a different authorization server: a new issuer
    // and fallback endpoints, discovery left on "auto"
    let before = settled_config_version(&pool).await;
    let repinned = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(admin_token())
        .json(&json!({
            "client_id": "rolter",
            "default_scopes": ["tools:read"],
            "issuer": authz2,
            "authorize_url": format!("{authz2}/authorize"),
            "token_url": format!("{authz2}/token")
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(repinned.status(), 200, "{}", repinned.text().await.unwrap());
    assert_eq!(
        discovery_cache(&pool, server_uuid).await,
        (None, None, None, false, false),
        "re-pinning the issuer must clear the endpoints discovered for the old one"
    );
    // the clearing rides on the update that wrote the issuer; a second
    // statement would bump config_version twice for one logical edit
    assert!(
        config_version(&pool).await - before <= 1,
        "clearing the cache must not cost a second config_version bump"
    );

    // a refresh never probes (#1432): with the cache gone it must fall back to
    // the newly configured endpoint rather than the stale cached one
    let calls_before = (stub.calls(), stub2.calls());
    stub2.answer(
        200,
        json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "token_type": "Bearer",
            "expires_in": 3600
        }),
    );
    let refreshed: Value = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(refreshed["id"], json!(session_id));
    assert_eq!(
        (stub.calls(), stub2.calls()),
        (calls_before.0, calls_before.1 + 1),
        "the refresh must reach the corrected authorization server, not the stale cached one"
    );
}

/// Cross-tenant isolation on the exchange path: a member of another org may not
/// refresh or exchange a session they do not own, and the answer is a 404 —
/// whether a session exists elsewhere is not something to probe for.
#[tokio::test]
async fn mcp_oauth_sessions_are_not_reachable_across_owners() {
    skip_without_db!();
    let password = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "OwnerOrg", "slug": "owner-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = uuid::Uuid::parse_str(org["id"].as_str().unwrap()).unwrap();

    use argon2::password_hash::PasswordHasher;
    let mut tokens = Vec::new();
    let mut ids = Vec::new();
    for email in ["owner@example.com", "other@example.com"] {
        let hash = argon2::Argon2::default()
            .hash_password(password.as_bytes())
            .unwrap()
            .to_string();
        let id: uuid::Uuid = sqlx::query_scalar(
            "insert into users (email, password_hash) values ($1, $2) returning id",
        )
        .bind(email)
        .bind(&hash)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
            .bind(id)
            .bind(org_id)
            .execute(&pool)
            .await
            .unwrap();
        let login: Value = client
            .post(format!("{base}/api/v1/auth/login"))
            .json(&json!({"email": email, "password": password}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        tokens.push(login["token"].as_str().unwrap().to_string());
        ids.push(id);
    }

    // a grant + session owned by the first user, written directly: this test is
    // about the guard, not about the exchange
    let server_id: uuid::Uuid = sqlx::query_scalar(
        "insert into mcp_servers (org_id, name, slug, url) \
         values ($1, 'Docs', 'docs', 'https://mcp.example.com') returning id",
    )
    .bind(org_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let grant_id: uuid::Uuid = sqlx::query_scalar(
        "insert into mcp_oauth_grants (server_id, user_id, scopes) \
         values ($1, $2, '{tools:read}') returning id",
    )
    .bind(server_id)
    .bind(ids[0])
    .fetch_one(&pool)
    .await
    .unwrap();
    let session_id: uuid::Uuid = sqlx::query_scalar(
        "insert into mcp_oauth_sessions (grant_id, access_ciphertext, access_nonce, scopes, \
                expires_at) \
         values ($1, '\\x00', '\\x00', '{tools:read}', now() + interval '1 hour') returning id",
    )
    .bind(grant_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    for path in ["refresh", "exchange"] {
        let denied = client
            .post(format!("{base}/api/v1/mcp/sessions/{session_id}/{path}"))
            .bearer_auth(&tokens[1])
            .send()
            .await
            .unwrap();
        assert_eq!(
            denied.status(),
            403,
            "a co-tenant must not act on someone else's session via {path}"
        );
    }
}

/// A static MCP credential is sealed at rest, never comes back out of the read
/// API, and the auth kind and the credential columns cannot disagree (#952).

#[tokio::test]
async fn mcp_static_credential_seals_at_rest_and_never_reads_back() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);

    let db = fresh_db().await;
    let pool = db.pool().clone();
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
    let org_id = org["id"].as_str().unwrap().to_string();

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .json(&json!({
            "name": "Search",
            "slug": "search",
            "url": "https://mcp.example.com/mcp",
            "transport": "streamable_http",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    assert_eq!(
        server["auth_kind"], "none",
        "a server registers unauthenticated until told otherwise: {server}"
    );
    assert_eq!(server["has_credential"], false);

    const TOKEN: &str = "mcp-upstream-token-value";
    let armed: Value = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "bearer", "credential": TOKEN}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(armed["auth_kind"], "bearer");
    assert_eq!(armed["has_credential"], true);
    assert!(
        !armed.to_string().contains(TOKEN),
        "the credential must not come back in the write response"
    );

    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !listed.to_string().contains(TOKEN),
        "the credential must not come back in the read API"
    );

    // sealed at rest: the row holds ciphertext, and the plaintext appears
    // nowhere in the column
    let stored: (Option<Vec<u8>>, Option<Vec<u8>>) = sqlx::query_as(
        "select credential_ciphertext, credential_nonce from mcp_servers where id = $1::uuid",
    )
    .bind(&server_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let ciphertext = stored.0.expect("ciphertext stored");
    assert!(stored.1.is_some(), "nonce stored beside the ciphertext");
    assert!(
        !String::from_utf8_lossy(&ciphertext).contains(TOKEN),
        "the credential must not be recoverable from the stored bytes"
    );

    // the store unseals it for the gateway's snapshot, and the same load feeds
    // the anonymous config view, which must not pass it on (#1938)
    let snapshot = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        snapshot.contains(TOKEN),
        "the gateway needs the unsealed credential to reach the server"
    );
    let config_view = client
        .get(format!("{base}/api/v1/config"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !config_view.contains(TOKEN),
        "the config view handed out a static mcp credential"
    );
    assert!(
        !config_view.contains("mcp.example.com"),
        "the config view lists a tenant's mcp servers"
    );

    // renaming nothing but the kind keeps the stored credential: an operator
    // cannot read it back, so requiring a re-type would make it unchangeable
    let to_header: Value = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "header", "auth_header_name": "X-Api-Key"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(to_header["auth_kind"], "header");
    assert_eq!(to_header["auth_header_name"], "X-Api-Key");
    assert_eq!(to_header["has_credential"], true);

    // header mode must not be able to forge the bearer path
    let forged = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "header", "auth_header_name": "Authorization"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        forged.status(),
        400,
        "an api key must not be presentable as Authorization"
    );

    let malformed = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "header", "auth_header_name": "X Api Key"}))
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);

    let headerless = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "header"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        headerless.status(),
        400,
        "header mode without a header name is not a usable configuration"
    );

    // dropping to 'none' clears the credential rather than orphaning it, so
    // `rolter kek verify` is not left auditing a secret nothing can use
    let disarmed: Value = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "none"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(disarmed["auth_kind"], "none");
    assert_eq!(disarmed["has_credential"], false);
    let cleared: (Option<Vec<u8>>,) =
        sqlx::query_as("select credential_ciphertext from mcp_servers where id = $1::uuid")
            .bind(&server_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(cleared.0.is_none(), "the sealed credential must be gone");

    // and a kind that needs one cannot be armed without supplying it
    let empty_handed = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "bearer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(empty_handed.status(), 400);
}

/// Per-server transport overrides, and the null that gives one back to the org
/// default (#952).
#[tokio::test]
async fn mcp_transport_overrides_are_per_server_and_revertible() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
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
    let org_id = org["id"].as_str().unwrap().to_string();

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .json(&json!({
            "name": "Slow",
            "slug": "slow",
            "url": "https://mcp.example.com/mcp",
            "transport": "streamable_http",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    assert!(
        server["request_timeout_ms"].is_null(),
        "a new server inherits rather than pinning a copy of the org default"
    );

    let slowed: Value = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .json(&json!({"request_timeout_ms": 120000}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(slowed["request_timeout_ms"], 120000);

    // an unrelated PATCH must not disturb the override
    let renamed: Value = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .json(&json!({"description": "the slow one"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        renamed["request_timeout_ms"], 120000,
        "absent means leave it, not clear it"
    );

    let out_of_range = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .json(&json!({"request_timeout_ms": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(out_of_range.status(), 400);

    // explicit null is how an operator gives the server back to the org default
    let reverted: Value = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .json(&json!({"request_timeout_ms": null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        reverted["request_timeout_ms"].is_null(),
        "null must clear the override: {reverted}"
    );
}
