//! The signed-in account's own profile, preferences, saved views and personal keys.

use crate::support::*;
use serde_json::{json, Value};

/// A superadmin account holds no membership, and still gets what an org admin
/// gets from the self-service key routes: the seeded operator could not open
/// the Playground on a fresh deployment (#1847).
#[tokio::test]
async fn a_superadmin_without_membership_mints_personal_and_playground_keys() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("keys".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    let project: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'app') returning id")
            .bind(team)
            .fetch_one(&pool)
            .await
            .unwrap();
    // a playground key is scoped to the project's routes
    sqlx::query(
        "insert into routes (project_id, model, strategy) values ($1, 'gpt-4o', 'round_robin')",
    )
    .bind(project)
    .execute(&pool)
    .await
    .unwrap();
    let operator = seed_user(&pool, "operator@acme.test", true).await;
    let session = seed_session(&pool, operator, "keys_operator").await;

    let playground = client
        .post(format!(
            "{base}/api/v1/me/projects/{project}/playground-key"
        ))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap();
    assert_eq!(playground.status(), 200);
    let personal = client
        .post(format!("{base}/api/v1/me/projects/{project}/virtual-keys"))
        .bearer_auth(&session)
        .json(&json!({"name": "operator laptop", "expires_in_days": 7}))
        .send()
        .await
        .unwrap();
    assert_eq!(personal.status(), 200);
}

/// Offboarding stops a person's own keys at the gateway and leaves the shared
/// keys an admin minted for the project alone (#1841). Deprovisioning through
/// SCIM moves the snapshot version, so a polling gateway refetches; the same
/// keys come back on reprovisioning; and deleting the account disables them
/// rather than letting the foreign key turn them into shared keys.
#[tokio::test]
async fn a_leavers_personal_keys_leave_the_snapshot_and_shared_keys_stay() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("leaver".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("leaver")
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org: uuid::Uuid = org["id"].as_str().unwrap().parse().unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    let project: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'app') returning id")
            .bind(team)
            .fetch_one(&pool)
            .await
            .unwrap();
    let scim: Value = client
        .post(format!("{base}/api/v1/orgs/{org}/scim-tokens"))
        .bearer_auth("leaver")
        .json(&json!({"name": "idp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim = scim["secret"].as_str().unwrap().to_string();
    let provisioned: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&scim)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "engineer@acme.test",
            "emails": [{"value": "engineer@acme.test", "primary": true}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim_id = provisioned["id"].as_str().unwrap().to_string();
    let engineer: uuid::Uuid = scim_id.parse().unwrap();
    seed_membership(&pool, engineer, None, None, Some(project), "member").await;
    let session = seed_session(&pool, engineer, "leaver_engineer").await;

    let mint = |url: String, bearer: String| {
        let client = client.clone();
        async move {
            let row: Value = client
                .post(url)
                .bearer_auth(bearer)
                .json(&json!({"name": "k", "expires_in_days": 7}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            row["id"].as_str().unwrap().to_string()
        }
    };
    let personal = mint(
        format!("{base}/api/v1/me/projects/{project}/virtual-keys"),
        session.clone(),
    )
    .await;
    let shared = mint(
        format!("{base}/api/v1/projects/{project}/virtual-keys"),
        "leaver".to_string(),
    )
    .await;
    // the version and, per served key, whether it is disabled
    let snapshot = || {
        let client = client.clone();
        let base = base.clone();
        async move {
            let snapshot: Value = client
                .get(format!("{base}/internal/snapshot"))
                .bearer_auth("leaver")
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let keys = snapshot["config"]["db_virtual_keys"]
                .as_array()
                .unwrap()
                .iter()
                .map(|key| {
                    (
                        key["id"].as_str().unwrap().to_string(),
                        key["disabled"].as_bool().unwrap_or(false),
                    )
                })
                .collect::<std::collections::HashMap<String, bool>>();
            (snapshot["version"].as_i64().unwrap(), keys)
        }
    };
    let set_active = |active: bool| {
        let client = client.clone();
        let url = format!("{base}/scim/v2/Users/{scim_id}");
        let scim = scim.clone();
        async move {
            let response = client
                .patch(url)
                .bearer_auth(scim)
                .json(&json!({
                    "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                    "Operations": [{"op": "replace", "path": "active", "value": active}]
                }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
        }
    };
    let (version, keys) = snapshot().await;
    assert_eq!(keys.get(&personal), Some(&false), "{keys:?}");
    assert_eq!(keys.get(&shared), Some(&false), "{keys:?}");

    // deprovisioned by the IdP: the personal key goes, and the version moves
    // so the gateways refetch; the shared one stays
    set_active(false).await;
    let (after, keys) = snapshot().await;
    assert!(
        after > version,
        "deactivation left the version at {version}"
    );
    assert!(
        !keys.contains_key(&personal),
        "a deactivated creator's key is served"
    );
    assert_eq!(keys.get(&shared), Some(&false));
    let detail: Value = sqlx::query_scalar(
        "select detail from audit_log where action = 'scim.user.update' order by at desc limit 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(detail["personal_keys"], 1, "{detail}");

    // reprovisioned: the same key is back, nothing about it changed
    set_active(true).await;
    let (again, keys) = snapshot().await;
    assert!(again > after, "reactivation left the version at {after}");
    assert_eq!(keys.get(&personal), Some(&false));

    // their last role that reaches the project is removed: gone again
    sqlx::query("delete from memberships where user_id = $1")
        .bind(engineer)
        .execute(&pool)
        .await
        .unwrap();
    let (_, keys) = snapshot().await;
    assert!(
        !keys.contains_key(&personal),
        "a creator with no reach keeps their key"
    );
    assert_eq!(keys.get(&shared), Some(&false));

    // the account is deleted: the key outlives it as a row, disabled, rather
    // than being served as a shared key with its owner's policy gone
    seed_membership(&pool, engineer, None, None, Some(project), "member").await;
    let deleted = client
        .delete(format!("{base}/api/v1/users/{engineer}"))
        .bearer_auth("leaver")
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    let (_, keys) = snapshot().await;
    assert_eq!(keys.get(&personal), Some(&true), "{keys:?}");
    assert_eq!(keys.get(&shared), Some(&false));
    // written to the org it left, since its memberships went with it (#1854)
    let (audited_org, detail): (Option<uuid::Uuid>, Value) = sqlx::query_as(
        "select org_id, detail from audit_log where action = 'user.delete' and target_id = $1",
    )
    .bind(engineer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited_org, Some(org));
    assert_eq!(detail["personal_keys"], 1, "{detail}");
}

/// The `/me/*` 401 distinguishes "you are not signed in" from "this deployment
/// has no accounts to sign in to" (#942).
///
/// With an admin token configured the control plane is gated, so an anonymous
/// `/me/*` call is an ordinary missing session and signing in fixes it. In open
/// mode it is not fixable that way at all, and the two must not render alike.
#[tokio::test]
async fn a_gated_control_plane_reports_a_plain_missing_session_on_me() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool, Some(admin_token().to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();

    let anon = client
        .get(format!("http://{addr}/api/v1/me/virtual-keys"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);
    let body: Value = anon.json().await.unwrap();
    assert_eq!(
        body["error"]["code"].as_str(),
        Some("unauthenticated"),
        "a gated deployment must not claim open mode: {body}"
    );

    // the admin token is not a session either: it opens admin routes, but
    // `/me/*` is per-user and there is no user behind a machine token
    let with_admin = client
        .get(format!("http://{addr}/api/v1/me/virtual-keys"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(with_admin.status(), 401);
    let body: Value = with_admin.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str(), Some("unauthenticated"));
}

/// Self-service key lifecycle (ROL-224): a logged-in member mints, lists,
/// rotates and deletes their own virtual keys, and usage 503s without
/// ClickHouse. Runs in open mode; `/me/*` still requires a real session.
#[tokio::test]
async fn self_service_key_lifecycle() {
    skip_without_db!();
    let password = random_password();
    let (app, db) = fresh_app().await;
    let addr = serve(app).await;
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
        json!({"email": "member@example.com", "password": password, "role": "member"}),
    )
    .await;

    // log in to get a session token
    let login = post(
        &client,
        format!("{base}/api/v1/auth/login"),
        json!({"email": "member@example.com", "password": password}),
    )
    .await;
    let token = login["token"].as_str().unwrap().to_string();

    // /me/* requires a session: unauthenticated is rejected. this app runs in
    // open mode, where every admin route beside it passes as superadmin — so
    // the 401 has to say *which* 401 it is, or it reads as a broken login
    // rather than an endpoint this deployment cannot serve (#942)
    let anon = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);
    let anon: Value = anon.json().await.unwrap();
    assert_eq!(
        anon["error"]["code"].as_str(),
        Some("open_mode_no_session"),
        "open-mode 401 must be distinguishable: {anon}"
    );

    // mint a key I own in the project I belong to
    let minted = client
        .post(format!(
            "{base}/api/v1/me/projects/{project_id}/virtual-keys"
        ))
        .bearer_auth(&token)
        .json(&json!({
            "name": "laptop",
            "models": ["gpt-4o"],
            "budget": {"limit_usd": "25"},
            "rate_limit": {"rpm": 10},
        }))
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
    // rotating a bounded key must not hand back an unbounded one (#1171)
    let caps = |id: String| {
        let pool = db.pool().clone();
        async move {
            sqlx::query_as::<_, (i64, i64)>(
                "select (select count(*) from budgets where scope_type = 'virtual_key' and scope_id = $1::uuid),
                        (select count(*) from rate_limits where scope_type = 'virtual_key' and scope_id = $1::uuid)",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(caps(key_id.clone()).await, (1, 1));
    assert_eq!(caps(new_id.clone()).await, (1, 1));

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

    // a window ClickHouse would read as the epoch is refused first, naming the
    // bound, rather than widening the scan to 1970 (#1192)
    for param in ["since", "until"] {
        let usage = client
            .get(format!("{base}/api/v1/me/usage?{param}=not-a-date"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(usage.status(), 400, "{param}");
        let body: Value = usage.json().await.unwrap();
        assert_eq!(body["error"]["param"], param);
        assert_eq!(body["error"]["code"], "invalid_time_bound");
        assert_eq!(body["error"]["type"], "invalid_request_error");
    }
    // and a valid one still reaches the handler, which 503s as before
    let usage = client
        .get(format!(
            "{base}/api/v1/me/usage?since=2026-07-01T00:00:00Z&until=2026-07-08%2000:00:00"
        ))
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

/// The playground mints a key the *server* scopes (#1640).
///
/// The point of the endpoint is what a caller cannot do with it: it takes no
/// body, so it cannot ask for a model the project does not route, cannot ask
/// for a longer life, and cannot be pointed at a project the caller does not
/// belong to. The dashboard half (#944) puts the returned key straight into the
/// Playground rather than asking an operator to paste a long-lived one.
#[tokio::test]
async fn playground_key_is_scoped_by_the_server() {
    skip_without_db!();
    let password = random_password();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Playground Co", "slug": "playground-co"}),
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

    post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/users"),
        json!({"email": "operator@example.com", "password": password, "role": "member"}),
    )
    .await;
    let login = post(
        &client,
        format!("{base}/api/v1/auth/login"),
        json!({"email": "operator@example.com", "password": password}),
    )
    .await;
    let token = login["token"].as_str().unwrap().to_string();

    // a project with no routes yet still mints, so the first Getting started
    // call works on a fresh deployment (#2300). an empty `models` list on a
    // virtual key means *every* model, so the key is scoped to the builtin
    // `fake-llm` alone rather than left empty
    let routeless = client
        .post(format!(
            "{base}/api/v1/me/projects/{project_id}/playground-key"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(routeless.status(), 200, "a routeless project mints a key");
    let routeless: Value = routeless.json().await.unwrap();
    assert_eq!(
        routeless["models"],
        json!(["fake-llm"]),
        "a routeless project's key reaches the builtin and nothing else"
    );
    assert_eq!(routeless["purpose"], "playground");
    let routeless_id = routeless["id"].as_str().unwrap().to_string();

    post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "claude-sonnet-4", "strategy": "round_robin"}),
    )
    .await;

    let minted = client
        .post(format!(
            "{base}/api/v1/me/projects/{project_id}/playground-key"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert!(minted.status().is_success());
    let minted: Value = minted.json().await.unwrap();
    assert!(minted["key"].as_str().unwrap().starts_with("sk-rolter-"));

    // scoped to exactly the routes this project has, written out rather than
    // left empty: a key minted before a third route exists must not reach it
    let mut models: Vec<&str> = minted["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    models.sort_unstable();
    assert_eq!(models, vec!["claude-sonnet-4", "gpt-4o"]);

    // minutes, not days: the mint endpoint's floor is one day, which is the
    // whole reason this endpoint exists
    let expires_at = minted["expires_at"].as_str().expect("an expiry");
    let expires_at: chrono::DateTime<chrono::Utc> = expires_at.parse().unwrap();
    let lifetime = expires_at - chrono::Utc::now();
    assert!(
        lifetime < chrono::Duration::hours(1) && lifetime > chrono::Duration::minutes(1),
        "playground key should live minutes, lives {lifetime}"
    );

    // and it says what it is, so the Keys screen can label it rather than
    // leaving a reader to infer it from the expiry
    assert_eq!(minted["purpose"], "playground");
    let keys: Value = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed = keys.as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|k| k["purpose"] == "playground"));
    let minted_id = minted["id"].as_str().unwrap();

    // a route added after the key was minted is out of its reach: the list was
    // resolved once, at mint time, which is what makes the key a snapshot of
    // what the caller could reach rather than a standing grant
    post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "o3-mini", "strategy": "round_robin"}),
    )
    .await;
    let keys: Value = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for id in [minted_id, routeless_id.as_str()] {
        let key = keys
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["id"] == id)
            .expect("the minted key is listed");
        let still: Vec<&str> = key["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap())
            .collect();
        assert!(
            !still.contains(&"o3-mini"),
            "a key must not widen itself as routes appear: {still:?}"
        );
    }

    // a disabled route serves nothing, so a key minted afterwards must not
    // name it (#2759)
    let retired = post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "retired-model", "strategy": "round_robin"}),
    )
    .await;
    let retired_id = retired["id"].as_str().unwrap();
    let off = client
        .put(format!("{base}/api/v1/routes/{retired_id}"))
        .json(&json!({"enabled": false}))
        .send()
        .await
        .unwrap();
    assert!(
        off.status().is_success(),
        "disable failed: {}",
        off.status()
    );
    let after: Value = client
        .post(format!(
            "{base}/api/v1/me/projects/{project_id}/playground-key"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut models: Vec<&str> = after["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    models.sort_unstable();
    assert_eq!(models, vec!["claude-sonnet-4", "gpt-4o", "o3-mini"]);

    // a session is required: the endpoint mints a credential, so it is never
    // reachable without one
    let anon = client
        .post(format!(
            "{base}/api/v1/me/projects/{project_id}/playground-key"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);
}

/// #945: naming and expiry are the two choices the mint path used to let a
/// caller skip. Both are now decided at creation, and both are decided by the
/// server — the client sends a day count, not an instant.
#[tokio::test]
async fn a_minted_key_must_be_named_and_carries_the_ttl_the_caller_chose() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme-945"}),
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
    let keys_url = format!("{base}/api/v1/projects/{project_id}/virtual-keys");

    // a blank name is refused, and so is one that is only whitespace: the
    // plaintext is shown once, so an unnamed key is unattributable forever
    for name in [json!(""), json!("   ")] {
        let rejected = client
            .post(&keys_url)
            .json(&json!({"name": name}))
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), 400, "name {name} should be refused");
        let body: Value = rejected.json().await.unwrap();
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("name is required"),
            "{body}"
        );
    }

    // omitting the field entirely is a 422 from serde, not a silently unnamed
    // key: the field is no longer `Option`
    let missing = client
        .post(&keys_url)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert!(missing.status().is_client_error(), "{}", missing.status());

    // a day count becomes an instant the server computed
    let before = chrono::Utc::now();
    let minted = post(
        &client,
        keys_url.clone(),
        json!({"name": "  billing  ", "expires_in_days": 30}),
    )
    .await;
    assert_eq!(minted["name"], "billing", "the name is stored trimmed");
    let expires: chrono::DateTime<chrono::Utc> = minted["expires_at"]
        .as_str()
        .unwrap_or_else(|| panic!("a key minted with a ttl must carry one: {minted}"))
        .parse()
        .unwrap();
    assert!(expires > before + chrono::Duration::days(29));
    assert!(expires < before + chrono::Duration::days(31));

    // "never" is still reachable, but only by omitting the field — which is
    // what the dashboard sends for an explicit choice, never for an untouched
    // control
    let immortal = post(&client, keys_url.clone(), json!({"name": "build box"})).await;
    assert!(
        immortal["expires_at"].is_null(),
        "omitting the ttl must mint a key that never expires: {immortal}"
    );

    // a ttl of zero reads like "no expiry" but would mean "already expired";
    // refusing it keeps that ambiguity out of the store
    let zero = client
        .post(&keys_url)
        .json(&json!({"name": "zero", "expires_in_days": 0}))
        .send()
        .await
        .unwrap();
    assert_eq!(zero.status(), 400);
}

/// `PATCH /api/v1/me/profile` (#1823): any signed-in account, a viewer
/// included, edits its own display name and bio; omitted fields stay, `null`
/// and `""` clear, bad input is a 400, and the audit row names fields only.
#[tokio::test]
async fn a_viewer_edits_their_own_profile() {
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

    let viewer = seed_user(&pool, "profile-viewer@example.com", false).await;
    let token = seed_session(&pool, viewer, "profileviewer").await;
    let bystander = seed_user(&pool, "profile-bystander@example.com", false).await;
    let bystander_token = seed_session(&pool, bystander, "profilebystander").await;

    let me = |token: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .get(format!("{base}/api/v1/auth/me"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let patch = |token: String, body: Value| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let res = client
                .patch(format!("{base}/api/v1/me/profile"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = res.status().as_u16();
            (status, res.json::<Value>().await.unwrap())
        }
    };

    let before = me(token.clone()).await;
    assert_eq!(before["user"]["display_name"], Value::Null);
    assert_eq!(before["user"]["bio"], Value::Null);
    assert_eq!(before["display_name_managed"], false);

    // set both, with surrounding whitespace normalised away
    let (status, body) = patch(
        token.clone(),
        json!({"display_name": "  Grace Hopper ", "bio": "Compilers.\nCOBOL."}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["display_name"], "Grace Hopper");
    assert_eq!(body["bio"], "Compilers.\nCOBOL.");
    let after = me(token.clone()).await;
    assert_eq!(after["user"]["display_name"], "Grace Hopper");
    assert_eq!(after["user"]["bio"], "Compilers.\nCOBOL.");

    // omitted leaves the other field alone
    let (status, body) = patch(token.clone(), json!({"display_name": "Grace"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["bio"], "Compilers.\nCOBOL.");

    // null clears one field, an empty string the other
    let (status, body) = patch(token.clone(), json!({"display_name": null, "bio": ""})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["display_name"], Value::Null);
    assert_eq!(body["bio"], Value::Null);
    let cleared = me(token.clone()).await;
    assert_eq!(cleared["user"]["display_name"], Value::Null);
    assert_eq!(cleared["user"]["bio"], Value::Null);

    // validation
    for bad in [
        json!({"display_name": "x".repeat(81)}),
        json!({"bio": "x".repeat(501)}),
        json!({"display_name": "line\u{0007}bell"}),
        json!({"display_name": "two\nlines"}),
        json!({"display_name": "   "}),
        json!({"bio": " \n\t "}),
        json!({"display_name": 7}),
    ] {
        let (status, body) = patch(token.clone(), bad.clone()).await;
        assert_eq!(status, 400, "{bad} was accepted: {body}");
    }
    // exactly at the limits is fine
    let (status, _) = patch(
        token.clone(),
        json!({"display_name": "n".repeat(80), "bio": "b".repeat(500)}),
    )
    .await;
    assert_eq!(status, 200);

    // the route writes only the caller's row
    let other = me(bystander_token.clone()).await;
    assert_eq!(other["user"]["display_name"], Value::Null);
    assert_eq!(other["user"]["bio"], Value::Null);

    // no session, no profile
    let anon = client
        .patch(format!("{base}/api/v1/me/profile"))
        .json(&json!({"bio": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    // `user:update` is still superadmin-only: a viewer cannot edit an account
    let denied = client
        .put(format!("{base}/api/v1/users/{bystander}"))
        .bearer_auth(&token)
        .json(&json!({"email": "renamed@example.com"}))
        .send()
        .await
        .unwrap();
    assert!(
        matches!(denied.status().as_u16(), 403 | 404),
        "viewer edited an account: {}",
        denied.status()
    );

    // audit rows name the fields, never the bio text
    let rows: Vec<(Value,)> = sqlx::query_as(
        "select detail from audit_log
         where action = 'user.profile.update' and target_id = $1 order by at",
    )
    .bind(viewer)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        rows.len() >= 4,
        "expected an audit row per change: {rows:?}"
    );
    assert_eq!(rows[0].0, json!({"fields": ["display_name", "bio"]}));
    assert_eq!(rows[1].0, json!({"fields": ["display_name"]}));
    assert!(
        !rows.iter().any(|(d,)| d.to_string().contains("COBOL")),
        "a bio leaked into the audit log"
    );
}

/// `GET`/`PUT /api/v1/me/preferences` (#1824): a viewer saves and reads back
/// their own document; bad input is a 400; unset keys read as null; the
/// document is whole-replace; the audit row names keys only.
#[tokio::test]
async fn a_viewer_saves_and_reads_their_own_preferences() {
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

    let viewer = seed_user(&pool, "prefs-viewer@example.com", false).await;
    let token = seed_session(&pool, viewer, "prefsviewer").await;
    let other = seed_user(&pool, "prefs-other@example.com", false).await;
    let other_token = seed_session(&pool, other, "prefsother").await;

    let get = |token: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let res = client
                .get(format!("{base}/api/v1/me/preferences"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            (res.status().as_u16(), res.json::<Value>().await.unwrap())
        }
    };
    let put = |token: String, body: Value| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let res = client
                .put(format!("{base}/api/v1/me/preferences"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            (res.status().as_u16(), res.json::<Value>().await.unwrap())
        }
    };

    // nothing saved: every key reads as absent and there is no scope to open on
    let (status, empty) = get(token.clone()).await;
    assert_eq!(status, 200, "{empty}");
    for key in [
        "language",
        "default_org_id",
        "default_team_id",
        "default_project_id",
        "default_playground_model",
        "chart_time_zone",
        "effective_default_scope",
    ] {
        assert_eq!(empty[key], Value::Null, "{key}: {empty}");
    }

    // save, read back
    let (status, saved) = put(
        token.clone(),
        json!({
            "language": "ru",
            "default_playground_model": "  gpt-4o ",
            "chart_time_zone": "Europe/Berlin",
        }),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    let (_, read) = get(token.clone()).await;
    assert_eq!(read["language"], "ru");
    assert_eq!(read["default_playground_model"], "gpt-4o");
    assert_eq!(read["chart_time_zone"], "Europe/Berlin");
    assert_eq!(read["default_project_id"], Value::Null);

    // PUT replaces the whole document: a key left out is cleared
    let (status, _) = put(token.clone(), json!({"language": "en"})).await;
    assert_eq!(status, 200);
    let (_, read) = get(token.clone()).await;
    assert_eq!(read["language"], "en");
    assert_eq!(read["chart_time_zone"], Value::Null);
    assert_eq!(read["default_playground_model"], Value::Null);

    // validation
    for bad in [
        json!({"unknown_key": 1}),
        json!({"language": "xx"}),
        json!({"language": 7}),
        json!({"chart_time_zone": "Not A Zone"}),
        json!({"chart_time_zone": "../../etc/passwd"}),
        json!({"default_project_id": "not-a-uuid"}),
        json!({"default_org_id": 5}),
        json!({"default_playground_model": "x".repeat(201)}),
        json!({"default_playground_model": "   "}),
    ] {
        let (status, body) = put(token.clone(), bad.clone()).await;
        assert_eq!(status, 400, "{bad} was accepted: {body}");
    }
    // a refused write changed nothing
    let (_, read) = get(token.clone()).await;
    assert_eq!(read["language"], "en");

    // each user sees only their own document: there is no route naming another
    let (_, theirs) = get(other_token.clone()).await;
    assert_eq!(theirs["language"], Value::Null);
    let (status, _) = put(other_token.clone(), json!({"language": "ru"})).await;
    assert_eq!(status, 200);
    let (_, mine) = get(token.clone()).await;
    assert_eq!(mine["language"], "en");

    // no session, no preferences
    let anon = client
        .get(format!("{base}/api/v1/me/preferences"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    // audit names keys, never values
    let rows: Vec<(Value,)> = sqlx::query_as(
        "select detail from audit_log
         where action = 'user.preferences.update' and target_id = $1 order by at",
    )
    .bind(viewer)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        rows[0].0,
        json!({"keys": ["chart_time_zone", "default_playground_model", "language"]})
    );
    assert_eq!(
        rows[1].0,
        json!({"keys": ["chart_time_zone", "default_playground_model", "language"]})
    );
    assert!(!rows.iter().any(|(d,)| d.to_string().contains("gpt-4o")));
}

/// A stored default scope never widens access (#1824): it is kept as written,
/// but `effective_default_scope` is computed from the caller's current
/// memberships, so a scope they lost, or never had, is never handed back.
#[tokio::test]
async fn a_default_scope_the_user_lost_is_never_the_effective_one() {
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

    async fn make(client: &reqwest::Client, url: String, body: Value) -> uuid::Uuid {
        let v: Value = client
            .post(url)
            .bearer_auth(admin_token())
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["id"].as_str().unwrap().parse().unwrap()
    }
    let org = make(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "PrefsOrg", "slug": "prefs-org"}),
    )
    .await;
    let team = make(
        &client,
        format!("{base}/api/v1/orgs/{org}/teams"),
        json!({"name": "T"}),
    )
    .await;
    let proj_a = make(
        &client,
        format!("{base}/api/v1/teams/{team}/projects"),
        json!({"name": "A"}),
    )
    .await;
    let proj_b = make(
        &client,
        format!("{base}/api/v1/teams/{team}/projects"),
        json!({"name": "B"}),
    )
    .await;
    let foreign_org = make(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Foreign", "slug": "prefs-foreign"}),
    )
    .await;

    let user = seed_user(&pool, "prefs-scope@example.com", false).await;
    let token = seed_session(&pool, user, "prefsscope").await;
    seed_membership(&pool, user, None, None, Some(proj_a), "viewer").await;
    seed_membership(&pool, user, None, None, Some(proj_b), "viewer").await;

    let read = || async {
        client
            .get(format!("{base}/api/v1/me/preferences"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let put = |body: Value| {
        let client = client.clone();
        let url = format!("{base}/api/v1/me/preferences");
        let token = token.clone();
        async move {
            client
                .put(url)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };

    // a readable default project is honoured, with its chain filled in
    let res = put(json!({"default_project_id": proj_b})).await;
    assert_eq!(res.status(), 200);
    let got = read().await;
    assert_eq!(
        got["effective_default_scope"],
        json!({"org_id": org, "team_id": team, "project_id": proj_b})
    );

    // the membership goes away: the id is still stored, but no longer effective
    sqlx::query("delete from memberships where user_id = $1 and project_id = $2")
        .bind(user)
        .bind(proj_b)
        .execute(&pool)
        .await
        .unwrap();
    let got = read().await;
    assert_eq!(got["default_project_id"], json!(proj_b));
    assert_eq!(
        got["effective_default_scope"],
        json!({"org_id": org, "team_id": team, "project_id": proj_a}),
        "fell back to the scope still readable"
    );

    // a scope the user never had is accepted at write time and never effective
    let res = put(json!({"default_org_id": foreign_org})).await;
    assert_eq!(res.status(), 200);
    let got = read().await;
    assert_eq!(got["default_org_id"], json!(foreign_org));
    assert_eq!(got["effective_default_scope"]["project_id"], json!(proj_a));
    assert_ne!(got["effective_default_scope"]["org_id"], json!(foreign_org));

    // and with nothing readable left there is no scope at all
    sqlx::query("delete from memberships where user_id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    let got = read().await;
    assert_eq!(got["effective_default_scope"], Value::Null);

    // a default naming a row that no longer exists is not an error
    let res = put(json!({"default_project_id": uuid::Uuid::new_v4()})).await;
    assert_eq!(res.status(), 200);
    assert_eq!(read().await["effective_default_scope"], Value::Null);
}

/// `/api/v1/me/saved-views` (#1825): a viewer saves, lists, renames, updates
/// and deletes filter presets on both surfaces; bad input is refused; the
/// audit rows name the preset and never what it filters by.
#[tokio::test]
async fn a_viewer_manages_their_own_saved_views() {
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

    let viewer = seed_user(&pool, "views-viewer@example.com", false).await;
    let token = seed_session(&pool, viewer, "viewsviewer").await;
    let views = format!("{base}/api/v1/me/saved-views");

    let send = |method: reqwest::Method, url: String, body: Option<Value>| {
        let client = client.clone();
        let token = token.clone();
        async move {
            let mut req = client.request(method, url).bearer_auth(token);
            if let Some(body) = body {
                req = req.json(&body);
            }
            let res = req.send().await.unwrap();
            let status = res.status().as_u16();
            let text = res.text().await.unwrap();
            (
                status,
                serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
            )
        }
    };
    let create = |body: Value| send(reqwest::Method::POST, views.clone(), Some(body));

    // nothing saved yet
    let (status, empty) = send(reqwest::Method::GET, views.clone(), None).await;
    assert_eq!(status, 200, "{empty}");
    assert_eq!(empty, json!([]));

    // one preset per surface
    let (status, logs) = create(json!({
        "surface": "llm_logs",
        "name": "  Errors last week ",
        "filters": {"window": "7d", "status": "error", "model": "gpt-4o"},
    }))
    .await;
    assert_eq!(status, 200, "{logs}");
    assert_eq!(logs["name"], "Errors last week");
    assert_eq!(logs["filters"]["status"], "error");
    assert_eq!(logs["effective_filters"], logs["filters"]);
    assert_eq!(logs["unavailable"], json!([]));
    let logs_preset = logs["id"].as_str().unwrap().to_string();
    let (status, dash) = create(json!({
        "surface": "dashboard", "name": "Month to date",
        "filters": {"window": "mtd", "bucket": "day"},
    }))
    .await;
    assert_eq!(status, 200, "{dash}");
    let dash_preset = dash["id"].as_str().unwrap().to_string();

    // list, all and per surface
    let (_, all) = send(reqwest::Method::GET, views.clone(), None).await;
    assert_eq!(all.as_array().unwrap().len(), 2);
    let (_, only_logs) = send(
        reqwest::Method::GET,
        format!("{views}?surface=llm_logs"),
        None,
    )
    .await;
    assert_eq!(only_logs.as_array().unwrap().len(), 1);
    assert_eq!(only_logs[0]["id"], logs_preset);
    let (status, _) = send(reqwest::Method::GET, format!("{views}?surface=nope"), None).await;
    assert_eq!(status, 400);

    // get one
    let (status, one) = send(reqwest::Method::GET, format!("{views}/{dash_preset}"), None).await;
    assert_eq!(status, 200, "{one}");
    assert_eq!(one["filters"], json!({"window": "mtd", "bucket": "day"}));

    // rename leaves the filters alone; a case-only rename of itself is fine
    let (status, renamed) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"name": "Weekly errors"})),
    )
    .await;
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(renamed["name"], "Weekly errors");
    assert_eq!(renamed["filters"]["model"], "gpt-4o");
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"name": "WEEKLY ERRORS"})),
    )
    .await;
    assert_eq!(status, 200);

    // update replaces the whole filter set and leaves the name alone
    let (status, updated) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"filters": {"window": "30d"}})),
    )
    .await;
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["filters"], json!({"window": "30d"}));
    assert_eq!(updated["name"], "WEEKLY ERRORS");

    // a patch that changes nothing is refused, and so is a surface change
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 400);
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"surface": "dashboard"})),
    )
    .await;
    assert_eq!(status, 400);

    // the same name on another surface is fine, on the same one is a 409 even
    // when only the case differs
    let (status, _) = create(json!({"surface": "dashboard", "name": "weekly errors"})).await;
    assert_eq!(status, 200);
    let (status, dup) = create(json!({"surface": "llm_logs", "name": "weekly ERRORS"})).await;
    assert_eq!(status, 409, "{dup}");
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"name": "Month to date"})),
    )
    .await;
    assert_eq!(status, 200, "a name taken on another surface is free here");
    let (status, _) = create(json!({"surface": "dashboard", "name": "month TO date"})).await;
    assert_eq!(status, 409);
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{dash_preset}"),
        Some(json!({"name": "weekly errors"})),
    )
    .await;
    assert_eq!(status, 409, "renaming onto an existing name");

    // validation
    for bad in [
        json!({"surface": "billing", "name": "x"}),
        json!({"surface": "llm_logs", "name": "   "}),
        json!({"surface": "llm_logs", "name": "x".repeat(81)}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"limit": 500}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"cursor": "a|b"}}),
        json!({"surface": "dashboard", "name": "ok", "filters": {"status": "error"}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"window": "forever"}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"window": 7}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"status": ["error"]}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"key": "not-a-uuid"}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"customer": "a-string"}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"business_unit": [1]}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": []}),
        json!({"surface": "llm_logs", "name": "ok", "extra": 1}),
        json!({"name": "no surface"}),
    ] {
        let (status, body) = create(bad.clone()).await;
        assert_eq!(status, 400, "{bad} was accepted: {body}");
    }
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"filters": {"surface_is_fixed": true}})),
    )
    .await;
    assert_eq!(status, 400);

    // delete, then it is gone
    let (status, _) = send(
        reqwest::Method::DELETE,
        format!("{views}/{dash_preset}"),
        None,
    )
    .await;
    assert_eq!(status, 204);
    let (status, _) = send(reqwest::Method::GET, format!("{views}/{dash_preset}"), None).await;
    assert_eq!(status, 404);
    let (status, _) = send(
        reqwest::Method::DELETE,
        format!("{views}/{dash_preset}"),
        None,
    )
    .await;
    assert_eq!(status, 404);

    // no session, no presets
    for req in [
        client.get(&views),
        client.post(&views).json(&json!({})),
        client.get(format!("{views}/{logs_preset}")),
        client
            .patch(format!("{views}/{logs_preset}"))
            .json(&json!({})),
        client.delete(format!("{views}/{logs_preset}")),
    ] {
        assert_eq!(req.send().await.unwrap().status(), 401);
    }

    // the audit rows name the preset and the surface, never a filter value
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "select action, detail from audit_log
         where action like 'user.saved_view.%' and actor_user_id = $1 order by at",
    )
    .bind(viewer)
    .fetch_all(&pool)
    .await
    .unwrap();
    let actions: Vec<&str> = rows.iter().map(|(a, _)| a.as_str()).collect();
    assert!(actions.contains(&"user.saved_view.create"), "{actions:?}");
    assert!(actions.contains(&"user.saved_view.update"), "{actions:?}");
    assert!(actions.contains(&"user.saved_view.delete"), "{actions:?}");
    let dump = serde_json::to_string(&rows).unwrap();
    for value in ["gpt-4o", "30d", "mtd", "\"window\"", "\"status\""] {
        assert!(
            !dump.contains(value),
            "{value} leaked into the audit log: {dump}"
        );
    }
    let (_, created) = rows
        .iter()
        .find(|(a, _)| a == "user.saved_view.create")
        .unwrap();
    assert_eq!(
        created,
        &json!({"surface": "llm_logs", "name": "Errors last week"})
    );
}

/// An account holds at most 50 presets per surface (#1825); the 51st is a 409,
/// the cap is per surface, and deleting one frees a slot.
#[tokio::test]
async fn saved_views_are_capped_per_user_and_surface() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let views = format!("http://{addr}/api/v1/me/saved-views");

    let user = seed_user(&pool, "views-cap@example.com", false).await;
    let token = seed_session(&pool, user, "viewscap").await;
    let other = seed_user(&pool, "views-cap-other@example.com", false).await;
    let other_token = seed_session(&pool, other, "viewscapother").await;

    let create = |token: String, surface: &'static str, name: String| {
        let client = client.clone();
        let url = views.clone();
        async move {
            client
                .post(url)
                .bearer_auth(token)
                .json(&json!({"surface": surface, "name": name}))
                .send()
                .await
                .unwrap()
        }
    };
    let mut first = String::new();
    for n in 0..50 {
        let res = create(token.clone(), "llm_logs", format!("preset {n}")).await;
        assert_eq!(res.status(), 200, "preset {n}");
        if n == 0 {
            first = res.json::<Value>().await.unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string();
        }
    }
    let over = create(token.clone(), "llm_logs", "one too many".to_string()).await;
    assert_eq!(over.status(), 409);
    let body: Value = over.json().await.unwrap();
    assert!(
        body["error"]["message"].as_str().unwrap().contains("50"),
        "{body}"
    );

    // another surface and another user have their own allowance
    assert_eq!(
        create(token.clone(), "dashboard", "fine".to_string())
            .await
            .status(),
        200
    );
    assert_eq!(
        create(other_token, "llm_logs", "fine".to_string())
            .await
            .status(),
        200
    );

    // deleting one frees a slot
    let gone = client
        .delete(format!("{views}/{first}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 204);
    assert_eq!(
        create(token, "llm_logs", "one more".to_string())
            .await
            .status(),
        200
    );
}

/// Presets are private (#1825): another account's list excludes them and
/// every by-id route answers 404 for them, as it does for an id that does not
/// exist, so an id cannot be probed.
#[tokio::test]
async fn one_users_saved_view_is_invisible_to_another() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let views = format!("http://{addr}/api/v1/me/saved-views");

    let alice = seed_user(&pool, "views-alice@example.com", false).await;
    let alice_token = seed_session(&pool, alice, "viewsalice").await;
    let bob = seed_user(&pool, "views-bob@example.com", false).await;
    let bob_token = seed_session(&pool, bob, "viewsbob").await;
    let admin = seed_user(&pool, "views-admin@example.com", true).await;
    let admin_token = seed_session(&pool, admin, "viewsadmin").await;

    let made: Value = client
        .post(&views)
        .bearer_auth(&alice_token)
        .json(&json!({"surface": "dashboard", "name": "Alice's", "filters": {"window": "7d"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let preset_id = made["id"].as_str().unwrap().to_string();
    let missing = uuid::Uuid::new_v4();

    for (who, token) in [("bob", &bob_token), ("a superadmin", &admin_token)] {
        let list: Value = client
            .get(&views)
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(list, json!([]), "{who} saw another account's preset");

        // the same answer for a preset that exists and one that does not
        for id in [preset_id.clone(), missing.to_string()] {
            let url = format!("{views}/{id}");
            let get = client.get(&url).bearer_auth(token).send().await.unwrap();
            assert_eq!(get.status(), 404, "{who} get {id}");
            let patch = client
                .patch(&url)
                .bearer_auth(token)
                .json(&json!({"name": "hijacked"}))
                .send()
                .await
                .unwrap();
            assert_eq!(patch.status(), 404, "{who} patch {id}");
            let delete = client.delete(&url).bearer_auth(token).send().await.unwrap();
            assert_eq!(delete.status(), 404, "{who} delete {id}");
        }
    }

    // untouched, and bob may reuse the name because names are per user
    let mine: Value = client
        .get(format!("{views}/{preset_id}"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mine["name"], "Alice's");
    let reuse = client
        .post(&views)
        .bearer_auth(&bob_token)
        .json(&json!({"surface": "dashboard", "name": "Alice's"}))
        .send()
        .await
        .unwrap();
    assert_eq!(reuse.status(), 200);
}

/// A preset that names something the caller can no longer read still applies
/// the rest (#1825): `effective_filters` drops the ids and `unavailable` says
/// which, for a key, a business unit and a customer, whether access was lost by
/// removing the membership or the row was deleted.
#[tokio::test]
async fn a_saved_view_reports_the_filters_the_user_can_no_longer_read() {
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

    async fn make(client: &reqwest::Client, url: String, body: Value) -> uuid::Uuid {
        let v: Value = client
            .post(url)
            .bearer_auth(admin_token())
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["id"].as_str().unwrap().parse().unwrap()
    }
    let org = make(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "ViewsOrg", "slug": "views-org"}),
    )
    .await;
    let other_org = make(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "ViewsOther", "slug": "views-other"}),
    )
    .await;
    let team = make(
        &client,
        format!("{base}/api/v1/orgs/{org}/teams"),
        json!({"name": "T"}),
    )
    .await;
    let project = make(
        &client,
        format!("{base}/api/v1/teams/{team}/projects"),
        json!({"name": "P"}),
    )
    .await;
    let kept_key = make(
        &client,
        format!("{base}/api/v1/projects/{project}/virtual-keys"),
        json!({"name": "kept"}),
    )
    .await;
    let doomed_key = make(
        &client,
        format!("{base}/api/v1/projects/{project}/virtual-keys"),
        json!({"name": "doomed"}),
    )
    .await;
    let unit = make(
        &client,
        format!("{base}/api/v1/orgs/{org}/business-units"),
        json!({"name": "Finance"}),
    )
    .await;
    let foreign_unit = make(
        &client,
        format!("{base}/api/v1/orgs/{other_org}/business-units"),
        json!({"name": "Elsewhere"}),
    )
    .await;
    let customer = make(
        &client,
        format!("{base}/api/v1/orgs/{org}/customers"),
        json!({"name": "Acme"}),
    )
    .await;

    let member = seed_user(&pool, "views-member@example.com", false).await;
    let token = seed_session(&pool, member, "viewsmember").await;
    seed_membership(&pool, member, None, None, Some(project), "viewer").await;

    let views = format!("{base}/api/v1/me/saved-views");
    let res = client
        .post(&views)
        .bearer_auth(&token)
        .json(&json!({
            "surface": "llm_logs",
            "name": "Everything",
            "filters": {
                "window": "30d",
                "model": "gpt-4o",
                "key": doomed_key,
                "business_unit": [unit, foreign_unit],
                "customer": [customer],
            },
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let created: Value = res.json().await.unwrap();
    let preset_id = created["id"].as_str().unwrap().to_string();
    let read = || async {
        client
            .get(format!("{views}/{preset_id}"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let sorted = |v: &Value| {
        let mut items: Vec<(String, String)> = v["unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| {
                (
                    u["filter"].as_str().unwrap().to_string(),
                    u["id"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        items.sort();
        items
    };

    // give the account a role at the org itself so that the unit and customer
    // in it are readable, and only the foreign unit is not
    seed_membership(&pool, member, Some(org), None, None, "viewer").await;
    let got = read().await;
    assert_eq!(
        sorted(&got),
        vec![("business_unit".to_string(), foreign_unit.to_string())],
        "{got}"
    );
    assert_eq!(got["effective_filters"]["key"], json!(doomed_key));
    assert_eq!(got["effective_filters"]["business_unit"], json!([unit]));
    assert_eq!(got["effective_filters"]["customer"], json!([customer]));
    // the stored filters are never rewritten by a read
    assert_eq!(got["filters"]["business_unit"], json!([unit, foreign_unit]));

    // the key is deleted: it is unavailable, the rest still applies
    let gone = client
        .delete(format!("{base}/api/v1/virtual-keys/{doomed_key}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert!(gone.status().is_success(), "{}", gone.status());
    let got = read().await;
    assert_eq!(
        sorted(&got),
        vec![
            ("business_unit".to_string(), foreign_unit.to_string()),
            ("key".to_string(), doomed_key.to_string()),
        ],
        "{got}"
    );
    let effective = &got["effective_filters"];
    assert!(effective.get("key").is_none(), "{got}");
    assert_eq!(effective["window"], "30d");
    assert_eq!(effective["model"], "gpt-4o");
    assert_eq!(effective["business_unit"], json!([unit]));
    assert_eq!(effective["customer"], json!([customer]));

    // both memberships go: the unit and customer are lost too, and the list
    // route says the same thing as the get route
    sqlx::query("delete from memberships where user_id = $1")
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();
    let listed: Value = client
        .get(format!("{views}?surface=llm_logs"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sorted(&listed[0]).len(), 4, "{listed}");
    assert_eq!(
        listed[0]["effective_filters"],
        json!({"window": "30d", "model": "gpt-4o"}),
        "the lists emptied out drop their keys"
    );

    // a preset that names a key still readable is untouched
    let res = client
        .post(&views)
        .bearer_auth(&token)
        .json(&json!({"surface": "llm_logs", "name": "Kept", "filters": {"key": kept_key}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let kept: Value = res.json().await.unwrap();
    // the membership is gone, so even this one reads as lost
    assert_eq!(
        kept["unavailable"],
        json!([{"filter": "key", "id": kept_key}])
    );
    seed_membership(&pool, member, None, None, Some(project), "viewer").await;
    let kept: Value = client
        .get(format!("{views}/{}", kept["id"].as_str().unwrap()))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(kept["unavailable"], json!([]));
    assert_eq!(kept["effective_filters"], json!({"key": kept_key}));
}

/// The `dashboard` surface takes the same row filters as `llm_logs` (#2453),
/// and a dashboard preset that names a key, business unit or customer the
/// caller can no longer read reports it exactly as an `llm_logs` one does:
/// `effective_filters` drops the id and `unavailable` names it.
#[tokio::test]
async fn a_dashboard_saved_view_holds_the_row_filters_and_reports_lost_ones() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn make(client: &reqwest::Client, url: String, body: Value) -> uuid::Uuid {
        let v: Value = client
            .post(url)
            .bearer_auth("admintok")
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["id"].as_str().unwrap().parse().unwrap()
    }
    let org = make(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "DashViewsOrg", "slug": "dash-views-org"}),
    )
    .await;
    let other_org = make(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "DashViewsOther", "slug": "dash-views-other"}),
    )
    .await;
    let team = make(
        &client,
        format!("{base}/api/v1/orgs/{org}/teams"),
        json!({"name": "T"}),
    )
    .await;
    let project = make(
        &client,
        format!("{base}/api/v1/teams/{team}/projects"),
        json!({"name": "P"}),
    )
    .await;
    let key = make(
        &client,
        format!("{base}/api/v1/projects/{project}/virtual-keys"),
        json!({"name": "dash"}),
    )
    .await;
    let unit = make(
        &client,
        format!("{base}/api/v1/orgs/{org}/business-units"),
        json!({"name": "Finance"}),
    )
    .await;
    let foreign_unit = make(
        &client,
        format!("{base}/api/v1/orgs/{other_org}/business-units"),
        json!({"name": "Elsewhere"}),
    )
    .await;
    let foreign_customer = make(
        &client,
        format!("{base}/api/v1/orgs/{other_org}/customers"),
        json!({"name": "Globex"}),
    )
    .await;

    let member = seed_user(&pool, "dash-views-member@example.com", false).await;
    let token = seed_session(&pool, member, "dashviewsmember").await;
    seed_membership(&pool, member, Some(org), None, None, "viewer").await;

    let views = format!("{base}/api/v1/me/saved-views");
    let res = client
        .post(&views)
        .bearer_auth(&token)
        .json(&json!({
            "surface": "dashboard",
            "name": "One team's spend",
            "filters": {
                "window": "7d",
                "bucket": "day",
                "model": "gpt-4o",
                "key": key,
                "business_unit": [unit, foreign_unit],
                "customer": [foreign_customer],
            },
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let created: Value = res.json().await.unwrap();
    let mut lost: Vec<(String, String)> = created["unavailable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            (
                u["filter"].as_str().unwrap().to_string(),
                u["id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    lost.sort();
    assert_eq!(
        lost,
        vec![
            ("business_unit".to_string(), foreign_unit.to_string()),
            ("customer".to_string(), foreign_customer.to_string()),
        ],
        "{created}"
    );
    assert_eq!(
        created["effective_filters"],
        json!({
            "window": "7d",
            "bucket": "day",
            "model": "gpt-4o",
            "key": key,
            "business_unit": [unit],
        }),
        "the readable filters still apply and the emptied list drops its key"
    );
    assert_eq!(
        created["filters"]["business_unit"],
        json!([unit, foreign_unit])
    );

    // the dashboard surface still refuses a filter its routes do not read
    let res = client
        .post(&views)
        .bearer_auth(&token)
        .json(&json!({
            "surface": "dashboard", "name": "Errors", "filters": {"status": "error"},
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}
