//! Administrator-initiated password resets (#2240).
//!
//! A reset is a one-time link, never a password an administrator picks or
//! reads. The administrator receives the link once and hands it to the member,
//! who chooses their own password at it, so no one else ever knows the
//! credential. This is the invitation model ([`crate::invitations`]) applied to
//! an existing local account: only the peppered digest of the token is stored,
//! and redeeming is single-use even under a race.
//!
//! What issuing a reset does:
//!
//! - ends every live session of the member and any sign-in halfway through its
//!   second-factor step, so whoever holds the old credential is cut off now;
//! - revokes the account's earlier pending reset, so only the newest link works;
//! - leaves the old password valid until the link is used, so a mistaken click
//!   does not lock the member out. Deactivating the account is the lever when
//!   the old password is known to be compromised;
//! - writes a `user.password_reset` audit row naming the administrator, never
//!   the token.
//!
//! What redeeming does: stores the new password, ends every session again and
//! signs **nobody** in. The member then signs in normally, so an armed second
//! factor still applies: a leaked or administrator-held link cannot get past
//! it. The factor itself is never cleared here; `rolter mfa reset` is the
//! host-side break-glass for that.
//!
//! Who may issue one: a caller holding `password_reset:create` (org admin) at
//! the org the member belongs to. Below a superadmin the account must belong to
//! that org alone and must not be a superadmin, since the global account
//! reaches every other org and a link holder could take it over. A superadmin
//! may reset any account but their own. Accounts with no local password
//! (single sign-on, LDAP) and deactivated accounts are refused.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_store::postgres::models::PasswordReset;
use rolter_store::postgres::repo::{
    AuditLogRepo, MembershipRepo, MfaRepo, PasswordResetRepo, SessionRepo, UserRepo,
};

use crate::auth::session_pepper;
use crate::crud::{hash_password, log_audit, pool, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

/// how long a reset link stays usable. A reset is acted on at once, so this is
/// much shorter than an invitation's
const RESET_TTL_HOURS: i64 = 24;

pub fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/users/{id}/password-reset",
            post(issue_reset),
        )
        // unauthenticated by necessity: the member has lost their password. the
        // token in the path is the credential
        .route("/api/v1/password-reset/{token}", get(preview).post(redeem))
}

/// the reset plus the token, which is shown exactly once
#[derive(Debug, Serialize)]
struct CreatedPasswordReset {
    reset: PasswordReset,
    /// hand this to the member; it is not recoverable afterwards
    token: String,
    /// ready-made link, so the caller does not have to know the url shape
    reset_url: String,
    /// how many of the member's sessions the reset ended
    sessions_revoked: u64,
}

async fn issue_reset(
    principal: Principal,
    State(state): State<ControlState>,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<CreatedPasswordReset>> {
    let pool_ref = pool(&state);
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("password_reset", Create),
    )
    .await?;

    if principal.account_id() == Some(id) {
        return Err(invalid(
            "change your own password from the account settings instead",
        ));
    }
    let target = UserRepo(pool_ref).get(id).await?;

    // the org of every grant the account holds; a project grant names its org
    // only through its team
    let orgs: Vec<Option<Uuid>> = MembershipRepo(pool_ref)
        .ancestors_for_user(id)
        .await?
        .into_iter()
        .map(|(_membership, org, _team)| org)
        .collect();
    if !orgs.contains(&Some(org_id)) {
        return Err(ApiError::Core(rolter_core::Error::NotFound(format!(
            "user {id} in this org"
        ))));
    }
    if !matches!(principal, Principal::Superadmin { .. }) {
        // the link lets its holder choose the password of the whole account,
        // which reaches every org it belongs to and, for a superadmin, the
        // deployment. an org admin gets only accounts that are theirs alone
        if target.is_superadmin || orgs.iter().any(|org| *org != Some(org_id)) {
            return Err(ApiError::Forbidden);
        }
    }
    if target.deactivated_at.is_some() {
        return Err(ApiError::Core(rolter_core::Error::Config(
            "the account is deactivated; reactivate it before resetting its password".to_string(),
        )));
    }
    // an account without a local password signs in elsewhere. giving it one
    // would add a way in the org's identity provider does not control
    if target.password_hash.is_none() {
        return Err(invalid(
            "the account has no local password; it signs in through single sign-on",
        ));
    }

    let (token, token_hash) = generate_reset_token();
    let (reset, replaced) = PasswordResetRepo(pool_ref)
        .issue(
            id,
            &token_hash,
            principal.account_id(),
            Utc::now() + Duration::hours(RESET_TTL_HOURS),
        )
        .await?;

    // cut the old credential's access off now; the caller is never the target
    let sessions_revoked = SessionRepo(pool_ref)
        .delete_for_user_except(id, None)
        .await?;
    // a sign-in halfway through its second-factor step was started with the
    // old password (#1852)
    MfaRepo(pool_ref).delete_challenges_for_user(id).await?;

    log_audit(
        &state,
        &principal,
        Some(org_id),
        "user.password_reset",
        "user",
        id,
        serde_json::json!({
            "email": target.email,
            "reset_id": reset.id,
            "replaced": replaced,
            "expires_at": reset.expires_at,
            "sessions_revoked": sessions_revoked,
        }),
    )
    .await;

    Ok(Json(CreatedPasswordReset {
        reset_url: reset_url(crate::sso::public_base_url(&state), &token),
        reset,
        token,
        sessions_revoked,
    }))
}

/// what the reset screen needs to render, and nothing more
#[derive(Debug, Serialize)]
struct ResetPreview {
    email: String,
    expires_at: chrono::DateTime<Utc>,
}

async fn preview(
    State(state): State<ControlState>,
    client: crate::login_throttle::ClientAddr,
    headers: axum::http::HeaderMap,
    Path(token): Path<String>,
) -> ApiResult<Json<ResetPreview>> {
    let reset = live_reset_throttled(&state, client, &headers, &token).await?;
    let user = UserRepo(pool(&state)).get(reset.user_id).await?;
    Ok(Json(ResetPreview {
        email: user.email,
        expires_at: reset.expires_at,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RedeemReset {
    password: String,
}

/// The password is stored; the member signs in through the normal login.
#[derive(Debug, Serialize)]
struct Redeemed {
    email: String,
    sign_in_required: bool,
}

async fn redeem(
    State(state): State<ControlState>,
    client: crate::login_throttle::ClientAddr,
    headers: axum::http::HeaderMap,
    Path(token): Path<String>,
    SafeJson(body): SafeJson<RedeemReset>,
) -> ApiResult<Json<Redeemed>> {
    let reset = live_reset_throttled(&state, client, &headers, &token).await?;
    let pool_ref = pool(&state);

    // settled before the link is spent, so a too-short password is refused
    // without costing the member their link
    let hash = hash_password(&body.password)?;
    let user = UserRepo(pool_ref).get(reset.user_id).await?;

    let Some(sessions_revoked) = PasswordResetRepo(pool_ref)
        .redeem(reset.id, reset.user_id, &hash)
        .await?
    else {
        return Err(ApiError::Unauthenticated);
    };
    // a sign-in begun with the old password, and any enrolment token among
    // them, must not outlive it (#1852). The armed factor is left alone
    MfaRepo(pool_ref)
        .delete_challenges_for_user(user.id)
        .await?;

    if let Err(err) = AuditLogRepo(pool_ref)
        .create(
            None,
            Some(user.id),
            "user.password_reset_complete",
            Some("user"),
            Some(user.id),
            Some(serde_json::json!({
                "reset_id": reset.id,
                "issued_by": reset.created_by,
                "sessions_revoked": sessions_revoked,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write password reset audit entry");
    }

    Ok(Json(Redeemed {
        email: user.email,
        sign_in_required: true,
    }))
}

/// Resolve a token to a live reset. Expired, revoked, spent and wrong tokens
/// are all the same 401.
async fn live_reset(state: &ControlState, token: &str) -> ApiResult<PasswordReset> {
    let hash = rolter_auth::hash_key(&session_pepper(), token);
    PasswordResetRepo(pool(state))
        .find_live_by_hash(&hash)
        .await?
        .ok_or(ApiError::Unauthenticated)
}

/// [`live_reset`] behind the same failed-attempt counters as the password login
/// (#1079), keyed on the submitted token and the client address.
async fn live_reset_throttled(
    state: &ControlState,
    client: crate::login_throttle::ClientAddr,
    headers: &axum::http::HeaderMap,
    token: &str,
) -> ApiResult<PasswordReset> {
    let ip = client.resolve(headers, state.trust_forwarded_for);
    let subjects = state.login_throttle.subjects(token, ip);
    if let Some(locked) = state.login_throttle.check(&subjects).await {
        tracing::warn!(
            scope = locked.scope.as_str(),
            client = ?ip,
            "rejected password reset lookup: too many failed attempts"
        );
        return Err(ApiError::TooManyAttempts(locked.retry_after));
    }
    match live_reset(state, token).await {
        Ok(reset) => {
            state.login_throttle.record_success(&subjects).await;
            Ok(reset)
        }
        Err(err) => {
            let penalty = state.login_throttle.record_failure(&subjects).await;
            if !penalty.delay.is_zero() {
                tokio::time::sleep(penalty.delay).await;
            }
            Err(err)
        }
    }
}

fn generate_reset_token() -> (String, String) {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = format!("rolter_reset_{}", rolter_auth::hex::encode(&bytes));
    let hash = rolter_auth::hash_key(&session_pepper(), &token);
    (token, hash)
}

fn reset_url(base: &str, token: &str) -> String {
    format!("{base}/reset-password/{token}")
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(rolter_core::Error::Config(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_tokens_are_prefixed_random_and_never_their_own_digest() {
        let (a, hash_a) = generate_reset_token();
        let (b, _) = generate_reset_token();
        assert!(a.starts_with("rolter_reset_"));
        assert_ne!(a, b, "two resets must not collide");
        assert_ne!(a, hash_a, "the stored value must be a digest");
        assert_eq!(hash_a, rolter_auth::hash_key(&session_pepper(), &a));
    }

    #[test]
    fn the_reset_url_is_built_from_configuration_not_a_request() {
        assert_eq!(
            reset_url("https://rolter.example", "rolter_reset_abc"),
            "https://rolter.example/reset-password/rolter_reset_abc"
        );
    }
}
