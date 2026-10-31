//! Alert rules and channels owned by the people whose slice they watch (#1829).

use crate::support::*;
use serde_json::{json, Value};
use uuid::Uuid;

struct Tenants {
    pool: sqlx::PgPool,
    base: String,
    root: String,
    org_a: Uuid,
    team_a: Uuid,
    project_a: Uuid,
    org_b: Uuid,
    project_b: Uuid,
    admin_a: String,
    admin_b: String,
    team_admin_a: String,
    viewer_a: String,
    _db: rolter_store::postgres::test_schema::TestSchema,
}

async fn insert(pool: &sqlx::PgPool, sql: &'static str, parent: Option<Uuid>, name: &str) -> Uuid {
    let mut query = sqlx::query_scalar(sql);
    if let Some(parent) = parent {
        query = query.bind(parent);
    }
    query.bind(name).fetch_one(pool).await.unwrap()
}

async fn tenants() -> Tenants {
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("al".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let mut ids = Vec::new();
    for slug in ["org-a", "org-b"] {
        let org = sqlx::query_scalar::<_, Uuid>(
            "insert into orgs (name, slug) values ($1, $1) returning id",
        )
        .bind(slug)
        .fetch_one(&pool)
        .await
        .unwrap();
        let team = insert(
            &pool,
            "insert into teams (org_id, name) values ($1, $2) returning id",
            Some(org),
            "core",
        )
        .await;
        let project = insert(
            &pool,
            "insert into projects (team_id, name) values ($1, $2) returning id",
            Some(team),
            "app",
        )
        .await;
        ids.push((org, team, project));
    }
    let (org_a, team_a, project_a) = ids[0];
    let (org_b, _, project_b) = ids[1];
    let mut sessions = Vec::new();
    for (email, org, team, role) in [
        ("admin-a@x.test", Some(org_a), None, "admin"),
        ("admin-b@x.test", Some(org_b), None, "admin"),
        ("team-a@x.test", None, Some(team_a), "admin"),
        ("viewer-a@x.test", Some(org_a), None, "viewer"),
    ] {
        let user = seed_user(&pool, email, false).await;
        seed_membership(&pool, user, org, team, None, role).await;
        sessions.push(seed_session(&pool, user, &email.replace(['@', '.', '-'], "_")).await);
    }
    Tenants {
        base: format!("http://{addr}"),
        root: "al".to_string(),
        admin_a: sessions[0].clone(),
        admin_b: sessions[1].clone(),
        team_admin_a: sessions[2].clone(),
        viewer_a: sessions[3].clone(),
        pool,
        org_a,
        team_a,
        project_a,
        org_b,
        project_b,
        _db: db,
    }
}

async fn send(
    t: &Tenants,
    method: reqwest::Method,
    path: &str,
    bearer: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = reqwest::Client::new()
        .request(method, format!("{}{path}", t.base))
        .bearer_auth(bearer);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

fn rule(name: &str, scope: Option<(&str, Uuid)>, channel: Option<&str>) -> Value {
    let mut body = json!({
        "name": name, "signal": "error_rate", "threshold": 0.1,
        "window_secs": 300, "channel_id": channel, "enabled": false
    });
    if let Some((scope_type, scope_id)) = scope {
        body["scope_type"] = json!(scope_type);
        body["scope_id"] = json!(scope_id);
    }
    body
}

fn channel(name: &str, org: Option<Uuid>) -> Value {
    json!({"name": name, "endpoint": "https://hooks.example.com/alerts?token=t", "enabled": false, "org_id": org})
}

async fn post(t: &Tenants, path: &str, bearer: &str, body: Value) -> (u16, Value) {
    send(t, reqwest::Method::POST, path, bearer, Some(body)).await
}

async fn get(t: &Tenants, path: &str, bearer: &str) -> (u16, Value) {
    send(t, reqwest::Method::GET, path, bearer, None).await
}

#[tokio::test]
async fn creating_a_rule_takes_admin_at_its_scope_and_deployment_rules_stay_superadmin() {
    skip_without_db!();
    let t = tenants().await;

    // an org admin owns a rule for their org and for a project in it
    for (name, scope) in [
        ("org errors", ("org", t.org_a)),
        ("app errors", ("project", t.project_a)),
    ] {
        let (status, body) = post(
            &t,
            "/api/v1/alert-rules",
            &t.admin_a,
            rule(name, Some(scope), None),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["org_id"], json!(t.org_a));
        assert_eq!(body["scope_type"], scope.0);
    }
    // a team admin owns the team's, but not the org's above it
    let (status, _) = post(
        &t,
        "/api/v1/alert-rules",
        &t.team_admin_a,
        rule("team errors", Some(("team", t.team_a)), None),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = post(
        &t,
        "/api/v1/alert-rules",
        &t.team_admin_a,
        rule("org too", Some(("org", t.org_a)), None),
    )
    .await;
    assert_eq!(status, 403);
    // a viewer owns nothing; another org's admin owns nothing here
    let (status, _) = post(
        &t,
        "/api/v1/alert-rules",
        &t.viewer_a,
        rule("v", Some(("org", t.org_a)), None),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = post(
        &t,
        "/api/v1/alert-rules",
        &t.admin_b,
        rule("x", Some(("project", t.project_a)), None),
    )
    .await;
    assert_eq!(status, 403);
    // the whole deployment is the superadmin's, whoever asks
    let (status, _) = post(
        &t,
        "/api/v1/alert-rules",
        &t.admin_a,
        rule("everything", None, None),
    )
    .await;
    assert_eq!(status, 403);
    let (status, body) = post(
        &t,
        "/api/v1/alert-rules",
        &t.root,
        rule("everything", None, None),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["org_id"].is_null() && body["scope_type"].is_null());

    // a scope and its id come together, and health flaps are not per team
    let mut half = rule("half", None, None);
    half["scope_type"] = json!("org");
    let (status, _) = post(&t, "/api/v1/alert-rules", &t.root, half).await;
    assert_eq!(status, 400);
    let mut flaps = rule("flaps", Some(("team", t.team_a)), None);
    flaps["signal"] = json!("provider_health_flaps");
    let (status, _) = post(&t, "/api/v1/alert-rules", &t.admin_a, flaps).await;
    assert_eq!(status, 400);

    // two orgs may both have a rule of the same name
    let (status, _) = post(
        &t,
        "/api/v1/alert-rules",
        &t.admin_b,
        rule("org errors", Some(("org", t.org_b)), None),
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn a_rule_notifies_only_a_channel_of_its_own_org_or_the_deployment() {
    skip_without_db!();
    let t = tenants().await;
    let (status, own) = post(
        &t,
        "/api/v1/alert-channels",
        &t.admin_a,
        channel("a hook", Some(t.org_a)),
    )
    .await;
    assert_eq!(status, 200, "{own}");
    let (_, theirs) = post(
        &t,
        "/api/v1/alert-channels",
        &t.admin_b,
        channel("b hook", Some(t.org_b)),
    )
    .await;
    let (status, shared) = post(
        &t,
        "/api/v1/alert-channels",
        &t.root,
        channel("shared", None),
    )
    .await;
    assert_eq!(status, 200, "{shared}");
    let (own, theirs, shared) = (
        own["id"].as_str().unwrap().to_string(),
        theirs["id"].as_str().unwrap().to_string(),
        shared["id"].as_str().unwrap().to_string(),
    );
    let scope = Some(("org", t.org_a));

    // its own org's and the deployment's are fine
    for (name, id) in [("one", &own), ("two", &shared)] {
        let (status, body) = post(
            &t,
            "/api/v1/alert-rules",
            &t.admin_a,
            rule(name, scope, Some(id)),
        )
        .await;
        assert_eq!(status, 200, "{body}");
    }
    // another org's is refused, and so is one that does not exist, alike
    let (status, refused) = post(
        &t,
        "/api/v1/alert-rules",
        &t.admin_a,
        rule("three", scope, Some(&theirs)),
    )
    .await;
    assert_eq!(status, 400);
    let (status, unknown) = post(
        &t,
        "/api/v1/alert-rules",
        &t.admin_a,
        rule("four", scope, Some(&Uuid::new_v4().to_string())),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(refused, unknown);
    // a deployment-wide rule reads every tenant, so it may not notify one org's
    let (status, _) = post(
        &t,
        "/api/v1/alert-rules",
        &t.root,
        rule("five", None, Some(&own)),
    )
    .await;
    assert_eq!(status, 400);
    let (status, _) = post(
        &t,
        "/api/v1/alert-rules",
        &t.root,
        rule("six", None, Some(&shared)),
    )
    .await;
    assert_eq!(status, 200);

    // an update cannot repoint a rule at another org's channel either
    let (_, listed) = get(&t, "/api/v1/alert-rules", &t.admin_a).await;
    let id = listed[0]["id"].as_str().unwrap().to_string();
    let mut edit = rule("one", scope, Some(&theirs));
    edit["scope_type"] = Value::Null;
    edit["scope_id"] = Value::Null;
    let (status, _) = send(
        &t,
        reqwest::Method::PUT,
        &format!("/api/v1/alert-rules/{id}"),
        &t.admin_a,
        Some(edit),
    )
    .await;
    assert_eq!(status, 400);

    // an org admin owns an org channel, never a deployment one
    let (status, _) = post(
        &t,
        "/api/v1/alert-channels",
        &t.admin_a,
        channel("deployment", None),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = post(
        &t,
        "/api/v1/alert-channels",
        &t.admin_a,
        channel("b's", Some(t.org_b)),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = send(
        &t,
        reqwest::Method::DELETE,
        &format!("/api/v1/alert-channels/{shared}"),
        &t.admin_a,
        None,
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = send(
        &t,
        reqwest::Method::DELETE,
        &format!("/api/v1/alert-channels/{theirs}"),
        &t.admin_a,
        None,
    )
    .await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn lists_are_narrowed_to_what_the_caller_owns() {
    skip_without_db!();
    let t = tenants().await;
    let (_, shared) = post(
        &t,
        "/api/v1/alert-channels",
        &t.root,
        channel("shared", None),
    )
    .await;
    post(
        &t,
        "/api/v1/alert-channels",
        &t.admin_a,
        channel("a hook", Some(t.org_a)),
    )
    .await;
    post(
        &t,
        "/api/v1/alert-channels",
        &t.admin_b,
        channel("b hook", Some(t.org_b)),
    )
    .await;
    post(
        &t,
        "/api/v1/alert-rules",
        &t.admin_a,
        rule("a org", Some(("org", t.org_a)), None),
    )
    .await;
    post(
        &t,
        "/api/v1/alert-rules",
        &t.team_admin_a,
        rule("a team", Some(("team", t.team_a)), None),
    )
    .await;
    let (_, b_rule) = post(
        &t,
        "/api/v1/alert-rules",
        &t.admin_b,
        rule("b org", Some(("project", t.project_b)), None),
    )
    .await;
    post(
        &t,
        "/api/v1/alert-rules",
        &t.root,
        rule("everything", None, None),
    )
    .await;

    let names = |rows: &Value| -> Vec<String> {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect()
    };
    // org A's admin: their org's rules and nothing of B's or the deployment's
    let (_, rules) = get(&t, "/api/v1/alert-rules", &t.admin_a).await;
    assert_eq!(names(&rules), ["a org", "a team"]);
    // the team's admin: only the team's
    let (_, rules) = get(&t, "/api/v1/alert-rules", &t.team_admin_a).await;
    assert_eq!(names(&rules), ["a team"]);
    // the superadmin: all
    let (_, rules) = get(&t, "/api/v1/alert-rules", &t.root).await;
    assert_eq!(rules.as_array().unwrap().len(), 4);
    // a viewer has no alerting role anywhere
    assert_eq!(get(&t, "/api/v1/alert-rules", &t.viewer_a).await.0, 403);
    assert_eq!(get(&t, "/api/v1/alert-channels", &t.viewer_a).await.0, 403);
    assert_eq!(
        get(&t, "/api/v1/alert-notifications", &t.viewer_a).await.0,
        403
    );

    // channels: their org's and the shared one, whose address stays hidden
    let (_, channels) = get(&t, "/api/v1/alert-channels", &t.admin_a).await;
    assert_eq!(names(&channels), ["a hook", "shared"]);
    let rows = channels.as_array().unwrap();
    assert_eq!(rows[0]["manageable"], true);
    assert!(rows[0]["endpoint"]
        .as_str()
        .unwrap()
        .contains("hooks.example.com"));
    assert_eq!(rows[1]["id"], shared["id"]);
    assert_eq!(rows[1]["manageable"], false);
    assert_eq!(rows[1]["endpoint"], "");
    // a team admin sees their org's channel to name it, but cannot manage it
    let (_, channels) = get(&t, "/api/v1/alert-channels", &t.team_admin_a).await;
    assert_eq!(channels[0]["manageable"], false);
    assert_eq!(channels[0]["endpoint"], "");

    // another org's rule is invisible and untouchable
    let id = b_rule["id"].as_str().unwrap();
    let edit = rule("b org", None, None);
    let (status, _) = send(
        &t,
        reqwest::Method::PUT,
        &format!("/api/v1/alert-rules/{id}"),
        &t.admin_a,
        Some(edit),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = send(
        &t,
        reqwest::Method::DELETE,
        &format!("/api/v1/alert-rules/{id}"),
        &t.admin_a,
        None,
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = post(
        &t,
        &format!("/api/v1/alert-rules/{id}/evaluate"),
        &t.admin_a,
        json!({}),
    )
    .await;
    assert_eq!(status, 403);

    // history follows the rules the caller may see
    for (rule_name, state) in [
        ("a org", "firing"),
        ("b org", "firing"),
        ("everything", "firing"),
    ] {
        sqlx::query(
            "insert into alert_notification_history (rule_id, state, delivery_status)
             select id, $2, 'skipped' from alert_rules where name = $1",
        )
        .bind(rule_name)
        .bind(state)
        .execute(&t.pool)
        .await
        .unwrap();
    }
    let (_, history) = get(&t, "/api/v1/alert-notifications", &t.admin_a).await;
    assert_eq!(history.as_array().unwrap().len(), 1);
    let (_, history) = get(&t, "/api/v1/alert-notifications", &t.root).await;
    assert_eq!(history.as_array().unwrap().len(), 3);

    // deleting an org takes its channels and rules with it
    sqlx::query("delete from orgs where id = $1")
        .bind(t.org_b)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, rules) = get(&t, "/api/v1/alert-rules", &t.root).await;
    assert_eq!(rules.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn an_orgs_alert_changes_are_in_that_orgs_audit_log() {
    skip_without_db!();
    let t = tenants().await;
    // an org-scoped channel and rule: create, update and delete
    let (_, channel) = post(
        &t,
        "/api/v1/alert-channels",
        &t.admin_a,
        channel("a hook", Some(t.org_a)),
    )
    .await;
    let channel_id = channel["id"].as_str().unwrap().to_string();
    let (_, created) = post(
        &t,
        "/api/v1/alert-rules",
        &t.admin_a,
        rule("a org", Some(("org", t.org_a)), None),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_string();
    let mut edit = rule("a org renamed", None, None);
    edit["threshold"] = json!(0.2);
    let (status, _) = send(
        &t,
        reqwest::Method::PUT,
        &format!("/api/v1/alert-rules/{id}"),
        &t.admin_a,
        Some(edit),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = send(
        &t,
        reqwest::Method::DELETE,
        &format!("/api/v1/alert-rules/{id}"),
        &t.admin_a,
        None,
    )
    .await;
    assert_eq!(status, 204);
    let (status, _) = send(
        &t,
        reqwest::Method::DELETE,
        &format!("/api/v1/alert-channels/{channel_id}"),
        &t.admin_a,
        None,
    )
    .await;
    assert_eq!(status, 204);
    // a deployment-wide rule is not any org's to read about
    post(
        &t,
        "/api/v1/alert-rules",
        &t.root,
        rule("everything", None, None),
    )
    .await;

    let (status, log) = get(
        &t,
        &format!("/api/v1/orgs/{}/audit-log?limit=100", t.org_a),
        &t.admin_a,
    )
    .await;
    assert_eq!(status, 200, "{log}");
    let mut actions: Vec<String> = log["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items in {log}"))
        .iter()
        .filter_map(|e| e["action"].as_str())
        .filter(|a| a.starts_with("alert."))
        .map(String::from)
        .collect();
    actions.sort();
    assert_eq!(
        actions,
        [
            "alert.channel.create",
            "alert.channel.delete",
            "alert.rule.create",
            "alert.rule.delete",
            "alert.rule.update"
        ]
    );
    // and they are in no other org's log
    let (_, other) = get(
        &t,
        &format!("/api/v1/orgs/{}/audit-log?limit=100", t.org_b),
        &t.admin_b,
    )
    .await;
    assert!(other["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| !e["action"].as_str().unwrap_or("").starts_with("alert.")));
}

/// #3361: a scoped admin's rules are chosen in SQL from their reach. Whatever
/// other orgs hold, the listings answer the same (what is read from the table is
/// pinned by `alerting::tests::delivery::only_the_rules_inside_the_reach_are_read`).
#[tokio::test]
async fn other_orgs_rules_do_not_change_a_scoped_admins_listings() {
    skip_without_db!();
    let t = tenants().await;
    for (bearer, name, scope) in [
        (&t.admin_a, "a project", ("project", t.project_a)),
        (&t.team_admin_a, "a team", ("team", t.team_a)),
    ] {
        let (status, body) = post(
            &t,
            "/api/v1/alert-rules",
            bearer,
            rule(name, Some(scope), None),
        )
        .await;
        assert_eq!(status, 200, "{body}");
    }
    let names = |rows: &Value| -> Vec<String> {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect()
    };
    let paths = ["/api/v1/alert-rules", "/api/v1/alert-notifications"];
    let mut before = Vec::new();
    for path in paths {
        let (status, body) = get(&t, path, &t.admin_a).await;
        assert_eq!(status, 200, "{path}: {body}");
        before.push(names(&body));
    }

    // another org's project rules, each of which the old walk resolved with
    // queries of its own
    for i in 0..25 {
        let (status, body) = post(
            &t,
            "/api/v1/alert-rules",
            &t.admin_b,
            rule(&format!("b {i}"), Some(("project", t.project_b)), None),
        )
        .await;
        assert_eq!(status, 200, "{body}");
    }
    for (path, was_names) in paths.iter().zip(before) {
        let (status, body) = get(&t, path, &t.admin_a).await;
        assert_eq!(status, 200, "{path}: {body}");
        assert_eq!(names(&body), was_names, "{path}");
    }
    let (_, rules) = get(&t, "/api/v1/alert-rules", &t.admin_a).await;
    assert_eq!(names(&rules), ["a project", "a team"]);
}

/// #3362: a deployment operator runs every rule and channel through the same
/// guard a scoped admin does, and the matrix and `effective` say so.
#[tokio::test]
async fn a_deployment_operator_runs_every_rule_and_the_matrix_publishes_it() {
    skip_without_db!();
    let t = tenants().await;
    let mut sessions = Vec::new();
    for (email, role) in [
        ("operator@x.test", "operator"),
        ("auditor@x.test", "security_auditor"),
    ] {
        let user = seed_user(&t.pool, email, false).await;
        sqlx::query("update users set deployment_role = $2 where id = $1")
            .bind(user)
            .bind(role)
            .execute(&t.pool)
            .await
            .unwrap();
        sessions.push(seed_session(&t.pool, user, &email.replace(['@', '.'], "_")).await);
    }
    let (operator, auditor) = (&sessions[0], &sessions[1]);

    // published: an admin at the scope, or an operator
    let (_, matrix) = get(&t, "/api/v1/rbac/matrix", operator).await;
    let update = matrix["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["resource"] == "alert_rule")
        .and_then(|r| {
            r["actions"]
                .as_array()?
                .iter()
                .find(|a| a["action"] == "update")
        })
        .expect("alert_rule:update in the matrix")
        .clone();
    assert_eq!(update["minimum_role"], "admin");
    assert_eq!(update["deployment_roles"], json!(["operator"]));
    let (_, effective) = get(&t, "/api/v1/rbac/effective", operator).await;
    let allowed: Vec<&str> = effective["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for pair in [
        "alert_channel:create",
        "alert_rule:update",
        "alert_history:create",
    ] {
        assert!(allowed.contains(&pair), "operator lacks {pair}");
    }
    let (_, effective) = get(&t, "/api/v1/rbac/effective", auditor).await;
    assert!(
        !effective["allowed"].to_string().contains("alert_rule"),
        "{effective}"
    );

    // and honoured: any org's rule, a deployment rule, a channel, no membership
    let (status, theirs) = post(
        &t,
        "/api/v1/alert-rules",
        operator,
        rule("b by operator", Some(("project", t.project_b)), None),
    )
    .await;
    assert_eq!(status, 200, "{theirs}");
    let (status, everything) = post(
        &t,
        "/api/v1/alert-rules",
        operator,
        rule("everything", None, None),
    )
    .await;
    assert_eq!(status, 200, "{everything}");
    let (status, _) = post(
        &t,
        "/api/v1/alert-channels",
        operator,
        channel("a hook", Some(t.org_a)),
    )
    .await;
    assert_eq!(status, 200);
    let (_, rules) = get(&t, "/api/v1/alert-rules", operator).await;
    assert_eq!(rules.as_array().unwrap().len(), 2);
    let id = theirs["id"].as_str().unwrap();
    let (status, _) = send(
        &t,
        reqwest::Method::DELETE,
        &format!("/api/v1/alert-rules/{id}"),
        operator,
        None,
    )
    .await;
    assert_eq!(status, 204);
    // a deployment role that is not the operator's reaches none of it
    assert_eq!(get(&t, "/api/v1/alert-rules", auditor).await.0, 403);
    assert_eq!(
        post(
            &t,
            "/api/v1/alert-rules",
            auditor,
            rule("x", Some(("org", t.org_a)), None)
        )
        .await
        .0,
        403
    );
}
