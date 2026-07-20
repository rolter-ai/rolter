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

use argon2::password_hash::{PasswordHash, PasswordVerifier};
use argon2::Argon2;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use rolter_store::postgres::models::{Membership, Session, User};
use rolter_store::postgres::repo::{AuditLogRepo, MembershipRepo, SessionRepo, UserRepo};

use crate::ControlState;

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
    Internal(String),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::InvalidCredentials => (StatusCode::UNAUTHORIZED, "invalid email or password"),
            Self::Unauthenticated => (StatusCode::UNAUTHORIZED, "missing or invalid session"),
            Self::Internal(ref msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.as_str()),
        };
        (
            status,
            Json(serde_json::json!({"error": {"message": message}})),
        )
            .into_response()
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
struct LoginResponse {
    /// bearer token; send as `Authorization: Bearer <token>` on subsequent
    /// requests. Shown once — only its digest is persisted
    token: String,
    expires_at: chrono::DateTime<Utc>,
    user: User,
}

async fn login(
    State(state): State<ControlState>,
    Json(body): Json<LoginRequest>,
) -> AuthResult<Json<LoginResponse>> {
    let email = body.email.trim();
    let pool = pool(&state);
    let user = UserRepo(pool)
        .find_by_email(email)
        .await?
        .ok_or(AuthError::InvalidCredentials)?;

    if user.deactivated_at.is_some() {
        // deactivated accounts keep their row but cannot authenticate; reject
        // like a bad credential rather than revealing the account is disabled
        return Err(AuthError::InvalidCredentials);
    }

    let Some(hash) = &user.password_hash else {
        // sso-only account (no local password set); reject like a wrong
        // password rather than leaking which accounts exist
        return Err(AuthError::InvalidCredentials);
    };
    let parsed = PasswordHash::new(hash).map_err(|e| AuthError::Internal(e.to_string()))?;
    if Argon2::default()
        .verify_password(body.password.as_bytes(), &parsed)
        .is_err()
    {
        return Err(AuthError::InvalidCredentials);
    }

    let (token, token_hash) = generate_session_token(&session_pepper());
    let expires_at = Utc::now() + Duration::hours(SESSION_TTL_HOURS);
    SessionRepo(pool)
        .create(user.id, &token_hash, expires_at)
        .await?;

    // best-effort; login must succeed even if the audit write fails
    let _ = AuditLogRepo(pool)
        .create(
            None,
            Some(user.id),
            "auth.login",
            Some("user"),
            Some(user.id),
            None,
        )
        .await;

    Ok(Json(LoginResponse {
        token,
        expires_at,
        user,
    }))
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
fn generate_session_token(pepper: &str) -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = format!("rolter_sess_{}", hex_encode(&bytes));
    let hash = rolter_auth::hash_key(pepper, &token);
    (token, hash)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
        let token = bearer_token(&parts.headers).ok_or(AuthError::Unauthenticated)?;

        let token_hash = rolter_auth::hash_key(&session_pepper(), token);
        let pool = pool(state);
        let session = SessionRepo(pool)
            .find_active_by_hash(&token_hash)
            .await
            .map_err(AuthError::from)?
            .ok_or(AuthError::Unauthenticated)?;
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
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::{PasswordHasher, SaltString};

    fn hash_password(password: &str) -> String {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
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
}
