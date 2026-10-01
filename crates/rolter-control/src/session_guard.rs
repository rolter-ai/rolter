//! The guard for endpoints any signed-in caller may read but no anonymous one
//! may (#1840).
//!
//! The control plane's router has no blanket auth layer: every handler opts in
//! by taking an extractor. [`AnySession`] is the lightest one, for a read that
//! carries no tenant scoping of its own (the dashboard's config view, the
//! provider-kind catalog) and only needs to know the caller is someone. It
//! answers exactly the way the analytics routes do, so a deployment has one
//! idea of "signed in": open mode passes everyone, the admin token passes, and
//! with a database a live session of any role passes.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::ControlState;

/// Proof that the request carried a credential this control plane accepts.
/// Holds nothing: a handler that needs the caller's identity or role takes a
/// `Principal` or `CurrentUser` instead.
pub(crate) struct AnySession;

fn unauthenticated() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": {"message": "missing or invalid credentials"}})),
    )
        .into_response()
}

impl FromRequestParts<ControlState> for AnySession {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ControlState,
    ) -> Result<Self, Self::Rejection> {
        // open mode: the CRUD API already treats every caller as superadmin,
        // so this view is no more exposed than the rows behind it
        let Some(expected) = state.admin_token.as_deref() else {
            return Ok(AnySession);
        };
        // with a database a session resolves exactly as it does for the CRUD
        // API: the admin token, superadmin sessions and every role pass
        #[cfg(feature = "postgres")]
        if state.pool.is_some() {
            return match crate::rbac::Principal::from_request_parts(parts, state).await {
                Ok(_) => Ok(AnySession),
                Err(error) => Err(error.into_response()),
            };
        }
        // without one there are no sessions and the admin token is the only
        // credential that exists
        let presented = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or_default();
        if !presented.is_empty()
            && bool::from(subtle::ConstantTimeEq::ct_eq(
                presented.as_bytes(),
                expected.as_bytes(),
            ))
        {
            Ok(AnySession)
        } else {
            Err(unauthenticated())
        }
    }
}
