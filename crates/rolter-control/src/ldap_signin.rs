//! LDAP sign-in for the control plane (#1826).
//!
//! [`crate::ldap`] knows how to authenticate a login against a directory. This
//! module is everything around it: where an org's directory settings live, the
//! admin API that edits them, and the step `POST /api/v1/auth/login` takes when
//! a local password does not verify.
//!
//! ## Where it sits in sign-in
//!
//! The login form is unchanged: one identifier and one password. The local
//! account is tried first; when it does not verify and any directory is
//! enabled, each enabled directory is asked in turn. The first one that
//! authenticates the login wins, and from there the session is the ordinary one
//! — the second-factor policy, the throttle and the audit trail apply to a
//! directory user exactly as to a local one.
//!
//! ## What a directory sign-in grants
//!
//! Directory groups are mapped through the same scoped-grant model SSO and SCIM
//! use (`group → role at an org, team or project`), stored in
//! `ldap_group_mappings`. A user in no mapped group gets the directory's
//! `default_role`; when that is unset they are refused, with the same answer as
//! a wrong password. The memberships a sign-in produces carry
//! `source = 'ldap'` and are reconciled on every sign-in, so dropping a user
//! from a mapped group revokes what it granted the next time they sign in, while
//! a role an administrator granted by hand is never touched.
//!
//! ## What it refuses
//!
//! * **A superadmin is never adopted.** A directory entry whose mail matches a
//!   superadmin account does not sign in as it. The break-glass account has to
//!   stay reachable when the directory is down and must not be reachable by
//!   whoever can edit a directory attribute.
//! * **An account with a role in another org is never adopted.** The directory
//!   is run by an org's admins, who choose every entry's mail and password, so a
//!   directory may sign in as an existing account only when every role it holds
//!   is inside the directory's own org. Otherwise an org admin could give an
//!   entry another org's user's address and sign in with that user's grants.
//! * **A deactivated account stays out**, whatever the directory says.
//! * **Not while the org has password login turned off.** A directory sign-in
//!   is a password typed into rolter's own form, so an org that sets
//!   `allow_password_login = false` to send its members through its identity
//!   provider is not reachable through its directory either.
//! * **No mail, no account.** Accounts are keyed on email across providers, so
//!   an entry that yields no usable address is refused rather than given a
//!   made-up one.
//!
//! A directory that is down is logged for the operator and answered like a
//! wrong password: the sign-in form must not become a probe of the directory's
//! health, and a local account's own failed login must not turn into a 500
//! because an unrelated directory is unreachable.

// spec-ref: ldap
use std::collections::HashSet;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, post, put};
use axum::{Json, Router};
use rolter_auth::{Credential, Identity, IdentityError, IdentityProvider};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use rolter_store::postgres::crypto::Kek;
use rolter_store::postgres::models::{LdapDirectory, LdapGroupMapping};
use rolter_store::postgres::repo::{
    AuditLogRepo, LdapDirectoryValues, LdapRepo, MembershipRepo, OrgAuthPolicyRepo, SecretUpdate,
    UserRepo,
};

use crate::crud::{
    invalid_field, log_audit, pool, require_allowed_egress, require_non_empty, validate_email,
    ApiError, ApiResult, SafeJson,
};
use crate::ldap::{LdapConfig, LdapIdentityProvider};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::sso::{parse_role, reconcile_grants, ScopedGrant};
use crate::ControlState;

/// `memberships.source` of a grant an LDAP sign-in produced.
const LDAP_SOURCE: &str = "ldap";

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/ldap-directories",
            post(create_directory).get(list_directories),
        )
        .route(
            "/api/v1/ldap-directories/{id}",
            put(update_directory).delete(delete_directory),
        )
        .route(
            "/api/v1/ldap-directories/{id}/group-mappings",
            post(create_mapping).get(list_mappings),
        )
        .route("/api/v1/ldap-group-mappings/{id}", delete(delete_mapping))
}

// ---------------------------------------------------------------------------
// sign-in
// ---------------------------------------------------------------------------

/// Whether any directory can take part in a sign-in. The cheap check the login
/// handler makes before it spends anything on the directory path.
pub(crate) async fn any_enabled(state: &ControlState) -> bool {
    state.ldap.available()
        && LdapRepo(pool(state))
            .any_enabled()
            .await
            .unwrap_or_else(|err| {
                tracing::warn!(error = %err, "could not read the ldap directories");
                false
            })
}

/// Whether the directory's org lets its members sign in with a password typed
/// into rolter's form, which a directory sign-in is. A policy that cannot be
/// read counts as not allowed.
async fn password_login_allowed(state: &ControlState, directory: &LdapDirectory) -> bool {
    match OrgAuthPolicyRepo(pool(state)).get(directory.org_id).await {
        Ok(policy) => policy.allow_password_login,
        Err(err) => {
            tracing::warn!(
                directory_id = %directory.id,
                error = %err,
                "ldap directory skipped: its org's login policy could not be read"
            );
            false
        }
    }
}

/// The directory settings as the provider takes them. `None` when the stored
/// bind password cannot be opened, which is logged and skips that directory.
async fn config_for(state: &ControlState, directory: &LdapDirectory) -> Option<LdapConfig> {
    let bind_password = match (&directory.bind_secret_ciphertext, Kek::from_env()) {
        (None, _) => String::new(),
        (Some(_), None) => {
            tracing::warn!(
                directory_id = %directory.id,
                "ldap directory skipped: ROLTER_KEK is not configured, so its bind password \
                 cannot be opened"
            );
            return None;
        }
        (Some(_), Some(kek)) => match LdapRepo(pool(state)).bind_password(&kek, directory).await {
            Ok(password) => password.unwrap_or_default(),
            Err(err) => {
                tracing::warn!(
                    directory_id = %directory.id,
                    error = %err,
                    "ldap directory skipped: its bind password cannot be opened with this KEK"
                );
                return None;
            }
        },
    };
    Some(LdapConfig {
        url: directory.url.clone(),
        bind_dn: directory.bind_dn.clone(),
        bind_password,
        base_dn: directory.base_dn.clone(),
        user_filter: directory.user_filter.clone(),
        email_attr: directory.email_attr.clone(),
        display_name_attr: directory.display_name_attr.clone(),
        group_attr: directory.group_attr.clone(),
        require_tls: directory.require_tls,
    })
}

/// Try `login` + `password` against every enabled directory.
///
/// Returns the identity of the local account the sign-in resolved to, its
/// `subject` being the user id exactly as [`crate::auth::LocalIdentityProvider`]
/// reports it, so the caller carries on as for any other password sign-in.
/// Every refusal is [`IdentityError::NotVerified`]; nothing here distinguishes
/// an unknown user, a wrong password, an unmapped user or a directory that is
/// down.
pub(crate) async fn sign_in(
    state: &ControlState,
    login: &str,
    password: &str,
) -> Result<Identity, IdentityError> {
    let directories = LdapRepo(pool(state)).list_enabled().await.map_err(|err| {
        tracing::warn!(error = %err, "could not read the ldap directories");
        IdentityError::NotVerified
    })?;
    for directory in directories {
        if !password_login_allowed(state, &directory).await {
            continue;
        }
        let Some(config) = config_for(state, &directory).await else {
            continue;
        };
        let Some(source) = state.ldap.open(config.clone()) else {
            continue;
        };
        let provider = match LdapIdentityProvider::new(config, source) {
            Ok(provider) => provider,
            Err(problem) => {
                tracing::warn!(
                    directory_id = %directory.id,
                    problem = %problem,
                    "ldap directory skipped: its settings cannot be used"
                );
                continue;
            }
        };
        let resolved = provider
            .resolve(Credential::Password {
                email: login.trim().to_string(),
                password: password.to_string(),
            })
            .await;
        let identity = match resolved {
            Ok(identity) => identity,
            // already logged by the provider; the next directory may still
            // know this user
            Err(_) => continue,
        };
        match grant_for(state, &directory, identity).await {
            Ok(Some(identity)) => return Ok(identity),
            Ok(None) => continue,
            Err(err) => {
                tracing::warn!(
                    directory_id = %directory.id,
                    error = ?err,
                    "ldap sign-in could not be completed"
                );
                continue;
            }
        }
    }
    Err(IdentityError::NotVerified)
}

/// Turn an authenticated directory entry into a local account with the grants
/// the directory's groups imply. `None` is a refusal.
async fn grant_for(
    state: &ControlState,
    directory: &LdapDirectory,
    identity: Identity,
) -> ApiResult<Option<Identity>> {
    let pool = pool(state);
    let Ok(email) = validate_email(&identity.email) else {
        tracing::info!(
            directory_id = %directory.id,
            "ldap sign-in refused: the entry yields no usable email address"
        );
        return Ok(None);
    };

    let held: HashSet<String> = identity.groups.iter().map(|g| normalize_group(g)).collect();
    let mappings = LdapRepo(pool).list_mappings(directory.id).await?;
    let matched: Vec<&LdapGroupMapping> = mappings
        .iter()
        .filter(|m| held.contains(&normalize_group(&m.group_name)))
        .collect();

    let known = UserRepo(pool).find_by_email(&email).await?;
    if let Some(user) = &known {
        if user.is_superadmin {
            tracing::warn!(
                directory_id = %directory.id,
                user_id = %user.id,
                "ldap sign-in refused: the address belongs to a superadmin, who signs in locally"
            );
            return Ok(None);
        }
        if user.deployment_role.is_some() {
            tracing::warn!(
                directory_id = %directory.id,
                user_id = %user.id,
                "ldap sign-in refused: the address belongs to an account holding a deployment \
                 role, who signs in locally"
            );
            return Ok(None);
        }
        if user.deactivated_at.is_some() {
            return Ok(None);
        }
        // an account that holds a role in another org is not this directory's
        // to sign in as. whoever runs the directory chooses every entry's mail
        // and password, so adopting such an account would hand an org admin a
        // session carrying another org's grants
        let outside = MembershipRepo(pool)
            .ancestors_for_user(user.id)
            .await?
            .into_iter()
            .any(|(_, org, _)| org != Some(directory.org_id));
        if outside {
            tracing::warn!(
                directory_id = %directory.id,
                user_id = %user.id,
                "ldap sign-in refused: the address belongs to an account with roles outside \
                 this directory's org"
            );
            return Ok(None);
        }
    }

    if matched.is_empty() && directory.default_role.is_none() {
        // authenticated, but nothing grants them anything. If they held ldap
        // grants from an earlier sign-in this is the moment to take them away:
        // being dropped from every mapped group is how an operator deprovisions
        // through the directory
        if let Some(user) = &known {
            reconcile_grants(state, directory.org_id, &[], user.id, LDAP_SOURCE).await?;
        }
        return Ok(None);
    }

    let user = match known {
        Some(user) => user,
        None => UserRepo(pool).create(&email, None, false).await?,
    };

    // a first-sign-in default only: the name is the directory's suggestion, not
    // managed, so the user can change it and a later sign-in never reverts it
    if user.display_name.is_none() {
        if let Some(name) = identity
            .display_name
            .as_deref()
            .and_then(crate::me::sanitise_directory_name)
        {
            UserRepo(pool).default_display_name(user.id, &name).await?;
        }
    }

    let mut wanted: Vec<ScopedGrant> = matched
        .iter()
        .map(|m| (m.org_id, m.team_id, m.project_id, m.role.clone()))
        .collect();
    if wanted.is_empty() {
        if let Some(role) = &directory.default_role {
            wanted.push((Some(directory.org_id), None, None, role.clone()));
        }
    }
    let granted = reconcile_grants(state, directory.org_id, &wanted, user.id, LDAP_SOURCE).await?;

    let _ = AuditLogRepo(pool)
        .create(
            Some(directory.org_id),
            Some(user.id),
            "auth.ldap_sign_in",
            Some("ldap_directory"),
            Some(directory.id),
            Some(json!({ "roles": granted })),
        )
        .await;

    Ok(Some(Identity {
        subject: user.id.to_string(),
        email,
        display_name: identity.display_name,
        groups: identity.groups,
    }))
}

/// Directories are inconsistent about DN case and padding, and an operator who
/// types a group the way their directory UI shows it should not silently get no
/// access.
fn normalize_group(group: &str) -> String {
    group.trim().to_lowercase()
}

// ---------------------------------------------------------------------------
// admin api
// ---------------------------------------------------------------------------

fn default_true() -> bool {
    true
}

fn default_user_filter() -> String {
    "(uid={login})".into()
}

fn default_email_attr() -> String {
    "mail".into()
}

fn default_display_name_attr() -> String {
    "cn".into()
}

fn default_group_attr() -> String {
    "memberOf".into()
}

#[derive(Debug, Deserialize)]
struct CreateDirectory {
    name: String,
    /// `ldaps://` or `ldap://` url of the directory
    url: String,
    /// DN of the read-only service account that finds users
    bind_dn: String,
    /// write-only; sealed before it reaches the database and never returned
    #[serde(default)]
    bind_password: Option<String>,
    base_dn: String,
    #[serde(default = "default_user_filter")]
    user_filter: String,
    #[serde(default = "default_email_attr")]
    email_attr: String,
    #[serde(default = "default_display_name_attr")]
    display_name_attr: String,
    #[serde(default = "default_group_attr")]
    group_attr: String,
    /// refuse a plain `ldap://` url; on unless the operator opts out
    #[serde(default = "default_true")]
    require_tls: bool,
    /// role for a user in no mapped group; omit to refuse those users
    #[serde(default)]
    default_role: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateDirectory {
    name: String,
    url: String,
    bind_dn: String,
    /// write-only and three-valued: absent leaves the sealed password alone, an
    /// empty string clears it (an anonymous search) and a value replaces it
    #[serde(default)]
    bind_password: Option<String>,
    base_dn: String,
    #[serde(default = "default_user_filter")]
    user_filter: String,
    #[serde(default = "default_email_attr")]
    email_attr: String,
    #[serde(default = "default_display_name_attr")]
    display_name_attr: String,
    #[serde(default = "default_group_attr")]
    group_attr: String,
    #[serde(default = "default_true")]
    require_tls: bool,
    #[serde(default)]
    default_role: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

/// The fields both create and update validate.
struct Settings<'a> {
    name: &'a str,
    url: &'a str,
    bind_dn: &'a str,
    base_dn: &'a str,
    user_filter: &'a str,
    email_attr: &'a str,
    display_name_attr: &'a str,
    group_attr: &'a str,
    require_tls: bool,
    default_role: Option<&'a str>,
}

impl<'a> Settings<'a> {
    fn check(&self, state: &ControlState) -> ApiResult<()> {
        if !state.ldap.available() {
            return Err(invalid_field(
                "url",
                "this build of rolter does not include LDAP sign-in; rebuild with the `ldap` \
                 cargo feature to use a directory",
            ));
        }
        require_non_empty(self.name, "name")?;
        require_non_empty(self.url, "url")?;
        require_non_empty(self.email_attr, "email_attr")?;
        require_non_empty(self.display_name_attr, "display_name_attr")?;
        require_non_empty(self.group_attr, "group_attr")?;
        // the same rules the provider enforces before it will authenticate
        // anyone, said at the point of the mistake instead of at the first
        // failed sign-in
        LdapConfig {
            url: self.url.to_string(),
            bind_dn: self.bind_dn.to_string(),
            bind_password: String::new(),
            base_dn: self.base_dn.to_string(),
            user_filter: self.user_filter.to_string(),
            email_attr: self.email_attr.to_string(),
            display_name_attr: self.display_name_attr.to_string(),
            group_attr: self.group_attr.to_string(),
            require_tls: self.require_tls,
        }
        .validate()
        .map_err(|problem| invalid_field("directory", problem))?;
        require_allowed_egress(state, self.url, "url")?;
        if let Some(role) = self.default_role {
            parse_role(role, "default_role")?;
        }
        Ok(())
    }

    fn values(&self, enabled: bool) -> LdapDirectoryValues<'a> {
        LdapDirectoryValues {
            name: self.name,
            url: self.url,
            bind_dn: self.bind_dn,
            base_dn: self.base_dn,
            user_filter: self.user_filter,
            email_attr: self.email_attr,
            display_name_attr: self.display_name_attr,
            group_attr: self.group_attr,
            require_tls: self.require_tls,
            default_role: self.default_role,
            enabled,
        }
    }
}

/// Seal a bind password before anything is written, so a missing `ROLTER_KEK`
/// fails the whole request rather than half-applying it.
fn seal_bind_password(password: &str) -> ApiResult<(Vec<u8>, Vec<u8>)> {
    let kek = Kek::from_env().ok_or_else(|| {
        ApiError::KekUnavailable("ROLTER_KEK must be configured to store a bind password".into())
    })?;
    Ok(kek.encrypt(password)?)
}

async fn create_directory(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateDirectory>,
) -> ApiResult<Json<LdapDirectory>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("ldap_directory", Create),
    )
    .await?;
    let settings = Settings {
        name: &body.name,
        url: &body.url,
        bind_dn: &body.bind_dn,
        base_dn: &body.base_dn,
        user_filter: &body.user_filter,
        email_attr: &body.email_attr,
        display_name_attr: &body.display_name_attr,
        group_attr: &body.group_attr,
        require_tls: body.require_tls,
        default_role: body.default_role.as_deref(),
    };
    settings.check(&state)?;
    let sealed = match body.bind_password.as_deref() {
        Some(password) if !password.is_empty() => Some(seal_bind_password(password)?),
        _ => None,
    };
    let directory = LdapRepo(pool(&state))
        .create_directory(
            org_id,
            settings.values(body.enabled),
            sealed.as_ref().map(|(c, n)| (c.as_slice(), n.as_slice())),
        )
        .await
        .map_err(|err| match err {
            rolter_core::Error::AlreadyExists(_) => ApiError::Conflict(
                "this organization already has an LDAP directory; edit or delete it first".into(),
            ),
            other => other.into(),
        })?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "ldap_directory.create",
        "ldap_directory",
        directory.id,
        json!({"name": directory.name, "url": directory.url}),
    )
    .await;
    Ok(Json(directory))
}

async fn list_directories(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<LdapDirectory>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("ldap_directory", Read),
    )
    .await?;
    Ok(Json(
        LdapRepo(pool(&state))
            .find_for_org(org_id)
            .await?
            .into_iter()
            .collect(),
    ))
}

async fn update_directory(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateDirectory>,
) -> ApiResult<Json<LdapDirectory>> {
    let repo = LdapRepo(pool(&state));
    let existing = repo.get_directory(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("ldap_directory", Update),
    )
    .await?;
    let settings = Settings {
        name: &body.name,
        url: &body.url,
        bind_dn: &body.bind_dn,
        base_dn: &body.base_dn,
        user_filter: &body.user_filter,
        email_attr: &body.email_attr,
        display_name_attr: &body.display_name_attr,
        group_attr: &body.group_attr,
        require_tls: body.require_tls,
        default_role: body.default_role.as_deref(),
    };
    settings.check(&state)?;
    let sealed = match body.bind_password.as_deref() {
        Some(password) if !password.is_empty() => Some(seal_bind_password(password)?),
        _ => None,
    };
    let secret = match (&body.bind_password, &sealed) {
        (Some(_), Some((ciphertext, nonce))) => SecretUpdate::Set(ciphertext, nonce),
        (Some(_), None) => SecretUpdate::Clear,
        (None, _) => SecretUpdate::Keep,
    };
    let directory = repo
        .update_directory(id, settings.values(body.enabled), secret)
        .await?;
    // what moved, never what the password is: whether it was rotated is the
    // interesting fact, and the only one safe to record
    log_audit(
        &state,
        &principal,
        Some(directory.org_id),
        "ldap_directory.update",
        "ldap_directory",
        directory.id,
        json!({
            "name": directory.name,
            "url": directory.url,
            "enabled": directory.enabled,
            "bind_password": match secret {
                SecretUpdate::Keep => "unchanged",
                SecretUpdate::Clear => "cleared",
                SecretUpdate::Set(..) => "rotated",
            },
        }),
    )
    .await;
    Ok(Json(directory))
}

async fn delete_directory(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = LdapRepo(pool(&state));
    let directory = repo.get_directory(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(directory.org_id),
        cap!("ldap_directory", Delete),
    )
    .await?;
    repo.delete_directory(id).await?;
    log_audit(
        &state,
        &principal,
        Some(directory.org_id),
        "ldap_directory.delete",
        "ldap_directory",
        id,
        json!({"name": directory.name}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct CreateMapping {
    group_name: String,
    role: String,
    #[serde(default)]
    org_id: Option<Uuid>,
    #[serde(default)]
    team_id: Option<Uuid>,
    #[serde(default)]
    project_id: Option<Uuid>,
}

async fn create_mapping(
    principal: Principal,
    State(state): State<ControlState>,
    Path(directory_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateMapping>,
) -> ApiResult<Json<LdapGroupMapping>> {
    let repo = LdapRepo(pool(&state));
    let directory = repo.get_directory(directory_id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(directory.org_id),
        cap!("ldap_group_mapping", Create),
    )
    .await?;
    require_non_empty(&body.group_name, "group_name")?;
    parse_role(&body.role, "role")?;
    // a mapping may only grant inside the directory's own org, or a directory
    // admin could hand their users a foothold in someone else's tenant
    let scope_org = match (body.org_id, body.team_id, body.project_id) {
        (Some(org), None, None) => org,
        (_, Some(team), None) => ScopeChain::from_team(pool(&state), team)
            .await?
            .org
            .unwrap_or_default(),
        (_, _, Some(project)) => ScopeChain::from_project(pool(&state), project)
            .await?
            .org
            .unwrap_or_default(),
        (None, None, None) => directory.org_id,
    };
    if scope_org != directory.org_id {
        return Err(invalid_field(
            "org_id",
            "a group mapping may only grant a role inside the directory's own org",
        ));
    }
    let mapping = repo
        .add_mapping(
            directory_id,
            &body.group_name,
            // an org-scoped grant only when the mapping names neither a team
            // nor a project; a narrower scope carries the org implicitly
            (body.team_id.is_none() && body.project_id.is_none())
                .then(|| body.org_id.unwrap_or(directory.org_id)),
            body.team_id,
            body.project_id,
            &body.role,
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(directory.org_id),
        "ldap_group_mapping.create",
        "ldap_group_mapping",
        mapping.id,
        json!({"group": mapping.group_name, "role": mapping.role}),
    )
    .await;
    Ok(Json(mapping))
}

async fn list_mappings(
    principal: Principal,
    State(state): State<ControlState>,
    Path(directory_id): Path<Uuid>,
) -> ApiResult<Json<Vec<LdapGroupMapping>>> {
    let repo = LdapRepo(pool(&state));
    let directory = repo.get_directory(directory_id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(directory.org_id),
        cap!("ldap_group_mapping", Read),
    )
    .await?;
    Ok(Json(repo.list_mappings(directory_id).await?))
}

async fn delete_mapping(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = LdapRepo(pool(&state));
    let mapping = repo.get_mapping(id).await?;
    let directory = repo.get_directory(mapping.directory_id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(directory.org_id),
        cap!("ldap_group_mapping", Delete),
    )
    .await?;
    repo.delete_mapping(id).await?;
    log_audit(
        &state,
        &principal,
        Some(directory.org_id),
        "ldap_group_mapping.delete",
        "ldap_group_mapping",
        id,
        json!({"group": mapping.group_name}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_names_match_regardless_of_case_and_padding() {
        assert_eq!(
            normalize_group("  CN=Rolter-Admins,OU=Groups,DC=example,DC=com  "),
            normalize_group("cn=rolter-admins,ou=groups,dc=example,dc=com")
        );
    }
}
