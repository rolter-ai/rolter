//! Per-org login policy, and the unauthenticated discovery endpoint the login
//! screen uses to decide what to render (#240).
//!
//! rolter deliberately supports three deployments, and an operator picks one
//! by configuration rather than by building a different binary:
//!
//! * **no IdP** — nobody registers an SSO provider, so nothing about single
//!   sign-on is ever reachable. Accounts arrive by invitation and log in with
//!   a password.
//! * **IdP only** — a provider is registered and the org sets
//!   `allow_password_login = false`. The login screen shows one button.
//! * **both** — a provider is registered and password login stays on, so
//!   invited contractors and IdP-managed staff share a deployment.
//!
//! The superadmin is exempt from `allow_password_login = false` on purpose: an
//! IdP outage or a mistyped issuer would otherwise lock the deployment out with
//! no way back in. That exemption is the whole reason the flag is safe to turn
//! on.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_store::postgres::models::OrgAuthPolicy;
use rolter_store::postgres::repo::{LockoutGuard, OrgAuthPolicyRepo, SsoRepo};

use crate::crud::{log_audit, pool, ApiError, ApiResult, SafeJson};

use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

/// The values `mfa_policy` accepts, matching the check constraint in
/// `migrations/0067_totp_second_factor.sql`. Rejected here as well as there so
/// a typo is a 400 naming the alternatives rather than a 500 from a constraint
/// violation.
const MFA_POLICIES: &[&str] = &["off", "optional", "required_superadmin", "required_all"];

pub fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/auth/methods", get(methods))
        .route(
            "/api/v1/orgs/{org_id}/auth-policy",
            get(get_policy).put(set_policy),
        )
}

/// What the login screen may offer. Unauthenticated by necessity — it is read
/// before anyone has a session — so it carries no secrets: provider names and
/// slugs only, which are already visible in the login URL.
#[derive(Debug, Serialize)]
struct AuthMethods {
    /// whether to render the email + password form
    password: bool,
    /// one entry per enabled provider; empty means "no sso configured", which
    /// is the default deployment
    sso: Vec<SsoOption>,
}

#[derive(Debug, Serialize)]
struct SsoOption {
    slug: String,
    name: String,
    /// where the button points
    start_url: String,
}

async fn methods(State(state): State<ControlState>) -> ApiResult<Json<AuthMethods>> {
    let providers = SsoRepo(pool(&state)).list_enabled_providers().await?;
    let password = OrgAuthPolicyRepo(pool(&state))
        .any_password_login_allowed()
        .await?;
    Ok(Json(AuthMethods {
        // an empty deployment (no orgs yet) must still be able to log the
        // seeded superadmin in
        password: password || providers.is_empty(),
        sso: providers
            .into_iter()
            .map(|p| SsoOption {
                start_url: format!("/auth/sso/{}/start", p.slug),
                slug: p.slug,
                name: p.name,
            })
            .collect(),
    }))
}

async fn get_policy(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<OrgAuthPolicy>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("org_auth_policy", Read),
    )
    .await?;
    Ok(Json(OrgAuthPolicyRepo(pool(&state)).get(org_id).await?))
}

#[derive(Debug, Deserialize)]
struct SetPolicy {
    allow_password_login: bool,
    allow_sso: bool,
    /// `off`, `optional`, `required_superadmin` or `required_all` (#1078).
    /// Optional in the body so a client written before second factors existed
    /// keeps working; absent means the org's current setting is kept, not
    /// silently reset to `off`
    #[serde(default)]
    mfa_policy: Option<String>,
    /// When a `required_*` policy starts making unenrolled members enrol
    /// before they get a session (#1852). An explicit `null` means at once;
    /// an absent key keeps the stored window, so a client written before the
    /// window existed can re-send `mfa_policy` without silently cancelling a
    /// date an admin announced. Only read alongside `mfa_policy`: a body that
    /// omits the policy changes neither. Dropped under `off` and `optional`,
    /// where there is nothing for it to postpone
    #[serde(default, deserialize_with = "present")]
    mfa_enforce_after: Option<Option<chrono::DateTime<chrono::Utc>>>,
}

/// Tell an explicit `null` apart from an absent key: the key's value, `null`
/// included, arrives wrapped in `Some`, and an absent key never reaches here
/// and stays at the field's `None` default.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

async fn set_policy(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<SetPolicy>,
) -> ApiResult<Json<OrgAuthPolicy>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("org_auth_policy", Update),
    )
    .await?;
    if !body.allow_password_login && !body.allow_sso {
        // both off is not a policy, it is an outage
        return Err(ApiError::Conflict(
            "at least one login method must stay enabled".into(),
        ));
    }
    let current = OrgAuthPolicyRepo(pool(&state)).get(org_id).await?;
    let (mfa_policy, mfa_enforce_after) = match body.mfa_policy.clone() {
        Some(policy) => (
            policy,
            body.mfa_enforce_after.unwrap_or(current.mfa_enforce_after),
        ),
        None => (current.mfa_policy, current.mfa_enforce_after),
    };
    if !MFA_POLICIES.contains(&mfa_policy.as_str()) {
        return Err(ApiError::Core(rolter_core::Error::Config(format!(
            "mfa_policy must be one of {}",
            MFA_POLICIES.join(", ")
        ))));
    }
    if body.mfa_policy.is_some()
        && mfa_policy.starts_with("required_")
        && !crate::mfa::can_enrol(&state)
    {
        // without a KEK no secret can be sealed, so nobody can enrol from their
        // account or at sign-in, and every account the policy binds -- the
        // admin saving it included, once their session runs out -- is refused
        // at the next sign-in. Refused here for the same reason passwords-off
        // is refused before an IdP exists
        return Err(ApiError::Conflict(
            "a required second factor needs ROLTER_KEK set on the control plane: without it \
             nobody can enrol, so every account this policy binds would be locked out. Set \
             ROLTER_KEK and restart the control plane, or choose optional"
                .into(),
        ));
    }
    // a window only postpones a requirement; stored under `off` it would sit
    // there to surprise whoever next turns the requirement on
    let mfa_enforce_after = mfa_enforce_after.filter(|_| mfa_policy.starts_with("required_"));
    // the provider check lives in the store, under the lock the provider
    // writes take, so it cannot race a concurrent disable (#2233)
    let policy = match OrgAuthPolicyRepo(pool(&state))
        .set(
            org_id,
            body.allow_password_login,
            body.allow_sso,
            &mfa_policy,
            mfa_enforce_after,
        )
        .await?
    {
        LockoutGuard::Done(policy) => policy,
        LockoutGuard::WouldLockOut => {
            // refusing passwords before an IdP exists locks every non-superadmin
            // out, and the operator almost certainly meant to register the
            // provider first
            return Err(ApiError::Conflict(
                "register an enabled sso provider before disabling password login".into(),
            ));
        }
    };
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "org_auth_policy.update",
        "org_auth_policy",
        org_id,
        serde_json::json!({
            "allow_password_login": body.allow_password_login,
            "allow_sso": body.allow_sso,
            "mfa_policy": policy.mfa_policy,
            "mfa_enforce_after": policy.mfa_enforce_after,
        }),
    )
    .await;
    Ok(Json(policy))
}
