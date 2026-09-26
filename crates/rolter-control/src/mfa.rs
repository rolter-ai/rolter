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
//! in. The two `required_*` values never let a user who has no armed factor
//! in unprotected: the login exchange hands them an **enrolment challenge**
//! instead of a session (#1852) -- a token that can mint a secret and prove
//! it, and nothing else -- and proving it is what issues the session.
//!
//! An org may announce the requirement before it bites
//! (`org_auth_policies.mfa_enforce_after`). Until that moment an unenrolled
//! member signs in with the password alone, and the session says by when they
//! have to enrol.
//!
//! Unlike `allow_password_login`, `required_all` does **not** exempt
//! superadmins. It does not need to: the break-glass path here is
//! `rolter mfa reset`, which runs on the host with database access and is
//! audited, so exempting the most privileged account would weaken the policy
//! and buy no recoverability.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_auth::totp;
use rolter_core::Error;
use rolter_store::postgres::crypto::Kek;
use rolter_store::postgres::models::{MfaPolicyBinding, User};
use rolter_store::postgres::repo::{
    mfa_policy_rank, AuditLogRepo, ChallengePurpose, MfaRepo, OrgAuthPolicyRepo, SessionRepo,
    UserRepo,
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

/// How long a login bound by a `required_*` policy has to enrol. Longer than a
/// step-up, because the person may have to install an authenticator app
/// first; still short enough that an abandoned one is gone before anyone
/// thinks to look for it.
const ENROLMENT_TTL_MINUTES: i64 = 10;

/// Codes one enrolment challenge accepts before it is spent. Not a guessing
/// budget -- whoever holds the token also holds the secret, so there is
/// nothing to guess -- but a bound on how long one token stays useful. Five
/// leaves room for a phone whose clock is off and a second try after fixing
/// it.
const MAX_ENROLMENT_ATTEMPTS: i32 = 5;

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
        // the same for an enrolment challenge (#1852): the token in the body is
        // the only credential, and it opens these two routes and no others
        .route("/api/v1/auth/mfa/enroll", post(sign_in_enrol))
        .route("/api/v1/auth/mfa/confirm", post(sign_in_confirm))
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

/// Whether this control plane can seal a new secret at all.
///
/// The login exchange asks before it hands out an enrolment challenge: without
/// a KEK the challenge would lead to a refusal one step later, and a user is
/// better served by the refusal up front, with a code that says the remedy is
/// an operator's.
pub(crate) fn can_enrol() -> bool {
    Kek::from_env().is_some()
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
    /// set while the requirement is announced but not yet in force: from this
    /// moment an unenrolled sign-in has to enrol before it gets a session
    /// (#1852). `null` when nothing is required, or it already applies
    pub(crate) enforce_after: Option<DateTime<Utc>>,
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
    let effective = effective_policy(state, user).await?;
    Ok(MfaStatus {
        enabled: factor.as_ref().is_some_and(|f| f.confirmed_at.is_some()),
        enrolment_pending: factor.is_some_and(|f| f.confirmed_at.is_none()),
        recovery_codes_remaining: MfaRepo(pool).remaining_recovery_codes(user.id).await?,
        policy: effective.policy,
        required: effective.required,
        enforce_after: effective.enforce_after,
    })
}

/// The second-factor policy as it applies to one user at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectivePolicy {
    /// the strictest `mfa_policy` across the user's orgs; `off` when none
    pub(crate) policy: String,
    /// whether any of those policies makes a factor mandatory for this user
    pub(crate) required: bool,
    /// while every requirement that binds the user is still in its grace
    /// window, the earliest moment one of them starts to apply. `None` when
    /// nothing is required or a requirement already applies
    pub(crate) enforce_after: Option<DateTime<Utc>>,
}

impl EffectivePolicy {
    /// Whether a sign-in with no armed factor has to enrol before it gets a
    /// session.
    pub(crate) fn enforced(&self) -> bool {
        self.required && self.enforce_after.is_none()
    }
}

/// The policy that applies to the user right now, across all their orgs.
pub(crate) async fn effective_policy(
    state: &ControlState,
    user: &User,
) -> ApiResult<EffectivePolicy> {
    let bindings = OrgAuthPolicyRepo(pool(state))
        .mfa_policies_for_user(user.id)
        .await?;
    Ok(resolve_policy(&bindings, user.is_superadmin, Utc::now()))
}

/// Whether one org's `mfa_policy` makes a factor mandatory for this user.
fn binds(policy: &str, is_superadmin: bool) -> bool {
    match policy {
        "required_all" => true,
        "required_superadmin" => is_superadmin,
        _ => false,
    }
}

/// Reduce every org policy that applies to a user to the one decision the
/// login exchange needs.
///
/// Strictest wins for the policy, and the *earliest* start wins for the grace
/// window: one hardened org already enforcing is enough, and a later window in
/// another org must not postpone it. Split out of [`effective_policy`] so the
/// reduction is tested without a database.
fn resolve_policy(
    bindings: &[MfaPolicyBinding],
    is_superadmin: bool,
    now: DateTime<Utc>,
) -> EffectivePolicy {
    let policy = bindings
        .iter()
        .map(|binding| binding.policy.as_str())
        .max_by_key(|policy| mfa_policy_rank(policy))
        .filter(|policy| mfa_policy_rank(policy) > 0)
        .unwrap_or("off")
        .to_string();
    let binding: Vec<&MfaPolicyBinding> = bindings
        .iter()
        .filter(|binding| binds(&binding.policy, is_superadmin))
        .collect();
    let required = !binding.is_empty();
    let in_force = binding
        .iter()
        .any(|binding| binding.enforce_after.is_none_or(|at| at <= now));
    let enforce_after = if in_force {
        None
    } else {
        binding
            .iter()
            .filter_map(|binding| binding.enforce_after)
            .min()
    };
    EffectivePolicy {
        policy,
        required,
        enforce_after,
    }
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
    Ok(Json(mint_secret(&state, &current.user).await?))
}

/// Seal a fresh secret as the user's pending factor and return it, once.
///
/// Shared by the session-authenticated enrolment and the one a sign-in is
/// sent through, so both hand out exactly the same thing. Deliberately not
/// audited: an enrolment that is never confirmed is not an event, and the
/// confirm is the one that changes how the account authenticates.
async fn mint_secret(state: &ControlState, user: &User) -> ApiResult<EnrolmentResponse> {
    let kek = kek()?;
    let mut secret = [0u8; totp::SECRET_BYTES];
    rand::rng().fill_bytes(&mut secret);
    MfaRepo(pool(state))
        .begin_enrolment(user.id, &secret, &kek)
        .await?;
    Ok(EnrolmentResponse {
        otpauth_uri: totp::otpauth_uri(&issuer(), &user.email, &secret),
        secret: totp::base32_encode(&secret),
        digits: totp::DIGITS,
        period: totp::STEP_SECONDS,
    })
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
    // a grace window does not change this: it lets the unenrolled in for a
    // while, and says nothing about letting the enrolled back out
    if effective_policy(&state, &current.user).await?.required {
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
    // scoped to step-ups: an enrolment token presented here is not found, so
    // it cannot be turned into a session by a code from a secret it minted
    // but never armed
    let Ok(Some(challenge)) = MfaRepo(pool)
        .charge_challenge_attempt(
            &token_hash,
            ChallengePurpose::Verify,
            MAX_CHALLENGE_ATTEMPTS,
        )
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
    let (mfa_token, expires_at) = mint_challenge(
        state,
        user_id,
        "rolter_mfa",
        CHALLENGE_TTL_MINUTES,
        ChallengePurpose::Verify,
    )
    .await?;
    Ok(MfaChallengeResponse {
        mfa_required: true,
        mfa_token,
        expires_at,
    })
}

/// Store a hashed challenge and return the plaintext token, once.
async fn mint_challenge(
    state: &ControlState,
    user_id: Uuid,
    prefix: &str,
    ttl_minutes: i64,
    purpose: ChallengePurpose,
) -> Result<(String, DateTime<Utc>), AuthError> {
    let pool = pool(state);
    // the table only ever holds logins in flight, so this stays small
    let _ = MfaRepo(pool).purge_expired_challenges().await;
    let (token, token_hash) = crate::auth::generate_token(prefix, &session_pepper());
    let expires_at = Utc::now() + Duration::minutes(ttl_minutes);
    MfaRepo(pool)
        .create_challenge(user_id, &token_hash, expires_at, purpose)
        .await
        .map_err(|err| AuthError::Internal(err.to_string()))?;
    Ok((token, expires_at))
}

// ---------------------------------------------------------------------------
// enrolment at sign-in (unauthenticated: the token in the body is all there is)
// ---------------------------------------------------------------------------

/// What `auth::login` returns instead of a session when a `required_*` policy
/// binds the account and it has no armed factor (#1852).
#[derive(Debug, Serialize)]
pub(crate) struct MfaEnrolmentChallengeResponse {
    /// always `true`; present so a client can branch on one field
    pub(crate) mfa_enrolment_required: bool,
    /// present this to `POST /api/v1/auth/mfa/enroll` for a secret, then to
    /// `POST /api/v1/auth/mfa/confirm` with a code from it. It opens nothing
    /// else: it is not a session, and the step-up does not accept it either
    pub(crate) enrolment_token: String,
    pub(crate) expires_at: DateTime<Utc>,
}

/// Issue an enrolment challenge for a login the policy will not let in
/// without a factor.
pub(crate) async fn issue_enrolment_challenge(
    state: &ControlState,
    user_id: Uuid,
) -> Result<MfaEnrolmentChallengeResponse, AuthError> {
    let (enrolment_token, expires_at) = mint_challenge(
        state,
        user_id,
        "rolter_enrol",
        ENROLMENT_TTL_MINUTES,
        ChallengePurpose::Enrol,
    )
    .await?;
    Ok(MfaEnrolmentChallengeResponse {
        mfa_enrolment_required: true,
        enrolment_token,
        expires_at,
    })
}

#[derive(Debug, Deserialize)]
struct SignInEnrolRequest {
    /// the `enrolment_token` the login exchange returned
    enrolment_token: String,
}

#[derive(Debug, Deserialize)]
struct SignInConfirmRequest {
    enrolment_token: String,
    code: String,
}

/// A session that came out of an enrolment, with the recovery codes that
/// enrolment issued.
#[derive(Debug, Serialize)]
struct EnrolledSignIn {
    #[serde(flatten)]
    session: crate::auth::LoginResponse,
    /// shown once; only peppered digests are stored
    recovery_codes: Vec<String>,
}

/// The account an enrolment token names, if the token may still be used.
///
/// Every way it may not -- unknown, expired, spent, minted for the step-up,
/// the account deactivated since, or a factor armed since (another tab
/// finished first) -- is the same `invalid_credentials` the step-up answers,
/// and the remedy is the same too: sign in with the password again, which
/// now hands out whatever challenge fits.
async fn enrolling_user(
    state: &ControlState,
    token_hash: &str,
    charge: bool,
) -> Result<User, AuthError> {
    let repo = MfaRepo(pool(state));
    let challenge = if charge {
        repo.charge_challenge_attempt(token_hash, ChallengePurpose::Enrol, MAX_ENROLMENT_ATTEMPTS)
            .await
    } else {
        repo.live_challenge(token_hash, ChallengePurpose::Enrol, MAX_ENROLMENT_ATTEMPTS)
            .await
    };
    let Ok(Some(challenge)) = challenge else {
        return Err(AuthError::InvalidCredentials);
    };
    let user = UserRepo(pool(state))
        .get(challenge.user_id)
        .await
        .map_err(|err| AuthError::Internal(err.to_string()))?;
    if user.deactivated_at.is_some() {
        return Err(AuthError::InvalidCredentials);
    }
    let armed = repo
        .has_armed_factor(user.id)
        .await
        .map_err(|err| AuthError::Internal(err.to_string()))?;
    if armed {
        return Err(AuthError::InvalidCredentials);
    }
    Ok(user)
}

fn enrolment_token_hash(token: &str) -> String {
    rolter_auth::hash_key(&session_pepper(), token.trim())
}

/// Mint the secret a bound sign-in enrols with, and return it once.
///
/// Callable again with the same token -- it replaces the pending secret, the
/// same way a second `POST /me/mfa/enroll` does -- and it does not charge the
/// token's budget, because asking for a secret proves nothing.
async fn sign_in_enrol(
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<SignInEnrolRequest>,
) -> Result<Json<EnrolmentResponse>, EnrolError> {
    let token_hash = enrolment_token_hash(&body.enrolment_token);
    let user = enrolling_user(&state, &token_hash, false).await?;
    Ok(Json(mint_secret(&state, &user).await?))
}

/// Arm the factor with a code from the minted secret, and sign in.
///
/// The one place an enrolment token turns into a session, and only once: the
/// code is checked first, then the token is consumed by a delete that exactly
/// one request can win, and only that request arms the factor and mints the
/// recovery codes and the session. The session is issued by the same path
/// every other sign-in takes, so it is audited as an `auth.login` like any
/// other.
async fn sign_in_confirm(
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<SignInConfirmRequest>,
) -> Result<Json<EnrolledSignIn>, EnrolError> {
    let pool = pool(&state);
    let token_hash = enrolment_token_hash(&body.enrolment_token);
    let user = enrolling_user(&state, &token_hash, true).await?;
    let factor = MfaRepo(pool)
        .open_secret(user.id, &kek()?)
        .await
        .map_err(ApiError::from)?;
    let Some(factor) = factor else {
        return Err(invalid("no enrolment in progress; request a secret first").into());
    };
    let Some(step) = totp::verify_at(&factor.secret, &body.code, now_seconds()) else {
        audit(
            &state,
            user.id,
            "auth.mfa_confirm_failed",
            serde_json::json!({ "at_sign_in": true }),
        )
        .await;
        return Err(invalid(
            "that code did not match; check the clock on the device and try again",
        )
        .into());
    };
    let taken = MfaRepo(pool)
        .take_challenge(&token_hash, ChallengePurpose::Enrol)
        .await
        .map_err(ApiError::from)?;
    if taken.is_none() {
        // another request with this token got here first, or it just expired
        return Err(AuthError::InvalidCredentials.into());
    }
    MfaRepo(pool)
        .confirm(user.id, step as i64)
        .await
        .map_err(ApiError::from)?;
    let codes = mint_recovery_codes(&state, user.id).await?;
    audit(
        &state,
        user.id,
        "auth.mfa_enabled",
        serde_json::json!({ "recovery_codes": codes.len(), "at_sign_in": true }),
    )
    .await;
    let session = crate::auth::issue_session(&state, user, false).await?;
    Ok(Json(EnrolledSignIn {
        session,
        recovery_codes: codes,
    }))
}

/// What the two enrolment routes refuse with.
///
/// A dead token is the step-up's `invalid_credentials` ([`AuthError`]), so a
/// client handles both challenges the same way; a wrong code or a missing KEK
/// is the step's own answer ([`ApiError`]), the same one the account screen's
/// enrolment gives. Kept as two small variants rather than a rendered
/// `Response`, which is several times their size.
enum EnrolError {
    Token(AuthError),
    Step(ApiError),
}

impl From<AuthError> for EnrolError {
    fn from(err: AuthError) -> Self {
        Self::Token(err)
    }
}

impl From<ApiError> for EnrolError {
    fn from(err: ApiError) -> Self {
        Self::Step(err)
    }
}

impl IntoResponse for EnrolError {
    fn into_response(self) -> Response {
        match self {
            Self::Token(err) => err.into_response(),
            Self::Step(err) => err.into_response(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(policy: &str, enforce_after: Option<DateTime<Utc>>) -> MfaPolicyBinding {
        MfaPolicyBinding {
            policy: policy.to_string(),
            org_id: Uuid::new_v4(),
            enforce_after,
        }
    }

    fn now() -> DateTime<Utc> {
        "2026-09-26T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn no_memberships_means_off_and_nothing_owed() {
        let effective = resolve_policy(&[], true, now());
        assert_eq!(effective.policy, "off");
        assert!(!effective.required);
        assert!(!effective.enforced());
    }

    #[test]
    fn a_required_policy_with_no_window_applies_at_once() {
        let effective = resolve_policy(&[binding("required_all", None)], false, now());
        assert!(effective.required);
        assert!(effective.enforced());
        assert_eq!(effective.enforce_after, None);
    }

    #[test]
    fn required_superadmin_binds_only_superadmins() {
        let bindings = [binding("required_superadmin", None)];
        assert!(!resolve_policy(&bindings, false, now()).required);
        assert!(resolve_policy(&bindings, true, now()).enforced());
    }

    #[test]
    fn a_future_window_announces_the_requirement_without_enforcing_it() {
        let later = now() + Duration::days(7);
        let effective = resolve_policy(&[binding("required_all", Some(later))], false, now());
        // still required, so the factor cannot be removed in the meantime
        assert!(effective.required);
        assert!(!effective.enforced());
        assert_eq!(effective.enforce_after, Some(later));
    }

    #[test]
    fn a_window_that_has_passed_is_no_window() {
        let earlier = now() - Duration::minutes(1);
        let effective = resolve_policy(&[binding("required_all", Some(earlier))], false, now());
        assert!(effective.enforced());
    }

    #[test]
    fn one_org_already_enforcing_beats_another_orgs_window() {
        // a later grace window elsewhere must not postpone a requirement that
        // already applies
        let bindings = [
            binding("required_all", Some(now() + Duration::days(30))),
            binding("required_all", None),
        ];
        assert!(resolve_policy(&bindings, false, now()).enforced());
    }

    #[test]
    fn the_earliest_window_wins_when_every_org_has_one() {
        let soon = now() + Duration::days(3);
        let bindings = [
            binding("required_all", Some(now() + Duration::days(30))),
            binding("required_all", Some(soon)),
        ];
        assert_eq!(
            resolve_policy(&bindings, false, now()).enforce_after,
            Some(soon)
        );
    }

    #[test]
    fn a_window_on_a_policy_that_does_not_bind_is_ignored() {
        // a superadmin-only requirement says nothing to a member, whatever its
        // window says, and must not surface a date they do not owe
        let bindings = [
            binding("required_superadmin", Some(now() + Duration::days(3))),
            binding("optional", None),
        ];
        let effective = resolve_policy(&bindings, false, now());
        assert_eq!(effective.policy, "required_superadmin");
        assert!(!effective.required);
        assert_eq!(effective.enforce_after, None);
    }
}
