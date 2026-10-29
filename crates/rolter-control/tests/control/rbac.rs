//! Role checks: the capability matrix, custom roles and the guards that follow it.

use crate::support::*;
use serde_json::{json, Value};

/// One org cannot point its routes and groups at another org's providers, but
/// may reuse every name another org has (#1844, #1857): the snapshot serves
/// both orgs' rows, and a gateway that predates per-org namespaces is served
/// neither org's colliding rows rather than a snapshot it would refuse.
#[tokio::test]
async fn tenancy_guards_refuse_cross_org_references_but_not_shared_names() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let post = |url: String, body: Value| {
        let client = client.clone();
        async move {
            let resp = client.post(&url).json(&body).send().await.unwrap();
            let status = resp.status().as_u16();
            let json: Value = resp.json().await.unwrap_or(Value::Null);
            (status, json)
        }
    };
    let tenant = |slug: &'static str| {
        let base = base.clone();
        async move {
            let (_, org) = post(
                format!("{base}/api/v1/orgs"),
                json!({"name": slug, "slug": slug}),
            )
            .await;
            let org_id = org["id"].as_str().unwrap().to_string();
            let (_, team) = post(
                format!("{base}/api/v1/orgs/{org_id}/teams"),
                json!({"name": "core"}),
            )
            .await;
            let team_id = team["id"].as_str().unwrap().to_string();
            let (_, project) = post(
                format!("{base}/api/v1/teams/{team_id}/projects"),
                json!({"name": "app"}),
            )
            .await;
            (org_id, project["id"].as_str().unwrap().to_string(), team_id)
        }
    };
    let (org_a, project_a, team_a) = tenant("tenant-a").await;
    let (org_b, project_b, _) = tenant("tenant-b").await;
    let provider = |org: String, name: &'static str| {
        post(
            format!("{base}/api/v1/orgs/{org}/providers"),
            json!({"name": name, "kind": "openai_compatible", "api_base": "http://127.0.0.1:9"}),
        )
    };

    let (status, provider_a) = provider(org_a.clone(), "edge-a").await;
    assert_eq!(status, 200, "{provider_a}");
    let provider_a = provider_a["id"].as_str().unwrap().to_string();
    let (status, route_a) = post(
        format!("{base}/api/v1/projects/{project_a}/routes"),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    assert_eq!(status, 200, "{route_a}");
    let (status, _) = post(
        format!(
            "{base}/api/v1/routes/{}/targets",
            route_a["id"].as_str().unwrap()
        ),
        json!({"provider_id": provider_a, "weight": 1}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = post(
        format!("{base}/api/v1/orgs/{org_a}/provider-groups"),
        json!({"name": "pool-a", "strategy": "round_robin",
               "members": [{"provider_id": provider_a}]}),
    )
    .await;
    assert_eq!(status, 200);

    // names are per org: another org reuses each of them (#1857)
    let (status, same_name) = provider(org_b.clone(), "edge-a").await;
    assert_eq!(status, 200, "provider name: {same_name}");
    let (status, route_b_same) = post(
        format!("{base}/api/v1/projects/{project_b}/routes"),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    assert_eq!(status, 200, "route name: {route_b_same}");
    let (status, body) = post(
        format!(
            "{base}/api/v1/routes/{}/targets",
            route_b_same["id"].as_str().unwrap()
        ),
        json!({"provider_id": same_name["id"], "weight": 1}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    // within one org a name is still held once, across its projects, and the
    // refusal speaks of the org alone
    let (_, project_a2) = post(
        format!("{base}/api/v1/teams/{team_a}/projects"),
        json!({"name": "app-two"}),
    )
    .await;
    let (status, body) = post(
        format!(
            "{base}/api/v1/projects/{}/routes",
            project_a2["id"].as_str().unwrap()
        ),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    assert_eq!(
        status, 409,
        "route name in another project of the org: {body}"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("in this organization"),
        "{body}"
    );
    let (status, body) = post(
        format!("{base}/api/v1/orgs/{org_a}/providers"),
        json!({"name": "edge-a2", "slug": "edge-a", "kind": "openai_compatible",
               "api_base": "http://127.0.0.1:9"}),
    )
    .await;
    assert_eq!(status, 409, "provider slug in the org: {body}");

    // another org's provider is refused as a target and as a member, as if unknown
    let (status, provider_b) = provider(org_b.clone(), "edge-b").await;
    assert_eq!(status, 200, "{provider_b}");
    let (status, route_b) = post(
        format!("{base}/api/v1/projects/{project_b}/routes"),
        json!({"model": "gpt-4o-b", "strategy": "round_robin"}),
    )
    .await;
    assert_eq!(status, 200, "{route_b}");
    let (status, body) = post(
        format!(
            "{base}/api/v1/routes/{}/targets",
            route_b["id"].as_str().unwrap()
        ),
        json!({"provider_id": provider_a, "weight": 1}),
    )
    .await;
    assert_eq!(status, 404, "cross-org target: {body}");
    let (status, body) = post(
        format!("{base}/api/v1/orgs/{org_b}/provider-groups"),
        json!({"name": "pool-b", "strategy": "round_robin",
               "members": [{"provider_id": provider_a}]}),
    )
    .await;
    assert_eq!(status, 404, "cross-org member: {body}");
    let (status, body) = post(
        format!("{base}/api/v1/orgs/{org_b}/provider-groups"),
        json!({"name": "pool-a", "strategy": "round_robin",
               "members": [{"provider_id": provider_b["id"]}]}),
    )
    .await;
    assert_eq!(status, 200, "group slug another org uses: {body}");
    let (status, body) = post(
        format!("{base}/api/v1/orgs/{org_a}/provider-groups"),
        json!({"name": "pool-a again", "slug": "pool-a", "strategy": "round_robin",
               "members": [{"provider_id": provider_a}]}),
    )
    .await;
    assert_eq!(status, 409, "group slug in the org: {body}");

    // the fleet still converges and serves both orgs' rows, each naming the
    // org that owns it
    let snapshot: Value = client
        .get(format!("{base}/internal/snapshot?formats=1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let config = &snapshot["config"];
    assert!(config.is_object(), "snapshot refused: {snapshot}");
    let owners = |kind: &str, field: &str, name: &str| -> Vec<String> {
        let mut owners: Vec<String> = config[kind]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row[field] == name)
            .map(|row| row["tenancy"]["org_id"].as_str().unwrap().to_string())
            .collect();
        owners.sort();
        owners
    };
    let mut both = vec![org_a.clone(), org_b.clone()];
    both.sort();
    assert_eq!(owners("routes", "model", "gpt-4o"), both, "{config}");
    assert_eq!(owners("providers", "name", "edge-a"), both, "{config}");
    assert_eq!(
        owners("provider_groups", "slug", "pool-a"),
        both,
        "{config}"
    );
    let route = config["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["model"] == "gpt-4o" && r["tenancy"]["org_id"] == org_a.as_str())
        .unwrap();
    assert_eq!(route["tenancy"]["project_id"], project_a.as_str());
    // a gateway that states no formats predates per-org namespaces and would
    // refuse a repeated name: it is served neither org's colliding rows, and
    // told so, rather than a snapshot it would refuse (ADR-0040)
    let legacy: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(legacy["config"].is_object(), "snapshot refused: {legacy}");
    for (kind, field, name) in [
        ("routes", "model", "gpt-4o"),
        ("providers", "name", "edge-a"),
        ("provider_groups", "slug", "pool-a"),
    ] {
        assert!(
            legacy["config"][kind]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row[field] != name),
            "{kind} {name} served to a legacy gateway: {legacy}"
        );
    }
    assert!(
        legacy["problems"].as_array().unwrap().iter().any(|p| p
            .as_str()
            .unwrap_or_default()
            .contains("predates per-org namespaces")),
        "{legacy}"
    );
    // a row no other org shares is still served to it
    assert!(legacy["config"]["providers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "edge-b"));

    // deleting a model two orgs share names the org, and removes its rows only
    let delete = |query: String| {
        let client = client.clone();
        let url = format!("{base}/api/v1/models/gpt-4o{query}");
        async move { client.delete(&url).send().await.unwrap().status().as_u16() }
    };
    assert_eq!(delete(String::new()).await, 400, "an ambiguous delete");
    assert_eq!(delete(format!("?org_id={org_b}")).await, 204);
    let served: Value = client
        .get(format!("{base}/internal/snapshot?formats=1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let left: Vec<&str> = served["config"]["routes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["model"] == "gpt-4o")
        .filter_map(|r| r["tenancy"]["org_id"].as_str())
        .collect();
    assert_eq!(left, [org_a.as_str()], "{served}");
    // with one org left holding the name, the org may be left out again
    assert_eq!(delete(String::new()).await, 204);
    let edge_b = config["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "edge-b")
        .unwrap();
    assert_eq!(edge_b["tenancy"]["org_id"], org_b.as_str());

    // the anonymous dashboard config names no tenant
    let public: Value = client
        .get(format!("{base}/api/v1/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !public.to_string().contains(&org_a),
        "public config names an org"
    );
}

/// #3467: the organization-wide key list holds exactly the keys the per-project
/// list would show the caller, project by project: one request replaces a read
/// per project, and it widens nothing.
#[tokio::test]
async fn the_org_wide_key_list_holds_only_the_keys_the_caller_may_read() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("keylist".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let id_of = |sql: &'static str, parent: Option<uuid::Uuid>, name: &'static str| {
        let pool = pool.clone();
        async move {
            let query = sqlx::query_scalar::<_, uuid::Uuid>(sql);
            let query = match parent {
                Some(parent) => query.bind(parent),
                None => query,
            };
            query.bind(name).fetch_one(&pool).await.unwrap()
        }
    };
    let org_sql = "insert into orgs (name, slug) values ($1, $1) returning id";
    let team_sql = "insert into teams (org_id, name) values ($1, $2) returning id";
    let project_sql = "insert into projects (team_id, name) values ($1, $2) returning id";
    let acme = id_of(org_sql, None, "acme").await;
    let globex = id_of(org_sql, None, "globex").await;
    let core = id_of(team_sql, Some(acme), "core").await;
    let research = id_of(team_sql, Some(acme), "research").await;
    let payments = id_of(team_sql, Some(globex), "payments").await;
    let app_project = id_of(project_sql, Some(core), "app").await;
    let batch = id_of(project_sql, Some(core), "batch").await;
    let evals = id_of(project_sql, Some(research), "evals").await;
    let checkout = id_of(project_sql, Some(payments), "checkout").await;
    let mut keys = std::collections::HashMap::new();
    for (project, name) in [
        (app_project, "app-key"),
        (batch, "batch-key"),
        (evals, "evals-key"),
        (checkout, "checkout-key"),
    ] {
        let id: uuid::Uuid = sqlx::query_scalar(
            "insert into virtual_keys (project_id, key_hash, key_prefix, name) \
             values ($1, $2, $3, $4) returning id",
        )
        .bind(project)
        .bind(format!("hash-{name}"))
        .bind(&name[..3])
        .bind(name)
        .fetch_one(&pool)
        .await
        .unwrap();
        keys.insert(name, id.to_string());
    }

    let lead = seed_user(&pool, "lead@acme.test", false).await;
    seed_membership(&pool, lead, None, Some(core), None, "admin").await;
    let member = seed_user(&pool, "pm@acme.test", false).await;
    seed_membership(&pool, member, None, None, Some(app_project), "member").await;
    let org_admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, org_admin, Some(acme), None, None, "admin").await;
    let outsider = seed_user(&pool, "ops@globex.test", false).await;
    seed_membership(&pool, outsider, None, None, Some(checkout), "member").await;
    let lead_session = seed_session(&pool, lead, "keylist_lead").await;
    let pm = seed_session(&pool, member, "keylist_pm").await;
    let org_session = seed_session(&pool, org_admin, "keylist_org").await;
    let ops = seed_session(&pool, outsider, "keylist_ops").await;

    let list = |org: uuid::Uuid, token: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let resp = client
                .get(format!("{base}/api/v1/orgs/{org}/virtual-keys"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let body = resp.json::<Value>().await.unwrap_or(Value::Null);
            let mut names: Vec<String> = body
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .map(|row| row["name"].as_str().unwrap().to_string())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            (status, names, body)
        }
    };

    // an org admin: every key of the organization, none of another's
    let (status, names, body) = list(acme, org_session).await;
    assert_eq!(status, 200);
    assert_eq!(names, ["app-key", "batch-key", "evals-key"]);
    // each row names its project, and the secret's digest of a retired key
    // is never part of it
    let app_row = body
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "app-key")
        .unwrap();
    assert_eq!(app_row["project_id"], app_project.to_string());
    assert_eq!(app_row["id"], keys["app-key"]);
    // and no digest of any secret is part of a listing
    for field in ["key_hash", "previous_key_hash", "previous_key_prefix"] {
        assert!(app_row.get(field).is_none(), "{field} leaked: {app_row}");
    }
    assert_eq!(app_row["key_prefix"], "app");

    // a team admin: the keys of their own team's projects, not the sibling team's
    let (status, names, _) = list(acme, lead_session).await;
    assert_eq!(
        (status, names),
        (200, vec!["app-key".into(), "batch-key".into()])
    );
    // a project member: their own project's key only
    let (status, names, _) = list(acme, pm.clone()).await;
    assert_eq!((status, names), (200, vec!["app-key".to_string()]));

    // no role in the organization: refused, and an organization that is not
    // theirs reads the same as one that does not exist for them
    assert_eq!(list(acme, ops.clone()).await.0, 403);
    assert_eq!(list(globex, pm).await.0, 403);
    let (status, names, _) = list(globex, ops).await;
    assert_eq!((status, names), (200, vec!["checkout-key".to_string()]));
}

/// Listings answer a caller whose role sits below the org with what that
/// caller reaches, instead of refusing the whole list (#1846, #1850). A project
/// member can navigate to their own project, a team admin sees and manages
/// their own team's people, and nobody learns another tenant's org exists.
#[tokio::test]
async fn members_below_the_org_list_what_they_reach() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("listing".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let insert = |sql: &'static str, parent: Option<uuid::Uuid>, name: &'static str| {
        let pool = pool.clone();
        async move {
            let query = sqlx::query_scalar::<_, uuid::Uuid>(sql);
            let query = match parent {
                Some(parent) => query.bind(parent),
                None => query,
            };
            query.bind(name).fetch_one(&pool).await.unwrap()
        }
    };
    let org_sql = "insert into orgs (name, slug) values ($1, $1) returning id";
    let team_sql = "insert into teams (org_id, name) values ($1, $2) returning id";
    let project_sql = "insert into projects (team_id, name) values ($1, $2) returning id";
    let acme = insert(org_sql, None, "acme").await;
    let globex = insert(org_sql, None, "globex").await;
    let core = insert(team_sql, Some(acme), "core").await;
    let research = insert(team_sql, Some(acme), "research").await;
    let payments = insert(team_sql, Some(globex), "payments").await;
    let app_project = insert(project_sql, Some(core), "app").await;
    let batch = insert(project_sql, Some(core), "batch").await;
    let evals = insert(project_sql, Some(research), "evals").await;
    let checkout = insert(project_sql, Some(payments), "checkout").await;

    let member = seed_user(&pool, "pm@acme.test", false).await;
    seed_membership(&pool, member, None, None, Some(app_project), "member").await;
    let lead = seed_user(&pool, "lead@acme.test", false).await;
    seed_membership(&pool, lead, None, Some(core), None, "admin").await;
    let org_admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, org_admin, Some(acme), None, None, "admin").await;
    let outsider = seed_user(&pool, "ops@globex.test", false).await;
    seed_membership(&pool, outsider, None, None, Some(checkout), "member").await;
    let pm = seed_session(&pool, member, "listing_pm").await;
    let lead_session = seed_session(&pool, lead, "listing_lead").await;
    let org_session = seed_session(&pool, org_admin, "listing_org").await;
    let ops = seed_session(&pool, outsider, "listing_ops").await;

    let get = |path: String, token: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let resp = client
                .get(format!("{base}{path}"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let ids = |rows: &Value| -> Vec<String> {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_string())
            .collect()
    };

    // a project member: their own org, team and project, nothing else
    let (_, orgs) = get("/api/v1/orgs".into(), pm.clone()).await;
    assert_eq!(ids(&orgs), vec![acme.to_string()]);
    let (_, teams) = get(format!("/api/v1/orgs/{acme}/teams"), pm.clone()).await;
    assert_eq!(ids(&teams), vec![core.to_string()]);
    let (_, projects) = get(format!("/api/v1/teams/{core}/projects"), pm.clone()).await;
    assert_eq!(ids(&projects), vec![app_project.to_string()]);
    let (_, projects) = get(format!("/api/v1/orgs/{acme}/projects"), pm.clone()).await;
    assert_eq!(ids(&projects), vec![app_project.to_string()]);
    assert_eq!(
        get(format!("/api/v1/orgs/{globex}/teams"), pm.clone())
            .await
            .0,
        403
    );
    assert_eq!(
        get(format!("/api/v1/teams/{research}/projects"), pm.clone())
            .await
            .0,
        403
    );
    // and /auth/me says which org and team the project sits under
    let (_, me) = get("/api/v1/auth/me".into(), pm.clone()).await;
    assert_eq!(me["memberships"][0]["scope_org_id"], acme.to_string());
    assert_eq!(me["memberships"][0]["scope_team_id"], core.to_string());
    assert_eq!(me["memberships"][0]["project_id"], app_project.to_string());
    // the saved gateway base URL rides along for a role that cannot read
    // client settings (#2512); null until one is saved
    assert!(me["gateway_base_url"].is_null());
    sqlx::query(
        "update client_settings set public_base_url = 'https://gw.acme.test' where id = true",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, me) = get("/api/v1/auth/me".into(), pm.clone()).await;
    assert_eq!(me["gateway_base_url"], "https://gw.acme.test");
    let (denied, _) = get("/api/v1/client-settings".into(), pm.clone()).await;
    assert_eq!(denied, 403);
    // and the org's rule table, which the dashboard reads to say why a control
    // is disabled: the custom roles are the org's, a project role is enough
    let (status, matrix) = get(format!("/api/v1/rbac/matrix?org_id={acme}"), pm.clone()).await;
    assert_eq!(status, 200, "{matrix}");
    assert!(matrix["custom_roles"].is_array(), "{matrix}");

    // another tenant's member never sees that acme exists
    let (_, orgs) = get("/api/v1/orgs".into(), ops.clone()).await;
    assert_eq!(ids(&orgs), vec![globex.to_string()]);
    assert_eq!(
        get(format!("/api/v1/rbac/matrix?org_id={acme}"), ops.clone())
            .await
            .0,
        403
    );

    // a team admin: every project of their team, and their team's people
    let (_, projects) = get(
        format!("/api/v1/teams/{core}/projects"),
        lead_session.clone(),
    )
    .await;
    let mut both = vec![app_project.to_string(), batch.to_string()];
    both.sort();
    let mut listed = ids(&projects);
    listed.sort();
    assert_eq!(listed, both);
    let (_, memberships) = get(
        format!("/api/v1/orgs/{acme}/memberships"),
        lead_session.clone(),
    )
    .await;
    let people: Vec<&str> = memberships
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["user_id"].as_str().unwrap())
        .collect();
    assert!(people.contains(&lead.to_string().as_str()), "{memberships}");
    assert!(
        people.contains(&member.to_string().as_str()),
        "{memberships}"
    );
    assert!(
        !people.contains(&org_admin.to_string().as_str()),
        "org row leaked: {memberships}"
    );
    let (_, users) = get(format!("/api/v1/orgs/{acme}/users"), lead_session.clone()).await;
    let emails: Vec<&str> = users
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["email"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["lead@acme.test", "pm@acme.test"]);

    // invitations: the team admin sees and revokes their team's, not research's
    let invite = |scope_id: uuid::Uuid, email: &'static str| {
        client
            .post(format!("{base}/api/v1/orgs/{acme}/invitations"))
            .bearer_auth("listing")
            .json(&json!({"email": email, "role": "member",
                          "scope_type": "project", "scope_id": scope_id}))
            .send()
    };
    let ours: Value = invite(batch, "new@acme.test")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let theirs: Value = invite(evals, "eval@acme.test")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ours = ours["invitation"]["id"].as_str().unwrap().to_string();
    let theirs = theirs["invitation"]["id"].as_str().unwrap().to_string();
    let (_, invitations) = get(
        format!("/api/v1/orgs/{acme}/invitations"),
        lead_session.clone(),
    )
    .await;
    assert_eq!(ids(&invitations), vec![ours.clone()]);
    let revoke = |id: String| {
        client
            .delete(format!("{base}/api/v1/invitations/{id}"))
            .bearer_auth(lead_session.clone())
            .send()
    };
    assert_eq!(revoke(ours).await.unwrap().status(), 200);
    assert_eq!(revoke(theirs).await.unwrap().status(), 403);

    // an org role still gets the whole list
    let (_, memberships) = get(format!("/api/v1/orgs/{acme}/memberships"), org_session).await;
    assert_eq!(memberships.as_array().unwrap().len(), 3, "{memberships}");
}

/// Editing a budget or rate limit takes `update` on it, which the matrix
/// grants to an admin of the scope and not to a viewer (#1285). Both reach the
/// row's own scope chain, so the check is against where the cap already lives.
#[tokio::test]
async fn editing_a_cap_takes_admin_on_its_scope() {
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

    async fn post_as(client: &reqwest::Client, url: String, body: Value) -> Value {
        let response = client
            .post(url)
            .bearer_auth(admin_token())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let json: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{status}: {json}");
        json
    }

    let org = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Caps", "slug": "caps"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();
    let budget = post_as(
        &client,
        format!("{base}/api/v1/budgets"),
        json!({"scope_type": "org", "scope_id": org_id, "limit_usd": "10"}),
    )
    .await;
    let limit = post_as(
        &client,
        format!("{base}/api/v1/rate-limits"),
        json!({"scope_type": "org", "scope_id": org_id, "rpm": 10}),
    )
    .await;
    let budget_url = format!("{base}/api/v1/budgets/{}", budget["id"].as_str().unwrap());
    let limit_url = format!(
        "{base}/api/v1/rate-limits/{}",
        limit["id"].as_str().unwrap()
    );

    let viewer = seed_user(&pool, "caps-viewer@example.com", false).await;
    seed_membership(&pool, viewer, Some(org_uuid), None, None, "viewer").await;
    let viewer_token = seed_session(&pool, viewer, "capsviewer").await;
    let admin = seed_user(&pool, "caps-admin@example.com", false).await;
    seed_membership(&pool, admin, Some(org_uuid), None, None, "admin").await;
    let admin_token = seed_session(&pool, admin, "capsadmin").await;
    // an admin, but of another org: the edit is authorized against the org
    // the cap lives in, so admin anywhere else is not enough
    let other_org = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Elsewhere", "slug": "elsewhere"}),
    )
    .await;
    let other_org_uuid: uuid::Uuid = other_org["id"].as_str().unwrap().parse().unwrap();
    let outsider = seed_user(&pool, "caps-outsider@example.com", false).await;
    seed_membership(&pool, outsider, Some(other_org_uuid), None, None, "admin").await;
    let outsider_token = seed_session(&pool, outsider, "capsoutsider").await;

    for (url, body) in [
        (&budget_url, json!({"limit_usd": "20"})),
        (&limit_url, json!({"rpm": 20})),
    ] {
        let refused = client
            .patch(url.as_str())
            .bearer_auth(&viewer_token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 403, "a viewer edited {url}");

        let refused = client
            .patch(url.as_str())
            .bearer_auth(&outsider_token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            refused.status(),
            403,
            "an admin of another org edited {url}"
        );

        let allowed = client
            .patch(url.as_str())
            .bearer_auth(&admin_token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(allowed.status(), 200, "an org admin could not edit {url}");
    }
}

/// Two edits that each lift a different cap must not together leave a rate
/// limit with none (#1285). The "keep at least one cap" rule is checked
/// against the row under the edit's own lock, so whichever edit lands second
/// sees the first one's result and is refused, and the row always limits
/// something. Checked against a read taken before the write, both edits would
/// pass and store a limit that admits everything.
#[tokio::test]
async fn concurrent_edits_cannot_lift_every_cap() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app(pool.clone())
            .await
            .expect("build app"),
    )
    .await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Racing", "slug": "racing"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().expect("org id").to_string();

    // a few rounds, since a single one can land both edits in order by luck
    for round in 0..5 {
        let created: Value = client
            .post(format!("{base}/api/v1/rate-limits"))
            .json(&json!({"scope_type": "org", "scope_id": org_id, "rpm": 60, "tpm": 1000}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = created["id"].as_str().expect("rate limit id").to_string();
        let url = format!("{base}/api/v1/rate-limits/{id}");

        let (lift_rpm, lift_tpm) = tokio::join!(
            client.patch(&url).json(&json!({"rpm": null})).send(),
            client.patch(&url).json(&json!({"tpm": null})).send(),
        );
        let mut statuses = [
            lift_rpm.unwrap().status().as_u16(),
            lift_tpm.unwrap().status().as_u16(),
        ];
        statuses.sort_unstable();
        assert_eq!(
            statuses,
            [200, 400],
            "round {round}: exactly one lift may land"
        );

        let (rpm, tpm): (Option<i32>, Option<i32>) =
            sqlx::query_as("select rpm, tpm from rate_limits where id = $1::uuid")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            rpm.is_some() != tpm.is_some(),
            "round {round}: one cap should remain, got rpm={rpm:?} tpm={tpm:?}"
        );
    }
}

/// With an admin token configured, the CRUD API and snapshot endpoint reject
/// unauthenticated calls and accept the bearer token.
#[tokio::test]
async fn admin_token_guards_crud_and_snapshot() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool, Some(admin_token().to_string()))
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
        .bearer_auth(admin_token())
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap();
    assert!(allowed.status().is_success(), "{}", allowed.status());
}

/// A forged bearer is refused on every operation the served OpenAPI document
/// does not mark public, once an admin token is configured (#1820, #2464).
///
/// The anonymous half of this used to live here too. It is now
/// `no_route_answers_an_anonymous_caller_unless_it_is_allowlisted` in
/// `src/public_routes.rs`, which covers every method rather than only GET,
/// pins `.public()` to a reasoned allowlist both ways, and needs no database:
/// an anonymous caller has to be refused before any handler reaches the pool.
///
/// What that guard cannot exercise is a bearer that is present but wrong. It
/// misses the admin token, so the extractor looks it up as a session token,
/// and that lookup needs a real database — against the guard's pool that
/// never connects it answers an error rather than "no such session". So this
/// is the one case that stays here: a forged token must come back `401`, not
/// a `500` from a lookup that trips over it nor a `200` from an extractor
/// that treats any bearer as good enough. The `/gw` proxy is no longer exempt:
/// it takes a session since #2463, and a forged one is refused like any other.
#[tokio::test]
async fn every_route_the_spec_does_not_mark_public_refuses_a_forged_bearer() {
    skip_without_db!();
    let db = fresh_db().await;
    let app =
        rolter_control::test_app_with_admin_token(db.pool().clone(), Some("sweep".to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let spec: Value = client
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let nil = uuid::Uuid::nil().to_string();
    let mut answered = Vec::new();
    let mut walked = 0;
    for (path, item) in spec["paths"].as_object().expect("the document has paths") {
        // every path parameter becomes the nil uuid: a guarded route has to
        // refuse the caller before it looks anything up
        let mut concrete = String::with_capacity(path.len());
        let mut rest = path.as_str();
        while let Some(open) = rest.find('{') {
            let close = rest[open..].find('}').expect("closed parameter") + open;
            concrete.push_str(&rest[..open]);
            concrete.push_str(&nil);
            rest = &rest[close + 1..];
        }
        concrete.push_str(rest);
        for method in ["get", "post", "put", "patch", "delete"] {
            let Some(op) = item.get(method) else {
                continue;
            };
            if op.get("security") == Some(&json!([])) {
                continue;
            }
            let verb = reqwest::Method::from_bytes(method.to_uppercase().as_bytes())
                .expect("a standard method");
            let status = client
                .request(verb, format!("http://{addr}{concrete}"))
                .bearer_auth("forged")
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status();
            if status != 401 {
                answered.push(format!("{} {path} -> {status}", method.to_uppercase()));
            }
            walked += 1;
        }
    }
    assert!(walked > 50, "the sweep only reached {walked} operations");
    assert!(
        answered.is_empty(),
        "operations that did not refuse a forged bearer: {answered:#?}"
    );
}

/// Only a project admin changes who reads a project's captured bodies, only to
/// a role the column admits, and the change is audited (#1820).
#[tokio::test]
async fn project_settings_are_read_by_viewers_and_changed_by_project_admins() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("settings".to_string()))
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
    let mut tokens = std::collections::HashMap::new();
    for role in ["viewer", "member", "admin"] {
        let user = seed_user(&pool, &format!("{role}@settings.test"), false).await;
        seed_membership(&pool, user, None, None, Some(project), role).await;
        tokens.insert(
            role,
            seed_session(&pool, user, &format!("settings_{role}")).await,
        );
    }
    let settings = format!("{base}/api/v1/projects/{project}/settings");
    let read = |role: &'static str| client.get(&settings).bearer_auth(&tokens[role]).send();
    let write = |role: &'static str, value: &'static str| {
        client
            .put(&settings)
            .bearer_auth(&tokens[role])
            .json(&json!({"payload_min_role": value}))
            .send()
    };

    // members by default, and any role on the project may read the setting
    let current: Value = read("viewer").await.unwrap().json().await.unwrap();
    assert_eq!(current, json!({"payload_min_role": "member"}));

    // a member cannot lower the bar for everyone else
    assert_eq!(write("member", "viewer").await.unwrap().status(), 403);
    // and an admin cannot set a role the column does not admit
    assert_eq!(write("admin", "admin").await.unwrap().status(), 400);

    let changed = write("admin", "viewer").await.unwrap();
    assert_eq!(changed.status(), 200);
    let current: Value = read("viewer").await.unwrap().json().await.unwrap();
    assert_eq!(current, json!({"payload_min_role": "viewer"}));

    // the viewer's effective permissions at the project now include the bodies
    let effective: Value = client
        .get(format!(
            "{base}/api/v1/rbac/effective?org_id={org}&team_id={team}&project_id={project}"
        ))
        .bearer_auth(&tokens["viewer"])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let allowed: Vec<&str> = effective["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(allowed.contains(&"request_payload:read"), "{allowed:?}");

    let audited: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action = 'project.settings.update' \
         and target_id = $1 and detail->>'payload_min_role' = 'viewer'",
    )
    .bind(project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, 1);

    // a viewer of another org who names acme's project beside their own org
    // resolves a role at that assembled chain, but holds none on the project's
    // real one: the setting is acme's, and it must not come back as theirs
    let umbrella: uuid::Uuid = sqlx::query_scalar(
        "insert into orgs (name, slug) values ('Umbrella', 'umbrella') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider = seed_user(&pool, "outsider@settings.test", false).await;
    seed_membership(&pool, outsider, Some(umbrella), None, None, "viewer").await;
    let outsider_token = seed_session(&pool, outsider, "settings_outsider").await;
    let effective: Value = client
        .get(format!(
            "{base}/api/v1/rbac/effective?org_id={umbrella}&project_id={project}"
        ))
        .bearer_auth(&outsider_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(effective["role"], json!("viewer"), "{effective}");
    let allowed: Vec<&str> = effective["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(!allowed.contains(&"request_payload:read"), "{allowed:?}");
}

/// The batched project→team lookup must reach the same verdict the
/// per-membership `ProjectRepo::get` loop did (#1048).
///
/// Covers the case the old loop was shaped for and the new query has to
/// reproduce: a user whose only route to the resource is a *project*
/// membership, holding several of them, where the qualifying one is not first.
/// A user with just as many project memberships and none in an allowed team is
/// still refused, so the batch is not simply answering "yes" on any row.
#[tokio::test]
async fn policy_allows_resolves_several_project_memberships_in_one_verdict() {
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

    async fn post_as(client: &reqwest::Client, url: String, body: Value) -> Value {
        let response = client
            .post(url)
            .bearer_auth(admin_token())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let json: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{status}: {json}");
        json
    }

    let org = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Batched", "slug": "batched"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();

    // three teams, one of which the skill allows
    let mut team_ids = Vec::new();
    for name in ["Alpha", "Beta", "Gamma"] {
        let team = post_as(
            &client,
            format!("{base}/api/v1/orgs/{org_id}/teams"),
            json!({ "name": name }),
        )
        .await;
        team_ids.push(team["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap());
    }
    // the allowed team is deliberately last, so a lookup that stopped at the
    // first row would answer "denied" and fail this test
    let allowed_team = team_ids[2];

    // one project per team
    let mut project_ids = Vec::new();
    for (index, team) in team_ids.iter().enumerate() {
        let project = post_as(
            &client,
            format!("{base}/api/v1/teams/{team}/projects"),
            json!({ "name": format!("project-{index}") }),
        )
        .await;
        project_ids.push(
            project["id"]
                .as_str()
                .unwrap()
                .parse::<uuid::Uuid>()
                .unwrap(),
        );
    }

    let skill = post_as(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/skills"),
        json!({
            "name": "batched-policy",
            "allowed_team_ids": [allowed_team],
            "minimum_role": "member"
        }),
    )
    .await;
    let skill_id = skill["id"].as_str().unwrap().to_string();
    post_as(
        &client,
        format!("{base}/api/v1/skills/{skill_id}/versions"),
        json!({"content": "content"}),
    )
    .await;

    // reaches the skill only through the third project, and only after two
    // project memberships that do not qualify
    // team_id is left unset on purpose. A membership carrying both would be
    // answered by the direct team check before the project lookup runs, so the
    // batched query — the thing under test — would never execute
    let allowed_user = seed_user(&pool, "batched-allowed@example.com", false).await;
    seed_membership(&pool, allowed_user, Some(org_uuid), None, None, "member").await;
    for project in &project_ids {
        seed_membership(
            &pool,
            allowed_user,
            Some(org_uuid),
            None,
            Some(*project),
            "viewer",
        )
        .await;
    }
    let allowed_token = seed_session(&pool, allowed_user, "batched-allowed").await;

    // just as many project memberships, none of them in the allowed team
    let denied_user = seed_user(&pool, "batched-denied@example.com", false).await;
    seed_membership(&pool, denied_user, Some(org_uuid), None, None, "member").await;
    for project in &project_ids[..2] {
        seed_membership(
            &pool,
            denied_user,
            Some(org_uuid),
            None,
            Some(*project),
            "viewer",
        )
        .await;
    }
    let denied_token = seed_session(&pool, denied_user, "batched-denied").await;

    async fn skills_for(client: &reqwest::Client, base: &str, org: &str, token: &str) -> Value {
        client
            .get(format!("{base}/api/v1/orgs/{org}/skills"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    let allowed = skills_for(&client, &base, &org_id, &allowed_token).await;
    assert_eq!(
        allowed.as_array().unwrap().len(),
        1,
        "a project membership in the allowed team must grant access: {allowed}"
    );

    let denied = skills_for(&client, &base, &org_id, &denied_token).await;
    assert!(
        denied.as_array().unwrap().is_empty(),
        "project memberships outside the allowed team must not grant access: {denied}"
    );
}

/// The capability matrix and the caller's effective permissions are served by
/// the control plane rather than assembled in the browser (#534): the matrix
/// describes the rules, `effective` answers for the caller at a scope, and the
/// answer matches what the CRUD guard actually does.
#[tokio::test]
async fn rbac_matrix_and_effective_permissions_are_api_backed() {
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

    // the matrix needs authentication but no particular role
    let unauth = client
        .get(format!("{base}/api/v1/rbac/matrix"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);

    let matrix: Value = client
        .get(format!("{base}/api/v1/rbac/matrix"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(matrix["roles"].as_array().unwrap().len(), 3);
    let provider = matrix["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["resource"] == "provider")
        .expect("provider in the matrix");
    let create = provider["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["action"] == "create")
        .unwrap();
    assert_eq!(create["minimum_role"], "admin");
    assert_eq!(create["superadmin_only"], false);
    // deployment-wide policy is not something an org admin can reach: the row
    // takes no scoped role, and names the one deployment role that also does
    let flags = matrix["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["resource"] == "feature_flags")
        .expect("feature_flags in the matrix");
    assert_eq!(flags["actions"][0]["minimum_role"], Value::Null);
    assert_eq!(flags["actions"][0]["deployment_roles"], json!(["operator"]));
    let grant = matrix["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["resource"] == "deployment_role")
        .expect("deployment_role in the matrix");
    assert_eq!(grant["actions"][0]["superadmin_only"], true);

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "MatrixOrg", "slug": "matrix-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();

    let viewer = seed_user(&pool, "matrix-viewer@example.com", false).await;
    seed_membership(&pool, viewer, Some(org_uuid), None, None, "viewer").await;
    let viewer_token = seed_session(&pool, viewer, "matrixviewer").await;

    let effective: Value = client
        .get(format!("{base}/api/v1/rbac/effective?org_id={org_id}"))
        .bearer_auth(&viewer_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(effective["superadmin"], false);
    assert_eq!(effective["role"], "viewer");
    let allowed: Vec<String> = serde_json::from_value(effective["allowed"].clone()).unwrap();
    assert!(allowed.contains(&"provider:read".to_string()));
    assert!(!allowed.contains(&"provider:create".to_string()));

    // and the guard agrees with what `effective` reported
    let denied = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .bearer_auth(&viewer_token)
        .json(&json!({"name": "T"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);

    // an org the caller has no membership in yields no permissions at all
    let other: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "OtherOrg", "slug": "other-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap();
    let elsewhere: Value = client
        .get(format!("{base}/api/v1/rbac/effective?org_id={other_id}"))
        .bearer_auth(&viewer_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(elsewhere["role"].is_null());
    // nothing org-scoped is reachable there; what remains is exactly the
    // global read-only facts and the account's own password, which take
    // authentication and no membership
    let elsewhere_allowed: Vec<&str> = elsewhere["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(
        elsewhere_allowed,
        vec![
            "model_label:read",
            "model_price:read",
            "model:read",
            "version:read",
            "stability:read",
            "public_url:read",
            "my_password:update",
            "my_budget:read",
            // the fleet's breaker state and gateway counts (#2254, #2013): the
            // handlers narrow or count, so they take authentication alone
            "breaker_state:read",
            "cluster_summary:read"
        ]
    );

    // a project-scoped admin inherits nothing upward: the same user is only an
    // admin inside the project chain they were granted
    let team: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "MatrixTeam"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let team_id = team["id"].as_str().unwrap().to_string();
    let team_uuid: uuid::Uuid = team_id.parse().unwrap();
    let project: Value = client
        .post(format!("{base}/api/v1/teams/{team_id}/projects"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "MatrixProject"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = project["id"].as_str().unwrap().to_string();
    let project_uuid: uuid::Uuid = project_id.parse().unwrap();

    let scoped = seed_user(&pool, "matrix-project-admin@example.com", false).await;
    seed_membership(
        &pool,
        scoped,
        Some(org_uuid),
        Some(team_uuid),
        Some(project_uuid),
        "admin",
    )
    .await;
    let scoped_token = seed_session(&pool, scoped, "matrixscoped").await;

    let in_project: Value = client
        .get(format!(
            "{base}/api/v1/rbac/effective?org_id={org_id}&team_id={team_id}&project_id={project_id}"
        ))
        .bearer_auth(&scoped_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(in_project["role"], "admin");

    let at_org: Value = client
        .get(format!("{base}/api/v1/rbac/effective?org_id={org_id}"))
        .bearer_auth(&scoped_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        at_org["role"].is_null(),
        "a project-scoped grant must not authorize the whole org"
    );
}

/// With an admin token configured (RBAC enforcement active), every control
/// mutation is checked against the caller's role at the resource's scope:
/// viewers are denied, scoped admins are allowed only within their scope,
/// cross-scope admins are denied, superadmins bypass, and the machine admin
/// token bypasses. Covers ROL-33 (resolver) + ROL-34 (enforcement).
#[tokio::test]
async fn rbac_enforced_on_every_mutation() {
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
        admin_token(),
        json!({"name": "OrgA", "slug": "org-a"}),
    )
    .await;
    let org_a_id = org_a["id"].as_str().unwrap().to_string();
    let org_b = post_as(
        &client,
        &format!("{base}/api/v1/orgs"),
        admin_token(),
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
        .bearer_auth(admin_token())
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
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
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

// ---------------------------------------------------------------------------
// configurable rbac (#534)
// ---------------------------------------------------------------------------

/// A custom role widens a member beyond what their base role allows, and only
/// inside the scope the profile assigns it — the deny case is the point.
#[tokio::test]
async fn custom_role_grant_widens_a_member_within_its_scope_only() {
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

    // two orgs, so "in scope" can be told apart from "authorized everywhere"
    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let other: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Other", "slug": "other"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap().to_string();

    // a plain member of both orgs: creating a provider is admin-only, so both
    // attempts must fail before any custom role exists
    let user = seed_user(&pool, "member@acme.test", false).await;
    seed_membership(
        &pool,
        user,
        Some(org_id.parse().unwrap()),
        None,
        None,
        "member",
    )
    .await;
    seed_membership(
        &pool,
        user,
        Some(other_id.parse().unwrap()),
        None,
        None,
        "member",
    )
    .await;
    let token = seed_session(&pool, user, "custom-role").await;

    let create_provider = |org: String| {
        client
            .post(format!("{base}/api/v1/orgs/{org}/providers"))
            .bearer_auth(token.clone())
            .json(&json!({
                "name": format!("p-{}", uuid::Uuid::new_v4()),
                "kind": "openai",
                "api_base": "https://api.openai.com",
            }))
            .send()
    };

    assert_eq!(
        create_provider(org_id.clone()).await.unwrap().status(),
        403,
        "a member must not create a provider before the grant exists"
    );

    // a custom role that grants exactly provider:create, still on the member base
    let role: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/custom-roles"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Provider Wrangler",
            "base_role": "member",
            "grants": [{"resource": "provider", "action": "create"}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let role_id = role["id"].as_str().unwrap().to_string();

    // composed into a profile scoped to the first org, assigned to the user
    let profile: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/access-profiles"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Acme Wranglers",
            "roles": [{"role_id": role_id, "org_id": org_id}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let profile_id = profile["id"].as_str().unwrap().to_string();

    let assigned = client
        .post(format!(
            "{base}/api/v1/access-profiles/{profile_id}/assignments"
        ))
        .bearer_auth(admin_token())
        .json(&json!({"user_id": user}))
        .send()
        .await
        .unwrap();
    assert!(assigned.status().is_success(), "{}", assigned.status());

    // in scope the grant applies; in the other org the same member is still refused
    let allowed = create_provider(org_id.clone()).await.unwrap();
    assert!(
        allowed.status().is_success(),
        "granted org must allow the create: {}",
        allowed.status()
    );
    assert_eq!(
        create_provider(other_id.clone()).await.unwrap().status(),
        403,
        "the grant must not leak into an org the profile does not name"
    );

    // and it is revocable: dropping the assignment closes the door again
    let assignments: Value = client
        .get(format!(
            "{base}/api/v1/access-profiles/{profile_id}/assignments"
        ))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let assignment_id = assignments[0]["id"].as_str().unwrap().to_string();
    let removed = client
        .delete(format!(
            "{base}/api/v1/access-profile-assignments/{assignment_id}"
        ))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert!(removed.status().is_success(), "{}", removed.status());
    assert_eq!(
        create_provider(org_id).await.unwrap().status(),
        403,
        "removing the assignment must withdraw the grant"
    );
}

/// The org-wide assignment read (#3025) answers what walking every profile's
/// assignments would, never wider: bounded to the org's own profiles, gated like
/// the per-profile read, and narrowed by one subject on request.
#[tokio::test]
async fn assignments_list_by_subject_within_the_org_and_gate_like_the_profile_read() {
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
    let send_json = |method: reqwest::Method, url: String, body: Value| {
        let client = client.clone();
        async move {
            let res = client
                .request(method, url)
                .bearer_auth(admin_token())
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = res.status().as_u16();
            (status, res.json::<Value>().await.unwrap())
        }
    };

    let mut orgs = Vec::new();
    for slug in ["acme", "globex"] {
        let (status, org) = send_json(
            reqwest::Method::POST,
            format!("{base}/api/v1/orgs"),
            json!({"name": slug, "slug": slug}),
        )
        .await;
        assert_eq!(status, 200, "{org}");
        orgs.push(org["id"].as_str().unwrap().to_string());
    }
    let (acme, globex) = (orgs[0].clone(), orgs[1].clone());
    let (status, team) = send_json(
        reqwest::Method::POST,
        format!("{base}/api/v1/orgs/{acme}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    assert_eq!(status, 200, "{team}");
    let team_id = team["id"].as_str().unwrap().to_string();
    let member = seed_user(&pool, "member@example.com", false).await;
    let other = seed_user(&pool, "other@example.com", false).await;

    let mut profiles = Vec::new();
    for (org, slug) in [
        (&acme, "support"),
        (&acme, "on-call"),
        (&globex, "auditors"),
    ] {
        let (status, profile) = send_json(
            reqwest::Method::POST,
            format!("{base}/api/v1/orgs/{org}/access-profiles"),
            json!({"name": slug, "slug": slug}),
        )
        .await;
        assert_eq!(status, 200, "{profile}");
        profiles.push(profile["id"].as_str().unwrap().to_string());
    }
    let assign = |profile: String, subject: Value| {
        let base = base.clone();
        let send_json = &send_json;
        async move {
            let (status, body) = send_json(
                reqwest::Method::POST,
                format!("{base}/api/v1/access-profiles/{profile}/assignments"),
                subject,
            )
            .await;
            assert_eq!(status, 200, "{body}");
        }
    };
    assign(profiles[0].clone(), json!({"user_id": member})).await;
    assign(profiles[1].clone(), json!({"team_id": team_id})).await;
    assign(profiles[1].clone(), json!({"user_id": other})).await;
    // the same person holds a profile in the other org too
    assign(profiles[2].clone(), json!({"user_id": member})).await;

    let list = |org: String, query: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let res = client
                .get(format!(
                    "{base}/api/v1/orgs/{org}/access-profile-assignments{query}"
                ))
                .bearer_auth(admin_token())
                .send()
                .await
                .unwrap();
            let status = res.status().as_u16();
            (status, res.json::<Value>().await.unwrap())
        }
    };
    let profile_ids = |body: &Value| -> Vec<String> {
        let mut ids: Vec<String> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["profile_id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    };
    let sorted = |ids: &[&String]| -> Vec<String> {
        let mut ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        ids.sort();
        ids
    };

    // unfiltered: every assignment of this org's profiles, none of the other's
    let (status, all) = list(acme.clone(), String::new()).await;
    assert_eq!(status, 200, "{all}");
    assert_eq!(all.as_array().unwrap().len(), 3, "{all}");
    assert_eq!(
        profile_ids(&all),
        sorted(&[&profiles[0], &profiles[1], &profiles[1]])
    );

    // one user: only the profiles of this org, though they hold another's too
    let (status, mine) = list(acme.clone(), format!("?user_id={member}")).await;
    assert_eq!(status, 200, "{mine}");
    assert_eq!(profile_ids(&mine), sorted(&[&profiles[0]]));
    let (_, theirs) = list(globex.clone(), format!("?user_id={member}")).await;
    assert_eq!(profile_ids(&theirs), sorted(&[&profiles[2]]));

    // one team
    let (status, held) = list(acme.clone(), format!("?team_id={team_id}")).await;
    assert_eq!(status, 200, "{held}");
    assert_eq!(profile_ids(&held), sorted(&[&profiles[1]]));
    assert_eq!(held[0]["team_id"], team_id.as_str(), "{held}");

    // a subject that holds nothing is an empty list, not an error
    let (status, none) = list(acme.clone(), format!("?user_id={}", uuid::Uuid::new_v4())).await;
    assert_eq!(status, 200, "{none}");
    assert!(none.as_array().unwrap().is_empty(), "{none}");

    // both subjects at once is a field refusal, a malformed id a coded query one
    let (status, body) = list(acme.clone(), format!("?user_id={member}&team_id={team_id}")).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid_field", "{body}");
    assert_eq!(body["error"]["field"], "team_id", "{body}");
    let (status, body) = list(acme.clone(), "?user_id=not-a-uuid".to_string()).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid_query", "{body}");

    // gated like the per-profile read: a member of another org is refused on
    // both, and a plain member of this org sees exactly what that read allows
    let outsider = seed_user(&pool, "outsider@example.com", false).await;
    seed_membership(
        &pool,
        outsider,
        Some(globex.parse().unwrap()),
        None,
        None,
        "admin",
    )
    .await;
    let outsider_token = seed_session(&pool, outsider, "assignments-outsider").await;
    let insider = seed_user(&pool, "insider@example.com", false).await;
    seed_membership(
        &pool,
        insider,
        Some(acme.parse().unwrap()),
        None,
        None,
        "member",
    )
    .await;
    let insider_token = seed_session(&pool, insider, "assignments-insider").await;
    for (token, name) in [(&outsider_token, "outsider"), (&insider_token, "insider")] {
        let by_profile = client
            .get(format!(
                "{base}/api/v1/access-profiles/{}/assignments",
                profiles[0]
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status();
        let by_org = client
            .get(format!(
                "{base}/api/v1/orgs/{acme}/access-profile-assignments?user_id={member}"
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(by_org, by_profile, "{name}: the org read must not be wider");
    }
    assert_eq!(
        client
            .get(format!(
                "{base}/api/v1/orgs/{acme}/access-profile-assignments"
            ))
            .bearer_auth(&outsider_token)
            .send()
            .await
            .unwrap()
            .status(),
        403,
        "another org's admin must not read this org's assignments"
    );
}

/// A custom role in use cannot be deleted out from under its assignments, and
/// every change to one is audited.
#[tokio::test]
async fn custom_role_changes_are_guarded_by_references_and_audited() {
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
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let role: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/custom-roles"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Auditor",
            "base_role": "viewer",
            "grants": [{"resource": "audit_log", "action": "read"}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let role_id = role["id"].as_str().unwrap().to_string();

    let profile: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/access-profiles"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Auditors",
            "roles": [{"role_id": role_id, "org_id": org_id}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let profile_id = profile["id"].as_str().unwrap().to_string();

    // referenced by a profile, so the delete must be refused rather than
    // silently stripping the profile's composition
    let refused = client
        .delete(format!("{base}/api/v1/custom-roles/{role_id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        409,
        "deleting a referenced role must conflict, not cascade"
    );

    // detach it, then the delete goes through
    let detached = client
        .put(format!("{base}/api/v1/access-profiles/{profile_id}"))
        .bearer_auth(admin_token())
        .json(&json!({"roles": []}))
        .send()
        .await
        .unwrap();
    assert!(detached.status().is_success(), "{}", detached.status());
    let deleted = client
        .delete(format!("{base}/api/v1/custom-roles/{role_id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert!(deleted.status().is_success(), "{}", deleted.status());

    let actions: Vec<String> =
        sqlx::query_scalar("select action from audit_log where action like 'custom_role%' or action like 'access_profile%' order by at")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        actions.iter().any(|a| a == "custom_role.create"),
        "creating a role must be audited: {actions:?}"
    );
    assert!(
        actions.iter().any(|a| a == "custom_role.delete"),
        "deleting a role must be audited: {actions:?}"
    );
}

/// The matrix endpoint is API-backed: a role created through the API shows up
/// in the next read, so the dashboard never has to trust its own state.
#[tokio::test]
async fn rbac_matrix_reflects_custom_roles_after_a_change() {
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
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let before: Value = client
        .get(format!("{base}/api/v1/rbac/matrix?org_id={org_id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        before["custom_roles"].as_array().unwrap().is_empty(),
        "a fresh org has no custom roles: {before}"
    );

    client
        .post(format!("{base}/api/v1/orgs/{org_id}/custom-roles"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Budget Keeper",
            "base_role": "member",
            "grants": [{"resource": "budget", "action": "update"}],
        }))
        .send()
        .await
        .unwrap();

    let after: Value = client
        .get(format!("{base}/api/v1/rbac/matrix?org_id={org_id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let roles = after["custom_roles"].as_array().unwrap();
    assert_eq!(roles.len(), 1, "matrix must show the new role: {after}");
    assert_eq!(roles[0]["name"], "Budget Keeper");
}

/// A route's complexity policy reads at the same bar as the route it hangs off
/// (#1666), and writes at the mutation bar as it always has.
///
/// #704 held the GET to `route:update`, which made the policy the one route
/// attribute a viewer could list but never see — the dashboard's Complexity
/// Router answered a reader with a permission error covering the whole screen.
/// The policy is configuration, not a secret: it already travels to the gateway
/// inside the route's `params`. This asserts both halves at once, because the
/// value of widening the read depends entirely on the write staying put.
#[tokio::test]
async fn a_viewer_reads_a_route_complexity_policy_but_cannot_write_one() {
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

    async fn post_as(client: &reqwest::Client, url: String, body: Value) -> Value {
        let response = client
            .post(url)
            .bearer_auth(admin_token())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let json: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{status}: {json}");
        json
    }

    let org = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Tiers", "slug": "tiers"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();

    let team = post_as(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().unwrap().to_string();

    let project = post_as(
        &client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().unwrap().to_string();

    // a small model to fall back to, and the route the policy hangs off
    post_as(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "gpt-4o-mini", "strategy": "round_robin"}),
    )
    .await;
    let route = post_as(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    let route_id = route["id"].as_str().unwrap().to_string();

    let policy = json!({"tiers": [
        {"name": "small", "max_input_bytes": 4096, "route": "gpt-4o-mini"},
        {"name": "large", "route": "gpt-4o"},
    ]});
    let written = client
        .put(format!("{base}/api/v1/routes/{route_id}/complexity"))
        .bearer_auth(admin_token())
        .json(&policy)
        .send()
        .await
        .unwrap();
    assert!(
        written.status().is_success(),
        "admin writes the policy: {}",
        written.status()
    );

    let viewer = seed_user(&pool, "complexity-viewer@example.com", false).await;
    seed_membership(&pool, viewer, Some(org_uuid), None, None, "viewer").await;
    let viewer_token = seed_session(&pool, viewer, "complexityviewer").await;

    let read = client
        .get(format!("{base}/api/v1/routes/{route_id}/complexity"))
        .bearer_auth(&viewer_token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        read.status(),
        200,
        "a viewer reads the policy of a route it can already list"
    );
    let body: Value = read.json().await.unwrap();
    assert_eq!(
        body["tiers"].as_array().map(Vec::len),
        Some(2),
        "the policy itself comes back, not an empty stand-in: {body}"
    );
    assert_eq!(body["tiers"][0]["name"], "small");

    // the write is untouched: a viewer still changes nothing. the body is a
    // policy the control plane would accept from an admin, so a widened write
    // bar shows up as a 200 here rather than hiding behind a validation error
    let refused = client
        .put(format!("{base}/api/v1/routes/{route_id}/complexity"))
        .bearer_auth(&viewer_token)
        .json(&json!({"tiers": [{"name": "large", "route": "gpt-4o"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403, "a viewer must not write the policy");

    // and the refusal was a refusal, not a silent no-op
    let after: Value = client
        .get(format!("{base}/api/v1/routes/{route_id}/complexity"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["tiers"].as_array().map(Vec::len), Some(2));

    let refused = client
        .delete(format!("{base}/api/v1/routes/{route_id}/complexity"))
        .bearer_auth(&viewer_token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        403,
        "a viewer must not disable the policy"
    );
}

/// Disabling a route's complexity policy removes only that key, and disabling a
/// route that carries none changes nothing: no version bump, no audit row
/// (#3221). Before, every stray or repeated disable rewrote the params, forced a
/// snapshot reload on every gateway and wrote a `{"complexity": null}` row that
/// read like a real routing change.
#[tokio::test]
async fn disabling_a_route_complexity_policy_is_a_no_op_when_none_is_set() {
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

    async fn post_as(client: &reqwest::Client, url: String, body: Value) -> Value {
        let response = client
            .post(url)
            .bearer_auth(admin_token())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let json: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{status}: {json}");
        json
    }

    let org = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Tiers", "slug": "tiers"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let team = post_as(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().unwrap().to_string();
    let project = post_as(
        &client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().unwrap().to_string();
    post_as(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "gpt-4o-mini", "strategy": "round_robin"}),
    )
    .await;
    let route = post_as(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    let route_id = route["id"].as_str().unwrap().to_string();
    let route_uuid: uuid::Uuid = route_id.parse().unwrap();

    async fn version(pool: &sqlx::PgPool) -> i64 {
        sqlx::query_scalar("select version from config_version where id = 1")
            .fetch_one(pool)
            .await
            .unwrap()
    }
    async fn audits(pool: &sqlx::PgPool) -> i64 {
        sqlx::query_scalar("select count(*) from audit_log where action = 'route.set_complexity'")
            .fetch_one(pool)
            .await
            .unwrap()
    }
    let disable = || async {
        client
            .delete(format!("{base}/api/v1/routes/{route_id}/complexity"))
            .bearer_auth(admin_token())
            .send()
            .await
            .unwrap()
    };

    // no policy: the route comes back as it is and nothing is written
    let version_before = version(&pool).await;
    let audits_before = audits(&pool).await;
    let response = disable().await;
    assert_eq!(response.status(), 200);
    let row: Value = response.json().await.unwrap();
    assert_eq!(row["id"], route["id"]);
    assert_eq!(row["params"], route["params"]);
    assert_eq!(
        version(&pool).await,
        version_before,
        "a disable with no policy must not move the config version"
    );
    assert_eq!(
        audits(&pool).await,
        audits_before,
        "a disable with no policy must not write an audit row"
    );

    // ordinary parameters and their locks survive the rollback
    let policy = json!({"tiers": [
        {"name": "small", "max_input_bytes": 4096, "route": "gpt-4o-mini"},
        {"name": "large", "route": "gpt-4o"},
    ]});
    let params = json!({"temperature": 0.4, "_rolter_complexity": policy});
    let param_policy = json!({"temperature": "locked"});
    sqlx::query("update routes set params = $1, param_policy = $2 where id = $3")
        .bind(&params)
        .bind(&param_policy)
        .bind(route_uuid)
        .execute(&pool)
        .await
        .unwrap();
    let before = version(&pool).await;
    let disabled = client
        .delete(format!("{base}/api/v1/routes/{route_id}/complexity"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(disabled.status(), 200);
    let row: Value = disabled.json().await.unwrap();
    assert_eq!(row["params"], json!({"temperature": 0.4}));
    assert_eq!(row["param_policy"], param_policy);
    assert!(version(&pool).await > before);
    let policy: Value = client
        .get(format!("{base}/api/v1/routes/{route_id}/complexity"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(policy, json!({"tiers": []}));
    let audits: i64 = sqlx::query_scalar("select count(*) from audit_log where action = 'route.set_complexity' and detail->'complexity' = 'null'::jsonb")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(audits, 1);
}

/// A caller whose only role is on a project reads the budgets and rate limits
/// that apply to it, at the project, its team and its org, and nothing else:
/// another project's, a sibling team's, and no write at any of them (#2527).
#[tokio::test]
async fn a_project_viewer_reads_the_caps_of_their_own_scope_chain_only() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("caps".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut teams = Vec::new();
    for name in ["core", "other"] {
        let id: uuid::Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, $2) returning id")
                .bind(org)
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
        teams.push(id);
    }
    let mut projects = Vec::new();
    for (team, name) in [(teams[0], "one"), (teams[0], "two"), (teams[1], "three")] {
        let id: uuid::Uuid =
            sqlx::query_scalar("insert into projects (team_id, name) values ($1, $2) returning id")
                .bind(team)
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
        projects.push(id);
    }
    let scopes = [
        ("org", org),
        ("team", teams[0]),
        ("team", teams[1]),
        ("project", projects[0]),
        ("project", projects[1]),
        ("project", projects[2]),
    ];
    for (scope_type, scope_id) in scopes {
        sqlx::query(
            "insert into budgets (scope_type, scope_id, limit_usd, period)
             values ($1, $2, '10', 'monthly')",
        )
        .bind(scope_type)
        .bind(scope_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("insert into rate_limits (scope_type, scope_id, rpm) values ($1, $2, 60)")
            .bind(scope_type)
            .bind(scope_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    let viewer = seed_user(&pool, "viewer@acme.test", false).await;
    seed_membership(&pool, viewer, None, None, Some(projects[0]), "viewer").await;
    let token = seed_session(&pool, viewer, "caps_viewer").await;

    let status_of = |method: reqwest::Method, path: String, body: Option<Value>| {
        let (client, base, token) = (client.clone(), base.clone(), token.clone());
        async move {
            let mut req = client
                .request(method, format!("{base}{path}"))
                .bearer_auth(token);
            if let Some(body) = body {
                req = req.json(&body);
            }
            let resp = req.send().await.unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };

    for resource in ["budgets", "rate-limits"] {
        for (scope_type, scope_id, readable) in [
            ("project", projects[0], true),
            ("team", teams[0], true),
            ("org", org, true),
            ("project", projects[1], false),
            ("project", projects[2], false),
            ("team", teams[1], false),
        ] {
            let (status, rows) = status_of(
                reqwest::Method::GET,
                format!("/api/v1/{resource}?scope_type={scope_type}&scope_id={scope_id}"),
                None,
            )
            .await;
            if readable {
                assert_eq!(status, 200, "{resource} {scope_type}: {rows}");
                assert_eq!(rows.as_array().unwrap().len(), 1, "{resource} {scope_type}");
            } else {
                assert_eq!(status, 403, "{resource} {scope_type} {scope_id}: {rows}");
            }
        }
    }

    // reading is all a viewer gets: no create, even on their own project
    let (status, _) = status_of(
        reqwest::Method::POST,
        "/api/v1/budgets".to_string(),
        Some(json!({"scope_type": "project", "scope_id": projects[0], "limit_usd": "5"})),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = status_of(
        reqwest::Method::POST,
        "/api/v1/rate-limits".to_string(),
        Some(json!({"scope_type": "project", "scope_id": projects[0], "rpm": 5})),
    )
    .await;
    assert_eq!(status, 403);

    // the advisory answer agrees: read yes, write no, at the project's chain
    let (status, effective) = status_of(
        reqwest::Method::GET,
        format!(
            "/api/v1/rbac/effective?org_id={org}&team_id={}&project_id={}",
            teams[0], projects[0]
        ),
        None,
    )
    .await;
    assert_eq!(status, 200, "{effective}");
    let allowed: Vec<&str> = effective["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for pair in ["budget:read", "rate_limit:read"] {
        assert!(allowed.contains(&pair), "{pair}");
    }
    for pair in [
        "budget:create",
        "budget:update",
        "budget:delete",
        "rate_limit:create",
    ] {
        assert!(!allowed.contains(&pair), "{pair}");
    }
}

/// The org-wide cap lists answer with the rows the caller may read, decided
/// row by row exactly as the per-scope read is: a project viewer gets their
/// project, its team and the org, an org admin gets everything in the org, and
/// another org's caps and a caller with no role in the org are never shown.
#[tokio::test]
async fn the_org_wide_cap_lists_apply_the_per_scope_read_rule_to_each_row() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("caps".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let mut orgs = Vec::new();
    for slug in ["acme", "globex"] {
        let id: uuid::Uuid =
            sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
                .bind(slug)
                .fetch_one(&pool)
                .await
                .unwrap();
        orgs.push(id);
    }
    let mut teams = Vec::new();
    for (org, name) in [(orgs[0], "core"), (orgs[0], "other"), (orgs[1], "away")] {
        let id: uuid::Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, $2) returning id")
                .bind(org)
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
        teams.push(id);
    }
    let mut projects = Vec::new();
    for (team, name) in [(teams[0], "one"), (teams[0], "two"), (teams[1], "three")] {
        let id: uuid::Uuid =
            sqlx::query_scalar("insert into projects (team_id, name) values ($1, $2) returning id")
                .bind(team)
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
        projects.push(id);
    }
    let unit: uuid::Uuid = sqlx::query_scalar(
        "insert into business_units (org_id, slug, name) values ($1, 'ops', 'Ops') returning id",
    )
    .bind(orgs[0])
    .fetch_one(&pool)
    .await
    .unwrap();
    let scopes = [
        ("org", orgs[0]),
        ("team", teams[0]),
        ("team", teams[1]),
        ("project", projects[0]),
        ("project", projects[1]),
        ("project", projects[2]),
        ("business_unit", unit),
        ("org", orgs[1]),
        ("team", teams[2]),
    ];
    for (scope_type, scope_id) in scopes {
        sqlx::query(
            "insert into budgets (scope_type, scope_id, limit_usd, period)
             values ($1, $2, '10', 'monthly')",
        )
        .bind(scope_type)
        .bind(scope_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("insert into rate_limits (scope_type, scope_id, rpm) values ($1, $2, 60)")
            .bind(scope_type)
            .bind(scope_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    let viewer = seed_user(&pool, "viewer@acme.test", false).await;
    seed_membership(&pool, viewer, None, None, Some(projects[0]), "viewer").await;
    let viewer_token = seed_session(&pool, viewer, "caps_viewer").await;
    let admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, admin, Some(orgs[0]), None, None, "admin").await;
    let admin_token = seed_session(&pool, admin, "caps_admin").await;

    let get = |token: String, path: String| {
        let (client, base) = (client.clone(), base.clone());
        async move {
            let resp = client
                .get(format!("{base}{path}"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let scope_ids = |rows: &Value| -> std::collections::BTreeSet<String> {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|row| row["scope_id"].as_str().unwrap().to_string())
            .collect()
    };

    for resource in ["budgets", "rate-limits"] {
        let path = format!("/api/v1/orgs/{}/{resource}", orgs[0]);

        let (status, rows) = get(viewer_token.clone(), path.clone()).await;
        assert_eq!(status, 200, "{resource}: {rows}");
        let want: std::collections::BTreeSet<String> = [orgs[0], teams[0], projects[0]]
            .iter()
            .map(|id| id.to_string())
            .collect();
        assert_eq!(scope_ids(&rows), want, "{resource} for a project viewer");

        let (status, rows) = get(admin_token.clone(), path.clone()).await;
        assert_eq!(status, 200, "{resource}: {rows}");
        let want: std::collections::BTreeSet<String> = [
            orgs[0],
            teams[0],
            teams[1],
            projects[0],
            projects[1],
            projects[2],
            unit,
        ]
        .iter()
        .map(|id| id.to_string())
        .collect();
        assert_eq!(scope_ids(&rows), want, "{resource} for an org admin");

        // no role anywhere in the other org
        let (status, _) = get(
            viewer_token.clone(),
            format!("/api/v1/orgs/{}/{resource}", orgs[1]),
        )
        .await;
        assert_eq!(
            status, 403,
            "{resource} in an org the caller has no role in"
        );
    }
}

/// A project's members see the providers and groups of their project plus the
/// org-wide ones, never another project's; a project admin may create and
/// delete their own project's, but widening to the org, another project, or
/// naming an environment variable is an org admin's call (#1919).
#[tokio::test]
async fn members_list_their_projects_providers_and_project_admins_scope_their_own() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("scoped".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut projects = Vec::new();
    for name in ["one", "two"] {
        let id: uuid::Uuid =
            sqlx::query_scalar("insert into projects (team_id, name) values ($1, $2) returning id")
                .bind(team)
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
        projects.push(id);
    }
    let (p1, p2) = (projects[0], projects[1]);
    for (name, project) in [
        ("shared", None),
        ("priv-one", Some(p1)),
        ("priv-two", Some(p2)),
    ] {
        let provider: uuid::Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, slug, kind, api_base, project_id)
             values ($1, $2, $2, 'openai_compatible', 'http://127.0.0.1:9', $3) returning id",
        )
        .bind(org)
        .bind(name)
        .bind(project)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into provider_groups (org_id, name, slug, project_id)
             values ($1, $2, $2, $3)",
        )
        .bind(org)
        .bind(format!("grp-{name}"))
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
        let _ = provider;
    }

    let one_admin = seed_user(&pool, "one-admin@acme.test", false).await;
    seed_membership(&pool, one_admin, None, None, Some(p1), "admin").await;
    let two_member = seed_user(&pool, "two@acme.test", false).await;
    seed_membership(&pool, two_member, None, None, Some(p2), "member").await;
    let org_admin = seed_user(&pool, "owner@acme.test", false).await;
    seed_membership(&pool, org_admin, Some(org), None, None, "admin").await;
    let stranger = seed_user(&pool, "stranger@elsewhere.test", false).await;
    let one = seed_session(&pool, one_admin, "scoped_one").await;
    let two = seed_session(&pool, two_member, "scoped_two").await;
    let owner = seed_session(&pool, org_admin, "scoped_owner").await;
    let nobody = seed_session(&pool, stranger, "scoped_nobody").await;

    let names = |rows: &Value, key: &str| -> Vec<String> {
        let mut names: Vec<String> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row[key].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    };
    let get = |path: String, token: &str| {
        let (client, base, token) = (client.clone(), base.clone(), token.to_string());
        async move {
            let resp = client
                .get(format!("{base}{path}"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };

    for (token, providers, groups) in [
        (
            &one,
            vec!["priv-one", "shared"],
            vec!["grp-priv-one", "grp-shared"],
        ),
        (
            &two,
            vec!["priv-two", "shared"],
            vec!["grp-priv-two", "grp-shared"],
        ),
        (
            &owner,
            vec!["priv-one", "priv-two", "shared"],
            vec!["grp-priv-one", "grp-priv-two", "grp-shared"],
        ),
    ] {
        let (status, rows) = get(format!("/api/v1/orgs/{org}/providers"), token).await;
        assert_eq!(status, 200, "{rows}");
        assert_eq!(names(&rows, "name"), providers);
        let (status, rows) = get(format!("/api/v1/orgs/{org}/provider-groups"), token).await;
        assert_eq!(status, 200, "{rows}");
        assert_eq!(names(&rows, "name"), groups);
    }
    assert_eq!(
        get(format!("/api/v1/orgs/{org}/providers"), &nobody)
            .await
            .0,
        403
    );
    assert_eq!(
        get(format!("/api/v1/orgs/{org}/provider-groups"), &nobody)
            .await
            .0,
        403
    );

    let create = |token: &str, body: Value| {
        let (client, base, token) = (client.clone(), base.clone(), token.to_string());
        async move {
            let resp = client
                .post(format!("{base}/api/v1/orgs/{org}/providers"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let new = |name: &str, project: Option<uuid::Uuid>| {
        let mut body = json!({"name": name, "kind": "openai_compatible",
                              "api_base": "http://127.0.0.1:9"});
        if let Some(project) = project {
            body["project_id"] = json!(project);
        }
        body
    };
    // a project admin creates and removes their own project's providers
    let (status, created) = create(&one, new("mine", Some(p1))).await;
    assert_eq!(status, 200, "{created}");
    // but not another project's, an org-wide one, or one that reads an env var
    assert_eq!(create(&one, new("theirs", Some(p2))).await.0, 403);
    assert_eq!(create(&one, new("wide", None)).await.0, 403);
    let mut env = new("env-reader", Some(p1));
    env["api_key_env"] = json!("OPENAI_API_KEY");
    assert_eq!(create(&one, env.clone()).await.0, 403);
    assert_eq!(create(&owner, env).await.0, 200);
    // a project member below admin creates nothing
    assert_eq!(create(&two, new("member-made", Some(p2))).await.0, 403);
    // and widening a scoped provider to the org is the org admin's
    let id = created["id"].as_str().unwrap();
    let widen = |token: &str| {
        let (client, base, token, id) = (
            client.clone(),
            base.clone(),
            token.to_string(),
            id.to_string(),
        );
        async move {
            client
                .put(format!("{base}/api/v1/providers/{id}"))
                .bearer_auth(token)
                .json(&json!({"project_id": null}))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(widen(&one).await, 403);
    assert_eq!(widen(&owner).await, 200);
    let resp = client
        .delete(format!("{base}/api/v1/providers/{id}"))
        .bearer_auth(&one)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "an org-wide provider is the org admin's"
    );
}

/// The common refusals carry a stable `code` the dashboard translates, next to
/// the unchanged message and status (#2567).
#[tokio::test]
async fn common_refusals_carry_a_stable_code() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let post = |url: String, body: Value| {
        let client = client.clone();
        async move {
            let res = client
                .post(url)
                .bearer_auth("admintok")
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = res.status().as_u16();
            (status, res.json::<Value>().await.unwrap())
        }
    };

    let (status, org) = post(
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    assert_eq!(status, 200, "{org}");
    let org_id = org["id"].as_str().unwrap().to_string();

    // a unique violation the handler does not check first is a 409, not a 500,
    // and says nothing about the constraint it hit
    let (status, body) = post(
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme again", "slug": "acme"}),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "name_taken", "{body}");
    assert!(
        !body.to_string().contains("orgs_slug"),
        "the constraint name leaked: {body}"
    );

    // the explicit name check carries the same code and keeps its message
    let provider =
        json!({"name": "openai", "kind": "openai", "api_base": "https://api.openai.com"});
    let url = format!("{base}/api/v1/orgs/{org_id}/providers");
    assert_eq!(post(url.clone(), provider.clone()).await.0, 200);
    let (status, body) = post(url.clone(), provider).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "name_taken", "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("already in use"),
        "{body}"
    );

    // a field refusal names the field
    let (status, body) = post(
        format!("{base}/api/v1/orgs"),
        json!({"name": "Bad", "slug": "Not A Slug"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid_field", "{body}");
    assert_eq!(body["error"]["field"], "slug", "{body}");
    let (status, body) = post(
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "a\u{0}b"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid_field", "{body}");
    assert_eq!(body["error"]["field"], "name", "{body}");

    // a body that is not the shape the endpoint takes is coded, with no field
    let (status, body) = post(
        format!("{base}/api/v1/orgs"),
        json!({"name": 7, "slug": "x"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid_body", "{body}");
    assert!(body["error"].get("field").is_none(), "{body}");

    // a list cursor that does not parse names the parameter
    let get = |url: String| {
        let client = client.clone();
        async move {
            let res = client
                .get(url)
                .bearer_auth("admintok")
                .send()
                .await
                .unwrap();
            let status = res.status().as_u16();
            (status, res.json::<Value>().await.unwrap())
        }
    };
    let (status, body) = get(format!("{base}/api/v1/audit-log?cursor=nonsense")).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid_field", "{body}");
    assert_eq!(body["error"]["field"], "cursor", "{body}");

    // so does an enum parameter the handler checks by hand
    let (status, body) = get(format!(
        "{base}/api/v1/budgets?scope_type=galaxy&scope_id={org_id}"
    ))
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid_field", "{body}");
    assert_eq!(body["error"]["field"], "scope_type", "{body}");

    // a query string that does not deserialize is coded, with no field
    let (status, body) = get(format!("{base}/api/v1/audit-log?limit=abc")).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid_query", "{body}");
    assert!(body["error"].get("field").is_none(), "{body}");

    // a settings refusal names the list row, not just the message prefix
    let res = client
        .put(format!("{base}/api/v1/client-settings"))
        .bearer_auth("admintok")
        .json(&json!({
            "forwarded_headers": [],
            "injected_headers": {"bad header": "v"},
            "request_id_header": "x-request-id"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 400);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_field", "{body}");
    assert_eq!(
        body["error"]["field"], "injected_headers.bad header",
        "{body}"
    );
}

/// #3126: a team is renamed at the team's own scope. Its admin renames it, a
/// viewer is refused, a name another team of the org holds is a 409
/// `name_taken`, an empty one a 400, and the audit row names both names.
#[tokio::test]
async fn a_team_is_renamed_by_its_own_admin() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("drift".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let call = |method: reqwest::Method, path: String, token: String, body: Value| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let resp = client
                .request(method, format!("{base}{path}"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let admin = |method: reqwest::Method, path: String, body: Value| {
        call(method, path, "drift".to_string(), body)
    };
    let (_, org) = admin(
        reqwest::Method::POST,
        "/api/v1/orgs".into(),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id: uuid::Uuid = org["id"].as_str().unwrap().parse().unwrap();
    let mut teams = Vec::new();
    for name in ["core", "research"] {
        let (_, team) = admin(
            reqwest::Method::POST,
            format!("/api/v1/orgs/{org_id}/teams"),
            json!({"name": name}),
        )
        .await;
        teams.push(team["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap());
    }
    let path = format!("/api/v1/teams/{}", teams[0]);

    let team_admin = seed_user(&pool, "team-admin@acme.test", false).await;
    seed_membership(&pool, team_admin, None, Some(teams[0]), None, "admin").await;
    let team_admin = seed_session(&pool, team_admin, "rename_team_admin").await;
    let other_admin = seed_user(&pool, "other-admin@acme.test", false).await;
    seed_membership(&pool, other_admin, None, Some(teams[1]), None, "admin").await;
    let other_admin = seed_session(&pool, other_admin, "rename_other_admin").await;
    let viewer = seed_user(&pool, "viewer@acme.test", false).await;
    seed_membership(&pool, viewer, None, Some(teams[0]), None, "viewer").await;
    let viewer = seed_session(&pool, viewer, "rename_viewer").await;

    let rename = |token: String, name: &str| {
        call(
            reqwest::Method::PUT,
            path.clone(),
            token,
            json!({"name": name}),
        )
    };
    let (status, body) = rename(viewer, "platform").await;
    assert_eq!(status, 403, "a viewer cannot rename: {body}");
    let (status, body) = rename(other_admin, "platform").await;
    assert_eq!(status, 403, "another team's admin cannot rename: {body}");

    let (status, body) = rename(team_admin.clone(), "research").await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "name_taken", "{body}");
    let (status, body) = rename(team_admin.clone(), "  ").await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["field"], "name", "{body}");

    let (status, body) = rename(team_admin, " platform ").await;
    assert_eq!(status, 200, "a team admin renames their team: {body}");
    assert_eq!(
        body["name"], "platform",
        "the name is stored trimmed: {body}"
    );
    assert_eq!(body["id"], teams[0].to_string());

    let (status, listed) = admin(
        reqwest::Method::GET,
        format!("/api/v1/orgs/{org_id}/teams"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{listed}");
    let names: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["platform", "research"], "{listed}");

    let (status, page) = admin(
        reqwest::Method::GET,
        format!("/api/v1/orgs/{org_id}/audit-log?action=team.update"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{page}");
    let rows = page["items"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "only the rename that went through: {page}");
    assert_eq!(rows[0]["target_id"], teams[0].to_string());
    assert_eq!(rows[0]["detail"]["old_name"], "core", "{page}");
    assert_eq!(rows[0]["detail"]["new_name"], "platform", "{page}");

    let (status, _) = call(
        reqwest::Method::PUT,
        format!("/api/v1/teams/{}", uuid::Uuid::new_v4()),
        "drift".to_string(),
        json!({"name": "ghost"}),
    )
    .await;
    assert_eq!(status, 404);
}

/// #3074: a guard names the scope its `CAPABILITIES` row declares. Deleting a
/// project is a team admin's, as creating one is, and deleting a team is an org
/// admin's; the admin of the project or team itself is refused. Granting a role
/// and inviting stay a team admin's inside their own team, and `effective`
/// now says so.
#[tokio::test]
async fn guards_follow_the_scope_their_capability_row_declares() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("drift".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let call = |method: reqwest::Method, path: String, token: String, body: Value| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let resp = client
                .request(method, format!("{base}{path}"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let admin = |method: reqwest::Method, path: String, body: Value| {
        call(method, path, "drift".to_string(), body)
    };

    let (_, org) = admin(
        reqwest::Method::POST,
        "/api/v1/orgs".into(),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id: uuid::Uuid = org["id"].as_str().unwrap().parse().unwrap();
    let (_, team) = admin(
        reqwest::Method::POST,
        format!("/api/v1/orgs/{org_id}/teams"),
        json!({"name": "core"}),
    )
    .await;
    let team_id: uuid::Uuid = team["id"].as_str().unwrap().parse().unwrap();
    let mut projects = Vec::new();
    for name in ["one", "two"] {
        let (_, project) = admin(
            reqwest::Method::POST,
            format!("/api/v1/teams/{team_id}/projects"),
            json!({"name": name}),
        )
        .await;
        projects.push(
            project["id"]
                .as_str()
                .unwrap()
                .parse::<uuid::Uuid>()
                .unwrap(),
        );
    }

    let team_admin = seed_user(&pool, "team-admin@acme.test", false).await;
    seed_membership(&pool, team_admin, None, Some(team_id), None, "admin").await;
    let team_admin = seed_session(&pool, team_admin, "drift_team_admin").await;
    let project_admin = seed_user(&pool, "project-admin@acme.test", false).await;
    seed_membership(&pool, project_admin, None, None, Some(projects[0]), "admin").await;
    let project_admin = seed_session(&pool, project_admin, "drift_project_admin").await;
    let org_admin = seed_user(&pool, "org-admin@acme.test", false).await;
    seed_membership(&pool, org_admin, Some(org_id), None, None, "admin").await;
    let org_admin = seed_session(&pool, org_admin, "drift_org_admin").await;

    // project:delete is a team row: its own admin is refused, the team's is not
    let project_path = format!("/api/v1/projects/{}", projects[0]);
    let (status, body) = call(
        reqwest::Method::DELETE,
        project_path.clone(),
        project_admin,
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 403,
        "a project admin cannot delete the project: {body}"
    );
    let (status, body) = call(
        reqwest::Method::DELETE,
        project_path,
        team_admin.clone(),
        Value::Null,
    )
    .await;
    assert_eq!(status, 204, "a team admin deletes a project: {body:?}");

    // team:delete is an org row: the team's admin is refused, the org's is not
    let team_path = format!("/api/v1/teams/{team_id}");
    let (status, body) = call(
        reqwest::Method::DELETE,
        team_path.clone(),
        team_admin.clone(),
        Value::Null,
    )
    .await;
    assert_eq!(status, 403, "a team admin cannot delete the team: {body}");

    // granting and inviting stay a team admin's inside their team
    let invitee = seed_user(&pool, "invitee@acme.test", false).await;
    let (status, body) = call(
        reqwest::Method::POST,
        format!("/api/v1/orgs/{org_id}/memberships"),
        team_admin.clone(),
        json!({"user_id": invitee, "scope_type": "team", "scope_id": team_id, "role": "viewer"}),
    )
    .await;
    assert_eq!(status, 200, "a team admin grants in their team: {body}");
    let (status, body) = call(
        reqwest::Method::POST,
        format!("/api/v1/orgs/{org_id}/invitations"),
        team_admin.clone(),
        json!({"email": "new@acme.test", "role": "viewer", "scope_type": "team", "scope_id": team_id}),
    )
    .await;
    assert_eq!(status, 200, "a team admin invites into their team: {body}");
    let (status, body) = call(
        reqwest::Method::POST,
        format!("/api/v1/orgs/{org_id}/memberships"),
        team_admin.clone(),
        json!({"user_id": invitee, "scope_type": "org", "scope_id": org_id, "role": "viewer"}),
    )
    .await;
    assert_eq!(status, 403, "but not in the org above: {body}");

    // the advisory answer follows the guard: asked at the team, a team admin is
    // told they may grant and invite there, and asked at the org, they are not
    let allowed = |body: &Value| -> Vec<String> {
        body["allowed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };
    let (status, at_team) = call(
        reqwest::Method::GET,
        format!("/api/v1/rbac/effective?org_id={org_id}&team_id={team_id}"),
        team_admin.clone(),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{at_team}");
    for pair in [
        "membership:create",
        "membership:delete",
        "invitation:create",
    ] {
        assert!(allowed(&at_team).contains(&pair.to_string()), "{pair}");
    }
    let (_, at_org) = call(
        reqwest::Method::GET,
        format!("/api/v1/rbac/effective?org_id={org_id}"),
        team_admin,
        Value::Null,
    )
    .await;
    for pair in ["membership:create", "invitation:create", "team:delete"] {
        assert!(!allowed(&at_org).contains(&pair.to_string()), "{pair}");
    }

    let (status, body) = call(reqwest::Method::DELETE, team_path, org_admin, Value::Null).await;
    assert_eq!(status, 204, "the org admin deletes the team: {body:?}");
}

/// #3107: the project and user lists declare their read at the project scope
/// while their writes keep the row's. For every role at every membership
/// level, `GET /api/v1/rbac/effective` asked at the whole chain answers each of
/// the four pairs the way the guard does: the lists open exactly for the callers
/// that reach a row, and the creates stay with the team and org admin.
#[tokio::test]
async fn effective_matches_the_guards_of_the_project_and_user_rows_for_every_role() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("peract".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let call = |method: reqwest::Method, path: String, token: String, body: Value| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let resp = client
                .request(method, format!("{base}{path}"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let admin = |method: reqwest::Method, path: String, body: Value| {
        call(method, path, "peract".to_string(), body)
    };

    let (_, org) = admin(
        reqwest::Method::POST,
        "/api/v1/orgs".into(),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id: uuid::Uuid = org["id"].as_str().unwrap().parse().unwrap();
    let (_, team) = admin(
        reqwest::Method::POST,
        format!("/api/v1/orgs/{org_id}/teams"),
        json!({"name": "core"}),
    )
    .await;
    let team_id: uuid::Uuid = team["id"].as_str().unwrap().parse().unwrap();
    let (_, project) = admin(
        reqwest::Method::POST,
        format!("/api/v1/teams/{team_id}/projects"),
        json!({"name": "one"}),
    )
    .await;
    let project_id: uuid::Uuid = project["id"].as_str().unwrap().parse().unwrap();

    let effective_path =
        format!("/api/v1/rbac/effective?org_id={org_id}&team_id={team_id}&project_id={project_id}");
    let mut checked = 0;
    for level in ["org", "team", "project"] {
        for role in ["viewer", "member", "admin"] {
            let label = format!("{role}-{level}");
            let user = seed_user(&pool, &format!("{label}@acme.test"), false).await;
            let (o, t, p) = match level {
                "org" => (Some(org_id), None, None),
                "team" => (None, Some(team_id), None),
                _ => (None, None, Some(project_id)),
            };
            seed_membership(&pool, user, o, t, p, role).await;
            let token = seed_session(&pool, user, &format!("peract_{label}")).await;

            let (status, effective) = call(
                reqwest::Method::GET,
                effective_path.clone(),
                token.clone(),
                Value::Null,
            )
            .await;
            assert_eq!(status, 200, "{label}: {effective}");
            let told = |pair: &str| {
                effective["allowed"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v == pair)
            };

            let (status, body) = call(
                reqwest::Method::GET,
                format!("/api/v1/teams/{team_id}/projects"),
                token.clone(),
                Value::Null,
            )
            .await;
            assert_eq!(
                status == 200,
                told("project:read"),
                "{label} project:read: {body}"
            );
            let (status, body) = call(
                reqwest::Method::GET,
                format!("/api/v1/orgs/{org_id}/projects"),
                token.clone(),
                Value::Null,
            )
            .await;
            assert_eq!(
                status == 200,
                told("project:read"),
                "{label} org project:read: {body}"
            );
            let (status, body) = call(
                reqwest::Method::POST,
                format!("/api/v1/teams/{team_id}/projects"),
                token.clone(),
                json!({"name": format!("made-by-{label}")}),
            )
            .await;
            assert_eq!(
                status != 403,
                told("project:create"),
                "{label} project:create: {body}"
            );
            let (status, body) = call(
                reqwest::Method::GET,
                format!("/api/v1/orgs/{org_id}/users"),
                token.clone(),
                Value::Null,
            )
            .await;
            assert_eq!(
                status == 200,
                told("user:read"),
                "{label} user:read: {body}"
            );
            let (status, body) = call(
                reqwest::Method::POST,
                format!("/api/v1/orgs/{org_id}/users"),
                token,
                json!({"email": format!("made-by-{label}@acme.test"), "role": "viewer"}),
            )
            .await;
            assert_eq!(
                status != 403,
                told("user:create"),
                "{label} user:create: {body}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 9);
}

/// #2235: editing a route's strategy and targets takes `route:update`, the
/// project admin's capability, like every other route write. A project member
/// can read the route but not change it.
#[tokio::test]
async fn a_route_update_takes_the_project_admin() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("routes".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let call = |method: reqwest::Method, path: String, token: String, body: Value| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let resp = client
                .request(method, format!("{base}{path}"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let admin = |method: reqwest::Method, path: String, body: Value| {
        call(method, path, "routes".to_string(), body)
    };

    let (_, org) = admin(
        reqwest::Method::POST,
        "/api/v1/orgs".into(),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let (_, team) = admin(
        reqwest::Method::POST,
        format!("/api/v1/orgs/{org_id}/teams"),
        json!({"name": "core"}),
    )
    .await;
    let team_id = team["id"].as_str().unwrap().to_string();
    let (_, project) = admin(
        reqwest::Method::POST,
        format!("/api/v1/teams/{team_id}/projects"),
        json!({"name": "app"}),
    )
    .await;
    let project_id: uuid::Uuid = project["id"].as_str().unwrap().parse().unwrap();
    let (_, provider) = admin(
        reqwest::Method::POST,
        format!("/api/v1/orgs/{org_id}/providers"),
        json!({"name": "primary", "kind": "openai_compatible", "api_base": "http://127.0.0.1:9"}),
    )
    .await;
    let provider_id = provider["id"].as_str().unwrap().to_string();
    let (status, route) = admin(
        reqwest::Method::POST,
        format!("/api/v1/projects/{project_id}/routes"),
        json!({"model": "chat"}),
    )
    .await;
    assert_eq!(status, 200, "{route}");
    let route_path = format!("/api/v1/routes/{}", route["id"].as_str().unwrap());

    let member = seed_user(&pool, "member@acme.test", false).await;
    seed_membership(&pool, member, None, None, Some(project_id), "member").await;
    let project_admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, project_admin, None, None, Some(project_id), "admin").await;
    let member = seed_session(&pool, member, "route_update_member").await;
    let project_admin = seed_session(&pool, project_admin, "route_update_admin").await;

    let edit = json!({
        "strategy": "weighted",
        "targets": [{"provider_id": provider_id, "weight": 2}]
    });
    let (status, body) = call(
        reqwest::Method::PUT,
        route_path.clone(),
        member,
        edit.clone(),
    )
    .await;
    assert_eq!(status, 403, "a project member cannot edit a route: {body}");
    let (status, body) = call(reqwest::Method::PUT, route_path, project_admin, edit).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["strategy"], "weighted");
}

/// An operator maintains the deployment and a security auditor reads how it is
/// secured; neither reaches tenant data or can grant a role, and only a
/// superadmin hands the roles out, with an audit row (#1834).
#[tokio::test]
async fn deployment_roles_open_their_capabilities_and_nothing_more() {
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
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let operator = seed_user(&pool, "ops@acme.test", false).await;
    let auditor = seed_user(&pool, "sec@acme.test", false).await;
    let org_admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(
        &pool,
        org_admin,
        Some(org_id.parse().unwrap()),
        None,
        None,
        "admin",
    )
    .await;
    let operator_token = seed_session(&pool, operator, "ops").await;
    let auditor_token = seed_session(&pool, auditor, "sec").await;
    let org_admin_token = seed_session(&pool, org_admin, "admin").await;

    let status = |method: reqwest::Method, path: String, token: String, body: Value| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .request(method, format!("{base}{path}"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    let get = |path: &str, token: &str| {
        status(
            reqwest::Method::GET,
            path.to_string(),
            token.to_string(),
            Value::Null,
        )
    };
    let grant = |token: &str, id: uuid::Uuid, role: Value| {
        status(
            reqwest::Method::PUT,
            format!("/api/v1/users/{id}/deployment-role"),
            token.to_string(),
            json!({ "role": role }),
        )
    };

    // before any grant, an account with no deployment role reaches none of it
    assert_eq!(get("/api/v1/feature-flags", &operator_token).await, 403);
    assert_eq!(get("/api/v1/security-settings", &auditor_token).await, 403);

    // only a superadmin grants a role: an org admin, and the roles themselves,
    // are refused, and the role must be one of the two
    assert_eq!(
        grant(&org_admin_token, operator, json!("operator")).await,
        403
    );
    assert_eq!(
        grant(admin_token(), operator, json!("superadmin")).await,
        400
    );
    assert_eq!(grant(admin_token(), operator, json!("operator")).await, 200);
    assert_eq!(
        grant(admin_token(), auditor, json!("security_auditor")).await,
        200
    );
    assert_eq!(
        grant(&operator_token, auditor, json!("operator")).await,
        403
    );
    assert_eq!(
        grant(&auditor_token, auditor, json!(null)).await,
        403,
        "a role cannot be cleared or widened by its holder"
    );

    // the operator maintains: reads and writes what it is named for, reads no
    // security settings, and has no way into a tenant
    assert_eq!(get("/api/v1/feature-flags", &operator_token).await, 200);
    assert_eq!(get("/api/v1/cluster/nodes", &operator_token).await, 200);
    assert_eq!(get("/api/v1/security-settings", &operator_token).await, 403);
    assert_eq!(get("/api/v1/audit-log", &operator_token).await, 403);
    assert_eq!(get("/api/v1/config/export", &operator_token).await, 403);
    assert_eq!(get("/api/v1/users", &operator_token).await, 403);
    assert_eq!(
        get(&format!("/api/v1/orgs/{org_id}/audit-log"), &operator_token).await,
        403
    );
    assert_eq!(
        get(&format!("/api/v1/orgs/{org_id}/users"), &operator_token).await,
        403
    );

    // the auditor reads and never writes
    assert_eq!(get("/api/v1/security-settings", &auditor_token).await, 200);
    assert_eq!(get("/api/v1/audit-log", &auditor_token).await, 200);
    assert_eq!(
        get("/api/v1/users", &auditor_token).await,
        200,
        "the audit log names its actors from this list"
    );
    assert_eq!(get("/api/v1/feature-flags", &auditor_token).await, 403);
    assert_eq!(get("/api/v1/cluster/nodes", &auditor_token).await, 403);
    assert_eq!(
        status(
            reqwest::Method::PUT,
            "/api/v1/security-settings".into(),
            auditor_token.clone(),
            json!({})
        )
        .await,
        403
    );
    assert_eq!(
        get(&format!("/api/v1/orgs/{org_id}/audit-log"), &auditor_token).await,
        403
    );

    // `rbac/effective` publishes the role and the pairs it opens
    let effective = |token: &str| {
        let client = client.clone();
        let url = format!("{base}/api/v1/rbac/effective");
        let token = token.to_string();
        async move {
            client
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let as_operator = effective(&operator_token).await;
    assert_eq!(as_operator["deployment_role"], "operator");
    assert_eq!(as_operator["superadmin"], false);
    let allowed: Vec<&str> = as_operator["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(allowed.contains(&"cluster_node:update"));
    assert!(!allowed.contains(&"security_settings:read"));
    assert!(!allowed.contains(&"deployment_role:update"));
    let as_auditor = effective(&auditor_token).await;
    assert_eq!(as_auditor["deployment_role"], "security_auditor");
    let allowed: Vec<&str> = as_auditor["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(allowed.contains(&"security_settings:read"));
    assert!(!allowed.contains(&"security_settings:update"));

    // the matrix names the roles on the rows they reach
    let matrix: Value = client
        .get(format!("{base}/api/v1/rbac/matrix"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        matrix["deployment_roles"],
        json!(["operator", "security_auditor"])
    );
    let flags = matrix["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["resource"] == "feature_flags")
        .unwrap();
    assert_eq!(flags["actions"][0]["superadmin_only"], false);
    assert_eq!(flags["actions"][0]["deployment_roles"], json!(["operator"]));

    // the grants are audited unscoped, naming the account and the change
    let log: Value = client
        .get(format!(
            "{base}/api/v1/audit-log?action=user.deployment_role"
        ))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = log["items"].as_array().unwrap();
    assert!(
        rows.iter().any(|row| {
            row["target_id"] == operator.to_string() && row["detail"]["to"] == "operator"
        }),
        "no audit row for the grant: {log}"
    );

    // clearing the role takes the access away on the next request
    assert_eq!(grant(admin_token(), operator, json!(null)).await, 200);
    assert_eq!(get("/api/v1/feature-flags", &operator_token).await, 403);
}
