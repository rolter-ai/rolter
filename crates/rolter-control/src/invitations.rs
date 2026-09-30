//! Invitation-based onboarding (#712).
//!
//! The point of an invitation is that **the invitee chooses their own
//! password**. `POST /api/v1/orgs/{org_id}/users` can create an account with a
//! password the admin picked, which is fine for seeding and for service
//! accounts but wrong for onboarding a colleague: it leaves their credential in
//! someone else's hands (and, usually, in someone else's chat history).
//!
//! So an invite is a one-time link. Only the peppered digest of the token is
//! stored — the same treatment sessions and virtual keys get — so a database
//! dump alone yields no usable link. Accepting is single-use even under a race:
//! the `UPDATE … WHERE accepted_at IS NULL` either affects one row or the
//! caller lost.
//!
//! Invitations are independent of single sign-on and co-exist with it. The
//! membership an acceptance grants carries `source = 'manual'`, so an IdP login
//! never reconciles it away; see [`crate::sso`] and `docs/dev-docs/architecture/sso.md`.
//!
//! The token proves that someone was sent the link, never who holds it: the
//! inviter gets the same token back from `create_invitation`. So accepting
//! never signs anyone in to an account that existed before the invitation.
//! For an existing account it only attaches the invited role, leaves every
//! credential as it was, and sends the invitee to the normal sign-in, where
//! their own password and any second factor still apply (#1935).

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_store::postgres::models::{Invitation, User};
use rolter_store::postgres::repo::{
    InvitationRepo, MembershipRepo, OrgRepo, SessionRepo, UserRepo,
};

use crate::auth::{generate_session_token, session_pepper};
use crate::crud::{
    hash_password, log_audit, pool, validate_email, validate_role, ApiError, ApiResult, SafeJson,
};
use crate::rbac::{authorize, reaches_org, Principal, ScopeChain, ScopeFilter};
use crate::rbac_matrix::cap;
use crate::ControlState;

/// how long an invite link stays usable. long enough to survive a weekend,
/// short enough that a link forwarded on and forgotten stops working
const INVITE_TTL_HOURS: i64 = 24 * 7;

/// session handed to a newly created invitee on acceptance, so they land signed
/// in rather than at a login form they have not used yet
const SESSION_TTL_HOURS: i64 = 24 * 7;

pub fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/invitations",
            get(list_invitations).post(create_invitation),
        )
        .route("/api/v1/invitations/{id}", axum::routing::delete(revoke))
        // unauthenticated by necessity: the invitee has no account yet. the
        // token in the path is the credential
        .route("/api/v1/invitations/accept/{token}", get(preview))
        .route("/api/v1/invitations/accept/{token}/accept", post(accept))
}

#[derive(Debug, Deserialize)]
struct CreateInvitation {
    email: String,
    /// one of `admin` | `member` | `viewer`
    role: String,
    /// `org` | `team` | `project`; defaults to the org named in the path
    #[serde(default)]
    scope_type: Option<String>,
    #[serde(default)]
    scope_id: Option<Uuid>,
}

/// the invitation plus the token, which is shown exactly once
#[derive(Debug, Serialize)]
struct CreatedInvitation {
    invitation: Invitation,
    /// hand this to the invitee; it is not recoverable afterwards
    token: String,
    /// ready-made link, so the caller does not have to know the url shape
    accept_url: String,
}

async fn create_invitation(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateInvitation>,
) -> ApiResult<Json<CreatedInvitation>> {
    let email = validate_email(&body.email)?;
    validate_role(&body.role)?;
    let pool_ref = pool(&state);

    let (chain, team, project) = match (body.scope_type.as_deref(), body.scope_id) {
        (None, _) | (Some("org"), None) => (ScopeChain::org(org_id), None, None),
        (Some("org"), Some(id)) => (ScopeChain::org(id), None, None),
        (Some("team"), Some(id)) => (ScopeChain::from_team(pool_ref, id).await?, Some(id), None),
        (Some("project"), Some(id)) => (
            ScopeChain::from_project(pool_ref, id).await?,
            None,
            Some(id),
        ),
        (Some(other), _) => {
            return Err(invalid(format!(
                "scope_type must be one of org, team, project (got '{other}')"
            )))
        }
    };
    if chain.org != Some(org_id) {
        return Err(invalid("scope does not belong to this org"));
    }
    // inviting someone into a scope is granting a role there, so it takes the
    // same admin bar as granting one directly
    authorize(&state, &principal, chain, cap!("invitation", Create)).await?;

    let (token, token_hash) = generate_invite_token();
    let invited_by = match &principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
    let invitation = InvitationRepo(pool_ref)
        .create(
            org_id,
            &email,
            &body.role,
            team,
            project,
            &token_hash,
            invited_by,
            Utc::now() + Duration::hours(INVITE_TTL_HOURS),
        )
        .await?;

    log_audit(
        &state,
        &principal,
        Some(org_id),
        "invitation.create",
        "invitation",
        invitation.id,
        serde_json::json!({"email": email, "role": body.role}),
    )
    .await;

    Ok(Json(CreatedInvitation {
        accept_url: accept_url(crate::sso::public_base_url(&state), &token),
        invitation,
        token,
    }))
}

async fn list_invitations(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Invitation>>> {
    let invitations = InvitationRepo(pool(&state)).list(org_id).await?;
    let filter = ScopeFilter::load(&state, &principal, cap!("invitation", Read)).await?;
    if filter.allows(ScopeChain::org(org_id)) {
        return Ok(Json(invitations));
    }
    // below the org: the invitations into the teams and projects the caller
    // administers, so a team admin can see and revoke what they sent (#1850)
    if !reaches_org(&filter.reach(pool(&state)).await?, org_id) {
        return Err(ApiError::Forbidden);
    }
    let mut visible = Vec::new();
    for invitation in invitations {
        if filter.allows(invitation_chain(&state, &invitation).await?) {
            visible.push(invitation);
        }
    }
    Ok(Json(visible))
}

/// The scope an invitation grants a role at: its project, else its team, else
/// its org.
async fn invitation_chain(state: &ControlState, invitation: &Invitation) -> ApiResult<ScopeChain> {
    match (invitation.project_id, invitation.team_id) {
        (Some(project), _) => ScopeChain::from_project(pool(state), project).await,
        (None, Some(team)) => ScopeChain::from_team(pool(state), team).await,
        (None, None) => Ok(ScopeChain::org(invitation.org_id)),
    }
}

async fn revoke(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Invitation>> {
    let existing = InvitationRepo(pool(&state)).get(id).await?;
    // at the scope the invitation grants, the way `create_invitation` is
    // authorized, so the team admin who sent it can take it back (#1850)
    authorize(
        &state,
        &principal,
        invitation_chain(&state, &existing).await?,
        cap!("invitation", Delete),
    )
    .await?;
    let invitation = InvitationRepo(pool(&state)).revoke(id).await?;
    log_audit(
        &state,
        &principal,
        Some(invitation.org_id),
        "invitation.revoke",
        "invitation",
        invitation.id,
        serde_json::json!({"email": invitation.email}),
    )
    .await;
    Ok(Json(invitation))
}

/// what the accept screen needs to render, and nothing more. Notably not the
/// inviter's identity or any other org member: a link that leaked should not
/// also leak a directory.
#[derive(Debug, Serialize)]
struct InvitationPreview {
    org_name: String,
    email: String,
    role: String,
    expires_at: chrono::DateTime<Utc>,
    /// an account already exists under this email, so accepting adds the role
    /// to it and the invitee then signs in as usual, rather than choosing a
    /// password (#1935). The only holders of the token are the invitee and the
    /// inviter, and the inviter could learn the same by accepting the link
    /// themselves: an existing account has to be answered differently from a
    /// new one, or the fix would be the hole it closes
    has_account: bool,
}

async fn preview(
    State(state): State<ControlState>,
    client: crate::login_throttle::ClientAddr,
    headers: axum::http::HeaderMap,
    Path(token): Path<String>,
) -> ApiResult<Json<InvitationPreview>> {
    let invitation = live_invitation_throttled(&state, client, &headers, &token).await?;
    let org = OrgRepo(pool(&state)).get(invitation.org_id).await?;
    let has_account = UserRepo(pool(&state))
        .find_by_email(&invitation.email)
        .await?
        .is_some();
    Ok(Json(InvitationPreview {
        org_name: org.name,
        email: invitation.email,
        role: invitation.role,
        expires_at: invitation.expires_at,
        has_account,
    }))
}

#[derive(Debug, Deserialize)]
struct AcceptInvitation {
    /// the password for the account the invitation creates. Required when no
    /// account exists under the invited email, and ignored when one does: an
    /// invite link is neither a sign-in nor a password reset
    #[serde(default)]
    password: Option<String>,
}

#[derive(Debug, Serialize)]
struct AcceptResponse {
    token: String,
    expires_at: chrono::DateTime<Utc>,
    user: User,
    /// set when an org requires a second factor this account has not armed yet
    /// and its grace window is still open, as on a password sign-in (#1852)
    #[serde(skip_serializing_if = "Option::is_none")]
    mfa_enrol_by: Option<chrono::DateTime<Utc>>,
}

/// Why an accepted invitation came back without a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SignInReason {
    /// the email already had an account; the token proves nothing about who
    /// holds it, so its own password (and factor) has to
    ExistingAccount,
    /// a new account in an org whose `required_*` policy is in force: a
    /// session without the factor is exactly what the policy forbids, and the
    /// sign-in is where the enrolment challenge is issued
    SecondFactor,
}

/// The invitation is accepted and its role granted, but the invitee has to
/// sign in through `POST /api/v1/auth/login` to get a session.
#[derive(Debug, Serialize)]
struct SignInRequired {
    sign_in_required: bool,
    email: String,
    reason: SignInReason,
}

/// What `POST /api/v1/invitations/accept/{token}/accept` answers with, told
/// apart by the client on `sign_in_required`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum AcceptOutcome {
    Session(AcceptResponse),
    SignIn(SignInRequired),
}

async fn accept(
    State(state): State<ControlState>,
    client: crate::login_throttle::ClientAddr,
    headers: axum::http::HeaderMap,
    Path(token): Path<String>,
    SafeJson(body): SafeJson<AcceptInvitation>,
) -> ApiResult<Json<AcceptOutcome>> {
    let invitation = live_invitation_throttled(&state, client, &headers, &token).await?;
    let pool_ref = pool(&state);

    // settled before the invitation is claimed, so a missing or too-short
    // password is refused without spending the link
    let existing = UserRepo(pool_ref).find_by_email(&invitation.email).await?;
    let hash = match (&existing, body.password.as_deref()) {
        (Some(_), _) => None,
        (None, Some(password)) => Some(hash_password(password)?),
        (None, None) => return Err(password_required()),
    };

    // claim the invitation first: if two accepts race, exactly one wins, and
    // the loser must not create an account or a membership
    if !InvitationRepo(pool_ref)
        .mark_accepted(invitation.id)
        .await?
    {
        return Err(ApiError::Unauthenticated);
    }

    // an account that already exists under this email is adopted rather than
    // forked -- someone may hold a login in another org, or have arrived
    // through sso first -- but never *entered*: its password, sso-only or not,
    // stays exactly as it was, and no session is minted for it. The token went
    // back to the inviter too, so anything else would let an org admin who
    // invites a superadmin's email sign in as that superadmin (#1935)
    let (user, created) = match existing {
        Some(existing) => {
            if existing.deactivated_at.is_some() {
                return Err(ApiError::Forbidden);
            }
            (existing, false)
        }
        None => {
            // settled above; refused rather than assumed, all the same
            let Some(hash) = hash else {
                return Err(password_required());
            };
            (
                UserRepo(pool_ref)
                    .create(&invitation.email, Some(&hash), false)
                    .await?,
                true,
            )
        }
    };

    let org_scope = if invitation.team_id.is_none() && invitation.project_id.is_none() {
        Some(invitation.org_id)
    } else {
        None
    };
    // 'manual' on purpose: an invited role is operator intent and must survive
    // every later sso reconciliation
    let memberships = MembershipRepo(pool_ref).list_for_user(user.id).await?;
    let already = memberships.iter().any(|m| {
        m.org_id == org_scope
            && m.team_id == invitation.team_id
            && m.project_id == invitation.project_id
            && m.role == invitation.role
    });
    if !already {
        MembershipRepo(pool_ref)
            .create_with_source(
                user.id,
                org_scope,
                invitation.team_id,
                invitation.project_id,
                &invitation.role,
                "manual",
            )
            .await?;
    }

    // a new account goes through the decision a password sign-in makes (#1852),
    // now that the membership binding it to the org's policy exists. It has no
    // factor armed yet, so the only question is whether one is enforced
    let outcome = if created {
        let effective = crate::mfa::effective_policy(&state, &user).await?;
        if effective.enforced() {
            Err(SignInReason::SecondFactor)
        } else {
            Ok(effective.enforce_after)
        }
    } else {
        Err(SignInReason::ExistingAccount)
    };

    let _ = rolter_store::postgres::repo::AuditLogRepo(pool_ref)
        .create(
            Some(invitation.org_id),
            Some(user.id),
            "invitation.accept",
            Some("invitation"),
            Some(invitation.id),
            Some(serde_json::json!({
                "role": invitation.role,
                "account_created": created,
                "signed_in": outcome.is_ok(),
            })),
        )
        .await;

    let mfa_enrol_by = match outcome {
        Ok(enrol_by) => enrol_by,
        Err(reason) => {
            return Ok(Json(AcceptOutcome::SignIn(SignInRequired {
                sign_in_required: true,
                email: user.email,
                reason,
            })))
        }
    };

    let (session_token, session_hash) = generate_session_token(&session_pepper());
    let expires_at = Utc::now() + Duration::hours(SESSION_TTL_HOURS);
    SessionRepo(pool_ref)
        .create(user.id, &session_hash, expires_at)
        .await?;

    Ok(Json(AcceptOutcome::Session(AcceptResponse {
        token: session_token,
        expires_at,
        user,
        mfa_enrol_by,
    })))
}

/// Resolve a token to a live invitation. Expired, revoked, accepted and simply
/// wrong tokens are all the same 401: the caller learns whether their link
/// works, not which of those it is.
async fn live_invitation(state: &ControlState, token: &str) -> ApiResult<Invitation> {
    let hash = rolter_auth::hash_key(&session_pepper(), token);
    InvitationRepo(pool(state))
        .find_live_by_hash(&hash)
        .await?
        .ok_or(ApiError::Unauthenticated)
}

/// [`live_invitation`] behind the same failed-attempt counters as the password
/// login (#1079).
///
/// An invitation token is 32 random bytes, so this is not really about someone
/// guessing one — it is about the endpoint being an unauthenticated door that
/// answers an unlimited number of requests, and about `accept` reaching an
/// argon2 hash once a token resolves. The counters key on the *submitted token*
/// and on the client address, so a run of rejected links costs the caller a
/// growing delay and then a temporary lock, exactly like a run of wrong
/// passwords.
async fn live_invitation_throttled(
    state: &ControlState,
    client: crate::login_throttle::ClientAddr,
    headers: &axum::http::HeaderMap,
    token: &str,
) -> ApiResult<Invitation> {
    let ip = client.resolve(headers, state.trust_forwarded_for);
    let subjects = state.login_throttle.subjects(token, ip);
    if let Some(locked) = state.login_throttle.check(&subjects).await {
        tracing::warn!(
            scope = locked.scope.as_str(),
            client = ?ip,
            "rejected invitation lookup: too many failed attempts"
        );
        return Err(ApiError::TooManyAttempts(locked.retry_after));
    }
    match live_invitation(state, token).await {
        Ok(invitation) => {
            state.login_throttle.record_success(&subjects).await;
            Ok(invitation)
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

fn generate_invite_token() -> (String, String) {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token: String = format!("rolter_invite_{}", rolter_auth::hex::encode(&bytes));
    let hash = rolter_auth::hash_key(&session_pepper(), &token);
    (token, hash)
}

fn accept_url(base: &str, token: &str) -> String {
    format!("{base}/invite/{token}")
}

fn password_required() -> ApiError {
    invalid("a password is required to create the account this invitation is for")
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(rolter_core::Error::Config(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_tokens_are_prefixed_random_and_never_their_own_digest() {
        let (a, hash_a) = generate_invite_token();
        let (b, _) = generate_invite_token();
        assert!(a.starts_with("rolter_invite_"));
        assert_ne!(a, b, "two invites must not collide");
        assert_ne!(
            a, hash_a,
            "the stored value must be a digest, not the token"
        );
        assert_eq!(hash_a, rolter_auth::hash_key(&session_pepper(), &a));
    }

    #[test]
    fn the_accept_url_is_built_from_configuration_not_a_request() {
        // the base is the deployment's own public url, injected rather than
        // taken from the request that asked for the invitation
        assert_eq!(
            accept_url("https://rolter.example", "rolter_invite_abc"),
            "https://rolter.example/invite/rolter_invite_abc"
        );
    }
}
