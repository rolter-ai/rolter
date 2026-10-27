//! SCIM provisioning of users and groups.

use crate::support::*;
use serde_json::{json, Value};

/// SCIM 2.0 Users provisioning (#540): an IdP token scopes every call to one
/// org, create/deactivate/reconcile are idempotent, no local password is ever
/// involved, and a revoked token stops working immediately.
#[tokio::test]
async fn scim_users_are_provisioned_scoped_and_idempotent() {
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
        .json(&json!({"name": "ScimOrg", "slug": "scim-org"}))
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
        .json(&json!({"name": "OtherScimOrg", "slug": "other-scim-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap().to_string();

    // minting a token returns the secret exactly once
    let minted: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "okta"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = minted["secret"].as_str().unwrap().to_string();
    assert!(secret.starts_with("rolter_scim_"));
    let token_id = minted["id"].as_str().unwrap().to_string();

    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed_text = listed.to_string();
    assert!(
        !listed_text.contains(&secret) && !listed_text.contains("token_hash"),
        "token material leaked into the listing: {listed_text}"
    );

    // an unauthenticated SCIM call is a SCIM-shaped 401, not an axum rejection
    let unauth = client
        .get(format!("{base}/scim/v2/Users"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);
    let body: Value = unauth.json().await.unwrap();
    assert_eq!(
        body["schemas"][0],
        "urn:ietf:params:scim:api:messages:2.0:Error"
    );

    // provision a user
    let created = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "ada@example.com",
            "externalId": "idp-1",
            "displayName": "Ada Lovelace",
            "emails": [{"value": "ada@example.com", "primary": true}],
            "password": random_password()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let created: Value = created.json().await.unwrap();
    assert_eq!(created["userName"], "ada@example.com");
    assert_eq!(created["active"], true);
    assert_eq!(created["externalId"], "idp-1");
    let scim_id = created["id"].as_str().unwrap().to_string();
    // the supplied password is ignored, never stored
    let hash: Option<String> = sqlx::query_scalar("select password_hash from users where id = $1")
        .bind(scim_id.parse::<uuid::Uuid>().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(hash.is_none(), "provisioning must not store a password");

    // a replayed create is a SCIM uniqueness conflict, not a second account
    let replay = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .json(&json!({"userName": "ada@example.com", "emails": [{"value": "ada@example.com"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 409);
    let replay: Value = replay.json().await.unwrap();
    assert_eq!(replay["scimType"], "uniqueness");

    // the filter IdPs reconcile with
    let found: Value = client
        .get(format!(
            "{base}/scim/v2/Users?filter=userName%20eq%20%22ada@example.com%22"
        ))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(found["totalResults"], 1);
    assert_eq!(found["Resources"][0]["id"], scim_id);

    // an unsupported filter is refused rather than answered with everything
    let bad_filter = client
        .get(format!(
            "{base}/scim/v2/Users?filter=displayName%20eq%20%22Ada%22"
        ))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(bad_filter.status(), 400);

    // a live session exists, then the IdP deactivates the leaver
    let user_uuid: uuid::Uuid = scim_id.parse().unwrap();
    let session_token = seed_session(&pool, user_uuid, "scimuser").await;
    let me_before = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&session_token)
        .send()
        .await
        .unwrap();
    assert_eq!(me_before.status(), 200);

    let patched: Value = client
        .patch(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{"op": "replace", "path": "active", "value": false}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(patched["active"], false);
    // deactivation logs the leaver out rather than only blocking future logins
    let me_after = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&session_token)
        .send()
        .await
        .unwrap();
    assert_eq!(me_after.status(), 401);

    // re-enabling is the same call with the other value
    let reenabled: Value = client
        .patch(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .json(&json!({"Operations": [{"op": "replace", "value": {"active": true}}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reenabled["active"], true);

    // another org's token cannot see or touch this resource
    let other_minted: Value = client
        .post(format!("{base}/api/v1/orgs/{other_id}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "entra"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_secret = other_minted["secret"].as_str().unwrap().to_string();
    let cross = client
        .get(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&other_secret)
        .send()
        .await
        .unwrap();
    assert_eq!(cross.status(), 404, "a token must not reach another tenant");
    let cross_list: Value = client
        .get(format!("{base}/scim/v2/Users"))
        .bearer_auth(&other_secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cross_list["totalResults"], 0);

    // delete deprovisions: the mapping goes, the account survives deactivated
    let deleted = client
        .delete(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    let gone = client
        .get(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 404);
    let still_there: Option<uuid::Uuid> = sqlx::query_scalar("select id from users where id = $1")
        .bind(user_uuid)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert!(
        still_there.is_some(),
        "the account row must outlive the IdP"
    );

    // revoking the token stops provisioning immediately
    let revoked = client
        .delete(format!("{base}/api/v1/scim-tokens/{token_id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 200);
    let after_revoke = client
        .get(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(after_revoke.status(), 401);

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'scim.user.deprovision' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("scim.user.deprovision"));
}

/// SCIM 2.0 Groups (#540): the group surface an IdP drives, and the org-scoped
/// group→team mapping that turns group membership into roles. Covers SCIM
/// semantics and error envelopes, cross-tenant scoping, membership
/// reconciliation converging on a replayed sync, and deprovisioning taking the
/// group-granted roles with it.
#[tokio::test]
async fn scim_groups_map_to_teams_and_reconcile_idempotently() {
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
        .json(&json!({"name": "GroupOrg", "slug": "group-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();

    let other: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "OtherGroupOrg", "slug": "other-group-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap().to_string();

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
    let team_uuid: uuid::Uuid = team_id.parse().unwrap();

    let minted: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "okta"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = minted["secret"].as_str().unwrap().to_string();

    // an unauthenticated Groups call is a SCIM-shaped 401, like Users
    let unauth = client
        .get(format!("{base}/scim/v2/Groups"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);
    let body: Value = unauth.json().await.unwrap();
    assert_eq!(
        body["schemas"][0],
        "urn:ietf:params:scim:api:messages:2.0:Error"
    );

    // provision two users through the Users surface first: a group may only
    // contain accounts this token already created
    let mut user_ids = Vec::new();
    for name in ["ada@example.com", "grace@example.com"] {
        let created: Value = client
            .post(format!("{base}/scim/v2/Users"))
            .bearer_auth(&secret)
            .json(&json!({"userName": name, "emails": [{"value": name, "primary": true}]}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        user_ids.push(created["id"].as_str().unwrap().to_string());
    }
    let ada = user_ids[0].clone();
    let grace = user_ids[1].clone();
    let ada_uuid: uuid::Uuid = ada.parse().unwrap();
    let grace_uuid: uuid::Uuid = grace.parse().unwrap();

    // an operator maps the group name to a role on the team, before the IdP has
    // ever mentioned the group — that must not error
    let mapping: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-group-mappings"))
        .bearer_auth(admin_token())
        .json(&json!({"group_name": "platform", "role": "member", "team_id": team_id}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mapping_id = mapping["id"].as_str().unwrap().to_string();
    assert_eq!(mapping["group_name"], "platform");

    // a mapping may not grant into another tenant's team
    let cross_team = client
        .post(format!("{base}/api/v1/orgs/{other_id}/scim-group-mappings"))
        .bearer_auth(admin_token())
        .json(&json!({"group_name": "platform", "role": "admin", "team_id": team_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross_team.status(),
        400,
        "a mapping must not reach another org's team"
    );

    // only the three built-in roles are mappable
    let bad_role = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-group-mappings"))
        .bearer_auth(admin_token())
        .json(&json!({"group_name": "platform", "role": "superadmin"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_role.status(), 400);

    // the IdP creates the group with one member
    let created = client
        .post(format!("{base}/scim/v2/Groups"))
        .bearer_auth(&secret)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
            "displayName": "platform",
            "externalId": "idp-group-1",
            "members": [{"value": ada}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let created: Value = created.json().await.unwrap();
    let group_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["displayName"], "platform");
    assert_eq!(created["externalId"], "idp-group-1");
    assert_eq!(created["members"][0]["value"], ada);
    assert_eq!(created["members"][0]["display"], "ada@example.com");

    // the mapping took effect: ada holds member on the team, sourced 'scim'
    let team_roles = |user: uuid::Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "select role from memberships where user_id = $1 and team_id = $2 \
                 and source = 'scim'",
            )
            .bind(user)
            .bind(team_uuid)
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(team_roles(ada_uuid).await, vec!["member".to_string()]);
    assert!(team_roles(grace_uuid).await.is_empty());

    // a replayed create is a uniqueness conflict, not a second group
    let replay = client
        .post(format!("{base}/scim/v2/Groups"))
        .bearer_auth(&secret)
        .json(&json!({"displayName": "platform"}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 409);
    let replay: Value = replay.json().await.unwrap();
    assert_eq!(replay["scimType"], "uniqueness");

    // the filter IdPs reconcile with, and the ones that are refused explicitly
    let found: Value = client
        .get(format!(
            "{base}/scim/v2/Groups?filter=displayName%20eq%20%22platform%22"
        ))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(found["totalResults"], 1);
    assert_eq!(found["Resources"][0]["id"], group_id);
    let bad_filter = client
        .get(format!(
            "{base}/scim/v2/Groups?filter=externalId%20eq%20%22idp-group-1%22"
        ))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(bad_filter.status(), 400);

    // PATCH add: the shape Okta and Entra send for a joiner
    let patched: Value = client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{"op": "add", "path": "members", "value": [{"value": grace}]}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(patched["members"].as_array().unwrap().len(), 2);
    assert_eq!(team_roles(grace_uuid).await, vec!["member".to_string()]);

    // replaying that exact operation must converge, not accumulate
    let replayed: Value = client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "Operations": [{"op": "add", "path": "members", "value": [{"value": grace}]}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replayed["members"].as_array().unwrap().len(), 2);
    assert_eq!(
        team_roles(grace_uuid).await,
        vec!["member".to_string()],
        "a replayed sync must not grant the same role twice"
    );

    // PATCH remove with the filtered path form, which is how a leaver arrives
    let removed: Value = client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "Operations": [{"op": "remove", "path": format!("members[value eq \"{grace}\"]")}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(removed["members"].as_array().unwrap().len(), 1);
    assert!(
        team_roles(grace_uuid).await.is_empty(),
        "leaving the group must revoke what it granted"
    );

    // a member the token never provisioned is refused
    let stranger = seed_user(&pool, "stranger@example.com", false).await;
    let outsider = client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "Operations": [{"op": "add", "path": "members", "value": [{"value": stranger}]}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(outsider.status(), 400);
    let outsider: Value = outsider.json().await.unwrap();
    assert_eq!(outsider["scimType"], "invalidValue");

    // an operation nothing supports is refused rather than answered with a
    // success the IdP would read as "the change landed"
    let unsupported = client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({"Operations": [{"op": "replace", "path": "urn:unknown", "value": "x"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(unsupported.status(), 400);

    // PUT replaces the whole member list
    let replaced: Value = client
        .put(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({"displayName": "platform", "members": [{"value": grace}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replaced["members"].as_array().unwrap().len(), 1);
    assert_eq!(replaced["members"][0]["value"], grace);
    assert!(team_roles(ada_uuid).await.is_empty());
    assert_eq!(team_roles(grace_uuid).await, vec!["member".to_string()]);

    // another org's token cannot see or touch this group
    let other_minted: Value = client
        .post(format!("{base}/api/v1/orgs/{other_id}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "entra"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_secret = other_minted["secret"].as_str().unwrap().to_string();
    let cross = client
        .get(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&other_secret)
        .send()
        .await
        .unwrap();
    assert_eq!(cross.status(), 404, "a token must not reach another tenant");
    let cross_list: Value = client
        .get(format!("{base}/scim/v2/Groups"))
        .bearer_auth(&other_secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cross_list["totalResults"], 0);
    let cross_delete = client
        .delete(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&other_secret)
        .send()
        .await
        .unwrap();
    assert_eq!(cross_delete.status(), 404);

    // a manual grant an operator made survives a sync
    let manual: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/memberships"))
        .bearer_auth(admin_token())
        .json(&json!({
            "user_id": ada,
            "scope_type": "team",
            "scope_id": team_id,
            "role": "admin"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let manual_id = manual["id"].as_str().unwrap().to_string();
    let resync: Value = client
        .put(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({"displayName": "platform", "members": [{"value": ada}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resync["members"].as_array().unwrap().len(), 1);
    let survived: Option<String> = sqlx::query_scalar("select role from memberships where id = $1")
        .bind(manual_id.parse::<uuid::Uuid>().unwrap())
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert_eq!(
        survived.as_deref(),
        Some("admin"),
        "reconciliation must not touch a grant an operator made"
    );

    // deprovisioning the user takes the group-granted role with it
    let deprovisioned = client
        .delete(format!("{base}/scim/v2/Users/{ada}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(deprovisioned.status(), 204);
    assert!(
        team_roles(ada_uuid).await.is_empty(),
        "a deprovisioned account must not keep a group-granted role"
    );

    // dropping the mapping revokes what it granted, without a sync
    client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "Operations": [{"op": "add", "path": "members", "value": [{"value": grace}]}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(team_roles(grace_uuid).await, vec!["member".to_string()]);
    let dropped = client
        .delete(format!("{base}/api/v1/scim-group-mappings/{mapping_id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(dropped.status(), 204);
    assert!(team_roles(grace_uuid).await.is_empty());

    // and deleting the group is idempotent from the IdP's point of view
    let deleted = client
        .delete(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    let gone = client
        .delete(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 404);

    let actions: Vec<String> = sqlx::query_scalar(
        "select action from audit_log where org_id = $1 and action like 'scim.group%'",
    )
    .bind(org_uuid)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(actions.iter().any(|a| a == "scim.group.create"));
    assert!(actions.iter().any(|a| a == "scim.group.update"));
    assert!(actions.iter().any(|a| a == "scim.group.delete"));
}

/// A SCIM token names the account that minted it in `created_by`, and in the
/// `scim_token.create` audit row, for a superadmin session as for an org admin
/// (#2844). Minted with the admin token it names nobody. The response carries the
/// plaintext token once, so nothing here prints it.
#[tokio::test]
async fn a_scim_token_records_the_superadmin_session_that_minted_it() {
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

    let callers = [
        (
            "operator",
            actors.operator_bearer.clone(),
            json!(actors.operator.to_string()),
        ),
        (
            "manager",
            actors.manager_bearer.clone(),
            json!(actors.manager.to_string()),
        ),
        ("admin-token", admin_token().to_string(), Value::Null),
    ];
    let mut minted = Vec::new();
    for (who, bearer, expected) in &callers {
        let response = client
            .post(format!("{base}/api/v1/orgs/{acme}/scim-tokens"))
            .bearer_auth(bearer)
            .json(&json!({ "name": format!("idp-{who}") }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "mint as {who}");
        let created: Value = response.json().await.unwrap();
        // what the response answered with is what is stored
        assert_eq!(&created["created_by"], expected, "response for {who}");
        let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();
        let stored: Option<uuid::Uuid> =
            sqlx::query_scalar("select created_by from scim_tokens where id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            stored.map_or(Value::Null, |id| json!(id.to_string())),
            *expected,
            "column for {who}"
        );
        minted.push((who, id, expected));
    }

    // the audit row for the mint names the same account
    let page: Value = client
        .get(format!(
            "{base}/api/v1/orgs/{acme}/audit-log?action=scim_token.create"
        ))
        .bearer_auth(&actors.operator_bearer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for (who, id, expected) in minted {
        let row = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["target_id"] == id.to_string())
            .unwrap_or_else(|| panic!("no scim_token.create row for {who}"));
        assert_eq!(&row["actor_user_id"], expected, "audit row for {who}");
    }
}

/// A SCIM-provisioned account's `displayName` is authoritative (#1823): it is
/// copied onto the account, `/auth/me` flags it managed, and the self-service
/// route refuses to change it while still letting the account edit its bio.
#[tokio::test]
async fn a_scim_managed_display_name_is_read_only_but_the_bio_is_not() {
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
        .json(&json!({"name": "ProfileScimOrg", "slug": "profile-scim-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let minted: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "okta"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = minted["secret"].as_str().unwrap().to_string();

    let created: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .json(&json!({
            "userName": "hopper@example.com",
            "displayName": "Grace Hopper",
            "emails": [{"value": "hopper@example.com", "primary": true}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim_id = created["id"].as_str().unwrap().to_string();
    let user_id: uuid::Uuid = scim_id.parse().unwrap();
    let token = seed_session(&pool, user_id, "profilescimuser").await;

    let me = || async {
        client
            .get(format!("{base}/api/v1/auth/me"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let patch = |body: Value| {
        let client = client.clone();
        let base = base.clone();
        let token = token.clone();
        async move {
            client
                .patch(format!("{base}/api/v1/me/profile"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };

    let seen = me().await;
    assert_eq!(seen["user"]["display_name"], "Grace Hopper");
    assert_eq!(seen["display_name_managed"], true);

    let refused = patch(json!({"display_name": "Someone Else"})).await;
    assert_eq!(refused.status(), 409);
    let cleared = patch(json!({"display_name": null})).await;
    assert_eq!(cleared.status(), 409);
    assert_eq!(me().await["user"]["display_name"], "Grace Hopper");

    // the bio is the user's, and resending the managed name unchanged is fine
    let ok = patch(json!({"display_name": "Grace Hopper", "bio": "Nanoseconds."})).await;
    assert_eq!(ok.status(), 200);
    let body: Value = ok.json().await.unwrap();
    assert_eq!(body["bio"], "Nanoseconds.");
    assert_eq!(body["display_name_managed"], true);

    // a later SCIM replace moves the name
    let replaced = client
        .put(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "userName": "hopper@example.com",
            "displayName": "Rear Admiral Hopper",
            "emails": [{"value": "hopper@example.com", "primary": true}],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(replaced.status(), 200);
    assert_eq!(me().await["user"]["display_name"], "Rear Admiral Hopper");
    assert_eq!(me().await["user"]["bio"], "Nanoseconds.");
}

/// #2705: a SCIM replace with `active:false` that the lockout guard refuses
/// must not have renamed the account or rewritten its identity first.
#[tokio::test]
async fn a_refused_scim_replace_leaves_the_identity_unchanged() {
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
    let org = org["id"].as_str().unwrap().to_string();
    let token: Value = client
        .post(format!("{base}/api/v1/orgs/{org}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "idp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = token["secret"].as_str().unwrap().to_string();
    let provisioned: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&token)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "boss@acme.test",
            "externalId": "ext-old",
            "displayName": "Old Name",
            "emails": [{"value": "boss@acme.test", "primary": true}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim_id = provisioned["id"].as_str().unwrap().to_string();
    let boss: uuid::Uuid = scim_id.parse().unwrap();
    sqlx::query("update users set is_superadmin = true where id = $1")
        .bind(boss)
        .execute(&pool)
        .await
        .unwrap();

    let res = client
        .put(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&token)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "renamed@acme.test",
            "externalId": "ext-new",
            "displayName": "New Name",
            "active": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let body: Value = res.json().await.unwrap();
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("last active superadmin"),
        "{body}"
    );
    let (user_name, external_id, display_name): (String, Option<String>, String) = sqlx::query_as(
        "select user_name, external_id, display_name from scim_identities where user_id = $1",
    )
    .bind(boss)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(user_name, "boss@acme.test");
    assert_eq!(external_id.as_deref(), Some("ext-old"));
    assert_eq!(display_name, "Old Name");
    assert_eq!(user_row(&pool, boss).await, Some((true, false)));
}

#[tokio::test]
async fn scim_cannot_deprovision_the_last_active_superadmin() {
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
    let org = org["id"].as_str().unwrap().to_string();
    let token: Value = client
        .post(format!("{base}/api/v1/orgs/{org}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "idp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = token["secret"].as_str().unwrap().to_string();
    let provisioned: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&token)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "boss@acme.test",
            "emails": [{"value": "boss@acme.test", "primary": true}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim_id = provisioned["id"].as_str().unwrap().to_string();
    let boss: uuid::Uuid = scim_id.parse().unwrap();
    sqlx::query("update users set is_superadmin = true where id = $1")
        .bind(boss)
        .execute(&pool)
        .await
        .unwrap();

    let res = client
        .delete(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    assert_eq!(user_row(&pool, boss).await, Some((true, false)));
    let res = client
        .patch(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&token)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{"op": "replace", "path": "active", "value": false}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    assert_eq!(user_row(&pool, boss).await, Some((true, false)));

    // with another active superadmin the same call deprovisions
    seed_user(&pool, "second@example.com", true).await;
    let res = client
        .delete(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(user_row(&pool, boss).await, Some((true, true)));
}

/// #2672: a SCIM create with `active:false` that adopts the last superadmin is
/// refused before the identity is linked, so the IdP's retry gets the same 409
/// rather than "userName already exists".
#[tokio::test]
async fn a_refused_scim_create_leaves_no_identity_behind() {
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
    let org = org["id"].as_str().unwrap().to_string();
    let token: Value = client
        .post(format!("{base}/api/v1/orgs/{org}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "idp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = token["secret"].as_str().unwrap().to_string();
    let boss = seed_user(&pool, "boss@acme.test", true).await;

    for attempt in ["first", "retry"] {
        let res = client
            .post(format!("{base}/scim/v2/Users"))
            .bearer_auth(&token)
            .json(&json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
                "userName": "boss@acme.test",
                "emails": [{"value": "boss@acme.test", "primary": true}],
                "active": false
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 409, "{attempt}");
        let body: Value = res.json().await.unwrap();
        assert!(
            body["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("last active superadmin"),
            "{attempt}: {body}"
        );
        let linked: i64 =
            sqlx::query_scalar("select count(*) from scim_identities where user_id = $1")
                .bind(boss)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(linked, 0, "{attempt}: a refused create must not link");
        assert_eq!(
            user_row(&pool, boss).await,
            Some((true, false)),
            "{attempt}"
        );
    }
}

/// #2558: SCIM deprovisioning and `active: false` answer a SCIM 409 instead of
/// deactivating the account that holds an org's last active admin grant.
#[tokio::test]
async fn scim_cannot_deactivate_an_orgs_last_admin() {
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
    let token: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "idp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = token["secret"].as_str().unwrap().to_string();
    let provisioned: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&token)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "boss@acme.test",
            "emails": [{"value": "boss@acme.test", "primary": true}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim_id = provisioned["id"].as_str().unwrap().to_string();
    let boss: uuid::Uuid = scim_id.parse().unwrap();
    seed_membership(&pool, boss, Some(org_id), None, None, "admin").await;

    let res = client
        .delete(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["schemas"][0], "urn:ietf:params:scim:api:messages:2.0:Error",
        "{body}"
    );
    assert!(body["detail"]
        .as_str()
        .unwrap()
        .contains("last active admin"));
    assert_eq!(user_row(&pool, boss).await, Some((false, false)));
    assert_eq!(org_admin_rows(&pool, boss, org_id).await, vec!["manual"]);

    let deactivate = json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
        "Operations": [{"op": "replace", "path": "active", "value": false}]
    });
    let res = client
        .patch(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&token)
        .json(&deactivate)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    assert_eq!(user_row(&pool, boss).await, Some((false, false)));

    // a deactivated admin is not a remainder
    let second = seed_user(&pool, "second@example.com", false).await;
    seed_membership(&pool, second, Some(org_id), None, None, "admin").await;
    sqlx::query("update users set deactivated_at = now() where id = $1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    let res = client
        .patch(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&token)
        .json(&deactivate)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    assert_eq!(user_row(&pool, boss).await, Some((false, false)));

    // with another active admin the same call deactivates
    sqlx::query("update users set deactivated_at = null where id = $1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    let res = client
        .patch(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&token)
        .json(&deactivate)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(user_row(&pool, boss).await, Some((false, true)));

    // and now `second` is the last one: deactivating them races nobody, but
    // the store refuses it the same way
    let refused = rolter_store::postgres::repo::UserRepo(&pool)
        .set_deactivated_guarding_org_admins(second, true)
        .await
        .unwrap();
    assert!(matches!(
        refused,
        rolter_store::postgres::repo::DeactivationGuard::LastOrgAdmin(org) if org == org_id
    ));
}

/// #2558: a SCIM group sync that drops the org's last admin from the group
/// keeps the grant and audits it; the sync itself succeeds, and a later sync
/// revokes the grant once the org has another admin.
#[tokio::test]
async fn scim_group_sync_keeps_an_orgs_last_admin_grant() {
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
        .json(&json!({"name": "GroupOrg", "slug": "group-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();
    let minted: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "okta"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = minted["secret"].as_str().unwrap().to_string();
    let created: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .json(&json!({
            "userName": "ada@example.com",
            "emails": [{"value": "ada@example.com", "primary": true}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim_id = created["id"].as_str().unwrap().to_string();
    let ada: uuid::Uuid = scim_id.parse().unwrap();
    let res = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-group-mappings"))
        .bearer_auth(admin_token())
        .json(&json!({"group_name": "owners", "role": "admin"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let create_group = || {
        client
            .post(format!("{base}/scim/v2/Groups"))
            .bearer_auth(&secret)
            .json(&json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
                "displayName": "owners",
                "members": [{"value": scim_id}]
            }))
            .send()
    };
    let group: Value = create_group().await.unwrap().json().await.unwrap();
    assert_eq!(org_admin_rows(&pool, ada, org_uuid).await, vec!["scim"]);

    // the IdP deletes the group: the sync answers success, the grant stays
    let res = client
        .delete(format!(
            "{base}/scim/v2/Groups/{}",
            group["id"].as_str().unwrap()
        ))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(org_admin_rows(&pool, ada, org_uuid).await, vec!["scim"]);
    assert_eq!(last_admin_kept_audits(&pool, org_uuid).await, 1);

    // with another active admin, the same sync revokes it
    let bob = seed_user(&pool, "bob@example.com", false).await;
    seed_membership(&pool, bob, Some(org_uuid), None, None, "admin").await;
    let group: Value = create_group().await.unwrap().json().await.unwrap();
    let res = client
        .delete(format!(
            "{base}/scim/v2/Groups/{}",
            group["id"].as_str().unwrap()
        ))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert!(org_admin_rows(&pool, ada, org_uuid).await.is_empty());
    assert_eq!(last_admin_kept_audits(&pool, org_uuid).await, 1);
}

/// The SCIM resource echoes the `displayName` the IdP sent, while the account
/// stores the sanitised name (#2731). An IdP that saw its own value come back
/// trimmed would treat it as drift and push it again on every sync.
#[tokio::test]
async fn a_scim_resource_echoes_the_raw_display_name_the_account_stores_it_sanitised() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "EchoScimOrg", "slug": "echo-scim-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let minted: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth("admintok")
        .json(&json!({"name": "okta"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = minted["secret"].as_str().unwrap().to_string();

    let raw = "  Ada\u{7} Lovelace  ";
    let created: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .json(&json!({
            "userName": "lovelace@example.com",
            "displayName": raw,
            "emails": [{"value": "lovelace@example.com", "primary": true}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["displayName"], raw);
    let scim_id = created["id"].as_str().unwrap().to_string();

    let fetched: Value = client
        .get(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fetched["displayName"], raw);

    let member_id: uuid::Uuid = scim_id.parse().unwrap();
    let token = seed_session(&pool, member_id, "echoscimuser").await;
    let me: Value = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["user"]["display_name"], "Ada Lovelace");
}

/// Mint a SCIM token for a fresh org and return the org id and the secret.
async fn scim_org_with_token(client: &reqwest::Client, base: &str, slug: &str) -> (String, String) {
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
    let org_id = org["id"].as_str().unwrap().to_string();
    let minted: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "okta"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (org_id, minted["secret"].as_str().unwrap().to_string())
}

/// #2435: a SCIM `PATCH` carrying `displayName` renames the account the way
/// `POST` and `PUT` do, sanitised the same way, and a request that also
/// deactivates is all-or-nothing: a refused deactivation writes no name.
#[tokio::test]
async fn scim_patch_syncs_display_name_and_a_refused_deactivation_writes_nothing() {
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
    let (_org_id, secret) = scim_org_with_token(&client, &base, "patch-name-org").await;

    let created: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .json(&json!({
            "userName": "hopper@example.com",
            "displayName": "Grace Hopper",
            "emails": [{"value": "hopper@example.com", "primary": true}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim_id = created["id"].as_str().unwrap().to_string();
    let stored_name = || async {
        sqlx::query_scalar::<_, Option<String>>("select display_name from users where id = $1")
            .bind(scim_id.parse::<uuid::Uuid>().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let patch = |ops: Value| {
        let client = client.clone();
        let url = format!("{base}/scim/v2/Users/{scim_id}");
        let secret = secret.clone();
        async move {
            client
                .patch(url)
                .bearer_auth(secret)
                .json(&json!({
                    "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                    "Operations": ops,
                }))
                .send()
                .await
                .unwrap()
        }
    };

    // the path form, sanitised like POST and PUT
    let res = patch(json!([
        {"op": "replace", "path": "displayName", "value": "  Rear\u{7} Admiral Hopper "}
    ]))
    .await;
    assert_eq!(res.status(), 200);
    // the SCIM resource echoes what the IdP sent; the account gets the clean one
    assert_eq!(stored_name().await.as_deref(), Some("Rear Admiral Hopper"));

    // the pathless object form
    let res = patch(json!([
        {"op": "replace", "value": {"displayName": "Grace B. Hopper"}}
    ]))
    .await;
    assert_eq!(res.status(), 200);
    assert_eq!(stored_name().await.as_deref(), Some("Grace B. Hopper"));
    let fetched: Value = client
        .get(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fetched["displayName"], "Grace B. Hopper");

    // an unsupported attribute alongside a supported one still errors, and
    // nothing of the request is applied
    let res = patch(json!([
        {"op": "replace", "path": "displayName", "value": "Applied Anyway"},
        {"op": "replace", "path": "nickName", "value": "Amazing"},
    ]))
    .await;
    assert_eq!(res.status(), 400);
    assert_eq!(stored_name().await.as_deref(), Some("Grace B. Hopper"));

    // the only active superadmin cannot be deactivated, so a patch that also
    // renames them must leave the name alone
    sqlx::query("update users set is_superadmin = true where id = $1")
        .bind(scim_id.parse::<uuid::Uuid>().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let res = patch(json!([
        {"op": "replace", "path": "displayName", "value": "Renamed In Passing"},
        {"op": "replace", "path": "active", "value": false},
    ]))
    .await;
    assert_eq!(res.status(), 409);
    assert_eq!(stored_name().await.as_deref(), Some("Grace B. Hopper"));
}

/// Provision `ada` through SCIM into an `owners` group whose mapping is her
/// only admin grant in a fresh org, and return the org, ada and the mapping id
/// (#2673).
async fn scim_mapping_sole_admin(
    client: &reqwest::Client,
    base: &str,
    slug: &str,
    pool: &sqlx::PgPool,
) -> (uuid::Uuid, uuid::Uuid, String) {
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
    let org_id = org["id"].as_str().unwrap().to_string();
    let minted: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "okta"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = minted["secret"].as_str().unwrap().to_string();
    let created: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .json(&json!({
            "userName": format!("ada@{slug}.test"),
            "emails": [{"value": format!("ada@{slug}.test"), "primary": true}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim_id = created["id"].as_str().unwrap().to_string();
    let mapping: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-group-mappings"))
        .bearer_auth(admin_token())
        .json(&json!({"group_name": "owners", "role": "admin"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let res = client
        .post(format!("{base}/scim/v2/Groups"))
        .bearer_auth(&secret)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
            "displayName": "owners",
            "members": [{"value": scim_id}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();
    let ada: uuid::Uuid = scim_id.parse().unwrap();
    assert_eq!(org_admin_rows(pool, ada, org_uuid).await, vec!["scim"]);
    (org_uuid, ada, mapping["id"].as_str().unwrap().to_string())
}

/// #2673: a superadmin deleting a SCIM group mapping may revoke the org's last
/// admin grant, as `DELETE /memberships/{id}` allows; nothing is kept or
/// audited as kept.
#[tokio::test]
async fn superadmin_scim_mapping_delete_revokes_the_last_admin_grant() {
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
    let (org, ada, mapping_id) = scim_mapping_sole_admin(&client, &base, "su-org", &pool).await;

    let res = client
        .delete(format!("{base}/api/v1/scim-group-mappings/{mapping_id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert!(org_admin_rows(&pool, ada, org).await.is_empty());
    assert_eq!(last_admin_kept_audits(&pool, org).await, 0);
}

/// #2673: an org admin deleting the SCIM group mapping that holds the org's
/// last admin grant keeps that grant and audits it, the same as an IdP sync.
#[tokio::test]
async fn org_admin_scim_mapping_delete_keeps_the_last_admin_grant() {
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
    let (org, ada, mapping_id) = scim_mapping_sole_admin(&client, &base, "admin-org", &pool).await;
    // ada is the org's only admin, through the very mapping she deletes
    let session = seed_session(&pool, ada, "mapping_delete_2673").await;

    let res = client
        .delete(format!("{base}/api/v1/scim-group-mappings/{mapping_id}"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(org_admin_rows(&pool, ada, org).await, vec!["scim"]);
    assert_eq!(last_admin_kept_audits(&pool, org).await, 1);
}
