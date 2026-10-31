//! Audit log listings, exports and who an audit row names.

use crate::support::*;
use serde_json::{json, Value};

/// Account events are written with no org. An org's audit log shows them for
/// its own people, and not for anyone else's (#1854).
#[tokio::test]
async fn an_orgs_audit_log_shows_its_own_peoples_account_events() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("audit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
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
    let admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, admin, Some(org), None, None, "admin").await;
    let member = seed_user(&pool, "pm@acme.test", false).await;
    seed_membership(&pool, member, None, None, Some(project), "member").await;
    let stranger = seed_user(&pool, "ops@globex.test", false).await;
    for (user, action) in [
        (member, "auth.mfa_break_glass_reset"),
        (member, "auth.login_failed"),
        (stranger, "auth.login_failed"),
    ] {
        sqlx::query(
            "insert into audit_log (org_id, actor_user_id, action, target_type, target_id, detail)
             values (null, $1, $2, 'user', $1, '{}')",
        )
        .bind(user)
        .bind(action)
        .execute(&pool)
        .await
        .unwrap();
    }
    let session = seed_session(&pool, admin, "audit_admin").await;
    let page: Value = client
        .get(format!("http://{addr}/api/v1/orgs/{org}/audit-log"))
        .bearer_auth(session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows: Vec<(String, String)> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["actor_user_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                row["action"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let member = member.to_string();
    assert!(
        rows.contains(&(member.clone(), "auth.mfa_break_glass_reset".to_string())),
        "{rows:?}"
    );
    assert!(
        rows.contains(&(member, "auth.login_failed".to_string())),
        "{rows:?}"
    );
    assert!(
        !rows.iter().any(|(actor, _)| actor == &stranger.to_string()),
        "another tenant's account events: {rows:?}"
    );
}

/// The deployment-wide read returns the rows no org read can: a superadmin's
/// own events and an attempt against an unregistered address. It is
/// superadmin-only, and pages and filters like the per-org read (#1858).
#[tokio::test]
async fn the_deployment_audit_log_is_a_superadmins_and_returns_org_less_rows() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("audit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let operator = seed_user(&pool, "operator@deploy.test", true).await;
    let org_admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, org_admin, Some(org), None, None, "admin").await;
    // oldest first; none of them carries an org
    for (minutes_ago, actor, action, target) in [
        (4, Some(operator), "auth.login", Some(operator)),
        (3, None, "auth.login_failed", None),
        (2, Some(operator), "auth.mfa_enrolled", Some(operator)),
        (1, Some(operator), "auth.login", Some(operator)),
    ] {
        sqlx::query(
            "insert into audit_log (org_id, actor_user_id, action, target_type, target_id, detail, at)
             values (null, $1, $2, case when $3::uuid is null then null else 'user' end, $3, '{}',
                     now() - make_interval(mins => $4))",
        )
        .bind(actor)
        .bind(action)
        .bind(target)
        .bind(minutes_ago)
        .execute(&pool)
        .await
        .unwrap();
    }
    let operator_session = seed_session(&pool, operator, "deploy_operator").await;
    let admin_session = seed_session(&pool, org_admin, "deploy_org_admin").await;
    let url = format!("http://{addr}/api/v1/audit-log");

    // an org admin does not read the deployment's log
    let denied = client
        .get(&url)
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);

    let page: Value = client
        .get(&url)
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let actions: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions.iter().filter(|a| **a == "auth.login").count(), 2);
    let unknown = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["action"] == "auth.login_failed")
        .expect("the unknown-address attempt is returned");
    assert!(unknown["actor_user_id"].is_null(), "{unknown}");
    assert!(unknown["org_id"].is_null(), "{unknown}");
    // the per-org read still cannot see the operator's events
    let per_org: Value = client
        .get(format!("http://{addr}/api/v1/orgs/{org}/audit-log"))
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(per_org["items"].as_array().unwrap().is_empty(), "{per_org}");

    // filters
    let filtered: Value = client
        .get(format!("{url}?action=auth.mfa_enrolled&include_total=true"))
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(filtered["items"].as_array().unwrap().len(), 1, "{filtered}");
    assert_eq!(filtered["total"], 1);
    let by_actor: Value = client
        .get(format!("{url}?actor={operator}"))
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(by_actor["items"].as_array().unwrap().len(), 3, "{by_actor}");

    // cursor: pages of two, newest first, then back
    let first: Value = client
        .get(format!("{url}?limit=2&include_total=true"))
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert_eq!(first["total"], 4);
    assert_eq!(first["has_next"], true);
    assert_eq!(first["has_previous"], false);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    let second: Value = client
        .get(&url)
        .query(&[("limit", "2"), ("cursor", cursor.as_str())])
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 2);
    assert_eq!(second["has_next"], false);
    assert_eq!(second["has_previous"], true);
    let mut seen: Vec<String> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["items"].as_array().unwrap())
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 4, "the pages overlap or skip a row");
    assert_eq!(second["items"][0]["action"], "auth.login_failed");
    let back: Value = client
        .get(&url)
        .query(&[
            ("limit", "2"),
            ("direction", "previous"),
            ("cursor", second["previous_cursor"].as_str().unwrap()),
        ])
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(back["items"], first["items"]);
}

/// The export walks the log past one page, so a long log arrives whole, under
/// the page read's filters and scoping, with the actor named and a formula
/// defused, and the export itself is on the record (#3051).
#[tokio::test]
async fn the_audit_export_streams_every_page_under_the_page_reads_scope() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("audit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let org: uuid::Uuid = sqlx::query_scalar(
        "insert into orgs (name, slug) values ('Acme Corp', 'acme-corp') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let other: uuid::Uuid = sqlx::query_scalar(
        "insert into orgs (name, slug) values ('Globex', 'globex') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, admin, Some(org), None, None, "admin").await;
    let viewer = seed_user(&pool, "viewer@acme.test", false).await;
    seed_membership(&pool, viewer, Some(org), None, None, "viewer").await;
    let outsider = seed_user(&pool, "admin@globex.test", false).await;
    seed_membership(&pool, outsider, Some(other), None, None, "admin").await;
    // 2,500 rows is three pages of 1,000; the odd ones carry another action
    sqlx::query(
        "insert into audit_log (org_id, actor_user_id, action, target_type, target_id, detail, at)
         select $1, $2, case when n % 2 = 0 then 'budget.update' else 'budget.create' end,
                'budget', gen_random_uuid(), jsonb_build_object('n', n),
                now() - make_interval(secs => n)
         from generate_series(1, 2500) n",
    )
    .bind(org)
    .bind(admin)
    .execute(&pool)
    .await
    .unwrap();
    // a value typed by someone else: a spreadsheet would run this as a formula
    sqlx::query(
        "insert into audit_log (org_id, actor_user_id, action, target_type, detail, at)
         values ($1, $2, 'budget.update', 'budget', to_jsonb('=HYPERLINK(\"http://x\")'::text),
                 now() + interval '1 minute')",
    )
    .bind(org)
    .bind(admin)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into audit_log (org_id, actor_user_id, action, target_type, target_id, detail)
         select $1, $2, 'budget.create', 'budget', gen_random_uuid(), '{}'
         from generate_series(1, 5)",
    )
    .bind(other)
    .bind(outsider)
    .execute(&pool)
    .await
    .unwrap();
    let admin_session = seed_session(&pool, admin, "export_admin").await;
    let viewer_session = seed_session(&pool, viewer, "export_viewer").await;
    let outsider_session = seed_session(&pool, outsider, "export_out").await;
    let url = format!("http://{addr}/api/v1/orgs/{org}/audit-log/export");

    let csv = client
        .get(&url)
        .query(&[("format", "csv")])
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap();
    assert_eq!(csv.status(), 200);
    let disposition = csv.headers()["content-disposition"].to_str().unwrap();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert_eq!(
        disposition,
        format!("attachment; filename=\"audit-log-acme-corp-all_to_{today}.csv\"")
    );
    assert!(csv.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/csv"));
    let body = csv.text().await.unwrap();
    let lines: Vec<&str> = body.split("\r\n").collect();
    assert_eq!(
        lines[0],
        "at,actor_email,actor_user_id,action,target_type,target_id,org_id,entry_id,detail"
    );
    // header + 2,501 rows + this export's own audit entry (written before the
    // first byte) + the empty tail after the last CRLF
    assert_eq!(lines.len(), 1 + 2502 + 1, "every page is in the file");
    assert!(lines.last().unwrap().is_empty());
    assert!(
        lines[1].contains("'=HYPERLINK"),
        "newest first, and the formula is defused: {}",
        lines[1]
    );
    assert!(lines[1].contains(",admin@acme.test,"), "{}", lines[1]);
    assert!(lines[1].split(',').next().unwrap().ends_with('Z'));
    assert!(!body.contains(&other.to_string()), "another org's rows");
    let mut ids: Vec<&str> = lines[1..2503]
        .iter()
        .map(|line| line.split(',').nth(7).unwrap())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 2502, "a page overlapped or skipped a row");

    // the filters of the page read narrow the file
    let filtered = client
        .get(&url)
        .query(&[("format", "csv"), ("action", "budget.update")])
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        filtered.split("\r\n").count(),
        1 + 1251 + 1,
        "{filtered:.200}"
    );
    let none = client
        .get(&url)
        .query(&[("format", "csv"), ("start_at", "2999-01-01T00:00:00Z")])
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap();
    assert!(none.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .contains("2999-01-01_to_"));
    assert_eq!(none.text().await.unwrap().split("\r\n").count(), 2);
    let backwards = client
        .get(&url)
        .query(&[
            ("format", "csv"),
            ("start_at", "2026-02-01T00:00:00Z"),
            ("end_at", "2026-01-01T00:00:00Z"),
        ])
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap();
    assert_eq!(backwards.status(), 400);

    // json is one valid array holding the same rows
    let json: Value = client
        .get(&url)
        .query(&[("format", "json"), ("action", "budget.create")])
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = json.as_array().unwrap();
    assert_eq!(rows.len(), 1250);
    assert_eq!(rows[0]["actor_email"], "admin@acme.test");
    assert_eq!(rows[0]["org_id"], org.to_string());
    assert!(rows[0]["detail"]["n"].is_number());

    // never wider than the page read: a viewer, another org's admin and the
    // deployment-wide route are all refused
    for session in [&viewer_session, &outsider_session] {
        let denied = client
            .get(&url)
            .query(&[("format", "csv")])
            .bearer_auth(session)
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), 403);
    }
    let wide = client
        .get(format!("http://{addr}/api/v1/audit-log/export"))
        .query(&[("format", "csv")])
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap();
    assert_eq!(wide.status(), 403);
    let bad = client
        .get(&url)
        .query(&[("format", "xml")])
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    // the export is on the record, with who ran it and under what filter;
    // the refused ones are not
    let recorded: Vec<(Option<uuid::Uuid>, Value)> = sqlx::query_as(
        "select actor_user_id, detail from audit_log where action = 'audit_log.export'
         order by at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(recorded.len(), 4, "{recorded:?}");
    assert!(recorded.iter().all(|(actor, _)| *actor == Some(admin)));
    assert_eq!(recorded[1].1["action"], "budget.update");
    assert_eq!(recorded[1].1["format"], "csv");
    assert_eq!(recorded[3].1["format"], "json");
}

/// The deployment-wide export is the superadmin's and includes org-less rows.
#[tokio::test]
async fn the_deployment_audit_export_is_a_superadmins() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("audit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let operator = seed_user(&pool, "operator@deploy.test", true).await;
    sqlx::query(
        "insert into audit_log (org_id, actor_user_id, action, target_type, target_id, detail)
         values (null, $1, 'auth.login', 'user', $1, '{}')",
    )
    .bind(operator)
    .execute(&pool)
    .await
    .unwrap();
    let session = seed_session(&pool, operator, "deploy_export").await;
    let url = format!("http://{addr}/api/v1/audit-log/export");
    let response = client
        .get(&url)
        .query(&[("format", "csv")])
        .bearer_auth(&session)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .contains("audit-log-deployment-all_to_"));
    let body = response.text().await.unwrap();
    assert!(body.contains(",operator@deploy.test,"), "{body}");
    assert!(body.contains(",auth.login,"), "{body}");
    let unauthenticated = client
        .get(&url)
        .query(&[("format", "csv")])
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), 401);
}

/// `end_at` is exclusive, as `GET /openapi.json` documents and the analytics
/// reads do: a row stamped exactly at the upper bound belongs to the next
/// window, in the page read, its total and the export, per org and
/// deployment-wide alike, while `start_at` keeps its row (#3141).
#[tokio::test]
async fn the_audit_logs_end_at_is_exclusive() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("audit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let org: uuid::Uuid = sqlx::query_scalar(
        "insert into orgs (name, slug) values ('Acme Corp', 'acme-corp') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, admin, Some(org), None, None, "admin").await;
    let operator = seed_user(&pool, "operator@deploy.test", true).await;
    // one row on each bound and one just inside the upper one
    for (action, at) in [
        ("budget.create", "2026-01-01T00:00:00Z"),
        ("budget.update", "2026-01-01T23:59:59.999Z"),
        ("budget.delete", "2026-01-02T00:00:00Z"),
    ] {
        sqlx::query(
            "insert into audit_log (org_id, actor_user_id, action, target_type, at)
             values ($1, $2, $3, 'budget', $4::timestamptz)",
        )
        .bind(org)
        .bind(admin)
        .bind(action)
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();
    }
    let admin_session = seed_session(&pool, admin, "bound_admin").await;
    let operator_session = seed_session(&pool, operator, "bound_operator").await;
    let window = [
        ("start_at", "2026-01-01T00:00:00Z"),
        ("end_at", "2026-01-02T00:00:00Z"),
    ];

    for (base, session) in [
        (
            format!("http://{addr}/api/v1/orgs/{org}/audit-log"),
            &admin_session,
        ),
        (format!("http://{addr}/api/v1/audit-log"), &operator_session),
    ] {
        let page: Value = client
            .get(&base)
            .query(&window)
            .query(&[("include_total", "true")])
            .bearer_auth(session)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let actions: Vec<&str> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["action"].as_str().unwrap())
            .collect();
        assert_eq!(
            actions,
            ["budget.update", "budget.create"],
            "{base}: {page}"
        );
        assert_eq!(page["total"], 2, "{base}: {page}");

        let export: Value = client
            .get(format!("{base}/export"))
            .query(&window)
            .query(&[("format", "json")])
            .bearer_auth(session)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let actions: Vec<&str> = export
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["action"].as_str().unwrap())
            .collect();
        assert_eq!(
            actions,
            ["budget.update", "budget.create"],
            "{base}/export: {export}"
        );
    }
}

/// The `actor_user_id` of the newest row with `action` in the deployment-wide
/// audit log, read the way the dashboard reads it, or `Null` for a row with no
/// actor.
async fn newest_deployment_actor(
    client: &reqwest::Client,
    base: &str,
    reader: &str,
    action: &str,
) -> Value {
    let page: Value = client
        .get(format!("{base}/api/v1/audit-log?action={action}&limit=1"))
        .bearer_auth(reader)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "no {action} row in the audit log");
    items[0]["actor_user_id"].clone()
}

/// The shared audit helper (`crud::log_audit`) records the signed-in account
/// behind every action that goes through it. A superadmin session used to be
/// written with no actor, so the audit log could not say which superadmin
/// created an org or removed a team -- the account an audit trail matters most
/// for. The admin token is a credential rather than a person and stays
/// unrecorded.
#[tokio::test]
async fn an_audited_action_names_the_superadmin_session_that_took_it() {
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

    // an org-scoped row (`team.create`) taken by each kind of caller
    let create_team = |bearer: String, name: &'static str| {
        let client = client.clone();
        let url = format!("{base}/api/v1/orgs/{acme}/teams");
        async move {
            let response = client
                .post(url)
                .bearer_auth(bearer)
                .json(&json!({ "name": name }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let team: Value = response.json().await.unwrap();
            team["id"].as_str().unwrap().to_string()
        }
    };
    let by_operator = create_team(actors.operator_bearer.clone(), "by-operator").await;
    let by_manager = create_team(actors.manager_bearer.clone(), "by-manager").await;
    let by_token = create_team(admin_token().to_string(), "by-token").await;

    let page: Value = client
        .get(format!(
            "{base}/api/v1/orgs/{acme}/audit-log?action=team.create"
        ))
        .bearer_auth(&actors.operator_bearer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let actor_of = |team: &str| {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["target_id"] == team)
            .map(|row| row["actor_user_id"].clone())
    };
    assert_eq!(
        actor_of(&by_operator),
        Some(json!(actors.operator.to_string()))
    );
    assert_eq!(
        actor_of(&by_manager),
        Some(json!(actors.manager.to_string()))
    );
    assert_eq!(actor_of(&by_token), Some(Value::Null));

    // a row scoped to the org the superadmin has just made: the org's own log
    // names who created it
    let created: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(&actors.operator_bearer)
        .json(&json!({"name": "Globex", "slug": "globex"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let globex = created["id"].as_str().unwrap().to_string();
    let globex_log: Value = client
        .get(format!(
            "{base}/api/v1/orgs/{globex}/audit-log?action=org.create"
        ))
        .bearer_auth(&actors.operator_bearer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        globex_log["items"][0]["actor_user_id"],
        json!(actors.operator.to_string())
    );

    // and the deployment-wide log can be asked what one superadmin did
    let by_actor: Value = client
        .get(format!("{base}/api/v1/audit-log?actor={}", actors.operator))
        .bearer_auth(&actors.operator_bearer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut actions: Vec<&str> = by_actor["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["action"].as_str().unwrap())
        .collect();
    actions.sort_unstable();
    assert_eq!(actions, ["org.create", "team.create"]);
}

/// The audit rows written by hand name the signed-in account too: each of the
/// deployment-wide settings, the connector and alert-channel helpers, and the
/// cluster inventory (#2844). They are the rows with no org, so the
/// deployment-wide audit log is where they are read. A table, because the sites
/// are separate copies of one rule and a missed one is a silent gap.
#[tokio::test]
async fn hand_written_audit_rows_name_the_superadmin_session_that_wrote_them() {
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

    // (method, path, body for the caller named, the audit action it writes)
    type Case = (&'static str, &'static str, fn(&str) -> Value, &'static str);
    let cases: [Case; 10] = [
        (
            "PUT",
            "/api/v1/feature-flags",
            |_| {
                json!({
                    "response_cache": true, "cache_aware_routing": true, "circuit_breaker": true,
                    "active_health_checks": true, "complexity_routing": true, "guardrails": true
                })
            },
            "feature_flags.update",
        ),
        (
            "PUT",
            "/api/v1/runtime-policy",
            |_| {
                json!({
                    "retry_max_retries": 4, "retry_base_ms": 150, "retry_max_ms": 1500,
                    "timeout_connect_s": 20, "timeout_request_s": 180, "queue_enabled": true,
                    "queue_capacity": 1024, "queue_workers": 16, "queue_backpressure": "block",
                    "queue_block_ms": 5000
                })
            },
            "runtime_policy.update",
        ),
        (
            "PUT",
            "/api/v1/compatibility-policy",
            |_| {
                json!({
                    "anthropic_version": "2023-06-01", "default_max_tokens": 1024
                })
            },
            "compatibility_policy.update",
        ),
        (
            "PUT",
            "/api/v1/adaptive-routing-policy",
            |_| {
                json!({
                    "enabled": true, "latency_weight": 0.5, "cost_weight": 0.3, "load_weight": 0.2,
                    "exploration_ratio": 0.05, "min_samples": 50
                })
            },
            "adaptive_routing_policy.update",
        ),
        (
            "PUT",
            "/api/v1/logging-settings",
            |_| {
                json!({
                    "sample_rate": 0.25, "payload_capture_enabled": false,
                    "payload_capture_max_bytes": 4096, "payload_capture_redact_fields": [],
                    "payload_capture_models": [], "payload_capture_virtual_key_ids": [],
                    "retention_days": 30, "payload_retention_hours": 24
                })
            },
            "logging_settings.update",
        ),
        (
            "PUT",
            "/api/v1/model-defaults",
            |_| json!({"enabled": false}),
            "model_defaults.update",
        ),
        (
            "PUT",
            "/api/v1/client-settings",
            |_| {
                json!({
                    "forwarded_headers": [], "injected_headers": {}, "request_id_header": "x-request-id"
                })
            },
            "client_settings.update",
        ),
        (
            "PUT",
            "/api/v1/security-settings",
            |_| {
                json!({
                    "allowed_origins": [], "allowed_headers": [], "required_headers": {},
                    "auth_bypass_routes": []
                })
            },
            "security.settings.update",
        ),
        (
            "POST",
            "/api/v1/connectors",
            |who| {
                json!({
                    "name": format!("sink-{who}"), "kind": "otlp_http",
                    "endpoint": "https://collector.example.com/v1/logs", "enabled": false,
                    "sampling_rate": 1.0
                })
            },
            "connector.create",
        ),
        (
            "POST",
            "/api/v1/alert-channels",
            |who| {
                json!({
                    "name": format!("hook-{who}"), "endpoint": "https://hooks.example.com/alert",
                    "enabled": false
                })
            },
            "alert.channel.create",
        ),
    ];

    let callers = [
        (
            "operator",
            actors.operator_bearer.clone(),
            json!(actors.operator.to_string()),
        ),
        ("admin-token", admin_token().to_string(), Value::Null),
    ];
    // collected rather than asserted one by one, so a failing run names every
    // site that records the wrong actor instead of stopping at the first
    let mut wrong: Vec<String> = Vec::new();
    for (method, path, body, action) in cases {
        for (who, bearer, expected) in &callers {
            let request = match method {
                "PUT" => client.put(format!("{base}{path}")),
                _ => client.post(format!("{base}{path}")),
            };
            let response = request
                .bearer_auth(bearer)
                .json(&body(who))
                .send()
                .await
                .unwrap();
            assert!(
                response.status().is_success(),
                "{action} as {who} answered {}",
                response.status()
            );
            let actor =
                newest_deployment_actor(&client, &base, &actors.operator_bearer, action).await;
            if &actor != expected {
                wrong.push(format!("{action} as {who}"));
            }
        }
    }

    // the cluster inventory: two writes, each by both kinds of caller
    for node in ["gw-1", "gw-2", "gw-3"] {
        let polled = client
            .get(format!("{base}/internal/snapshot?version=0"))
            .bearer_auth(admin_token())
            .header("x-rolter-node-id", node)
            .header("x-rolter-node-role", "gateway")
            .send()
            .await
            .unwrap();
        assert!(polled.status().is_success());
    }
    for (who, bearer, expected) in &callers {
        let drained = client
            .put(format!("{base}/api/v1/cluster/nodes/gw-1/drain"))
            .bearer_auth(bearer)
            .json(&json!({"draining": *who == "operator"}))
            .send()
            .await
            .unwrap();
        assert!(drained.status().is_success(), "drain as {who}");
        let actor = newest_deployment_actor(
            &client,
            &base,
            &actors.operator_bearer,
            "cluster_node.set_drain",
        )
        .await;
        if &actor != expected {
            wrong.push(format!("cluster_node.set_drain as {who}"));
        }
    }
    for ((who, bearer, expected), node) in callers.iter().zip(["gw-2", "gw-3"]) {
        let forgotten = client
            .delete(format!("{base}/api/v1/cluster/nodes/{node}"))
            .bearer_auth(bearer)
            .send()
            .await
            .unwrap();
        assert_eq!(forgotten.status(), 204, "forget as {who}");
        let actor = newest_deployment_actor(
            &client,
            &base,
            &actors.operator_bearer,
            "cluster_node.forget",
        )
        .await;
        if &actor != expected {
            wrong.push(format!("cluster_node.forget as {who}"));
        }
    }
    assert!(wrong.is_empty(), "recorded the wrong actor: {wrong:?}");
}

/// An org's audit log returns the account events of its people and nothing a
/// superadmin did to the deployment (#2857). The rows with no org are matched on
/// their actor, and a superadmin who also administers an org is one of that
/// org's people: with the actor now recorded (#2844), a rule that looked at the
/// actor alone handed the org's other admins every setting the superadmin
/// changed. The deployment-wide log still has all of them.
#[tokio::test]
async fn an_orgs_audit_log_omits_the_deployment_changes_of_a_superadmin_it_counts_among_its_people()
{
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
    // a superadmin who is also an admin of acme, and a second acme admin
    let both = seed_user(&pool, "both@example.com", true).await;
    seed_membership(&pool, both, Some(acme), None, None, "admin").await;
    let manager = seed_user(&pool, "manager@example.com", false).await;
    seed_membership(&pool, manager, Some(acme), None, None, "admin").await;
    let both_bearer = seed_session(&pool, both, &uuid::Uuid::new_v4().simple().to_string()).await;
    let manager_bearer =
        seed_session(&pool, manager, &uuid::Uuid::new_v4().simple().to_string()).await;

    // deployment changes, taken by the superadmin
    let changes = [
        (
            "PUT",
            "/api/v1/security-settings",
            json!({
                "allowed_origins": [], "allowed_headers": [], "required_headers": {},
                "auth_bypass_routes": []
            }),
            "security.settings.update",
        ),
        (
            "PUT",
            "/api/v1/model-defaults",
            json!({"enabled": false}),
            "model_defaults.update",
        ),
        (
            "POST",
            "/api/v1/connectors",
            json!({
                "name": "sink", "kind": "otlp_http",
                "endpoint": "https://collector.example.com/v1/logs", "enabled": false,
                "sampling_rate": 1.0
            }),
            "connector.create",
        ),
    ];
    for (method, path, body, action) in &changes {
        let request = match *method {
            "PUT" => client.put(format!("{base}{path}")),
            _ => client.post(format!("{base}{path}")),
        };
        let response = request
            .bearer_auth(&both_bearer)
            .json(body)
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "{action} answered {}",
            response.status()
        );
    }

    // account events of the same superadmin, which the org is meant to see
    let profile = client
        .patch(format!("{base}/api/v1/me/profile"))
        .bearer_auth(&both_bearer)
        .json(&json!({"bio": "on call"}))
        .send()
        .await
        .unwrap();
    assert_eq!(profile.status(), 200);
    sqlx::query(
        "insert into audit_log (org_id, actor_user_id, action, target_type, target_id, detail)
         values (null, $1, 'auth.login_failed', 'user', $1, '{}')",
    )
    .bind(both)
    .execute(&pool)
    .await
    .unwrap();

    let page: Value = client
        .get(format!(
            "{base}/api/v1/orgs/{acme}/audit-log?limit=500&include_total=true"
        ))
        .bearer_auth(&manager_bearer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = page["items"].as_array().unwrap();
    let mut actions: Vec<&str> = items
        .iter()
        .map(|row| row["action"].as_str().unwrap())
        .collect();
    actions.sort_unstable();
    assert_eq!(actions, ["auth.login_failed", "user.profile.update"]);
    // the count the dashboard pages by follows the same rule as the rows
    assert_eq!(page["total"], items.len());

    // and the deployment-wide log keeps every change, under the superadmin
    for (_, _, _, action) in &changes {
        let actor = newest_deployment_actor(&client, &base, &both_bearer, action).await;
        assert_eq!(actor, json!(both.to_string()), "{action}");
    }
}
