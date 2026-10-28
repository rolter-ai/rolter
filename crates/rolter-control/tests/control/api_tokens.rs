//! Scoped control-plane API tokens (#1836): minting, what a token can reach,
//! and the ways it stops working.

use crate::support::*;
use serde_json::{json, Value};

struct World {
    base: String,
    client: reqwest::Client,
    pool: sqlx::PgPool,
    org: String,
    team: String,
    project: String,
    other_project: String,
    _db: rolter_store::postgres::test_schema::TestSchema,
}

impl World {
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Value,
    ) -> (u16, Value) {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token);
        if !body.is_null() {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str, token: &str) -> (u16, Value) {
        self.send(reqwest::Method::GET, path, token, Value::Null)
            .await
    }

    async fn mint(&self, token: &str, body: Value) -> (u16, Value) {
        let path = format!("/api/v1/orgs/{}/api-tokens", self.org);
        self.send(reqwest::Method::POST, &path, token, body).await
    }
}

async fn world() -> World {
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let mut world = World {
        base: format!("http://{addr}"),
        client: reqwest::Client::new(),
        pool,
        org: String::new(),
        team: String::new(),
        project: String::new(),
        other_project: String::new(),
        _db: db,
    };
    let post = |path: String, body: Value| {
        let world = &world;
        async move {
            world
                .send(reqwest::Method::POST, &path, admin_token(), body)
                .await
                .1
        }
    };
    let org = post(
        "/api/v1/orgs".into(),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let team = post(
        format!("/api/v1/orgs/{org_id}/teams"),
        json!({"name": "core"}),
    )
    .await;
    let team_id = team["id"].as_str().unwrap().to_string();
    let project = post(
        format!("/api/v1/teams/{team_id}/projects"),
        json!({"name": "app"}),
    )
    .await;
    let other = post(
        format!("/api/v1/teams/{team_id}/projects"),
        json!({"name": "other"}),
    )
    .await;
    world.org = org_id;
    world.team = team_id;
    world.project = project["id"].as_str().unwrap().to_string();
    world.other_project = other["id"].as_str().unwrap().to_string();
    world
}

fn secret(minted: &Value) -> String {
    minted["secret"].as_str().expect("a secret").to_string()
}

/// A token resolves to its one role at its own scope and nowhere else, and
/// can never reach a deployment capability or the token endpoints.
#[tokio::test]
async fn a_token_holds_one_role_at_its_own_scope_and_nothing_above_it() {
    skip_without_db!();
    let w = world().await;
    let (status, minted) = w
        .mint(
            admin_token(),
            json!({"name": "ci", "project_id": w.project, "role": "member"}),
        )
        .await;
    assert_eq!(status, 200, "{minted}");
    let token = secret(&minted);
    assert!(token.starts_with("rolter_api_"));
    // the secret and its digest appear once, in the mint, and never again
    assert!(minted.get("token_hash").is_none());
    assert_eq!(minted["team_id"], w.team.as_str());
    assert_eq!(minted["project_id"], w.project.as_str());
    assert!(minted["expires_at"].is_string(), "a default expiry applies");

    let effective = |org: bool, project: &str| {
        let mut path = format!("/api/v1/rbac/effective?org_id={}", w.org);
        if !org {
            path = format!("{path}&team_id={}&project_id={project}", w.team);
        }
        let token = token.clone();
        let w = &w;
        async move { w.get(&path, &token).await }
    };
    let (status, mine) = effective(false, &w.project).await;
    assert_eq!(status, 200);
    assert_eq!(mine["role"], "member");
    assert_eq!(mine["superadmin"], false);
    assert!(mine["deployment_role"].is_null());
    let (_, elsewhere) = effective(false, &w.other_project).await;
    assert!(
        elsewhere["role"].is_null(),
        "another project is not its scope"
    );
    let (_, above) = effective(true, "").await;
    assert!(above["role"].is_null(), "the org above is not its scope");

    // deployment-wide capabilities and token management are
    // refused whatever the role
    for path in [
        "/api/v1/feature-flags",
        "/api/v1/security-settings",
        "/api/v1/cluster/nodes",
        "/api/v1/audit-log",
        "/api/v1/users",
    ] {
        assert_eq!(w.get(path, &token).await.0, 403, "{path}");
    }
    assert_eq!(
        w.get(&format!("/api/v1/orgs/{}/api-tokens", w.org), &token)
            .await
            .0,
        403
    );
    assert_eq!(
        w.mint(
            &token,
            json!({"name": "child", "project_id": w.project, "role": "viewer"})
        )
        .await
        .0,
        403,
        "a token cannot mint a successor"
    );
    let id = minted["id"].as_str().unwrap();
    for (method, path) in [
        (
            reqwest::Method::POST,
            format!("/api/v1/api-tokens/{id}/rotate"),
        ),
        (reqwest::Method::DELETE, format!("/api/v1/api-tokens/{id}")),
    ] {
        assert_eq!(
            w.send(method, &path, &token, Value::Null).await.0,
            403,
            "{path}"
        );
    }
    // a token is not a session, and a session is not a token
    assert_eq!(w.get("/api/v1/auth/me", &token).await.0, 401);
    assert_eq!(w.get("/api/v1/auth/me", "rolter_api_nope").await.0, 401);
}

#[tokio::test]
async fn a_custom_role_token_holds_the_role_s_grants() {
    skip_without_db!();
    let w = world().await;
    let (_, role) = w
        .send(
            reqwest::Method::POST,
            &format!("/api/v1/orgs/{}/custom-roles", w.org),
            admin_token(),
            json!({
                "name": "Auditor",
                "base_role": "viewer",
                "grants": [{"resource": "audit_log", "action": "read"}],
            }),
        )
        .await;
    let role_id = role["id"].as_str().unwrap().to_string();
    let (status, minted) = w
        .mint(
            admin_token(),
            json!({"name": "audit-bot", "custom_role_id": role_id}),
        )
        .await;
    assert_eq!(status, 200, "{minted}");
    let token = secret(&minted);
    let audit = format!("/api/v1/orgs/{}/audit-log", w.org);
    assert_eq!(w.get(&audit, &token).await.0, 200);
    // the grant is audit_log:read and no more
    assert_eq!(
        w.get(&format!("/api/v1/orgs/{}/scim-tokens", w.org), &token)
            .await
            .0,
        403
    );
    // dropping the role takes the token with it
    let (status, _) = w
        .send(
            reqwest::Method::DELETE,
            &format!("/api/v1/custom-roles/{role_id}"),
            admin_token(),
            Value::Null,
        )
        .await;
    assert_eq!(status, 204);
    assert_eq!(w.get(&audit, &token).await.0, 401);
}

#[tokio::test]
async fn minting_takes_an_admin_of_the_tokens_own_scope_and_validates_the_request() {
    skip_without_db!();
    let w = world().await;
    let project_admin = seed_user(&w.pool, "lead@acme.test", false).await;
    seed_membership(
        &w.pool,
        project_admin,
        None,
        None,
        Some(w.project.parse().unwrap()),
        "admin",
    )
    .await;
    let human = seed_session(&w.pool, project_admin, "lead").await;

    // an admin of the project mints for that project, and for nothing wider
    let (status, minted) = w
        .mint(
            &human,
            json!({"name": "deploy", "project_id": w.project, "role": "admin"}),
        )
        .await;
    assert_eq!(status, 200, "{minted}");
    assert_eq!(minted["created_by"], project_admin.to_string());
    for body in [
        json!({"name": "wide", "role": "viewer"}),
        json!({"name": "team", "team_id": w.team, "role": "viewer"}),
        json!({"name": "sibling", "project_id": w.other_project, "role": "viewer"}),
    ] {
        assert_eq!(w.mint(&human, body.clone()).await.0, 403, "{body}");
    }
    // the project admin lists only their own project's tokens
    let (status, listed) = w
        .get(&format!("/api/v1/orgs/{}/api-tokens", w.org), &human)
        .await;
    assert_eq!(status, 200);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(listed[0].get("secret").is_none() && listed[0].get("token_hash").is_none());

    for (body, why) in [
        (json!({"name": "x"}), "no role"),
        (
            json!({"name": "x", "role": "admin", "custom_role_id": uuid::Uuid::nil()}),
            "two roles",
        ),
        (json!({"name": "x", "role": "superadmin"}), "superadmin"),
        (
            json!({"name": "x", "role": "admin", "expires_in_days": 0}),
            "zero days",
        ),
        (
            json!({"name": "x", "role": "admin", "expires_in_days": 9999}),
            "too long",
        ),
        (
            json!({"name": "x", "role": "admin", "never_expires": true, "expires_in_days": 5}),
            "both expiries",
        ),
        (json!({"name": "  ", "role": "admin"}), "blank name"),
        (
            json!({"name": "x", "role": "admin", "extra": 1}),
            "unknown field",
        ),
        (
            json!({"name": "x", "role": "admin", "team_id": uuid::Uuid::new_v4(),
                   "project_id": w.project}),
            "team that is not the project's",
        ),
    ] {
        let (status, _) = w.mint(admin_token(), body).await;
        assert!(
            status == 400 || status == 422,
            "{why} should be refused, got {status}"
        );
    }

    // an explicit opt-out is allowed and stored as no expiry
    let (_, forever) = w
        .mint(
            admin_token(),
            json!({"name": "forever", "role": "viewer", "never_expires": true}),
        )
        .await;
    assert!(forever["expires_at"].is_null());
}

#[tokio::test]
async fn rotation_replaces_the_secret_and_revocation_and_expiry_end_the_token() {
    skip_without_db!();
    let w = world().await;
    let (_, minted) = w
        .mint(
            admin_token(),
            json!({"name": "ci", "project_id": w.project, "role": "viewer"}),
        )
        .await;
    let id = minted["id"].as_str().unwrap().to_string();
    let first = secret(&minted);
    let probe = format!(
        "/api/v1/rbac/effective?org_id={}&team_id={}&project_id={}",
        w.org, w.team, w.project
    );
    assert_eq!(w.get(&probe, &first).await.0, 200);

    let (status, rotated) = w
        .send(
            reqwest::Method::POST,
            &format!("/api/v1/api-tokens/{id}/rotate"),
            admin_token(),
            Value::Null,
        )
        .await;
    assert_eq!(status, 200, "{rotated}");
    let second = secret(&rotated);
    assert_ne!(first, second);
    assert_eq!(w.get(&probe, &first).await.0, 401, "the old secret is dead");
    assert_eq!(w.get(&probe, &second).await.0, 200);

    // the digest, not the secret, is what is stored
    let stored: String =
        sqlx::query_scalar("select token_hash from api_tokens where id = $1::uuid")
            .bind(&id)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_ne!(stored, second);
    assert_eq!(stored, rolter_auth::hash_key("", &second));

    // expiry is checked on every request
    sqlx::query(
        "update api_tokens set expires_at = now() - interval '1 second' where id = $1::uuid",
    )
    .bind(&id)
    .execute(&w.pool)
    .await
    .unwrap();
    assert_eq!(w.get(&probe, &second).await.0, 401);
    let (status, body) = w
        .send(
            reqwest::Method::POST,
            &format!("/api/v1/api-tokens/{id}/rotate"),
            admin_token(),
            Value::Null,
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(body["error"]["code"], "api_token_inactive");

    // revocation
    let (_, other) = w
        .mint(
            admin_token(),
            json!({"name": "second", "project_id": w.project, "role": "viewer"}),
        )
        .await;
    let other_secret = secret(&other);
    assert_eq!(w.get(&probe, &other_secret).await.0, 200);
    let other_id = other["id"].as_str().unwrap();
    let (status, revoked) = w
        .send(
            reqwest::Method::DELETE,
            &format!("/api/v1/api-tokens/{other_id}"),
            admin_token(),
            Value::Null,
        )
        .await;
    assert_eq!(status, 200);
    assert!(revoked["revoked_at"].is_string());
    assert_eq!(w.get(&probe, &other_secret).await.0, 401);
}

/// Mint, rotate, revoke and everything else a token does are audited, and the
/// rows name the token rather than an account.
#[tokio::test]
async fn the_audit_log_names_the_token_that_acted() {
    skip_without_db!();
    let w = world().await;
    let (_, minted) = w
        .mint(admin_token(), json!({"name": "gitops", "role": "admin"}))
        .await;
    let token = secret(&minted);
    let id = minted["id"].as_str().unwrap().to_string();
    let (status, team) = w
        .send(
            reqwest::Method::POST,
            &format!("/api/v1/orgs/{}/teams", w.org),
            &token,
            json!({"name": "from-ci"}),
        )
        .await;
    assert_eq!(status, 200, "{team}");
    w.send(
        reqwest::Method::POST,
        &format!("/api/v1/api-tokens/{id}/rotate"),
        admin_token(),
        Value::Null,
    )
    .await;

    let (_, log) = w
        .get(
            &format!("/api/v1/orgs/{}/audit-log?limit=100", w.org),
            admin_token(),
        )
        .await;
    let rows = log["items"].as_array().unwrap();
    let action = |name: &str| rows.iter().find(|row| row["action"] == name);
    let created = action("api_token.create").expect("the mint is audited");
    assert_eq!(created["target_id"], id.as_str());
    assert!(action("api_token.rotate").is_some());
    let team_row = action("team.create").expect("the token's own action is audited");
    assert!(team_row["actor_user_id"].is_null(), "no account acted");
    assert_eq!(team_row["detail"]["api_token"]["name"], "gitops");
    assert_eq!(team_row["detail"]["api_token"]["id"], id.as_str());
    // the secret is nowhere in the log
    assert!(!log.to_string().contains("rolter_api_"), "{log}");
}

/// A per-person cap that is not narrowed to an org or a project counts the
/// person everywhere and is the deployment's to manage (#1830), so even an org
/// admin's token is refused it, while the same cap narrowed to the token's own
/// org is the org's to set.
#[tokio::test]
async fn a_token_cannot_manage_a_cap_that_counts_a_person_everywhere() {
    skip_without_db!();
    let w = world().await;
    let (status, minted) = w
        .mint(admin_token(), json!({"name": "ops", "role": "admin"}))
        .await;
    assert_eq!(status, 200, "{minted}");
    let token = secret(&minted);
    let person = seed_user(&w.pool, "grace@acme.test", false).await;
    seed_membership(
        &w.pool,
        person,
        Some(w.org.parse().unwrap()),
        None,
        None,
        "member",
    )
    .await;

    for path in ["/api/v1/budgets", "/api/v1/rate-limits"] {
        let cap = if path.ends_with("budgets") {
            json!({"scope_type": "user", "scope_id": person, "limit_usd": "50", "period": "monthly"})
        } else {
            json!({"scope_type": "user", "scope_id": person, "rpm": 20})
        };
        let (status, body) = w
            .send(reqwest::Method::POST, path, &token, cap.clone())
            .await;
        assert_eq!(status, 403, "{path} everywhere: {body}");

        let mut narrowed = cap;
        narrowed["within_org_id"] = json!(w.org);
        let (status, body) = w.send(reqwest::Method::POST, path, &token, narrowed).await;
        assert_eq!(status, 200, "{path} within the org: {body}");
    }
}

/// A token never grants access to anyone, whatever role it holds: an admin
/// token reads who has access but cannot add a member, send an invitation or
/// change how people sign in, so a leaked one leaves nothing behind.
#[tokio::test]
async fn a_token_reads_access_but_never_grants_it() {
    skip_without_db!();
    let w = world().await;
    let (status, minted) = w
        .mint(admin_token(), json!({"name": "ops", "role": "admin"}))
        .await;
    assert_eq!(status, 200, "{minted}");
    let token = secret(&minted);
    let person = seed_user(&w.pool, "grace@acme.test", false).await;
    let org = &w.org;

    let refused = [
        (
            reqwest::Method::POST,
            format!("/api/v1/orgs/{org}/memberships"),
            json!({"user_id": person, "scope_type": "org", "scope_id": org, "role": "admin"}),
        ),
        (
            reqwest::Method::POST,
            format!("/api/v1/orgs/{org}/invitations"),
            json!({"email": "eve@acme.test", "role": "admin"}),
        ),
        (
            reqwest::Method::PUT,
            format!("/api/v1/orgs/{org}/auth-policy"),
            json!({"allow_password_login": true, "allow_sso": true, "mfa_policy": "off"}),
        ),
    ];
    for (method, path, body) in refused {
        let (status, answer) = w.send(method.clone(), &path, &token, body).await;
        assert_eq!(status, 403, "{method} {path}: {answer}");
    }
    for path in [
        format!("/api/v1/orgs/{org}/memberships"),
        format!("/api/v1/orgs/{org}/invitations"),
        format!("/api/v1/orgs/{org}/auth-policy"),
    ] {
        let (status, answer) = w.get(&path, &token).await;
        assert_eq!(status, 200, "GET {path}: {answer}");
    }
}
