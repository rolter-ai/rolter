//! Users: account lifecycle, deployment-wide listing and password rules.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::{Membership, User};
use rolter_store::postgres::repo::{
    AuditLogRepo, LockoutGuard, MembershipRepo, MfaRepo, SessionRepo, UserRepo, VirtualKeyRepo,
};

use super::{
    invalid_field, log_audit, name_taken, pool, project_teams, require_reach, row_chain, ApiError,
    ApiResult, Query, SafeJson,
};
use crate::rbac::{authorize, authorize_superadmin, Principal, ScopeChain, ScopeFilter};
use crate::rbac_matrix::{cap, superadmin_cap, DeploymentRole};
use crate::ControlState;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/users",
            get(list_users).post(create_user),
        )
        .route("/api/v1/users", get(list_deployment_users))
        .route("/api/v1/users/{id}", put(update_user).delete(delete_user))
        .route(
            "/api/v1/users/{id}/deployment-role",
            put(set_deployment_role),
        )
}

// --- users + memberships (ROL-223) ---
//
// account lifecycle vs. role assignment are deliberately split by authority:
// inviting a user into an org and granting roles within it are org-admin
// operations (scoped, safe for a tenant admin), while editing or deleting the
// underlying global account — and toggling the cross-org `is_superadmin`
// bit — is superadmin-only. in open mode (no admin token) every caller is a
// superadmin, so both paths pass through unchanged for local/single-tenant use.

const MIN_PASSWORD_LEN: usize = 8;

/// what a password shorter than [`MIN_PASSWORD_LEN`] is refused with, or `None`
/// when it is long enough. One rule for every path that sets a password, so the
/// admin reset, an invitation and the account's own change cannot disagree
pub(crate) fn password_length_problem(password: &str) -> Option<String> {
    (password.len() < MIN_PASSWORD_LEN)
        .then(|| format!("password must be at least {MIN_PASSWORD_LEN} characters"))
}

/// hash a plaintext password with argon2id for at-rest storage; the repo layer
/// only ever sees the digest
pub(crate) fn hash_password(password: &str) -> ApiResult<String> {
    use argon2::password_hash::PasswordHasher;
    use argon2::Argon2;

    if let Some(problem) = password_length_problem(password) {
        return Err(ApiError::Core(Error::Config(problem)));
    }
    // hash_password generates its own 16-byte salt from getrandom
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| {
            tracing::error!(error = %e, "failed to hash password");
            ApiError::Curated("failed to hash password".into())
        })
}

/// minimal email sanity check: trimmed, non-empty, with a single `@` separating
/// non-empty local and domain parts. deliberately permissive — full RFC 5322 is
/// not worth the surface here, we only guard against obvious junk
pub(crate) fn validate_email(email: &str) -> ApiResult<String> {
    let email = email.trim();
    let ok = match email.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty() && !domain.is_empty() && !domain.contains('@') && domain.contains('.')
        }
        None => false,
    };
    if !ok {
        return Err(invalid_field("email", "email must be a valid address"));
    }
    Ok(email.to_string())
}

pub(crate) fn validate_role(role: &str) -> ApiResult<()> {
    if !matches!(role, "admin" | "member" | "viewer") {
        return Err(invalid_field(
            "role",
            "role must be one of admin, member, viewer",
        ));
    }
    Ok(())
}

/// what `GET /api/v1/orgs/{org_id}/users` can be narrowed or widened by
#[derive(Deserialize, Default)]
struct ListUsersQuery {
    /// also list the accounts that hold no membership anywhere, such as the
    /// superadmin `rolter-seed --admin-email` creates. Only a superadmin can
    /// edit such an account, so only a superadmin is shown it: for anyone else
    /// the flag changes nothing, rather than failing a list the caller may
    /// read (#2804)
    #[serde(default)]
    include_unassigned: bool,
}

async fn list_users(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Query(query): Query<ListUsersQuery>,
) -> ApiResult<Json<Vec<User>>> {
    let repo = UserRepo(pool(&state));
    let users = if query.include_unassigned && matches!(principal, Principal::Superadmin { .. }) {
        repo.list_in_org_with_unassigned(org_id).await?
    } else {
        repo.list_in_org(org_id).await?
    };
    let filter = ScopeFilter::load(&state, &principal, cap!("user", Read)).await?;
    if filter.allows(ScopeChain::org(org_id)) {
        return Ok(Json(users));
    }
    // below the org: the people holding a role inside the teams and projects
    // the caller reads, so a team admin sees their own team (#1850)
    require_reach(&state, &filter, org_id).await?;
    let teams_of = project_teams(&state, org_id).await?;
    let visible: HashSet<Uuid> = MembershipRepo(pool(&state))
        .list_in_org(org_id)
        .await?
        .into_iter()
        .filter(|m| filter.allows(row_chain(org_id, m.team_id, m.project_id, &teams_of)))
        .map(|m| m.user_id)
        .collect();
    Ok(Json(
        users
            .into_iter()
            .filter(|user| visible.contains(&user.id))
            .collect(),
    ))
}

/// Every account in the deployment, email-ordered: the people of every org and
/// the ones that hold no membership at all. The deployment-wide audit log names
/// an actor from any org, and `GET /api/v1/orgs/{org_id}/users` answers for one
/// org only (#2871).
///
/// Superadmin only. Listing an org's people is an org admin's job, but this
/// lists the people of orgs the caller has no role in, so there is no scope to
/// check a role against.
async fn list_deployment_users(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<User>>> {
    authorize_superadmin(&principal, superadmin_cap!("deployment_user", Read))?;
    Ok(Json(UserRepo(pool(&state)).list_all().await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateUser {
    email: String,
    /// optional initial password; omit for an sso-only shell account that
    /// cannot log in locally until a password is set
    password: Option<String>,
    /// role granted at this org for the new account; defaults to `member`
    #[serde(default)]
    role: Option<String>,
}

#[derive(Serialize)]
struct CreatedUser {
    user: User,
    membership: Membership,
}

/// invite/create an account and grant it a role in this org atomically. an
/// org admin can onboard a user without superadmin: the new account carries no
/// superadmin bit and starts scoped to this org only.
async fn create_user(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateUser>,
) -> ApiResult<Json<CreatedUser>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("user", Create),
    )
    .await?;
    let email = validate_email(&body.email)?;
    let role = body.role.as_deref().unwrap_or("member");
    validate_role(role)?;
    let password_hash = match body.password.as_deref() {
        Some(p) => Some(hash_password(p)?),
        None => None,
    };

    let pool = pool(&state);
    if UserRepo(pool).find_by_email(&email).await?.is_some() {
        return Err(name_taken(format!(
            "a user with email '{email}' already exists"
        )));
    }

    let user = UserRepo(pool)
        .create(&email, password_hash.as_deref(), false)
        .await?;
    let membership = MembershipRepo(pool)
        .create(user.id, Some(org_id), None, None, role)
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "user.create",
        "user",
        user.id,
        serde_json::json!({"email": user.email, "role": role}),
    )
    .await;
    Ok(Json(CreatedUser { user, membership }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateUser {
    email: Option<String>,
    password: Option<String>,
    is_superadmin: Option<bool>,
    /// set to `true` to deactivate (block login, revoke sessions) or `false` to
    /// re-enable the account
    deactivated: Option<bool>,
}

/// stable code of the 409 for a write that would leave the deployment with no
/// active superadmin account (#2344)
pub(crate) const LAST_SUPERADMIN: &str = "last_superadmin";

/// the refusal for a write that would leave no active superadmin. the admin
/// token is not an account, so it never counts as the one that remains
pub(crate) fn last_superadmin() -> ApiError {
    ApiError::CodedConflict {
        code: LAST_SUPERADMIN,
        message: "this is the last active superadmin; make another account superadmin first"
            .to_string(),
    }
}

/// stable code of the 409 for a write that would leave an org with no active
/// org-scoped admin (#2311)
pub(crate) const LAST_ORG_ADMIN: &str = "last_org_admin";

/// the refusal for revoking an org's last admin grant. a superadmin is exempt
pub(crate) fn last_org_admin() -> ApiError {
    ApiError::CodedConflict {
        code: LAST_ORG_ADMIN,
        message: "this is the organization's last admin grant; grant admin to another person \
                  first, or ask a superadmin"
            .to_string(),
    }
}

/// Revoke a grant an identity provider produced and no longer implies (SSO
/// group reconciliation on login, SCIM group sync), unless it is the org's last
/// active admin grant (#2558).
///
/// The revoke goes through `MembershipRepo::delete_guarded`, so the count and
/// the delete share one transaction under the org's admin lock. A refused
/// revoke is not an error: the sign-in or the sync still succeeds, the grant
/// stays, and a `membership.last_admin_kept` audit row plus a warning name the
/// org. Both callers derive the wanted set from the IdP every time, so the
/// next login or sync revokes the grant once the org has another admin.
/// Returns whether the grant was kept.
///
/// `protect` is false only when a superadmin's operator action triggered the
/// reconciliation (deleting a SCIM group mapping), matching
/// `delete_membership`, where a superadmin may revoke an org's last admin
/// grant. Login and IdP sync always pass true (#2673).
pub(crate) async fn revoke_idp_grant(
    state: &ControlState,
    stale: &Membership,
    protect: bool,
) -> ApiResult<bool> {
    if MembershipRepo(pool(state))
        .delete_guarded(stale.id, protect)
        .await?
        != LockoutGuard::WouldLockOut
    {
        return Ok(false);
    }
    tracing::warn!(
        org_id = ?stale.org_id,
        user_id = %stale.user_id,
        membership_id = %stale.id,
        source = %stale.source,
        "kept an identity-provider admin grant: revoking it would leave the org without an admin"
    );
    if let Err(err) = AuditLogRepo(pool(state))
        .create(
            stale.org_id,
            None,
            "membership.last_admin_kept",
            Some("membership"),
            Some(stale.id),
            Some(serde_json::json!({
                "user_id": stale.user_id,
                "role": stale.role,
                "source": stale.source,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write audit log entry");
    }
    Ok(true)
}

/// edit a global account. superadmin-only because it reaches across every org
/// the user belongs to and can grant the cross-org superadmin bit.
async fn update_user(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    SafeJson(body): SafeJson<UpdateUser>,
) -> ApiResult<Json<User>> {
    authorize_superadmin(&principal, superadmin_cap!("user", Update))?;
    let pool = pool(&state);

    let email = match body.email.as_deref() {
        Some(e) => Some(validate_email(e)?),
        None => None,
    };
    let password_hash = match body.password.as_deref() {
        Some(p) => Some(hash_password(p)?),
        None => None,
    };

    // reject an email change that collides with a different account
    if let Some(ref new_email) = email {
        if let Some(existing) = UserRepo(pool).find_by_email(new_email).await? {
            if existing.id != id {
                return Err(name_taken(format!(
                    "a user with email '{new_email}' already exists"
                )));
            }
        }
    }

    // one transaction under the superadmin lock, so the guard and the write
    // cannot be split by a concurrent demotion (#2344)
    let user = match UserRepo(pool)
        .update_account(
            id,
            email.as_deref(),
            password_hash.as_deref(),
            body.is_superadmin,
            body.deactivated,
        )
        .await?
    {
        LockoutGuard::Done(user) => user,
        LockoutGuard::WouldLockOut => return Err(last_superadmin()),
    };

    let mut sessions_revoked = 0;
    let mut detail = serde_json::json!({"email": user.email, "deactivated": body.deactivated});
    if let Some(deactivated) = body.deactivated {
        if deactivated {
            // cut existing access immediately, not just at token expiry
            sessions_revoked = SessionRepo(pool).delete_for_user_except(id, None).await?;
        }
        // the gateways stop, or resume, serving the keys the account minted
        // for itself (#1841), and the audit row says how many
        detail["personal_keys"] = VirtualKeyRepo(pool).count_personal(id).await?.into();
    }
    if password_hash.is_some() && body.deactivated != Some(true) {
        // a reset usually means the old password is in someone else's hands,
        // and whoever signed in with it would otherwise keep a session for its
        // full lifetime (#1936). The session sending this request survives, so
        // a superadmin resetting their own password is not signed out of the
        // page they did it from; it matches no other account's rows
        let own = crate::auth::bearer_token(&headers)
            .map(|token| rolter_auth::hash_key(&crate::auth::session_pepper(), token));
        sessions_revoked = SessionRepo(pool)
            .delete_for_user_except(id, own.as_deref())
            .await?;
    }
    if password_hash.is_some() || body.deactivated == Some(true) {
        // a sign-in halfway through its second-factor step was started with
        // the old password, and an enrolment token among them could still arm
        // a factor for whoever held that password (#1852)
        MfaRepo(pool).delete_challenges_for_user(id).await?;
    }

    // global account edit spans orgs, so it's logged unscoped
    detail["password_changed"] = password_hash.is_some().into();
    detail["sessions_revoked"] = sessions_revoked.into();
    log_audit(&state, &principal, None, "user.update", "user", id, detail).await;
    Ok(Json(user))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetDeploymentRole {
    /// `operator`, `security_auditor`, or `null` to clear the account's role
    role: Option<String>,
}

/// Give an account a deployment role, or take it away (#1834). Superadmin only,
/// and deliberately not a field of `PUT /api/v1/users/{id}`: neither deployment
/// role can reach this, so an operator cannot widen their own reach or an
/// auditor's. The role changes what the guard answers on the account's next
/// request; a held session needs no revocation because nothing is cached.
async fn set_deployment_role(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetDeploymentRole>,
) -> ApiResult<Json<User>> {
    authorize_superadmin(&principal, superadmin_cap!("deployment_role", Update))?;
    let role = match body.role.as_deref() {
        Some(key) => Some(DeploymentRole::parse(key).ok_or_else(|| {
            invalid_field(
                "role",
                "role must be one of operator, security_auditor, null",
            )
        })?),
        None => None,
    };
    let before = UserRepo(pool(&state)).get(id).await?;
    let user = UserRepo(pool(&state))
        .set_deployment_role(id, role.map(DeploymentRole::key))
        .await?;
    // a deployment-wide grant is logged unscoped, like the account edit
    let detail = serde_json::json!({
        "email": user.email,
        "from": before.deployment_role,
        "to": user.deployment_role,
    });
    log_audit(
        &state,
        &principal,
        None,
        "user.deployment_role",
        "user",
        id,
        detail,
    )
    .await;
    Ok(Json(user))
}

async fn delete_user(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize_superadmin(&principal, superadmin_cap!("user", Delete))?;
    // memberships and sessions cascade on the users fk (on delete cascade), and
    // a trigger disables the keys the account minted for itself rather than
    // letting the fk orphan them into shared keys (#1841)
    let personal_keys = VirtualKeyRepo(pool(&state)).count_personal(id).await?;
    // read before the memberships go with the account: the deletion is written
    // once per org it belonged to, since afterwards nothing ties an org-less
    // row back to those orgs' audit logs (#1854)
    let orgs: HashSet<Uuid> = MembershipRepo(pool(&state))
        .ancestors_for_user(id)
        .await?
        .into_iter()
        .filter_map(|(_membership, org, _team)| org)
        .collect();
    if UserRepo(pool(&state)).delete(id).await? == LockoutGuard::WouldLockOut {
        return Err(last_superadmin());
    }
    let detail = serde_json::json!({"personal_keys": personal_keys});
    // an account that belonged to no org still gets its row, with none
    let scopes: Vec<Option<Uuid>> = if orgs.is_empty() {
        vec![None]
    } else {
        orgs.into_iter().map(Some).collect()
    };
    for org in scopes {
        log_audit(
            &state,
            &principal,
            org,
            "user.delete",
            "user",
            id,
            detail.clone(),
        )
        .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod user_tests {
    use super::*;
    use axum::response::IntoResponse;

    /// a 400 refusal, coded per field or not
    fn is_config_err<T: std::fmt::Debug>(res: ApiResult<T>) -> bool {
        matches!(
            res,
            Err(ApiError::Core(Error::Config(_)) | ApiError::InvalidField { .. })
        )
    }

    #[test]
    fn email_accepts_plain_addresses_and_trims() {
        assert_eq!(validate_email("  a@b.com ").unwrap(), "a@b.com");
        assert_eq!(
            validate_email("user.name@sub.example.io").unwrap(),
            "user.name@sub.example.io"
        );
    }

    #[test]
    fn email_rejects_junk() {
        assert!(is_config_err(validate_email("")));
        assert!(is_config_err(validate_email("nope")));
        assert!(is_config_err(validate_email("@b.com")));
        assert!(is_config_err(validate_email("a@")));
        assert!(is_config_err(validate_email("a@b"))); // no dot in domain
        assert!(is_config_err(validate_email("a@@b.com")));
    }

    #[test]
    fn role_accepts_known_and_rejects_others() {
        assert!(validate_role("admin").is_ok());
        assert!(validate_role("member").is_ok());
        assert!(validate_role("viewer").is_ok());
        assert!(is_config_err(validate_role("superadmin")));
        assert!(is_config_err(validate_role("")));
    }

    #[test]
    fn password_hash_enforces_min_length_and_verifies() {
        use argon2::password_hash::PasswordVerifier;
        use argon2::Argon2;
        use argon2::PasswordHash;

        // built at run time: a password written out in a test is one code
        // scanning reports as hard-coded (#2849)
        assert!(is_config_err(hash_password(&"s".repeat(5))));
        let password = format!("pw-{}", uuid::Uuid::new_v4());
        let hash = hash_password(&password).unwrap();
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok());
    }

    #[test]
    fn the_length_rule_is_one_shared_function() {
        // the account's own change reports the same problem the admin reset
        // does, from the same place, so the two cannot drift
        // built from lengths, so no test carries a password as a literal
        let problem = password_length_problem(&"x".repeat(5)).expect("5 characters is too short");
        assert!(
            problem.contains("at least 8"),
            "the refusal states the minimum"
        );
        assert!(password_length_problem(&"y".repeat(MIN_PASSWORD_LEN)).is_none());
        assert!(password_length_problem(&"x".repeat(0)).is_some());
    }

    #[tokio::test]
    async fn a_throttled_password_endpoint_says_so_with_a_stable_code() {
        let response =
            ApiError::TooManyAttempts(std::time::Duration::from_millis(1500)).into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        // rounded up, so a sub-second remainder never invites an instant retry
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("2")
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "too_many_attempts");
    }
}
