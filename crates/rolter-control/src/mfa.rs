//! TOTP second factor for local accounts: enrolment, recovery codes, the
//! login step-up, and the per-org enforcement policy (#1078).
//!
//! ## Where the factor is checked
//!
//! On the login exchange, once, and nowhere else. A session is the unit of
//! trust in this control plane (see the `auth` module): it is an opaque bearer
//! token backed by a row, revocable immediately, and every request presents
//! it. Re-checking a factor per request would mean either storing the secret
//! somewhere a request path can reach it or asking the user for six digits
//! every few seconds, and it would buy nothing a revoked session does not
//! already give.
//!
//! So `auth::login` does not issue a session when the account has an
//! armed factor. It issues a **challenge**: a separate, short-lived token that
//! authenticates nothing and names only which login is in flight. The
//! challenge is redeemed here, for a real session, by a valid TOTP code or an
//! unspent recovery code.
//!
//! ## Enforcement
//!
//! `org_auth_policies.mfa_policy` is one of `off`, `optional`,
//! `required_superadmin`, `required_all`, and a user's effective policy is the
//! strictest across their orgs. `optional` changes nothing about who may sign
//! in; the two `required_*` values refuse a session to a user who has no armed
//! factor, and say so, rather than letting them in unprotected.
//!
//! Unlike `allow_password_login`, `required_all` does **not** exempt
//! superadmins. It does not need to: the break-glass path here is
//! `rolter mfa reset`, which runs on the host with database access and is
//! audited, so exempting the most privileged account would weaken the policy
//! and buy no recoverability.

use axum::extract::State;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_auth::totp;
use rolter_core::Error;
use rolter_store::postgres::crypto::Kek;
use rolter_store::postgres::models::User;
use rolter_store::postgres::repo::{
    AuditLogRepo, MfaRepo, OrgAuthPolicyRepo, SessionRepo, UserRepo,
};

use crate::auth::{session_pepper, AuthError, CurrentUser};
use crate::crud::{pool, ApiError, ApiResult, SafeJson};
use crate::ControlState;

/// How long a login has to answer its second-factor challenge. Long enough to
/// unlock a phone and read a code, short enough that a stolen challenge token
/// is not a standing invitation.
const CHALLENGE_TTL_MINUTES: i64 = 5;

/// Guesses allowed against one challenge before it is spent. Three is enough
/// for a fat-fingered code and a re-read; it leaves an attacker holding a
/// correct password a 3-in-10^6 chance per password submission, and the
/// password half is already throttled by [`crate::login_throttle`].
const MAX_CHALLENGE_ATTEMPTS: i32 = 3;

/// How many recovery codes a batch holds. Ten is the number every comparable
/// console issues; it fits on a printed card and survives a few uses.
const RECOVERY_CODE_COUNT: usize = 10;

/// Bytes of entropy per recovery code (80 bits, rendered as 16 base32
/// characters). A recovery code bypasses the factor entirely, so it is sized
/// to be unguessable rather than to be typed often.
const RECOVERY_CODE_BYTES: usize = 10;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        // redeeming a challenge is by definition unauthenticated: the caller
        // has no session yet, which is the whole point
        .route("/api/v1/auth/mfa/verify", post(verify_challenge))
        .route("/api/v1/me/mfa", get(my_status))
        .route("/api/v1/me/mfa/enroll", post(begin_enrolment))
        .route("/api/v1/me/mfa/confirm", post(confirm_enrolment))
        .route("/api/v1/me/mfa/recovery-codes", post(regenerate_codes))
        .route("/api/v1/me/mfa", delete(disable_factor))
}

/// The deployment KEK, or a client-visible configuration error.
///
/// Never a plaintext fallback: a TOTP secret is a bearer credential, so a
/// deployment without a KEK must be told it cannot enrol rather than quietly
/// storing one in the clear.
fn kek() -> ApiResult<Kek> {
    Kek::from_env().ok_or_else(|| {
        ApiError::Core(Error::Config(
            "enrolling a second factor requires the ROLTER_KEK environment variable on the \
             control plane, so the shared secret is sealed at rest"
                .to_string(),
        ))
    })
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

// ---------------------------------------------------------------------------
// enrolment (authenticated: managing your own factor)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct MfaStatus {
    /// whether an armed factor is protecting this account
    pub(crate) enabled: bool,
    /// whether a secret has been issued but not yet proved
    pub(crate) enrolment_pending: bool,
    /// unspent recovery codes; `0` with `enabled` is a user one lost phone
    /// away from needing break-glass, which the dashboard should say
    pub(crate) recovery_codes_remaining: i64,
    /// the strictest policy across this user's orgs
    pub(crate) policy: String,
    /// whether that policy makes the factor mandatory for this user
    pub(crate) required: bool,
}

async fn my_status(
    current: CurrentUser,
    State(state): State<ControlState>,
) -> ApiResult<Json<MfaStatus>> {
    Ok(Json(status_for(&state, &current.user).await?))
}

pub(crate) async fn status_for(state: &ControlState, user: &User) -> ApiResult<MfaStatus> {
    let pool = pool(state);
    let factor = MfaRepo(pool).status(user.id).await?;
    let (policy, required) = effective_policy(state, user).await?;
    Ok(MfaStatus {
        enabled: factor.as_ref().is_some_and(|f| f.confirmed_at.is_some()),
        enrolment_pending: factor.is_some_and(|f| f.confirmed_at.is_none()),
        recovery_codes_remaining: MfaRepo(pool).remaining_recovery_codes(user.id).await?,
        policy,
        required,
    })
}

/// The strictest policy across the user's orgs, and whether it binds them.
pub(crate) async fn effective_policy(
    state: &ControlState,
    user: &User,
) -> ApiResult<(String, bool)> {
    let policy = OrgAuthPolicyRepo(pool(state))
        .strictest_mfa_policy_for_user(user.id)
        .await?
        .map(|(policy, _)| policy)
        .unwrap_or_else(|| "off".to_string());
    let required = match policy.as_str() {
        "required_all" => true,
        "required_superadmin" => user.is_superadmin,
        _ => false,
    };
    Ok((policy, required))
}

#[derive(Debug, Serialize)]
struct EnrolmentResponse {
    /// scanned by an authenticator app. Shown once -- the secret is sealed
    /// immediately and no read path returns it again
    otpauth_uri: String,
    /// the same secret, for manual entry when a camera is not available
    secret: String,
    digits: u32,
    period: u64,
}

/// Issue a fresh secret and return it once.
///
/// Refuses when a factor is already armed: re-enrolling would replace the
/// working secret, and a user who wanted that should disable the factor first
/// and see the confirmation that asks them to.
async fn begin_enrolment(
    current: CurrentUser,
    State(state): State<ControlState>,
) -> ApiResult<Json<EnrolmentResponse>> {
    let pool = pool(&state);
    if MfaRepo(pool).has_armed_factor(current.user.id).await? {
        return Err(ApiError::Conflict(
            "a second factor is already enabled for this account; disable it before enrolling \
             a new one"
                .to_string(),
        ));
    }
    let kek = kek()?;
    let mut secret = [0u8; totp::SECRET_BYTES];
    rand::rng().fill_bytes(&mut secret);
    MfaRepo(pool)
        .begin_enrolment(current.user.id, &secret, &kek)
        .await?;
    // deliberately not audited: an enrolment that is never confirmed is not an
    // event, and the confirm below is the one that changes how the account
    // authenticates
    Ok(Json(EnrolmentResponse {
        otpauth_uri: totp::otpauth_uri(&issuer(), &current.user.email, &secret),
        secret: totp::base32_encode(&secret),
        digits: totp::DIGITS,
        period: totp::STEP_SECONDS,
    }))
}

/// The issuer an authenticator app shows beside the account. Deployment-set so
/// an operator running several rolters can tell them apart.
fn issuer() -> String {
    std::env::var("ROLTER_MFA_ISSUER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "rolter".to_string())
}

#[derive(Debug, Deserialize)]
struct ConfirmRequest {
    code: String,
}

#[derive(Debug, Serialize)]
struct RecoveryCodesResponse {
    /// shown once; only peppered digests are stored
    recovery_codes: Vec<String>,
}

/// Arm the factor by proving a code from the issued secret, and hand back the
/// first batch of recovery codes.
async fn confirm_enrolment(
    current: CurrentUser,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<ConfirmRequest>,
) -> ApiResult<Json<RecoveryCodesResponse>> {
    let pool = pool(&state);
    let Some(factor) = MfaRepo(pool).open_secret(current.user.id, &kek()?).await? else {
        return Err(invalid("no enrolment in progress; request a secret first"));
    };
    let Some(step) = totp::verify_at(&factor.secret, &body.code, now_seconds()) else {
        audit(
            &state,
            current.user.id,
            "auth.mfa_confirm_failed",
            serde_json::json!({}),
        )
        .await;
        return Err(invalid(
            "that code did not match; check the clock on the device and try again",
        ));
    };
    MfaRepo(pool).confirm(current.user.id, step as i64).await?;
    let codes = mint_recovery_codes(&state, current.user.id).await?;
    audit(
        &state,
        current.user.id,
        "auth.mfa_enabled",
        serde_json::json!({ "recovery_codes": codes.len() }),
    )
    .await;
    Ok(Json(RecoveryCodesResponse {
        recovery_codes: codes,
    }))
}

/// Replace the recovery-code batch, invalidating whatever the user held.
async fn regenerate_codes(
    current: CurrentUser,
    State(state): State<ControlState>,
) -> ApiResult<Json<RecoveryCodesResponse>> {
    if !MfaRepo(pool(&state))
        .has_armed_factor(current.user.id)
        .await?
    {
        return Err(invalid(
            "recovery codes exist to get past a second factor; enable one first",
        ));
    }
    let codes = mint_recovery_codes(&state, current.user.id).await?;
    audit(
        &state,
        current.user.id,
        "auth.mfa_recovery_codes_regenerated",
        serde_json::json!({ "recovery_codes": codes.len() }),
    )
    .await;
    Ok(Json(RecoveryCodesResponse {
        recovery_codes: codes,
    }))
}

async fn mint_recovery_codes(state: &ControlState, user_id: Uuid) -> ApiResult<Vec<String>> {
    let pepper = session_pepper();
    let mut codes = Vec::with_capacity(RECOVERY_CODE_COUNT);
    let mut hashes = Vec::with_capacity(RECOVERY_CODE_COUNT);
    for _ in 0..RECOVERY_CODE_COUNT {
        let mut bytes = [0u8; RECOVERY_CODE_BYTES];
        rand::rng().fill_bytes(&mut bytes);
        // base32 for the same reason the TOTP secret uses it: no `0`/`1`/`8`
        // to confuse with `O`/`l`/`B` on a code someone reads off paper
        let code = totp::base32_encode(&bytes);
        hashes.push(rolter_auth::hash_key(&pepper, &code));
        codes.push(code);
    }
    MfaRepo(pool(state))
        .replace_recovery_codes(user_id, &hashes)
        .await?;
    Ok(codes)
}

#[derive(Debug, Deserialize)]
struct DisableRequest {
    /// a current code (or an unspent recovery code), so a hijacked session
    /// cannot quietly strip the factor it could not get past
    code: String,
}

async fn disable_factor(
    current: CurrentUser,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<DisableRequest>,
) -> ApiResult<axum::http::StatusCode> {
    let pool = pool(&state);
    if !MfaRepo(pool).has_armed_factor(current.user.id).await? {
        return Err(ApiError::Core(Error::NotFound(
            "no second factor is enabled for this account".to_string(),
        )));
    }
    let (_, required) = effective_policy(&state, &current.user).await?;
    if required {
        return Err(ApiError::Forbidden);
    }
    if !prove_factor(&state, current.user.id, &body.code).await? {
        audit(
            &state,
            current.user.id,
            "auth.mfa_disable_failed",
            serde_json::json!({}),
        )
        .await;
        return Err(invalid("that code did not match"));
    }
    MfaRepo(pool).disable(current.user.id).await?;
    audit(
        &state,
        current.user.id,
        "auth.mfa_disabled",
        serde_json::json!({}),
    )
    .await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// the login step-up (unauthenticated: this is what mints the session)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VerifyRequest {
    /// the `mfa_token` the login exchange returned
    mfa_token: String,
    code: String,
}

/// Redeem a challenge for a real session.
///
/// Every rejection here answers the same `invalid_credentials` as a wrong
/// password: an expired challenge, an exhausted one, and a wrong code are
/// indistinguishable to the caller, so a guesser cannot tell "keep going" from
/// "start over".
async fn verify_challenge(
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<VerifyRequest>,
) -> Result<Json<crate::auth::LoginResponse>, AuthError> {
    let pool = pool(&state);
    let token_hash = rolter_auth::hash_key(&session_pepper(), body.mfa_token.trim());
    let Ok(Some(challenge)) = MfaRepo(pool)
        .charge_challenge_attempt(&token_hash, MAX_CHALLENGE_ATTEMPTS)
        .await
    else {
        return Err(AuthError::InvalidCredentials);
    };

    let proved = prove_factor(&state, challenge.user_id, &body.code)
        .await
        .map_err(|_| AuthError::Internal("failed to verify second factor".into()))?;
    if !proved {
        audit(
            &state,
            challenge.user_id,
            "auth.mfa_failed",
            serde_json::json!({ "attempt": challenge.attempts }),
        )
        .await;
        state.metrics.record_login("mfa_invalid");
        return Err(AuthError::InvalidCredentials);
    }

    // the challenge is single-use whatever happens next
    let _ = MfaRepo(pool).delete_challenge(&token_hash).await;
    let user = UserRepo(pool)
        .get(challenge.user_id)
        .await
        .map_err(|err| AuthError::Internal(err.to_string()))?;
    // `after_lock` is the password step's business, and the challenge was only
    // reachable because that step already succeeded
    crate::auth::issue_session(&state, user, false)
        .await
        .map(Json)
}

/// Verify a presented string as either a TOTP code or a recovery code.
///
/// Both are tried because the two are told apart by shape, and asking the user
/// which one they are typing is a step that buys nothing. A TOTP code is six
/// digits; anything else can only be a recovery code.
async fn prove_factor(state: &ControlState, user_id: Uuid, presented: &str) -> ApiResult<bool> {
    let pool = pool(state);
    let presented = presented.trim();
    if presented.len() == totp::DIGITS as usize && presented.bytes().all(|b| b.is_ascii_digit()) {
        let Some(factor) = MfaRepo(pool).open_secret(user_id, &kek()?).await? else {
            return Ok(false);
        };
        let Some(step) = totp::verify_at(&factor.secret, presented, now_seconds()) else {
            return Ok(false);
        };
        // a code that verifies but names a step already spent is a replay, and
        // is refused exactly like a wrong code
        return Ok(MfaRepo(pool).spend_step(user_id, step as i64).await?);
    }
    let hash = rolter_auth::hash_key(&session_pepper(), presented);
    let consumed = MfaRepo(pool).consume_recovery_code(user_id, &hash).await?;
    if consumed {
        audit(
            state,
            user_id,
            "auth.mfa_recovery_code_used",
            serde_json::json!({
                "remaining": MfaRepo(pool).remaining_recovery_codes(user_id).await?
            }),
        )
        .await;
    }
    Ok(consumed)
}

/// What `auth::login` returns instead of a session when the account
/// has an armed factor.
#[derive(Debug, Serialize)]
pub(crate) struct MfaChallengeResponse {
    /// always `true`; present so a client can branch on one field
    pub(crate) mfa_required: bool,
    /// present this to `POST /api/v1/auth/mfa/verify` with a code
    pub(crate) mfa_token: String,
    pub(crate) expires_at: chrono::DateTime<Utc>,
}

/// Issue a challenge for a login that got past the password but still owes a
/// factor.
pub(crate) async fn issue_challenge(
    state: &ControlState,
    user_id: Uuid,
) -> Result<MfaChallengeResponse, AuthError> {
    let pool = pool(state);
    // the table only ever holds logins in flight, so this stays small
    let _ = MfaRepo(pool).purge_expired_challenges().await;
    let (token, token_hash) = crate::auth::generate_token("rolter_mfa", &session_pepper());
    let expires_at = Utc::now() + Duration::minutes(CHALLENGE_TTL_MINUTES);
    MfaRepo(pool)
        .create_challenge(user_id, &token_hash, expires_at)
        .await
        .map_err(|err| AuthError::Internal(err.to_string()))?;
    Ok(MfaChallengeResponse {
        mfa_required: true,
        mfa_token: token,
        expires_at,
    })
}

async fn audit(state: &ControlState, user_id: Uuid, action: &str, detail: serde_json::Value) {
    if let Err(err) = AuditLogRepo(pool(state))
        .create(
            None,
            Some(user_id),
            action,
            Some("user"),
            Some(user_id),
            Some(detail),
        )
        .await
    {
        tracing::warn!(error = %err, action, "failed to write mfa audit entry");
    }
}

/// Clear a user's factor and codes from outside the API (break-glass).
///
/// Shared with the `rolter mfa reset` CLI so the host-side path and the API
/// remove exactly the same rows, and so the reset is audited either way.
pub async fn break_glass_reset(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    reason: &str,
) -> rolter_core::Result<bool> {
    let removed = MfaRepo(pool).disable(user_id).await?;
    // every live session goes too: the point of a reset is that the account is
    // in unknown hands, and leaving a session alive would leave the factor
    // bypassed for up to a week
    SessionRepo(pool).delete_for_user(user_id).await?;
    let _ = AuditLogRepo(pool)
        .create(
            None,
            Some(user_id),
            "auth.mfa_break_glass_reset",
            Some("user"),
            Some(user_id),
            Some(serde_json::json!({ "reason": reason, "had_factor": removed })),
        )
        .await;
    Ok(removed)
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}
