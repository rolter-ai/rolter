//! The RBAC capability matrix — the single source of truth for what each
//! guarded route takes (#534, #704).
//!
//! The dashboard's Roles & Permissions screen rendered a hardcoded matrix, so
//! what it showed and what the control plane enforced could drift silently.
//! This module makes the matrix a server-owned artifact: [`CAPABILITIES`] is
//! the one table that backs `GET /api/v1/rbac/matrix` (what roles *can* do),
//! `GET /api/v1/rbac/effective` (what *this caller* can do, at a scope) **and**
//! the guard itself.
//!
//! Every guarded handler names a `(resource, action)` pair through the [`cap!`]
//! macro and hands the resulting [`Requirement`] to [`crate::rbac::authorize`]
//! (or [`crate::rbac::authorize_superadmin`]); no handler names a [`Role`]
//! directly. Because [`requirement_for`] is a `const fn` and [`cap!`] forces it
//! into a `const` item, a pair the table does not define is a **compile
//! error**, and the published matrix is by construction the rule set the guard
//! enforces.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_auth::Role;
use rolter_store::postgres::models::{AccessProfilePolicy, EffectiveGrant};
use rolter_store::postgres::repo::{AccessProfileRepo, CustomRoleRepo, MembershipRepo};

use crate::access_control::{merge_policies, MergedPolicy};
use crate::crud::{pool, ApiResult};
use crate::rbac::{
    authorize, best_role, custom_base_role, grant_applies, resolve_role, role_rank, Principal,
    ScopeChain, ROLES,
};
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/rbac/matrix", get(get_matrix))
        .route("/api/v1/rbac/effective", get(get_effective))
}

/// An action a caller can take on a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    Read,
    Create,
    Update,
    Delete,
}

impl Action {
    /// every action, in the order the matrix presents them
    const ALL: [Action; 4] = [Action::Read, Action::Create, Action::Update, Action::Delete];
}

/// The authority an action takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Authority {
    /// the minimum scoped role that authorizes the action
    Role(Role),
    /// deployment-wide settings only the admin token or a superadmin may touch
    Superadmin,
    /// any authenticated caller, with no membership anywhere. Reserved for
    /// global read-only catalogs that carry no tenant's data: there is no
    /// deployment-scoped membership to hold, so naming a role here would
    /// describe a floor nobody can stand on (#766)
    Authenticated,
}

/// What a caller must hold for one `(resource, action)` pair.
///
/// It carries the pair itself and not just the [`Authority`], because a custom
/// role grants exactly that pair (#534): the guard has to know *what* is being
/// asked for before it can look the answer up among a caller's explicit grants.
/// Handlers still name the pair once, through [`cap!`], and never construct
/// this by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Requirement {
    pub(crate) resource: &'static str,
    pub(crate) action: Action,
    pub(crate) authority: Authority,
}

impl Requirement {
    /// const-friendly discriminant test, so [`superadmin_cap!`] can reject a
    /// scoped requirement at compile time
    pub(crate) const fn is_superadmin(self) -> bool {
        matches!(self.authority, Authority::Superadmin)
    }

    /// A superadmin requirement that names no route capability, for the one
    /// data-integrity path that cleans up a row the schema should have made
    /// impossible. Deliberately not reachable through [`cap!`]: it guards no
    /// published resource, so it must not appear in the matrix either.
    pub(crate) const fn unscoped_superadmin() -> Self {
        Self {
            resource: "",
            action: Action::Delete,
            authority: Authority::Superadmin,
        }
    }
}

/// One resource and the authority each action on it takes. `None` means the
/// action does not exist for the resource at all (an audit log is append-only,
/// orgs have no update route), reported so a UI does not render a cell that can
/// never be true for anyone.
struct Capability {
    resource: &'static str,
    /// scope the resource lives under, surfaced so a UI can ask for the right
    /// `org_id`/`team_id`/`project_id` when checking effective permissions
    scope: &'static str,
    read: Option<Authority>,
    create: Option<Authority>,
    update: Option<Authority>,
    delete: Option<Authority>,
}

const VIEWER: Option<Authority> = Some(Authority::Role(Role::Viewer));
const MEMBER: Option<Authority> = Some(Authority::Role(Role::Member));
const ADMIN: Option<Authority> = Some(Authority::Role(Role::Admin));
const SUPER: Option<Authority> = Some(Authority::Superadmin);
/// any authenticated caller; see [`Authority::Authenticated`]
const ANYONE: Option<Authority> = Some(Authority::Authenticated);
/// the resource has no such action
const NA: Option<Authority> = None;

/// The capability table. Read access is a viewer's, mutations are an admin's,
/// and anything without a tenancy scope to be a member of is superadmin-only.
/// Each entry is what the guard on the corresponding route actually requires —
/// the routes read it from here, so the two cannot disagree.
const CAPABILITIES: &[Capability] = &[
    // an org is created out of band (seeding / the admin token) because there
    // is no wider scope to be an admin of; deleting one is its own admin's
    Capability {
        resource: "org",
        scope: "org",
        read: VIEWER,
        create: SUPER,
        update: NA,
        delete: ADMIN,
    },
    Capability {
        resource: "team",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: NA,
        delete: ADMIN,
    },
    Capability {
        resource: "project",
        scope: "team",
        read: VIEWER,
        create: ADMIN,
        update: NA,
        delete: ADMIN,
    },
    Capability {
        resource: "provider",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    Capability {
        resource: "plugin",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    Capability {
        resource: "provider_group",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    Capability {
        resource: "route",
        scope: "project",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    Capability {
        resource: "virtual_key",
        scope: "project",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    // a key a user mints for themself in a project they belong to: `member`,
    // not `admin`, so read-only viewers still cannot mint one
    Capability {
        resource: "my_virtual_key",
        scope: "project",
        read: NA,
        create: MEMBER,
        update: NA,
        delete: NA,
    },
    Capability {
        resource: "budget",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: NA,
        delete: ADMIN,
    },
    Capability {
        resource: "rate_limit",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: NA,
        delete: ADMIN,
    },
    // the pricing catalog and the effective model list are global (unscoped),
    // so their mutations are superadmin-only. their reads are every
    // authenticated caller's: both are deployment-wide catalogs of upstream
    // capability and list price, carrying no tenant's data, and there is no
    // deployment-scoped membership a role floor could be measured against
    Capability {
        resource: "model_price",
        scope: "deployment",
        read: ANYONE,
        create: NA,
        update: SUPER,
        delete: SUPER,
    },
    Capability {
        resource: "model",
        scope: "deployment",
        read: ANYONE,
        create: NA,
        update: NA,
        delete: SUPER,
    },
    Capability {
        resource: "business_unit",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    Capability {
        resource: "customer",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    Capability {
        resource: "prompt_template",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    Capability {
        resource: "skill",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    // account lifecycle vs. role assignment are split by authority: inviting a
    // user into an org is an org admin's, but editing or deleting the global
    // account (which reaches every org, and can grant the superadmin bit) is
    // superadmin-only
    Capability {
        resource: "user",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: SUPER,
        delete: SUPER,
    },
    Capability {
        resource: "membership",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: NA,
        delete: ADMIN,
    },
    // listing provisioning tokens is an admin read: the rows name the IdPs a
    // tenant trusts, which is not viewer-grade information
    Capability {
        resource: "scim_token",
        scope: "org",
        read: ADMIN,
        create: ADMIN,
        update: NA,
        delete: ADMIN,
    },
    // a group mapping is a way to grant a role, so it needs the same admin bar
    // as granting one directly — the rule sso_group_mapping already follows
    Capability {
        resource: "scim_group_mapping",
        scope: "org",
        read: ADMIN,
        create: ADMIN,
        update: NA,
        delete: ADMIN,
    },
    Capability {
        resource: "mcp_server",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    Capability {
        resource: "mcp_tool_group",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    Capability {
        resource: "mcp_settings",
        scope: "org",
        read: VIEWER,
        create: NA,
        update: ADMIN,
        delete: NA,
    },
    // the OAuth client rolter presents to an MCP server's authorization
    // server. registering one decides which external host receives a consent
    // code, so it is admin-only in both directions — even reading it, since
    // the row names a third party the tenant has chosen to trust
    Capability {
        resource: "mcp_oauth_client",
        scope: "org",
        read: ADMIN,
        create: NA,
        update: ADMIN,
        delete: ADMIN,
    },
    // a grant or session belongs to a user: an org admin sees and revokes any,
    // a member only their own — so the read/revoke floor is a viewer
    // membership at the org and the handler narrows it to the owner.
    //
    // minting is a rung higher. completing consent (`create`), renewing a
    // session (`update`) and exchanging one on-behalf-of (`create`) all hand
    // rolter the ability to act at a third party as the caller, which is a
    // write however read-only the eventual MCP call is — so it sits with
    // `my_virtual_key:create` at member, not with viewer reads
    Capability {
        resource: "mcp_oauth_grant",
        scope: "org",
        read: VIEWER,
        create: MEMBER,
        update: NA,
        delete: VIEWER,
    },
    Capability {
        resource: "mcp_oauth_session",
        scope: "org",
        read: VIEWER,
        create: MEMBER,
        update: MEMBER,
        delete: VIEWER,
    },
    // configurable rbac (#534). Defining a role or a profile is a way to grant
    // authority, so it takes the same admin bar as granting one directly;
    // reading the definitions is a viewer's, so the dashboard can render the
    // matrix for anyone who can see the org
    Capability {
        resource: "custom_role",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    Capability {
        resource: "access_profile",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: ADMIN,
        delete: ADMIN,
    },
    Capability {
        resource: "access_profile_assignment",
        scope: "org",
        read: VIEWER,
        create: ADMIN,
        update: NA,
        delete: ADMIN,
    },
    Capability {
        resource: "audit_log",
        scope: "org",
        read: ADMIN,
        create: NA,
        update: NA,
        delete: NA,
    },
    Capability {
        resource: "invitation",
        scope: "org",
        read: ADMIN,
        create: ADMIN,
        update: NA,
        delete: ADMIN,
    },
    // single sign-on: registering an identity provider or mapping its groups to
    // roles is a way to grant roles, so it needs the same admin bar as granting
    // one directly
    Capability {
        resource: "sso_provider",
        scope: "org",
        read: ADMIN,
        create: ADMIN,
        update: NA,
        delete: ADMIN,
    },
    Capability {
        resource: "sso_group_mapping",
        scope: "org",
        read: ADMIN,
        create: ADMIN,
        update: NA,
        delete: ADMIN,
    },
    Capability {
        resource: "org_auth_policy",
        scope: "org",
        read: ADMIN,
        create: NA,
        update: ADMIN,
        delete: NA,
    },
    // deployment-wide policy: no tenancy scope exists to be a member of, so
    // these are the admin token's (or a superadmin's) alone
    Capability {
        resource: "feature_flags",
        scope: "deployment",
        read: SUPER,
        create: NA,
        update: SUPER,
        delete: NA,
    },
    Capability {
        resource: "runtime_policy",
        scope: "deployment",
        read: SUPER,
        create: NA,
        update: SUPER,
        delete: NA,
    },
    Capability {
        resource: "logging_settings",
        scope: "deployment",
        read: SUPER,
        create: NA,
        update: SUPER,
        delete: NA,
    },
    Capability {
        resource: "compatibility_policy",
        scope: "deployment",
        read: SUPER,
        create: NA,
        update: SUPER,
        delete: NA,
    },
    Capability {
        resource: "client_settings",
        scope: "deployment",
        read: SUPER,
        create: NA,
        update: SUPER,
        delete: NA,
    },
    Capability {
        resource: "model_defaults",
        scope: "deployment",
        read: SUPER,
        create: NA,
        update: SUPER,
        delete: NA,
    },
    Capability {
        resource: "adaptive_routing_policy",
        scope: "deployment",
        read: SUPER,
        create: NA,
        update: SUPER,
        delete: NA,
    },
    Capability {
        resource: "guardrail_rule",
        scope: "deployment",
        read: SUPER,
        create: SUPER,
        update: SUPER,
        delete: SUPER,
    },
    Capability {
        resource: "guardrail_provider",
        scope: "deployment",
        read: SUPER,
        create: SUPER,
        update: SUPER,
        delete: SUPER,
    },
    Capability {
        // read-only: the data plane writes it over the internal channel, and
        // no operator role edits a measurement
        resource: "adaptive_routing_telemetry",
        scope: "deployment",
        read: SUPER,
        create: NA,
        update: NA,
        delete: NA,
    },
    Capability {
        resource: "cluster_node",
        scope: "deployment",
        read: SUPER,
        create: NA,
        update: SUPER,
        delete: SUPER,
    },
    Capability {
        resource: "security_settings",
        scope: "deployment",
        read: SUPER,
        create: NA,
        update: SUPER,
        delete: NA,
    },
    // outbound telemetry export is a deployment-wide egress decision, so it
    // sits at the same level as the security settings above rather than being
    // delegable per org (#511)
    Capability {
        resource: "connector",
        scope: "deployment",
        read: SUPER,
        create: SUPER,
        update: SUPER,
        delete: SUPER,
    },
    Capability {
        resource: "alert_channel",
        scope: "deployment",
        read: SUPER,
        create: SUPER,
        update: SUPER,
        delete: SUPER,
    },
    Capability {
        resource: "alert_rule",
        scope: "deployment",
        read: SUPER,
        create: SUPER,
        update: SUPER,
        delete: SUPER,
    },
    // notification history is append-only; "create" is asking the deployment to
    // evaluate a rule now, which can emit one
    Capability {
        resource: "alert_history",
        scope: "deployment",
        read: SUPER,
        create: SUPER,
        update: NA,
        delete: NA,
    },
    // MCP tool-call telemetry: written by the gateway, read by an operator
    Capability {
        resource: "mcp_log",
        scope: "deployment",
        read: SUPER,
        create: SUPER,
        update: NA,
        delete: NA,
    },
];

impl Capability {
    const fn authority(&self, action: Action) -> Option<Authority> {
        match action {
            Action::Read => self.read,
            Action::Create => self.create,
            Action::Update => self.update,
            Action::Delete => self.delete,
        }
    }
}

/// const-evaluable string equality (`str::eq` is not `const`)
const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The requirement the table records for `(resource, action)`.
///
/// A `const fn`, so [`cap!`] turns an unknown resource or an action the
/// resource does not support into a compile error at the call site rather than
/// a guard that silently disagrees with the published matrix.
pub(crate) const fn requirement_for(resource: &'static str, action: Action) -> Requirement {
    let mut i = 0;
    while i < CAPABILITIES.len() {
        if str_eq(CAPABILITIES[i].resource, resource) {
            return match CAPABILITIES[i].authority(action) {
                Some(authority) => Requirement {
                    resource,
                    action,
                    authority,
                },
                None => panic!(
                    "the rbac capability table marks this action unsupported for this resource"
                ),
            };
        }
        i += 1;
    }
    panic!("this resource is not in the rbac capability table (crates/rolter-control/src/rbac_matrix.rs)")
}

/// The [`Requirement`] a guarded route takes, resolved from [`CAPABILITIES`] at
/// compile time: `cap!("provider", Create)`.
macro_rules! cap {
    ($resource:literal, $action:ident) => {{
        const REQUIREMENT: $crate::rbac_matrix::Requirement =
            $crate::rbac_matrix::requirement_for($resource, $crate::rbac_matrix::Action::$action);
        REQUIREMENT
    }};
}

/// Same as [`cap!`], for a guard that has no scope to resolve a role in: also a
/// compile error unless the table says the pair is superadmin-only.
macro_rules! superadmin_cap {
    ($resource:literal, $action:ident) => {{
        const REQUIREMENT: $crate::rbac_matrix::Requirement =
            $crate::rbac_matrix::requirement_for($resource, $crate::rbac_matrix::Action::$action);
        const _: () = assert!(
            REQUIREMENT.is_superadmin(),
            "this guard requires a superadmin but the capability table does not"
        );
        REQUIREMENT
    }};
}

pub(crate) use {cap, superadmin_cap};

#[derive(Debug, Serialize)]
struct ActionView {
    action: Action,
    /// minimum scoped role, absent when the action is superadmin-only or open
    /// to every authenticated caller
    minimum_role: Option<Role>,
    superadmin_only: bool,
    /// no membership required: any authenticated caller may perform it
    authenticated_only: bool,
}

#[derive(Debug, Serialize)]
struct ResourceView {
    resource: &'static str,
    scope: &'static str,
    actions: Vec<ActionView>,
}

#[derive(Debug, Serialize)]
struct RoleView {
    role: Role,
    /// total order over roles: viewer `0` < member `1` < admin `2`
    rank: u8,
}

/// One `resource:action` an org-defined role grants explicitly.
#[derive(Debug, Serialize)]
struct CustomGrantView {
    resource: String,
    action: String,
}

/// An org-defined role as the matrix presents it, alongside the built-ins.
#[derive(Debug, Serialize)]
struct CustomRoleView {
    id: Uuid,
    slug: String,
    name: String,
    description: Option<String>,
    /// the built-in role this custom role is at least equivalent to
    base_role: Role,
    /// rank of `base_role`, so a UI can order custom roles against built-ins
    base_rank: u8,
    /// the pairs it grants on top of `base_role`
    grants: Vec<CustomGrantView>,
    /// pairs the role names that this build's capability table does not define.
    /// Reported rather than hidden: they grant nothing, and an operator who
    /// downgraded rolter should be able to see why a role went quiet.
    unknown_grants: Vec<CustomGrantView>,
}

#[derive(Debug, Serialize)]
struct MatrixView {
    roles: Vec<RoleView>,
    resources: Vec<ResourceView>,
    /// org-defined roles, present only when the caller asked for an org they
    /// may read. Empty for a deployment that defines none, which keeps the
    /// payload byte-identical to what shipped before #534.
    custom_roles: Vec<CustomRoleView>,
}

/// Whether this build's capability table defines `(resource, action)`. A custom
/// grant naming a pair it does not is inert, never an error: the table is code,
/// and a role must not become unloadable because a release retired a resource.
fn table_defines(resource: &str, action: Action) -> bool {
    CAPABILITIES
        .iter()
        .any(|cap| cap.resource == resource && cap.authority(action).is_some())
}

fn parse_action(action: &str) -> Option<Action> {
    Action::ALL.into_iter().find(|&a| action_key(a) == action)
}

/// The deployment's capability matrix. Any authenticated principal may read
/// it: it describes the rules, not anyone's access, and a caller learns
/// nothing about a tenant they cannot already see.
async fn get_matrix(
    principal: Principal,
    State(state): State<ControlState>,
    Query(query): Query<MatrixQuery>,
) -> ApiResult<Json<MatrixView>> {
    // the org-defined half is per-tenant, so it takes a membership in that
    // tenant; without `org_id` the answer is the built-in table alone, exactly
    // as before
    let custom_roles = match query.org_id {
        Some(org_id) => {
            authorize(
                &state,
                &principal,
                ScopeChain::org(org_id),
                cap!("custom_role", Read),
            )
            .await?;
            custom_role_views(&state, org_id).await?
        }
        None => Vec::new(),
    };
    Ok(Json(MatrixView {
        roles: ROLES
            .iter()
            .map(|&role| RoleView {
                role,
                rank: role_rank(role),
            })
            .collect(),
        resources: CAPABILITIES.iter().map(resource_view).collect(),
        custom_roles,
    }))
}

#[derive(Debug, Deserialize)]
struct MatrixQuery {
    /// include this org's custom roles; requires a membership there
    org_id: Option<Uuid>,
}

async fn custom_role_views(state: &ControlState, org_id: Uuid) -> ApiResult<Vec<CustomRoleView>> {
    let repo = CustomRoleRepo(pool(state));
    let roles = repo.list(org_id).await?;
    let ids: Vec<Uuid> = roles.iter().map(|r| r.id).collect();
    let grants = repo.list_grants_for_roles(&ids).await?;
    Ok(roles
        .into_iter()
        .map(|role| {
            let (known, unknown): (Vec<_>, Vec<_>) = grants
                .iter()
                .filter(|g| g.role_id == role.id)
                .partition(|g| {
                    parse_action(&g.action).is_some_and(|a| table_defines(&g.resource, a))
                });
            let view = |g: &&rolter_store::postgres::models::CustomRoleGrant| CustomGrantView {
                resource: g.resource.clone(),
                action: g.action.clone(),
            };
            let base_role = crate::access_control::parse_base_role(&role.base_role);
            CustomRoleView {
                id: role.id,
                slug: role.slug,
                name: role.name,
                description: role.description,
                base_role,
                base_rank: role_rank(base_role),
                grants: known.iter().map(view).collect(),
                unknown_grants: unknown.iter().map(view).collect(),
            }
        })
        .collect())
}

fn resource_view(cap: &Capability) -> ResourceView {
    ResourceView {
        resource: cap.resource,
        scope: cap.scope,
        actions: Action::ALL
            .iter()
            .filter_map(|&action| {
                cap.authority(action).map(|authority| ActionView {
                    action,
                    minimum_role: match authority {
                        Authority::Role(role) => Some(role),
                        Authority::Superadmin | Authority::Authenticated => None,
                    },
                    superadmin_only: authority == Authority::Superadmin,
                    authenticated_only: authority == Authority::Authenticated,
                })
            })
            .collect(),
    }
}

#[derive(Debug, Deserialize)]
struct EffectiveQuery {
    org_id: Option<Uuid>,
    team_id: Option<Uuid>,
    project_id: Option<Uuid>,
}

/// A custom role the caller holds at the requested scope, and the profile it
/// came from.
#[derive(Debug, Serialize)]
struct HeldRoleView {
    profile_id: Uuid,
    role_id: Uuid,
    role_slug: String,
    base_role: Role,
}

#[derive(Debug, Serialize)]
struct EffectiveView {
    /// true when the caller is the admin token or a superadmin user (which is
    /// also every caller while the control plane runs in open mode)
    superadmin: bool,
    /// the caller's resolved role at the requested scope chain, absent when no
    /// membership reaches it. Raised by a custom role's `base_role` where an
    /// access profile confers one at that scope
    role: Option<Role>,
    /// the `resource:action` pairs the caller may perform at that scope
    allowed: Vec<String>,
    /// the custom roles behind any pair `role` alone does not explain
    custom_roles: Vec<HeldRoleView>,
    /// model/route visibility the caller's access profiles impose. Absent when
    /// no profile carries a policy, which means "everything"
    model_policy: Option<MergedPolicy>,
}

/// What the calling principal may actually do at a scope chain, evaluated
/// server-side from their memberships. A UI uses this to disable controls; the
/// answer is advisory to the client and authoritative only here.
async fn get_effective(
    principal: Principal,
    State(state): State<ControlState>,
    Query(query): Query<EffectiveQuery>,
) -> ApiResult<Json<EffectiveView>> {
    let chain = ScopeChain {
        org: query.org_id,
        team: query.team_id,
        project: query.project_id,
    };
    let (superadmin, from_memberships, grants, policies) = match &principal {
        Principal::Superadmin => (true, None, Vec::new(), Vec::new()),
        Principal::User(user) => {
            let memberships = MembershipRepo(pool(&state)).list_for_user(user.id).await?;
            let profiles = AccessProfileRepo(pool(&state));
            (
                false,
                resolve_role(&memberships, chain.org, chain.team, chain.project),
                profiles.effective_grants_for_user(user.id).await?,
                profiles.policies_for_user(user.id).await?,
            )
        }
    };
    let role = best_role(from_memberships, custom_base_role(&grants, chain));
    Ok(Json(EffectiveView {
        superadmin,
        role,
        allowed: allowed_for(superadmin, role, &grants, chain),
        custom_roles: held_roles(&grants, chain),
        model_policy: merged_policy(&policies),
    }))
}

/// Distinct `(profile, role)` pairs the caller holds at `chain`.
fn held_roles(grants: &[EffectiveGrant], chain: ScopeChain) -> Vec<HeldRoleView> {
    let mut seen: std::collections::HashSet<(Uuid, Uuid)> = std::collections::HashSet::new();
    let mut views = Vec::with_capacity(grants.len());
    for grant in grants.iter().filter(|g| grant_applies(g, chain)) {
        if seen.contains(&(grant.profile_id, grant.role_id)) {
            continue;
        }
        seen.insert((grant.profile_id, grant.role_id));
        views.push(HeldRoleView {
            profile_id: grant.profile_id,
            role_id: grant.role_id,
            role_slug: grant.role_slug.clone(),
            base_role: crate::access_control::parse_base_role(&grant.base_role),
        });
    }
    views
}

fn merged_policy(policies: &[AccessProfilePolicy]) -> Option<MergedPolicy> {
    let merged = merge_policies(policies);
    (!merged.is_unrestricted()).then_some(merged)
}

/// The `resource:action` pairs a caller with `role` (or superadmin) may
/// perform. Default-deny: a caller with no membership at the scope gets an
/// empty list, exactly as `authorize` would.
fn allowed_for(
    superadmin: bool,
    role: Option<Role>,
    grants: &[EffectiveGrant],
    chain: ScopeChain,
) -> Vec<String> {
    let mut allowed = Vec::new();
    for cap in CAPABILITIES {
        for action in Action::ALL {
            let Some(authority) = cap.authority(action) else {
                continue;
            };
            let permitted = match authority {
                // deployment-wide policy is never reachable through a custom
                // role, so the grants are not consulted here
                Authority::Superadmin => superadmin,
                // reaching this code path means the caller is authenticated
                Authority::Authenticated => true,
                Authority::Role(required) => {
                    superadmin
                        || role.is_some_and(|r| role_rank(r) >= role_rank(required))
                        || grants.iter().any(|g| {
                            grant_applies(g, chain)
                                && g.resource.as_deref() == Some(cap.resource)
                                && g.action.as_deref() == Some(action_key(action))
                        })
                }
            };
            if permitted {
                allowed.push(format!("{}:{}", cap.resource, action_key(action)));
            }
        }
    }
    allowed
}

pub(crate) const fn action_key(action: Action) -> &'static str {
    match action {
        Action::Read => "read",
        Action::Create => "create",
        Action::Update => "update",
        Action::Delete => "delete",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_viewer_may_read_everything_scoped_and_write_nothing() {
        let allowed = allowed_for(false, Some(Role::Viewer), &[], ScopeChain::default());
        assert!(allowed.contains(&"provider:read".to_string()));
        assert!(allowed.contains(&"route:read".to_string()));
        assert!(!allowed.iter().any(|a| a.ends_with(":create")));
        // the audit log is an admin read, not a viewer one
        assert!(!allowed.contains(&"audit_log:read".to_string()));
        // the only delete a viewer reaches is revoking their own OAuth grant,
        // which the handler narrows to the owner
        assert_eq!(
            allowed
                .iter()
                .filter(|a| a.ends_with(":delete"))
                .collect::<Vec<_>>(),
            vec!["mcp_oauth_grant:delete", "mcp_oauth_session:delete"],
        );
    }

    /// What a member adds over a viewer is exactly the set of things done *on
    /// their own behalf*: minting their own virtual key, and completing or
    /// renewing their own MCP OAuth consent. Nothing here touches another
    /// user's rows — the handlers narrow every one of them to the owner.
    #[test]
    fn a_member_may_act_on_their_own_behalf_and_nothing_more() {
        let member = allowed_for(false, Some(Role::Member), &[], ScopeChain::default());
        let viewer = allowed_for(false, Some(Role::Viewer), &[], ScopeChain::default());
        let extra: Vec<_> = member.iter().filter(|a| !viewer.contains(a)).collect();
        assert_eq!(
            extra,
            vec![
                "my_virtual_key:create",
                "mcp_oauth_grant:create",
                "mcp_oauth_session:create",
                "mcp_oauth_session:update",
            ]
        );
    }

    #[test]
    fn an_admin_writes_scoped_resources_but_not_deployment_policy() {
        let allowed = allowed_for(false, Some(Role::Admin), &[], ScopeChain::default());
        assert!(allowed.contains(&"provider:create".to_string()));
        assert!(allowed.contains(&"virtual_key:delete".to_string()));
        assert!(allowed.contains(&"audit_log:read".to_string()));
        // deployment-wide policy has no tenancy scope to be an admin of
        assert!(!allowed.iter().any(|a| a.starts_with("feature_flags:")));
        assert!(!allowed
            .iter()
            .any(|a| a.starts_with("adaptive_routing_policy:")));
        // and neither is the global account lifecycle
        assert!(!allowed.contains(&"user:update".to_string()));
        assert!(!allowed.contains(&"user:delete".to_string()));
    }

    /// Without a membership anywhere, a caller holds exactly the global
    /// read-only catalogs and nothing else — those carry no tenant's data and
    /// have no scope a membership could be held at (#766).
    #[test]
    fn no_membership_means_only_the_global_catalogs() {
        let allowed = allowed_for(false, None, &[], ScopeChain::default());
        assert_eq!(allowed, vec!["model_price:read", "model:read"]);
    }

    #[test]
    fn superadmin_holds_every_supported_action() {
        let allowed = allowed_for(true, None, &[], ScopeChain::default());
        let supported: usize = CAPABILITIES
            .iter()
            .map(|cap| {
                Action::ALL
                    .iter()
                    .filter(|&&a| cap.authority(a).is_some())
                    .count()
            })
            .sum();
        assert_eq!(allowed.len(), supported);
    }

    #[test]
    fn unsupported_actions_are_absent_for_everyone() {
        for allowed in [
            allowed_for(true, None, &[], ScopeChain::default()),
            allowed_for(false, Some(Role::Admin), &[], ScopeChain::default()),
        ] {
            // an audit log is append-only; nobody deletes one through the API
            assert!(!allowed.contains(&"audit_log:delete".to_string()));
            // and an org has no update route
            assert!(!allowed.contains(&"org:update".to_string()));
        }
    }

    #[test]
    fn the_matrix_lists_every_capability_exactly_once() {
        let mut names: Vec<&str> = CAPABILITIES.iter().map(|c| c.resource).collect();
        names.sort_unstable();
        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(names, deduped, "duplicate resource in the capability table");
    }

    // ------------------------------------------------------------------- //
    // drift guards (#704): the table is the only place a requirement lives //
    // ------------------------------------------------------------------- //

    /// Every control-plane module, as `(file name, contents)`. Compiled in, so
    /// the checks below see exactly the source that shipped.
    const MODULES: &[(&str, &str)] = &[
        ("access_control.rs", include_str!("access_control.rs")),
        ("adaptive_policy.rs", include_str!("adaptive_policy.rs")),
        (
            "adaptive_telemetry.rs",
            include_str!("adaptive_telemetry.rs"),
        ),
        ("alerting.rs", include_str!("alerting.rs")),
        ("analytics.rs", include_str!("analytics.rs")),
        ("auth.rs", include_str!("auth.rs")),
        ("auth_policy.rs", include_str!("auth_policy.rs")),
        ("cluster.rs", include_str!("cluster.rs")),
        ("client_settings.rs", include_str!("client_settings.rs")),
        (
            "compatibility_policy.rs",
            include_str!("compatibility_policy.rs"),
        ),
        ("connectors.rs", include_str!("connectors.rs")),
        ("cors.rs", include_str!("cors.rs")),
        ("crud.rs", include_str!("crud.rs")),
        ("feature_flags.rs", include_str!("feature_flags.rs")),
        ("guardrails.rs", include_str!("guardrails.rs")),
        ("health.rs", include_str!("health.rs")),
        ("invitations.rs", include_str!("invitations.rs")),
        ("lib.rs", include_str!("lib.rs")),
        ("logging_settings.rs", include_str!("logging_settings.rs")),
        ("main.rs", include_str!("main.rs")),
        ("mcp_logs.rs", include_str!("mcp_logs.rs")),
        ("model_defaults.rs", include_str!("model_defaults.rs")),
        ("mcp_oauth.rs", include_str!("mcp_oauth.rs")),
        ("mcp_oauth_flow.rs", include_str!("mcp_oauth_flow.rs")),
        ("me.rs", include_str!("me.rs")),
        ("proxy.rs", include_str!("proxy.rs")),
        ("plugins.rs", include_str!("plugins.rs")),
        ("rbac.rs", include_str!("rbac.rs")),
        ("rbac_matrix.rs", include_str!("rbac_matrix.rs")),
        ("runtime_policy.rs", include_str!("runtime_policy.rs")),
        ("scim.rs", include_str!("scim.rs")),
        ("scim_groups.rs", include_str!("scim_groups.rs")),
        ("security.rs", include_str!("security.rs")),
        ("seed.rs", include_str!("seed.rs")),
        ("sso.rs", include_str!("sso.rs")),
        ("ui_config.rs", include_str!("ui_config.rs")),
        ("ui_events.rs", include_str!("ui_events.rs")),
    ];

    /// A module the checks below skip, and why.
    const EXEMPT: &[(&str, &str)] = &[
        ("rbac.rs", "owns the role ordering and the guard itself"),
        ("rbac_matrix.rs", "is the capability table"),
        (
            "lib.rs",
            "only enumerates the roles for `GET /api/v1/roles`",
        ),
        (
            "access_control.rs",
            "parses a custom role's base role out of a request body; the role \
             is the data it manages, not the authority it checks — its own \
             guards still go through `cap!`",
        ),
    ];

    fn is_exempt(name: &str) -> bool {
        EXEMPT.iter().any(|(exempt, _)| *exempt == name)
    }

    /// Whether `source` mentions `Role::` other than as the tail of a longer
    /// path — `DecoratorRole::System` in a seed fixture is a different enum.
    fn names_a_role(source: &str) -> bool {
        source.match_indices("Role::").any(|(at, _)| {
            !source[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
    }

    /// The module list must match what is actually on disk, so a new module
    /// cannot quietly escape the checks below.
    #[test]
    fn the_module_list_covers_the_whole_control_plane() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .expect("src/ is readable")
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().to_string_lossy().into_owned();
                name.ends_with(".rs").then_some(name)
            })
            .collect();
        on_disk.sort();
        let mut listed: Vec<String> = MODULES.iter().map(|(name, _)| name.to_string()).collect();
        listed.sort();
        assert_eq!(
            listed, on_disk,
            "add the new module to MODULES so its guards are checked",
        );
    }

    /// No handler names a `Role` — every guarded route resolves its requirement
    /// from [`CAPABILITIES`] through `cap!` / `superadmin_cap!`. This is what
    /// makes `GET /api/v1/rbac/matrix` provably the rule set the guard enforces.
    #[test]
    fn no_guard_names_a_role_literal() {
        for (name, source) in MODULES {
            if is_exempt(name) {
                continue;
            }
            assert!(
                !names_a_role(source),
                "{name} names a role directly; use cap!(\"resource\", Action) instead",
            );
        }
    }

    /// And none bypasses the table with a bare superadmin guard.
    #[test]
    fn no_guard_calls_require_superadmin_directly() {
        for (name, source) in MODULES {
            if is_exempt(name) {
                continue;
            }
            assert!(
                !source.contains("require_superadmin("),
                "{name} guards on superadmin directly; use \
                 authorize_superadmin(&principal, superadmin_cap!(...)) instead",
            );
        }
    }

    /// Every row in the table is claimed by at least one guard, so the matrix
    /// cannot publish a resource nothing enforces.
    #[test]
    fn every_capability_is_named_by_a_guard() {
        for capability in CAPABILITIES {
            let scoped = format!("cap!(\"{}\",", capability.resource);
            let named = MODULES
                .iter()
                .filter(|(name, _)| *name != "rbac_matrix.rs")
                .any(|(_, source)| {
                    source
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>()
                        .contains(&scoped)
                });
            assert!(
                named,
                "no guard names '{}'; either guard a route with it or drop the row",
                capability.resource,
            );
        }
    }

    #[test]
    fn the_macro_resolves_the_same_requirement_the_matrix_publishes() {
        assert_eq!(
            cap!("provider", Read).authority,
            Authority::Role(Role::Viewer)
        );
        assert_eq!(
            cap!("provider", Delete).authority,
            Authority::Role(Role::Admin)
        );
        assert_eq!(
            cap!("feature_flags", Update).authority,
            Authority::Superadmin
        );
        assert_eq!(
            superadmin_cap!("cluster_node", Delete).authority,
            Authority::Superadmin
        );
        // and the pair travels with the requirement, which is what lets the
        // guard look it up among a caller's explicit custom grants
        assert_eq!(cap!("provider", Delete).resource, "provider");
        assert_eq!(cap!("provider", Delete).action, Action::Delete);
    }
}
