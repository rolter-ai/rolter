//! Users, memberships, invitations and the last-admin lockout guards.

use crate::support::*;
use serde_json::{json, Value};

/// `GET /api/v1/orgs/{org_id}/users` lists the people with a role in an org, so
/// the superadmin `rolter-seed --admin-email` creates -- who belongs to no org
/// -- was never on the Users screen to edit. `include_unassigned` adds the
/// accounts that hold no membership anywhere, for a superadmin only, and never
/// another org's people.
#[tokio::test]
async fn a_superadmin_can_list_the_accounts_that_belong_to_no_org() {
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
    let other: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Other', 'other') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let member = seed_user(&pool, "member@example.com", false).await;
    seed_membership(&pool, member, Some(acme), None, None, "admin").await;
    let elsewhere = seed_user(&pool, "elsewhere@example.com", false).await;
    seed_membership(&pool, elsewhere, Some(other), None, None, "member").await;
    let seeded_admin = seed_user(&pool, "root@example.com", true).await;
    seed_user(&pool, "stranded@example.com", false).await;
    let root_token = seed_session(&pool, seeded_admin, "root").await;
    let member_token = seed_session(&pool, member, "member").await;

    let emails = |token: String, query: &'static str| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let response = client
                .get(format!("{base}/api/v1/orgs/{acme}/users{query}"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{query}");
            let users: Vec<Value> = response.json().await.unwrap();
            users
                .iter()
                .map(|u| u["email"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };

    // unchanged for everyone who does not ask
    assert_eq!(
        emails(admin_token().to_string(), "").await,
        ["member@example.com"]
    );
    assert_eq!(
        emails(root_token.clone(), "?include_unassigned=false").await,
        ["member@example.com"]
    );
    // a superadmin, by session or by the admin token, also gets the accounts no
    // org lists, in one email-ordered list, and not the other org's member
    let wanted = [
        "member@example.com",
        "root@example.com",
        "stranded@example.com",
    ];
    assert_eq!(emails(root_token, "?include_unassigned=true").await, wanted);
    assert_eq!(
        emails(admin_token().to_string(), "?include_unassigned=true").await,
        wanted
    );
    // an org admin asking for it is not shown them: they could not edit them
    assert_eq!(
        emails(member_token, "?include_unassigned=true").await,
        ["member@example.com"]
    );
}

/// Full user lifecycle (ROL-223): invite an account into an org, list it, grant
/// a team-scoped role, then deactivate it and confirm login is blocked while the
/// row and its memberships survive.
#[tokio::test]
async fn user_and_membership_lifecycle() {
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
        json!({"email": "dev@example.com", "password": password, "role": "member"}),
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
        .json(&json!({"email": "dev@example.com", "password": password}))
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
        .json(&json!({"email": "dev@example.com", "password": password}))
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
        .json(&json!({"email": "dev@example.com", "password": password}))
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

/// Invitation onboarding (#712): an admin mints a one-time link, the invitee
/// sets their own password, and every way the link can be misused fails.
#[tokio::test]
async fn invitations_onboard_accounts_once_and_expire_closed() {
    skip_without_db!();
    let password = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "InviteOrg", "slug": "invite-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let team: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Platform"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let team_id = team["id"].as_str().unwrap().to_string();

    // a role rolter does not have is refused before anything is stored
    let bad_role = client
        .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth(admin_token())
        .json(&json!({"email": "ada@example.com", "role": "root"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_role.status(), 400);

    let created: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth(admin_token())
        .json(&json!({
            "email": "ada@example.com", "role": "admin",
            "scope_type": "team", "scope_id": team_id
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = created["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("rolter_invite_"));
    assert!(created["accept_url"].as_str().unwrap().ends_with(&token));
    // the token is handed over once and only its digest is kept
    let stored: String = sqlx::query_scalar("select token_hash from invitations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(stored, token, "the raw token must not be stored");
    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(
        !listed.to_string().contains("token_hash"),
        "the digest must not be serialized back out: {listed}"
    );

    // the accept screen can render without a session, and learns nothing about
    // the org beyond what it must show
    let preview: Value = client
        .get(format!("{base}/api/v1/invitations/accept/{token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["org_name"], "InviteOrg");
    assert_eq!(preview["email"], "ada@example.com");
    assert_eq!(preview["role"], "admin");
    assert_eq!(preview["has_account"], false, "nobody holds this email yet");
    // the scope the link grants is named, so an invitee can tell a team admin
    // from an org admin
    assert_eq!(preview["scope_type"], "team");
    assert_eq!(preview["scope_name"], "Platform");

    // a made-up token is refused, and so is a short password
    let unknown = client
        .get(format!(
            "{base}/api/v1/invitations/accept/rolter_invite_nope"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 401);
    let too_short = client
        .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
        .json(&json!({"password": "short"}))
        .send()
        .await
        .unwrap();
    assert_eq!(too_short.status(), 400);

    // accepting creates the account with the invitee's own password and hands
    // back a live session
    let accepted: Value = client
        .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
        .json(&json!({"password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["user"]["email"], "ada@example.com");
    let session = accepted["token"].as_str().unwrap().to_string();

    // the granted membership is real and tagged manual, so a later sso login
    // cannot reconcile it away
    let rows: Vec<(String, String)> =
        sqlx::query_as("select role, source from memberships where user_id = $1")
            .bind(uuid::Uuid::parse_str(accepted["user"]["id"].as_str().unwrap()).unwrap())
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows, vec![("admin".to_string(), "manual".to_string())]);
    let project = client
        .post(format!("{base}/api/v1/teams/{team_id}/projects"))
        .bearer_auth(&session)
        .json(&json!({"name": "FromInvite"}))
        .send()
        .await
        .unwrap();
    assert_eq!(project.status(), 200);

    // the password the invitee chose is the one that works
    let login = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "ada@example.com", "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);

    // the link is single-use: the second accept is refused, and no second
    // membership appears
    let replay = client
        .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 401);
    let memberships: i64 = sqlx::query_scalar("select count(*) from memberships")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(memberships, 1);

    // a revoked invitation stops working immediately
    let second: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth(admin_token())
        .json(&json!({"email": "grace@example.com", "role": "viewer"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second_token = second["token"].as_str().unwrap().to_string();
    let second_id = second["invitation"]["id"].as_str().unwrap().to_string();
    let revoked = client
        .delete(format!("{base}/api/v1/invitations/{second_id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 200);
    let after_revoke = client
        .post(format!(
            "{base}/api/v1/invitations/accept/{second_token}/accept"
        ))
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(after_revoke.status(), 401);

    // an expired invitation is refused as firmly as a wrong one
    let third: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth(admin_token())
        .json(&json!({"email": "hopper@example.com", "role": "member"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let third_token = third["token"].as_str().unwrap().to_string();
    sqlx::query("update invitations set expires_at = now() - interval '1 hour' where email = $1")
        .bind("hopper@example.com")
        .execute(&pool)
        .await
        .unwrap();
    let expired = client
        .post(format!(
            "{base}/api/v1/invitations/accept/{third_token}/accept"
        ))
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), 401);

    let actions: Vec<String> =
        sqlx::query_scalar("select action from audit_log where action like 'invitation.%'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(actions.iter().any(|a| a == "invitation.create"));
    assert!(actions.iter().any(|a| a == "invitation.accept"));
    assert!(actions.iter().any(|a| a == "invitation.revoke"));
}

/// An invitation names its sender whichever signed-in account created it
/// (#2813). A superadmin session is not a `Principal::User`, and the operator
/// `rolter-seed` creates holds no membership anywhere, so the sender used to be
/// dropped for exactly the account that sets a deployment up. The admin token is
/// a credential rather than a person and stays unrecorded.
#[tokio::test]
async fn an_invitation_records_its_sender_for_a_superadmin_with_no_membership() {
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
    let operator = seed_user(&pool, "operator@example.com", true).await;
    let manager = seed_user(&pool, "manager@example.com", false).await;
    seed_membership(&pool, manager, Some(acme), None, None, "admin").await;
    let operator_session = seed_session(&pool, operator, "invite_sender_operator").await;
    let manager_session = seed_session(&pool, manager, "invite_sender_manager").await;

    // the premise of the bug: the superadmin is on no org's membership list
    let operator_grants: i64 =
        sqlx::query_scalar("select count(*) from memberships where user_id = $1")
            .bind(operator)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(operator_grants, 0);

    let invite = |bearer: String, email: &'static str| {
        let client = client.clone();
        let url = format!("{base}/api/v1/orgs/{acme}/invitations");
        async move {
            let response = client
                .post(url)
                .bearer_auth(bearer)
                .json(&json!({"email": email, "role": "member"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let created: Value = response.json().await.unwrap();
            created["invitation"].clone()
        }
    };
    let by_operator = invite(operator_session.clone(), "by-operator@example.com").await;
    let by_manager = invite(manager_session, "by-manager@example.com").await;
    let by_token = invite(admin_token().to_string(), "by-token@example.com").await;

    // what the create answered with is what is stored
    assert_eq!(by_operator["invited_by"], operator.to_string());
    assert_eq!(by_manager["invited_by"], manager.to_string());
    assert_eq!(by_token["invited_by"], Value::Null);

    // and the pending list the dashboard reads carries it
    let listed: Vec<Value> = client
        .get(format!("{base}/api/v1/orgs/{acme}/invitations"))
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sender_of = |email: &str| {
        listed
            .iter()
            .find(|row| row["email"] == email)
            .map(|row| row["invited_by"].clone())
    };
    assert_eq!(
        sender_of("by-operator@example.com"),
        Some(json!(operator.to_string()))
    );
    assert_eq!(
        sender_of("by-manager@example.com"),
        Some(json!(manager.to_string()))
    );
    assert_eq!(sender_of("by-token@example.com"), Some(Value::Null));

    // the column holds it, not only the response
    let stored: Vec<(String, Option<uuid::Uuid>)> =
        sqlx::query_as("select email, invited_by from invitations order by email")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        vec![
            ("by-manager@example.com".to_string(), Some(manager)),
            ("by-operator@example.com".to_string(), Some(operator)),
            ("by-token@example.com".to_string(), None),
        ]
    );
}

/// Inviting an address again replaces its pending invitation (#2324): the old
/// link stops working like a revoked one, an expired invitation no longer holds
/// the address, the match ignores case, and parallel creates neither 500 nor
/// leave two live rows.
#[tokio::test]
async fn reinviting_an_address_replaces_its_pending_invitation() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "ReinviteOrg", "slug": "reinvite-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let invite = |email: &'static str| {
        let client = client.clone();
        let url = format!("{base}/api/v1/orgs/{org_id}/invitations");
        async move {
            client
                .post(url)
                .bearer_auth(admin_token())
                .json(&json!({"email": email, "role": "member"}))
                .send()
                .await
                .unwrap()
        }
    };
    let live_count = |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, i64>(
            "select count(*) from invitations where accepted_at is null and revoked_at is null",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    // twice for one address, the second spelled differently
    let first = invite("ada@example.com").await;
    assert_eq!(first.status(), 200);
    let first: Value = first.json().await.unwrap();
    let second = invite("Ada@Example.COM").await;
    assert_eq!(second.status(), 200);
    let second: Value = second.json().await.unwrap();
    let first_token = first["token"].as_str().unwrap();
    let second_token = second["token"].as_str().unwrap();
    let first_id = first["invitation"]["id"].as_str().unwrap().to_string();

    let old_preview = client
        .get(format!("{base}/api/v1/invitations/accept/{first_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(old_preview.status(), 401);
    let old_accept = client
        .post(format!(
            "{base}/api/v1/invitations/accept/{first_token}/accept"
        ))
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(old_accept.status(), 401);
    let new_preview = client
        .get(format!("{base}/api/v1/invitations/accept/{second_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(new_preview.status(), 200);
    assert_eq!(live_count(pool.clone()).await, 1);

    // the audit entry of the replacement names what it replaced
    let detail: Value = sqlx::query_scalar(
        "select detail from audit_log where action = 'invitation.create' \
         and target_id = $1::uuid",
    )
    .bind(second["invitation"]["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(detail["replaced"], json!(first_id));
    let detail: Value = sqlx::query_scalar(
        "select detail from audit_log where action = 'invitation.create' \
         and target_id = $1::uuid",
    )
    .bind(first_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(detail["replaced"].is_null());

    // an invitation that expired unaccepted does not hold the address
    sqlx::query("update invitations set expires_at = now() - interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();
    let third = invite("ada@example.com").await;
    assert_eq!(third.status(), 200);
    let revoked: i64 =
        sqlx::query_scalar("select count(*) from invitations where revoked_at is not null")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(revoked, 2);
    assert_eq!(live_count(pool.clone()).await, 1);
    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pending = listed
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["revoked_at"].is_null() && i["accepted_at"].is_null())
        .count();
    assert_eq!(pending, 1);

    // parallel creates for one address: no failure, one live row
    let (a, b, c) = tokio::join!(
        invite("grace@example.com"),
        invite("GRACE@example.com"),
        invite("grace@example.com")
    );
    for response in [a, b, c] {
        assert_eq!(response.status(), 200);
    }
    let grace_live: i64 = sqlx::query_scalar(
        "select count(*) from invitations where lower(email) = 'grace@example.com' \
         and accepted_at is null and revoked_at is null",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(grace_live, 1);
    let grace_all: i64 = sqlx::query_scalar(
        "select count(*) from invitations where lower(email) = 'grace@example.com'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(grace_all, 3);
}

/// An invitation token proves someone was sent the link, not who holds it: the
/// inviter gets the same token back. So an org admin who invites an existing
/// account's email -- a superadmin's here, one with a password and one that
/// signs in through sso only -- and accepts it themselves gets no session for
/// that account, and cannot give it a password either (#1935). The role is
/// still granted, and the owner signs in with their own credentials as before.
#[tokio::test]
async fn accepting_an_invitation_never_signs_in_to_an_existing_account() {
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
        .json(&json!({"name": "Tenant", "slug": "tenant"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = uuid::Uuid::parse_str(org["id"].as_str().unwrap()).unwrap();
    let org_admin = seed_user(&pool, "tenant-admin@example.com", false).await;
    seed_membership(&pool, org_admin, Some(org_id), None, None, "admin").await;
    let org_admin_token = seed_session(&pool, org_admin, "tenantadmin").await;

    let root_password = random_password();
    // what the inviter would type into the accept form; random so no scanner
    // mistakes a fixture for a leaked credential
    let inviter_password = random_password();
    let root = seed_local_user(&pool, "root@example.com", &root_password).await;
    let sso_root = seed_user(&pool, "sso-root@example.com", true).await;

    for (email, target) in [
        ("root@example.com", root),
        ("sso-root@example.com", sso_root),
    ] {
        let created = client
            .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
            .bearer_auth(&org_admin_token)
            .json(&json!({"email": email, "role": "viewer"}))
            .send()
            .await
            .unwrap();
        assert_eq!(created.status(), 200, "an org admin may invite anyone");
        let created: Value = created.json().await.unwrap();
        let token = created["token"].as_str().unwrap().to_string();

        let preview: Value = client
            .get(format!("{base}/api/v1/invitations/accept/{token}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(preview["has_account"], true, "{email}: {preview}");

        let accepted = client
            .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
            .json(&json!({"password": inviter_password}))
            .send()
            .await
            .unwrap();
        assert_eq!(accepted.status(), 200);
        let accepted: Value = accepted.json().await.unwrap();
        assert_eq!(accepted["sign_in_required"], true, "{email}: {accepted}");
        assert_eq!(accepted["reason"], "existing_account");
        assert!(
            accepted["token"].is_null(),
            "{email}: the invite link minted a session for an existing account: {accepted}"
        );
        let sessions: i64 = sqlx::query_scalar("select count(*) from sessions where user_id = $1")
            .bind(target)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0, "{email}: no session for the account");

        // the role is granted all the same, tagged like any invited role
        let roles: Vec<(String, String)> = sqlx::query_as(
            "select role, source from memberships where user_id = $1 and org_id = $2",
        )
        .bind(target)
        .bind(org_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(roles, vec![("viewer".to_string(), "manual".to_string())]);
    }

    // the password the inviter sent opens neither account
    for email in ["root@example.com", "sso-root@example.com"] {
        let response = client
            .post(format!("{base}/api/v1/auth/login"))
            .json(&json!({"email": email, "password": inviter_password}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            401,
            "{email} took the inviter's password"
        );
    }
    let sso_hash: Option<String> =
        sqlx::query_scalar("select password_hash from users where id = $1")
            .bind(sso_root)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        sso_hash.is_none(),
        "an sso-only account gained a password from an invite link"
    );
    // and the owner's own password still works
    let signed_in = login_as(&client, &base, "root@example.com", &root_password).await;
    assert!(signed_in["token"].is_string(), "{signed_in}");

    let detail: Value = sqlx::query_scalar(
        "select detail from audit_log where action = 'invitation.accept' and actor_user_id = $1",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(detail["account_created"], false, "{detail}");
    assert_eq!(detail["signed_in"], false, "{detail}");
}

/// A new account created by an invitation into an org whose `required_all`
/// policy is in force gets no session from the link: it signs in with the
/// password it just chose, and that sign-in is the enrolment the policy asks
/// for (#1935, #1852). A missing password is refused without spending the link.
#[tokio::test]
async fn an_invitation_into_a_required_org_sends_the_new_account_to_enrol() {
    skip_without_db!();
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
    let (_, org_id) = seed_bound_member(
        &pool,
        "required-admin@example.com",
        &random_password(),
        "required_all",
        "null",
    )
    .await;

    let created: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth(admin_token())
        .json(&json!({"email": "newcomer@example.com", "role": "member"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = created["token"].as_str().unwrap().to_string();

    let no_password = client
        .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(no_password.status(), 400, "a new account needs a password");

    let password = random_password();
    let accepted = client
        .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
        .json(&json!({"password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 200, "the refusal left the link live");
    let accepted: Value = accepted.json().await.unwrap();
    assert_eq!(accepted["sign_in_required"], true, "{accepted}");
    assert_eq!(accepted["reason"], "second_factor");
    assert!(accepted["token"].is_null(), "{accepted}");
    let sessions: i64 = sqlx::query_scalar(
        "select count(*) from sessions s join users u on u.id = s.user_id where u.email = $1",
    )
    .bind("newcomer@example.com")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(sessions, 0, "no factor-less session under required_all");

    // the sign-in is where the policy is met
    let challenge = login_as(&client, &base, "newcomer@example.com", &password).await;
    assert_eq!(challenge["mfa_enrolment_required"], true, "{challenge}");
    assert!(challenge["token"].is_null(), "{challenge}");
}

// ---------------------------------------------------------------------------
// last active superadmin (#2344)
// ---------------------------------------------------------------------------

/// The three account writes that can take the deployment's last superadmin
/// away, as `(name, request)` pairs against `target_id`.
fn last_superadmin_calls(
    client: &reqwest::Client,
    base: &str,
    target_id: uuid::Uuid,
    bearer: &str,
) -> Vec<(&'static str, reqwest::RequestBuilder)> {
    let url = format!("{base}/api/v1/users/{target_id}");
    vec![
        (
            "demote",
            client
                .put(&url)
                .bearer_auth(bearer)
                .json(&json!({"is_superadmin": false})),
        ),
        (
            "deactivate",
            client
                .put(&url)
                .bearer_auth(bearer)
                .json(&json!({"deactivated": true})),
        ),
        ("delete", client.delete(&url).bearer_auth(bearer)),
    ]
}

#[tokio::test]
async fn the_last_active_superadmin_cannot_be_demoted_deactivated_or_deleted() {
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

    let only = seed_user(&pool, "only@example.com", true).await;
    let session = seed_session(&pool, only, "last_superadmin").await;
    // a deactivated superadmin and a plain user do not count as the remainder
    let gone = seed_user(&pool, "gone@example.com", true).await;
    sqlx::query("update users set deactivated_at = now() where id = $1")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();
    seed_user(&pool, "plain@example.com", false).await;

    // the account itself and the admin token get the same refusal
    for bearer in [session.as_str(), admin_token()] {
        for (name, request) in last_superadmin_calls(&client, &base, only, bearer) {
            let res = request.send().await.unwrap();
            assert_eq!(res.status(), 409, "{name} as {bearer}");
            let body: Value = res.json().await.unwrap();
            assert_eq!(body["error"]["code"], "last_superadmin", "{name}");
            assert!(
                body["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("last active superadmin"),
                "{body}"
            );
            assert_eq!(user_row(&pool, only).await, Some((true, false)), "{name}");
        }
    }
    let live: i64 = sqlx::query_scalar("select count(*) from sessions where user_id = $1")
        .bind(only)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(live, 1, "a refused deactivation must keep the sessions");

    // edits that leave the account an active superadmin still go through
    let res = client
        .put(format!("{base}/api/v1/users/{only}"))
        .bearer_auth(admin_token())
        .json(&json!({"email": "renamed@example.com", "is_superadmin": true, "deactivated": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn a_second_active_superadmin_lets_each_call_through() {
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

    seed_user(&pool, "keeper@example.com", true).await;
    for (idx, expected) in [
        (0, Some((false, false))),
        (1, Some((true, true))),
        (2, None),
    ] {
        let target_id = seed_user(&pool, &format!("target{idx}@example.com"), true).await;
        let mut calls = last_superadmin_calls(&client, &base, target_id, admin_token());
        let (name, request) = calls.remove(idx);
        let res = request.send().await.unwrap();
        assert!(res.status().is_success(), "{name}: {}", res.status());
        assert_eq!(user_row(&pool, target_id).await, expected, "{name}");
    }
}

#[tokio::test]
async fn a_deactivated_superadmin_does_not_count_as_the_remaining_one() {
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

    let active = seed_user(&pool, "active@example.com", true).await;
    let dormant = seed_user(&pool, "dormant@example.com", true).await;
    sqlx::query("update users set deactivated_at = now() where id = $1")
        .bind(dormant)
        .execute(&pool)
        .await
        .unwrap();

    for (name, request) in last_superadmin_calls(&client, &base, active, admin_token()) {
        assert_eq!(request.send().await.unwrap().status(), 409, "{name}");
    }
    // the dormant one is not the last active superadmin, so it can go, and
    // bringing it back makes the other one expendable
    let res = client
        .put(format!("{base}/api/v1/users/{dormant}"))
        .bearer_auth(admin_token())
        .json(&json!({"deactivated": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = client
        .put(format!("{base}/api/v1/users/{active}"))
        .bearer_auth(admin_token())
        .json(&json!({"is_superadmin": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn concurrent_demotions_cannot_both_remove_a_superadmin() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    // building the app runs the migrations
    let _app = rolter_control::test_app_with_admin_token(pool.clone(), None)
        .await
        .unwrap();
    let first = seed_user(&pool, "first@example.com", true).await;
    let second = seed_user(&pool, "second@example.com", true).await;
    let repo = |id| {
        let pool = pool.clone();
        async move {
            rolter_store::postgres::repo::UserRepo(&pool)
                .update_account(id, None, None, Some(false), None)
                .await
                .unwrap()
        }
    };
    let (a, b) = tokio::join!(repo(first), repo(second));
    let refused = [&a, &b]
        .iter()
        .filter(|r| matches!(r, rolter_store::postgres::repo::LockoutGuard::WouldLockOut))
        .count();
    assert_eq!(refused, 1, "exactly one demotion must be refused");
    let left: i64 = sqlx::query_scalar("select count(*) from users where is_superadmin")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 1);
}

#[tokio::test]
async fn a_membership_role_changes_in_one_request_and_is_audited() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let (base, org_id) = people_fixture(&pool).await;
    let client = reqwest::Client::new();

    let admin = seed_user(&pool, "admin@example.com", false).await;
    let other_admin = seed_user(&pool, "other-admin@example.com", false).await;
    let member = seed_user(&pool, "member@example.com", false).await;
    let admin_token_ = seed_session(&pool, admin, "role_admin").await;
    let member_token = seed_session(&pool, member, "role_member").await;
    seed_membership(&pool, admin, Some(org_id), None, None, "admin").await;
    seed_membership(&pool, member, Some(org_id), None, None, "member").await;
    let grant = grant_id(&pool, member, org_id).await;
    let put = |token: &str, id: uuid::Uuid, body: Value| {
        client
            .put(format!("{base}/api/v1/memberships/{id}"))
            .bearer_auth(token)
            .json(&body)
            .send()
    };

    // a member cannot change anyone's role
    let res = put(&member_token, grant, json!({"role": "admin"}))
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    // a bad role and an unknown field are refused without touching the row
    let res = put(&admin_token_, grant, json!({"role": "owner"}))
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let res = put(
        &admin_token_,
        grant,
        json!({"role": "viewer", "org_id": org_id}),
    )
    .await
    .unwrap();
    assert!(res.status().is_client_error());
    let res = put(
        &admin_token_,
        uuid::Uuid::new_v4(),
        json!({"role": "viewer"}),
    )
    .await
    .unwrap();
    assert_eq!(res.status(), 404);

    let res = put(&admin_token_, grant, json!({"role": "viewer"}))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["role"], "viewer", "{body}");
    assert_eq!(body["id"], grant.to_string(), "the grant keeps its id");
    let detail: Value = sqlx::query_scalar(
        "select detail from audit_log where action = 'membership.update' and target_id = $1",
    )
    .bind(grant)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(detail["old_role"], "member", "{detail}");
    assert_eq!(detail["new_role"], "viewer", "{detail}");

    // the same role again is a no-op with no second audit row
    let res = put(&admin_token_, grant, json!({"role": "viewer"}))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let rows: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action = 'membership.update' and target_id = $1",
    )
    .bind(grant)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 1);

    // the org's only admin cannot be demoted, a second admin lifts that, and
    // the superadmin may always repair the org
    let admin_grant = grant_id(&pool, admin, org_id).await;
    let res = put(&admin_token_, admin_grant, json!({"role": "member"}))
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "last_org_admin", "{body}");
    seed_membership(&pool, other_admin, Some(org_id), None, None, "admin").await;
    let res = put(&admin_token_, admin_grant, json!({"role": "member"}))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let other_grant = grant_id(&pool, other_admin, org_id).await;
    let res = put(admin_token(), other_grant, json!({"role": "member"}))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn an_invitation_names_its_sender_to_a_caller_who_cannot_list_them() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let (base, org_id) = people_fixture(&pool).await;
    let client = reqwest::Client::new();

    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let org_admin = seed_user(&pool, "boss@example.com", false).await;
    let team_admin = seed_user(&pool, "lead@example.com", false).await;
    seed_membership(&pool, org_admin, Some(org_id), None, None, "admin").await;
    seed_membership(&pool, team_admin, None, Some(team), None, "admin").await;
    let org_token = seed_session(&pool, org_admin, "inv_org").await;
    let team_token = seed_session(&pool, team_admin, "inv_team").await;

    for (token, email, scope) in [
        (&org_token, "by-boss@example.com", json!({})),
        (
            &team_token,
            "by-lead@example.com",
            json!({"scope_type": "team", "scope_id": team}),
        ),
    ] {
        let mut body = json!({"email": email, "role": "member"});
        body.as_object_mut()
            .unwrap()
            .extend(scope.as_object().unwrap().clone());
        let res = client
            .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }
    // sent with the admin token: names nobody
    let res = client
        .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth(admin_token())
        .json(&json!({"email": "by-token@example.com", "role": "member"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let list = |token: String| {
        let client = client.clone();
        let url = format!("{base}/api/v1/orgs/{org_id}/invitations");
        async move {
            let rows: Value = client
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            rows
        }
    };
    let sender = |rows: &Value, email: &str| -> Value {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|row| row["email"] == email)
            .unwrap_or_else(|| panic!("{email} missing from {rows}"))["invited_by_email"]
            .clone()
    };

    let all = list(org_token).await;
    assert_eq!(sender(&all, "by-boss@example.com"), "boss@example.com");
    assert_eq!(sender(&all, "by-lead@example.com"), "lead@example.com");
    assert_eq!(sender(&all, "by-token@example.com"), Value::Null);

    // the team admin sees only their team's invitation, and still sees who sent it
    let own = list(team_token).await;
    assert_eq!(own.as_array().unwrap().len(), 1, "{own}");
    assert_eq!(sender(&own, "by-lead@example.com"), "lead@example.com");

    // a sender who was deleted reads as null rather than a stale address
    sqlx::query("delete from users where id = $1")
        .bind(org_admin)
        .execute(&pool)
        .await
        .unwrap();
    let after = list(admin_token().to_string()).await;
    assert_eq!(sender(&after, "by-boss@example.com"), Value::Null);
}

#[tokio::test]
async fn the_last_org_admin_grant_cannot_be_revoked_except_by_a_superadmin() {
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
    let org_id: uuid::Uuid = org["id"].as_str().unwrap().parse().unwrap();

    let first = seed_user(&pool, "first@example.com", false).await;
    let second = seed_user(&pool, "second@example.com", false).await;
    let first_token = seed_session(&pool, first, "org_admin_first").await;
    seed_membership(&pool, first, Some(org_id), None, None, "admin").await;
    let first_grant = grant_id(&pool, first, org_id).await;

    // the only admin: refused with the stable code, grant kept
    let res = client
        .delete(format!("{base}/api/v1/memberships/{first_grant}"))
        .bearer_auth(&first_token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "last_org_admin", "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("last admin"));
    assert_eq!(grant_id(&pool, first, org_id).await, first_grant);

    // a deactivated admin is not a remainder
    seed_membership(&pool, second, Some(org_id), None, None, "admin").await;
    sqlx::query("update users set deactivated_at = now() where id = $1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    let res = client
        .delete(format!("{base}/api/v1/memberships/{first_grant}"))
        .bearer_auth(&first_token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);

    // a lower role is not an admin either
    sqlx::query("update users set deactivated_at = null where id = $1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    let second_grant = grant_id(&pool, second, org_id).await;
    sqlx::query("update memberships set role = 'member' where id = $1")
        .bind(second_grant)
        .execute(&pool)
        .await
        .unwrap();
    let res = client
        .delete(format!("{base}/api/v1/memberships/{first_grant}"))
        .bearer_auth(&first_token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);

    // a second active admin exists: the first can go, then the second is last
    sqlx::query("update memberships set role = 'admin' where id = $1")
        .bind(second_grant)
        .execute(&pool)
        .await
        .unwrap();
    let res = client
        .delete(format!("{base}/api/v1/memberships/{first_grant}"))
        .bearer_auth(&first_token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let second_token = seed_session(&pool, second, "org_admin_second").await;
    let res = client
        .delete(format!("{base}/api/v1/memberships/{second_grant}"))
        .bearer_auth(&second_token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);

    // the superadmin (here the admin token) may still repair the org
    let res = client
        .delete(format!("{base}/api/v1/memberships/{second_grant}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn concurrent_revokes_of_two_org_admins_leave_one() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    // applies the migrations
    let _app = rolter_control::test_app(pool.clone()).await.unwrap();
    let org_id: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Race', 'race') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let a = seed_user(&pool, "a@example.com", false).await;
    let b = seed_user(&pool, "b@example.com", false).await;
    seed_membership(&pool, a, Some(org_id), None, None, "admin").await;
    seed_membership(&pool, b, Some(org_id), None, None, "admin").await;
    let ga = grant_id(&pool, a, org_id).await;
    let gb = grant_id(&pool, b, org_id).await;

    let repo_a = rolter_store::postgres::repo::MembershipRepo(&pool);
    let repo_b = rolter_store::postgres::repo::MembershipRepo(&pool);
    let (ra, rb) = tokio::join!(
        repo_a.delete_guarded(ga, true),
        repo_b.delete_guarded(gb, true)
    );
    let refused = [ra.unwrap(), rb.unwrap()]
        .iter()
        .filter(|r| matches!(r, rolter_store::postgres::repo::LockoutGuard::WouldLockOut))
        .count();
    assert_eq!(refused, 1);
    let left: i64 =
        sqlx::query_scalar("select count(*) from memberships where org_id = $1 and role = 'admin'")
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(left, 1);
}

// ---------------------------------------------------------------------------
// every account in the deployment, for a superadmin (#2871)
// ---------------------------------------------------------------------------

/// `GET /api/v1/users` lists accounts of every org, accounts with no role
/// anywhere and deactivated ones, e-mail ordered and without a password hash.
/// A superadmin session and the admin token read it; an org admin, a plain
/// member and a caller with no credential do not.
#[tokio::test]
async fn the_deployment_user_list_is_every_account_and_superadmin_only() {
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

    let mut orgs = Vec::new();
    for slug in ["north-users", "south-users"] {
        let org: Value = client
            .post(format!("{base}/api/v1/orgs"))
            .bearer_auth(admin_token())
            .json(&json!({"name": slug, "slug": slug}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        orgs.push(org["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap());
    }
    let (north, south) = (orgs[0], orgs[1]);

    // inserted out of e-mail order, so the ordering is the query's
    let zed = seed_user(&pool, "zed@example.com", false).await;
    seed_membership(&pool, zed, Some(south), None, None, "member").await;
    let manager = seed_user(&pool, "manager@example.com", false).await;
    seed_membership(&pool, manager, Some(north), None, None, "admin").await;
    let operator = seed_user(&pool, "operator@example.com", true).await;
    let gone = seed_user(&pool, "gone@example.com", false).await;
    sqlx::query("update users set deactivated_at = now() where id = $1")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();
    let member = seed_user(&pool, "member@example.com", false).await;
    seed_membership(&pool, member, Some(north), None, None, "member").await;

    let operator_bearer = seed_session(&pool, operator, "deployment_users_operator").await;
    let manager_bearer = seed_session(&pool, manager, "deployment_users_manager").await;
    let member_bearer = seed_session(&pool, member, "deployment_users_member").await;

    let emails_of = |page: &Value| -> Vec<String> {
        page.as_array()
            .expect("a list")
            .iter()
            .map(|user| user["email"].as_str().unwrap().to_string())
            .collect()
    };
    let everyone = vec![
        "gone@example.com",
        "manager@example.com",
        "member@example.com",
        "operator@example.com",
        "zed@example.com",
    ];

    // the premise of the issue: an org's list cannot name another org's people
    let north_list: Value = client
        .get(format!("{base}/api/v1/orgs/{north}/users"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!emails_of(&north_list).contains(&"zed@example.com".to_string()));

    for (who, bearer) in [
        ("the admin token", admin_token()),
        ("a superadmin session", operator_bearer.as_str()),
    ] {
        let response = client
            .get(format!("{base}/api/v1/users"))
            .bearer_auth(bearer)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{who} reads the list");
        let page: Value = response.json().await.unwrap();
        assert_eq!(emails_of(&page), everyone, "{who} gets every account");
        let operator_row = page
            .as_array()
            .unwrap()
            .iter()
            .find(|user| user["email"] == "operator@example.com")
            .unwrap();
        assert_eq!(operator_row["is_superadmin"], true);
        assert_eq!(operator_row["id"], operator.to_string());
        let gone_row = page
            .as_array()
            .unwrap()
            .iter()
            .find(|user| user["email"] == "gone@example.com")
            .unwrap();
        assert!(
            gone_row["deactivated_at"].is_string(),
            "a deactivated account keeps its row, so an old audit entry still names it"
        );
        assert!(
            !page.to_string().contains("password_hash"),
            "no hash leaves the control plane"
        );
    }

    for (who, bearer) in [
        ("an org admin", manager_bearer.as_str()),
        ("a plain member", member_bearer.as_str()),
    ] {
        let response = client
            .get(format!("{base}/api/v1/users"))
            .bearer_auth(bearer)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{who} is refused");
    }
    let anonymous = client
        .get(format!("{base}/api/v1/users"))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401, "no credential, no list");
}
