//! Local-account login/logout and the `CurrentUser` request extractor (ROL-32).
//!
//! Only mounted when the control plane is started with `--database-url`, same
//! as [`crate::crud`], since these routes need direct pool access.
//!
//! ## Session strategy
//!
//! Sessions are opaque bearer tokens backed by a postgres `sessions` table
//! (`migrations/0013_sessions.sql`), not a stateless JWT. A stateless JWT
//! would need a server-side blocklist to support real logout/revocation
//! before its expiry, which is extra machinery for no benefit here: this
//! deployment already runs postgres for every other auth-adjacent concern
//! (`users`, `memberships`, `virtual_keys` are all postgres rows), so one
//! more table is the smallest addition, not the largest. Redis is already
//! wired into [`crate::ControlState`] for config pub/sub and rate-limit
//! counters, but it's optional (only present when `--redis-url` is set),
//! so making login depend on it would make auth unavailable in postgres-only
//! deployments. The token itself follows the same shape as virtual keys
//! (`rolter_auth::hash_key`/`verify_key`): the plaintext token is returned to
//! the client once and only its peppered SHA-256 digest is stored, so a
//! database leak does not hand out live sessions.
//!
//! `POST /api/v1/auth/logout` deletes the session row outright: revocation is
//! immediate, no blocklist bookkeeping needed.
//!
//! This module builds the `CurrentUser` extractor and proves it works via
//! `GET /api/v1/auth/me`. Wiring role checks into every CRUD mutation is
//! ROL-34, a separate follow-up.

use argon2::password_hash::PasswordVerifier;
use argon2::Argon2;
use argon2::PasswordHash;
use async_trait::async_trait;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use rand::Rng;
use rolter_auth::{Credential, Identity, IdentityError, IdentityProvider};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_store::postgres::models::{Membership, Session, User};
use rolter_store::postgres::repo::{
    AuditLogRepo, MembershipRepo, MfaRepo, OrgAuthPolicyRepo, SessionRepo, UserRepo,
};

use crate::ControlState;

/// [`IdentityProvider`] for rolter's own local accounts (email + argon2id
/// password hash). Implements ROL-35: local login is now one of potentially
/// several pluggable providers, alongside [`crate::sso::OidcIdentityProvider`]
/// and, eventually, LDAP (#241).
pub(crate) struct LocalIdentityProvider {
    pool: PgPool,
}

impl LocalIdentityProvider {
    pub(crate) fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl IdentityProvider for LocalIdentityProvider {
    fn kind(&self) -> &'static str {
        "local"
    }

    /// Verify an email + password pair against the stored argon2id hash.
    ///
    /// Every rejection path still runs exactly one argon2 verification, so
    /// the response time does not reveal whether the email is registered,
    /// deactivated, or sso-only. This is the same timing-safety property the
    /// pre-ROL-35 inline handler had; refactoring into a provider must not
    /// weaken it, so the "always verify once, decide the error afterward"
    /// structure is preserved exactly.
    async fn resolve(&self, credential: Credential) -> Result<Identity, IdentityError> {
        let Credential::Password { email, password } = credential else {
            return Err(IdentityError::UnsupportedCredential { provider: "local" });
        };
        let email = email.trim();

        // constant hash so an unknown/deactivated/sso-only account still costs
        // one argon2 verification, same as a real one
        const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$csPSM0eDz1Mw8vSYmpUZtA$B00EO0lHN1rK85A5RyDcvLIhc+7tTs0vVoBL4I0MOe0";

        let user_opt = UserRepo(&self.pool)
            .find_by_email(email)
            .await
            .map_err(|e| IdentityError::Provider(e.to_string()))?;

        let mut denied: Option<IdentityError> = None;
        let mut hash_to_check = DUMMY_HASH;
        if let Some(user) = &user_opt {
            if user.deactivated_at.is_some() {
                denied = Some(IdentityError::NotVerified);
            } else if let Some(hash) = &user.password_hash {
                hash_to_check = hash;
                // an org may require its members to come through the IdP; superadmins
                // are exempt on purpose, as the break-glass path back in when the IdP
                // is misconfigured or down
                let blocked = OrgAuthPolicyRepo(&self.pool)
                    .password_login_blocked_for_user(user.id)
                    .await
                    .map_err(|e| IdentityError::Provider(e.to_string()))?;
                if !user.is_superadmin && blocked {
                    denied = Some(IdentityError::PolicyDenied(
                        "password login is disabled for this organization; sign in with sso"
                            .to_string(),
                    ));
                }
            } else {
                // sso-only account (no local password set); reject like a wrong
                // password rather than leaking which accounts exist
                denied = Some(IdentityError::NotVerified);
            }
        }

        let parsed =
            PasswordHash::new(hash_to_check).map_err(|e| IdentityError::Provider(e.to_string()))?;
        let password_verified = Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();

        // a policy/status rejection recorded above wins: it ran before the
        // password check in the original sequential form and must keep its
        // more specific status
        if !password_verified {
            denied = denied.or(Some(IdentityError::NotVerified));
        }

        if let Some(err) = denied {
            return Err(err);
        }

        // unreachable with no user: the branch above always records a denial then
        let Some(user) = user_opt else {
            return Err(IdentityError::NotVerified);
        };

        Ok(Identity {
            subject: user.id.to_string(),
            email: user.email.clone(),
            display_name: None,
            groups: Vec::new(),
        })
    }
}

/// how long an issued session stays valid before the client must log in again
const SESSION_TTL_HOURS: i64 = 24 * 7;

pub fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
}

/// Deployment-wide pepper for session tokens (`ROLTER_SESSION_PEPPER`),
/// mirroring `ROLTER_KEY_PEPPER` for virtual keys: tokens are stored as
/// `rolter_auth::hash_key(pepper, token)` so a stolen database dump alone
/// cannot be replayed as a live session.
pub(crate) fn session_pepper() -> String {
    std::env::var("ROLTER_SESSION_PEPPER").unwrap_or_default()
}

fn pool(state: &ControlState) -> &PgPool {
    state
        .pool
        .as_ref()
        .expect("auth router is only mounted when a postgres pool is configured")
}

/// Error type shared by the login/logout/me handlers and the [`CurrentUser`]
/// extractor's rejection.
pub enum AuthError {
    InvalidCredentials,
    Unauthenticated,
    /// the account is real, but its org requires single sign-on (403). Said
    /// plainly rather than as a wrong-password: the user has no way to guess
    /// their way past a policy, and hiding it just sends them to support
    PasswordLoginDisabled,
    /// there is no session *and* the control plane is in open mode, so there
    /// is no account to have a session for. Distinct from
    /// [`Self::Unauthenticated`] because the remedy is completely different:
    /// this one is not fixed by signing in (#942)
    OpenModeNoSession,
    /// the account or the client address has spent its failed-login budget and
    /// is locked for a while (#1079). Carries the remaining lock so the client
    /// gets a `Retry-After` instead of having to poll
    TooManyAttempts(std::time::Duration),
    /// the password was right, but the account's org requires a second factor
    /// and this account has none armed (403). Distinct from a wrong password:
    /// there is nothing to retype, and the remedy is an admin's (#1078)
    MfaEnrolmentRequired,
    Internal(String),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        // `code` is the stable, machine-readable half: the dashboard branches on
        // it to explain an unreachable screen rather than rendering a bare
        // failure, and messages stay free to be reworded
        let (status, code, message) = match self {
            Self::InvalidCredentials => (
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "invalid email or password",
            ),
            Self::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                "missing or invalid session",
            ),
            Self::PasswordLoginDisabled => (
                StatusCode::FORBIDDEN,
                "password_login_disabled",
                "password login is disabled for this organization; sign in with sso",
            ),
            Self::OpenModeNoSession => (
                StatusCode::UNAUTHORIZED,
                "open_mode_no_session",
                "no local account session: this control plane is running in open mode \
                 (no ROLTER_ADMIN_TOKEN), so it has no user accounts to act as. Endpoints \
                 under /api/v1/me/ are per-user and cannot be served without one — set an \
                 admin token and create a local account, or use the admin virtual-key API",
            ),
            Self::TooManyAttempts(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_attempts",
                "too many failed sign-in attempts; try again later",
            ),
            Self::MfaEnrolmentRequired => (
                StatusCode::FORBIDDEN,
                "mfa_enrolment_required",
                "this organization requires a second factor and this account has none enrolled; \
                 an administrator must relax the policy or clear the account with \
                 `rolter mfa reset` so it can enrol",
            ),
            Self::Internal(ref msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", msg.as_str())
            }
        };
        let mut response = (
            status,
            Json(serde_json::json!({"error": {"message": message, "code": code}})),
        )
            .into_response();
        // a lock is a clock, so say what it reads: a client that is told how
        // long to wait does not have to poll to find out
        if let Self::TooManyAttempts(retry_after) = self {
            // round up, so a sub-second remainder never renders as `0` and
            // invites an immediate retry
            let secs = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
            if let Ok(value) = axum::http::HeaderValue::from_str(&secs.to_string()) {
                response
                    .headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, value);
            }
        }
        response
    }
}

impl From<rolter_core::Error> for AuthError {
    fn from(err: rolter_core::Error) -> Self {
        Self::Internal(err.to_string())
    }
}

type AuthResult<T> = Result<T, AuthError>;

#[derive(Debug, Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct LoginResponse {
    /// bearer token; send as `Authorization: Bearer <token>` on subsequent
    /// requests. Shown once — only its digest is persisted
    token: String,
    expires_at: chrono::DateTime<Utc>,
    user: User,
}

/// What `POST /api/v1/auth/login` answers with.
///
/// A single endpoint returning either a session or a challenge, rather than
/// two endpoints: the client cannot know which it will get until the password
/// has been checked, and asking first would leak whether an account has a
/// factor to anyone who can guess an email.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum LoginOutcome {
    /// no factor stands in the way; here is the session
    Session(LoginResponse),
    /// the password was right, but the account owes a second factor
    Challenge(crate::mfa::MfaChallengeResponse),
}

async fn login(
    State(state): State<ControlState>,
    client: crate::login_throttle::ClientAddr,
    headers: axum::http::HeaderMap,
    Json(body): Json<LoginRequest>,
) -> AuthResult<Json<LoginOutcome>> {
    let email = body.email.trim().to_string();
    let pool = pool(&state);

    let client_ip = client.resolve(&headers, state.trust_forwarded_for);
    let subjects = state.login_throttle.subjects(&email, client_ip);

    // checked *before* the password is verified. refusing afterwards would
    // still buy the attacker a full argon2 hash per request, which is the
    // cpu-exhaustion half of #1079 rather than the guessing half
    if let Some(locked) = state.login_throttle.check(&subjects).await {
        state.metrics.record_login("throttled");
        audit_login_failure(
            &state,
            &email,
            client_ip,
            "auth.login_throttled",
            serde_json::json!({
                "scope": locked.scope.as_str(),
                "retry_after_secs": locked.retry_after.as_secs(),
            }),
        );
        return Err(AuthError::TooManyAttempts(locked.retry_after));
    }

    let provider = LocalIdentityProvider::new(pool.clone());
    let resolved = provider
        .resolve(Credential::Password {
            email: email.clone(),
            password: body.password,
        })
        .await;

    let identity = match resolved {
        Ok(identity) => identity,
        Err(err) => {
            // a provider (database) error is the server's fault, not the
            // caller's: counting it would let a database blip lock out the
            // whole user base
            if let IdentityError::Provider(msg) = err {
                state.metrics.record_login("error");
                return Err(AuthError::Internal(msg));
            }
            let penalty = state.login_throttle.record_failure(&subjects).await;
            let mut detail = serde_json::json!({ "delay_ms": penalty.delay.as_millis() as u64 });
            if let Some(lock) = penalty.locked_for {
                detail["locked_for_secs"] = (lock.as_secs()).into();
                detail["locked_scope"] = penalty
                    .locked_scope
                    .map(|scope| scope.as_str())
                    .unwrap_or_default()
                    .into();
            }
            audit_login_failure(
                &state,
                &email,
                client_ip,
                if penalty.locked_for.is_some() {
                    "auth.login_locked"
                } else {
                    "auth.login_failed"
                },
                detail,
            );
            state.metrics.record_login(if penalty.locked_for.is_some() {
                "locked"
            } else {
                "invalid"
            });
            // the delay follows the attempt counter, which is keyed on the
            // submitted address whether or not anybody has registered it — so
            // it stays identical for an unknown account and a wrong password,
            // exactly like the constant-cost argon2 verification above
            if !penalty.delay.is_zero() {
                tokio::time::sleep(penalty.delay).await;
            }
            return Err(match err {
                IdentityError::PolicyDenied(_) => AuthError::PasswordLoginDisabled,
                _ => AuthError::InvalidCredentials,
            });
        }
    };

    let after_lock = state.login_throttle.record_success(&subjects).await;

    let user_id: Uuid = identity
        .subject
        .parse()
        .map_err(|_| AuthError::Internal("resolved identity carried an invalid subject".into()))?;
    let user = UserRepo(pool).get(user_id).await?;

    // the password is proved; the second factor, if there is one, is checked
    // by `POST /api/v1/auth/mfa/verify` against the challenge issued here. The
    // throttle counter was already cleared above, on purpose: the password was
    // correct, and holding the failed-password lock open across the step-up
    // would let a wrong code look like a wrong password
    let (policy, required) = crate::mfa::effective_policy(&state, &user)
        .await
        .map_err(|_| AuthError::Internal("failed to read the second-factor policy".into()))?;
    let armed = MfaRepo(pool).has_armed_factor(user.id).await?;
    if armed {
        state.metrics.record_login("mfa_challenge");
        return Ok(Json(LoginOutcome::Challenge(
            crate::mfa::issue_challenge(&state, user.id).await?,
        )));
    }
    if required {
        // refusing rather than letting them in unprotected: an org that set
        // `required_*` asked for exactly this. The message names the remedy,
        // because a user who cannot enrol without signing in and cannot sign
        // in without enrolling needs to be told an admin must relax the policy
        // or run the break-glass path
        state.metrics.record_login("mfa_required");
        let _ = AuditLogRepo(pool)
            .create(
                None,
                Some(user.id),
                "auth.mfa_enrolment_required",
                Some("user"),
                Some(user.id),
                Some(serde_json::json!({ "policy": policy })),
            )
            .await;
        return Err(AuthError::MfaEnrolmentRequired);
    }

    Ok(Json(LoginOutcome::Session(
        issue_session(&state, user, after_lock).await?,
    )))
}

/// Mint a session for a user whose credentials -- and second factor, where one
/// is armed -- have already been proved.
///
/// Shared with [`crate::mfa`] so a session that came through the step-up is
/// identical to one that came straight from a password, audit entry included.
pub(crate) async fn issue_session(
    state: &ControlState,
    user: User,
    after_lock: bool,
) -> AuthResult<LoginResponse> {
    let pool = pool(state);
    let (token, token_hash) = generate_session_token(&session_pepper());
    let expires_at = Utc::now() + Duration::hours(SESSION_TTL_HOURS);
    SessionRepo(pool)
        .create(user.id, &token_hash, expires_at)
        .await?;

    // best-effort; login must succeed even if the audit write fails.
    // a sign-in that follows a lockout is the one an operator investigating a
    // stuffing run needs to see, so it says so rather than looking like any
    // other login
    let _ = AuditLogRepo(pool)
        .create(
            None,
            Some(user.id),
            "auth.login",
            Some("user"),
            Some(user.id),
            after_lock.then(|| serde_json::json!({ "after_lock": true })),
        )
        .await;

    state.metrics.record_login("success");
    Ok(LoginResponse {
        token,
        expires_at,
        user,
    })
}

/// Record a rejected sign-in, off the response path.
///
/// Spawned rather than awaited for two reasons. It is best-effort — a failed
/// audit write must never turn a rejected login into a 500 — and, more
/// importantly, an entry only lands on the dashboard's per-org AuditLog screen
/// if it carries an org, which costs a lookup that *only exists when the
/// account does*. Awaiting that would make a registered address measurably
/// slower to reject than an unregistered one, handing back precisely the
/// enumeration oracle the constant-cost password path is built to avoid.
fn audit_login_failure(
    state: &ControlState,
    email: &str,
    client: Option<std::net::IpAddr>,
    action: &'static str,
    mut detail: serde_json::Value,
) {
    let pool = pool(state).clone();
    // the address is attacker-supplied, so it is bounded before it reaches a row
    let email: String = email.chars().take(320).collect();
    detail["email"] = email.clone().into();
    detail["client"] = client.map(|ip| ip.to_string()).into();
    // visible even on a deployment nobody is watching the audit screen of
    tracing::warn!(action, client = ?client, "rejected sign-in");
    tokio::spawn(async move {
        let user = UserRepo(&pool).find_by_email(&email).await.ok().flatten();
        let org_id = match &user {
            Some(user) => MembershipRepo(&pool)
                .list_for_user(user.id)
                .await
                .ok()
                .and_then(|memberships| memberships.first().and_then(|m| m.org_id)),
            // an attempt on an address nobody has registered belongs to no org,
            // so it is recorded org-less and read from the logs and the metric
            // rather than from the per-org screen
            None => None,
        };
        let actor = user.as_ref().map(|user| user.id);
        if let Err(err) = AuditLogRepo(&pool)
            .create(org_id, actor, action, Some("user"), actor, Some(detail))
            .await
        {
            tracing::warn!(error = %err, action, "failed to write login audit entry");
        }
    });
}

async fn logout(State(state): State<ControlState>, headers: axum::http::HeaderMap) -> StatusCode {
    // no-op if the header is missing or the session is already gone: logout
    // is idempotent from the client's point of view
    if let Some(token) = bearer_token(&headers) {
        let pool = pool(&state);
        let token_hash = rolter_auth::hash_key(&session_pepper(), token);
        if let Ok(Some(session)) = SessionRepo(pool).find_active_by_hash(&token_hash).await {
            let _ = AuditLogRepo(pool)
                .create(
                    None,
                    Some(session.user_id),
                    "auth.logout",
                    Some("user"),
                    Some(session.user_id),
                    None,
                )
                .await;
        }
        let _ = SessionRepo(pool).delete_by_hash(&token_hash).await;
    }
    StatusCode::NO_CONTENT
}

/// extract the bearer token from `Authorization: Bearer <token>`, if present
pub(crate) fn bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|t| !t.is_empty())
}

#[derive(Debug, Serialize)]
struct MeResponse {
    user: User,
    memberships: Vec<Membership>,
}

async fn me(
    current: CurrentUser,
    State(state): State<ControlState>,
) -> AuthResult<Json<MeResponse>> {
    let memberships = MembershipRepo(pool(&state))
        .list_for_user(current.user.id)
        .await?;
    Ok(Json(MeResponse {
        user: current.user,
        memberships,
    }))
}

/// generate a fresh opaque session token and its peppered digest; the digest
/// is what's persisted, the token is only ever returned to the client
pub(crate) fn generate_session_token(pepper: &str) -> (String, String) {
    generate_token("rolter_sess", pepper)
}

/// The same construction for any opaque token this control plane hands out:
/// 256 bits of CSPRNG output behind a prefix that says what it is, with only
/// the peppered digest persisted. `prefix` keeps a leaked token identifiable
/// in a log without being guessable.
pub(crate) fn generate_token(prefix: &str, pepper: &str) -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = format!("{prefix}_{}", hex_encode(&bytes));
    let hash = rolter_auth::hash_key(pepper, &token);
    (token, hash)
}

fn hex_encode(bytes: &[u8]) -> String {
    rolter_auth::hex::encode(bytes)
}

/// The authenticated user resolved from `Authorization: Bearer <token>` (a
/// live, unexpired [`Session`] row). Extracting this on a handler is enough
/// to require login; per-role authorization on top of it is ROL-34.
///
/// ```ignore
/// async fn protected(current: CurrentUser) -> Json<User> {
///     Json(current.user)
/// }
/// ```
pub struct CurrentUser {
    pub user: User,
    #[allow(dead_code)] // not consumed yet; kept for ROL-34's role checks
    pub session: Session,
}

impl FromRequestParts<ControlState> for CurrentUser {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ControlState,
    ) -> Result<Self, Self::Rejection> {
        // in open mode the admin `Principal` extractor passes every request as
        // superadmin, so admin routes work while these ones 401 — which reads
        // as "my login is broken" rather than "this endpoint needs an account
        // this deployment does not have" (#942). Same fact both extractors
        // branch on, so the two can never disagree about what open mode is
        let open_mode = state.admin_token.is_none();
        let unauthenticated = || {
            if open_mode {
                AuthError::OpenModeNoSession
            } else {
                AuthError::Unauthenticated
            }
        };

        let token = bearer_token(&parts.headers).ok_or_else(unauthenticated)?;

        let token_hash = rolter_auth::hash_key(&session_pepper(), token);
        let pool = pool(state);
        let session = SessionRepo(pool)
            .find_active_by_hash(&token_hash)
            .await
            .map_err(AuthError::from)?
            .ok_or_else(unauthenticated)?;
        let user = UserRepo(pool)
            .get(session.user_id)
            .await
            .map_err(AuthError::from)?;
        Ok(CurrentUser { user, session })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::password_hash::PasswordHasher;

    fn hash_password(password: &str) -> String {
        Argon2::default()
            .hash_password(password.as_bytes())
            .unwrap()
            .to_string()
    }

    #[test]
    fn password_hash_round_trips() {
        let hash = hash_password("correct horse battery staple");
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(Argon2::default()
            .verify_password(b"correct horse battery staple", &parsed)
            .is_ok());
    }

    #[test]
    fn wrong_password_is_rejected() {
        let hash = hash_password("correct horse battery staple");
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(Argon2::default()
            .verify_password(b"wrong password", &parsed)
            .is_err());
    }

    #[test]
    fn session_token_hash_round_trips_and_is_peppered() {
        let (token, hash) = generate_session_token("pepper");
        assert!(token.starts_with("rolter_sess_"));
        // 12 chars prefix + 64 chars hex (32 bytes) = 76 chars
        assert_eq!(token.len(), 76);
        assert!(token["rolter_sess_".len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
        // the same token under the same pepper always re-hashes to the
        // stored digest, which is how session lookup matches it
        assert_eq!(rolter_auth::hash_key("pepper", &token), hash);
        // a different pepper yields a different digest, same as virtual keys
        assert_ne!(rolter_auth::hash_key("other", &token), hash);
    }

    #[test]
    fn hex_encode_correctness() {
        let bytes = [0xde, 0xad, 0xbe, 0xef, 0x00, 0xff, 0x01, 0x0a];
        let encoded = hex_encode(&bytes);
        assert_eq!(encoded, "deadbeef00ff010a");
    }

    #[test]
    fn session_tokens_are_unique() {
        let (a, _) = generate_session_token("pepper");
        let (b, _) = generate_session_token("pepper");
        assert_ne!(a, b);
    }

    /// Read the `(status, code, message)` an [`AuthError`] renders as.
    async fn rendered(err: AuthError) -> (StatusCode, String, String) {
        let response = err.into_response();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (
            status,
            json["error"]["code"].as_str().unwrap().to_string(),
            json["error"]["message"].as_str().unwrap().to_string(),
        )
    }

    #[tokio::test]
    async fn every_auth_error_carries_a_machine_readable_code() {
        // the dashboard branches on `code` to explain a screen it cannot serve;
        // a variant that renders without one degrades to a bare failure
        for err in [
            AuthError::InvalidCredentials,
            AuthError::Unauthenticated,
            AuthError::PasswordLoginDisabled,
            AuthError::OpenModeNoSession,
            AuthError::TooManyAttempts(std::time::Duration::from_secs(30)),
            AuthError::Internal("boom".to_string()),
        ] {
            let (_, code, message) = rendered(err).await;
            assert!(!code.is_empty());
            assert!(!message.is_empty());
        }
    }

    #[tokio::test]
    async fn a_throttled_login_answers_429_with_a_retry_after() {
        // a client that is told how long the lock lasts does not have to poll to
        // find out, and polling is the thing the lock exists to stop
        let response =
            AuthError::TooManyAttempts(std::time::Duration::from_secs(90)).into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("90")
        );
    }

    #[tokio::test]
    async fn a_sub_second_lock_never_renders_as_retry_after_zero() {
        // `0` reads as "retry now", which would turn the tail of every lock into
        // a hot loop against the endpoint the lock is protecting
        let response =
            AuthError::TooManyAttempts(std::time::Duration::from_millis(120)).into_response();
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("1")
        );
    }

    #[tokio::test]
    async fn open_mode_is_distinguishable_from_a_missing_session() {
        // both are 401, and conflating them is the whole bug (#942): one is
        // fixed by signing in, the other cannot be
        let (plain_status, plain_code, _) = rendered(AuthError::Unauthenticated).await;
        let (open_status, open_code, open_message) = rendered(AuthError::OpenModeNoSession).await;

        assert_eq!(plain_status, StatusCode::UNAUTHORIZED);
        assert_eq!(open_status, StatusCode::UNAUTHORIZED);
        assert_ne!(plain_code, open_code);
        assert_eq!(open_code, "open_mode_no_session");

        // the message has to name the cause and the remedy, or the dashboard is
        // left saying "unauthorized" in more words
        assert!(open_message.contains("open mode"), "{open_message}");
        assert!(
            open_message.contains("ROLTER_ADMIN_TOKEN"),
            "{open_message}"
        );
    }
}

#[cfg(test)]
mod argon2_compat_tests {
    use argon2::password_hash::PasswordVerifier;
    use argon2::{Argon2, PasswordHash};

    /// a hash produced outside this crate must still verify, so that passwords
    /// stored before the argon2 0.6 upgrade keep working
    #[test]
    fn a_hash_from_another_implementation_still_verifies() {
        let stored = "$argon2id$v=19$m=19456,t=2,p=1$h6bR7aTJmF9VpbOjZnZ6lQ$50T6ADgGDSYvzmOfXCRTqwhrtiXkgKVPIQv7uLjJn0g";
        let parsed = PasswordHash::new(stored).expect("stored hash parses");
        assert!(Argon2::default()
            .verify_password(b"correct horse battery staple", &parsed)
            .is_ok());
        assert!(Argon2::default()
            .verify_password(b"wrong password", &parsed)
            .is_err());
    }
}
