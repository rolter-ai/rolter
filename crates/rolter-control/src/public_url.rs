//! The control plane's own public base URL, for the screens that hand out an
//! address for an identity provider to call (#2083).
//!
//! Registering an identity provider starts in the provider's console, which
//! wants rolter's redirect URI before it issues the client id and secret the
//! Single Sign-On screen asks for. The dashboard cannot work that address out
//! from its own origin: the control plane builds every URL it gives away from
//! `ROLTER_PUBLIC_URL`, never from the request, and the browser may be reaching
//! it under another name. So the base is served once here and the screen
//! appends the path, the same way the SSO redirect URI, the SSO login URL and
//! the SCIM base URL (#2079) are all built on the server.
//!
//! `configured` says whether the operator set the variable. When they did not,
//! the base is the built-in default, which an IdP can only send a browser back
//! to on the control plane's own host, and the dashboard says that beside the
//! URL instead of letting the IdP's redirect-mismatch error say it later.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::crud::ApiResult;
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route("/api/v1/public-url", get(get_public_url))
}

#[derive(Debug, Serialize)]
struct PublicUrlView {
    /// the base every IdP-facing address is built from, with no trailing slash
    public_url: String,
    /// whether `ROLTER_PUBLIC_URL` supplied it, rather than the default
    configured: bool,
}

/// The deployment's public base URL, and whether it was configured.
///
/// Readable by every authenticated caller: it is a fact about the deployment,
/// the same for everyone, and it is the address they are already using
/// whenever the deployment is configured correctly.
async fn get_public_url(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<PublicUrlView>> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("public_url", Read),
    )
    .await?;
    Ok(Json(PublicUrlView {
        public_url: state.public_url.base.clone(),
        configured: state.public_url.configured,
    }))
}
