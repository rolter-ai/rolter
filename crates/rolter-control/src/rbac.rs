//! Control-plane RBAC: principal resolution (ROL-33/ROL-34) and the
//! per-handler authorization guard the CRUD API calls before every mutation.
//!
//! ## Principal model
//!
//! A principal is resolved from `Authorization: Bearer <token>`:
//!
//! 1. if a `ROLTER_ADMIN_TOKEN` is configured and the bearer equals it
//!    (constant-time) → [`Principal::Superadmin`] (the machine/bootstrap escape
//!    hatch that keeps CI and existing deployments working unchanged);
//! 2. else if the bearer is a valid live session whose user `is_superadmin` →
//!    [`Principal::Superadmin`];
//! 3. else if the bearer is a live API token (`rolter_api_…`, #1836) →
//!    [`Principal::Token`], which holds one role at one org, team or project
//!    and never superadmin or a deployment role;
//! 4. else if the bearer is a valid live session → [`Principal::User`];
//! 5. else → unauthenticated (`401`).
//!
//! ## Open mode
//!
//! When **no** `ROLTER_ADMIN_TOKEN` is configured the control plane runs in
//! open mode (zero-cred local dev, ROL-250): today the whole CRUD API and
//! `/internal/snapshot` pass through with no auth at all. That behavior is
//! preserved exactly — the [`Principal`] extractor short-circuits to
//! [`Principal::Superadmin`] for *every* request (authenticated or not) when no
//! admin token is set, so the guard always allows. Role enforcement therefore
//! only becomes active once an operator configures an admin token; a deployment
//! that runs open today keeps running open, and one that wants per-user RBAC
//! opts in by setting the token.
//!
//! Open mode cannot be reached by accident from off the machine: the process
//! refuses to bind a non-loopback listener without an admin token unless
//! `--allow-open-mode` is passed, warns on every start, and tells the dashboard
//! to show a persistent banner while it holds. See [`crate::open_mode`].
//!
//! ## Precedence ([`resolve_role`])
//!
//! Most-specific membership wins: a project-scoped grant beats a team-scoped
//! grant beats an org-scoped grant. Within the same specificity the highest
//! role wins (admin > member > viewer). Superadmin is handled at the principal
//! level and never reaches [`resolve_role`]. The precedence logic is a pure
//! function unit-tested below without a database.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use subtle::ConstantTimeEq;

use rolter_auth::Role;
use rolter_store::postgres::models::{ApiToken, EffectiveGrant, Membership, User};
use rolter_store::postgres::repo::{
    AccessProfileRepo, BusinessUnitRepo, CustomerRepo, MembershipRepo, OrgRepo, ProjectRepo,
    SessionRepo, TeamRepo, UserRepo, VirtualKeyRepo,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{bearer_token, session_pepper};
use crate::crud::{pool, ApiError, ApiResult};
use crate::rbac_matrix::{action_key, Action, Authority, DeploymentRole, Requirement};
use crate::ControlState;

/// The authenticated caller behind a control-plane mutation.
pub(crate) enum Principal {
    /// full access: the configured admin token, or a session for a
    /// `is_superadmin` user; also the open-mode default (no admin token set)
    Superadmin {
        /// the account the request is signed in as, when it is a session for
        /// an `is_superadmin` user. `None` for the admin token and open mode,
        /// which are credentials rather than people. Authorization never reads
        /// it (a superadmin passes every check); it exists so a record of who
        /// did something can name a superadmin whose account holds no
        /// membership, which [`Principal::account_id`] hands out
        account: Option<Uuid>,
    },
    /// a logged-in local account, authorized per-scope via [`resolve_role`]
    User(User),
    /// a scoped automation credential (#1836), authorized per-scope by the same
    /// rules as a user holding its one role. It has no account behind it
    Token(Box<TokenPrincipal>),
}

/// An API token and the authority it holds, in the shapes the guard already
/// reads for a user: one membership for a built-in role, or the grants of a
/// custom role.
pub(crate) struct TokenPrincipal {
    pub(crate) token: ApiToken,
    pub(crate) memberships: Vec<Membership>,
    pub(crate) grants: Vec<EffectiveGrant>,
}

impl FromRequestParts<ControlState> for Principal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ControlState,
    ) -> Result<Self, Self::Rejection> {
        // open mode: no admin token configured → preserve today's pass-through
        // behavior by treating everyone as superadmin
        let Some(expected) = state.admin_token.as_deref() else {
            return Ok(Principal::Superadmin { account: None });
        };

        // the machine/bootstrap token: constant-time so it can't be recovered
        // byte by byte
        let presented = bearer_token(&parts.headers).unwrap_or_default();
        if !presented.is_empty()
            && bool::from(ConstantTimeEq::ct_eq(
                presented.as_bytes(),
                expected.as_bytes(),
            ))
        {
            return Ok(Principal::Superadmin { account: None });
        }

        // otherwise the bearer must be a live session token or an API token
        let token = bearer_token(&parts.headers).ok_or(ApiError::Unauthenticated)?;
        let token_hash = rolter_auth::hash_key(&session_pepper(), token);
        let pool = pool(state);
        // the prefix only picks which table to ask: a session never starts
        // with it, and an API token that is not live is refused, never retried
        // as a session
        if token.starts_with(crate::api_tokens::TOKEN_PREFIX) {
            return crate::api_tokens::resolve(pool, &token_hash)
                .await?
                .map(Principal::Token)
                .ok_or(ApiError::Unauthenticated);
        }
        let session = SessionRepo(pool)
            .find_active_by_hash(&token_hash)
            .await?
            .ok_or(ApiError::Unauthenticated)?;
        let user = UserRepo(pool).get(session.user_id).await?;
        Ok(Principal::for_user(user))
    }
}

/// The org → team → project scope chain a resource lives under. Any level may
/// be absent (`None`) for a resource that only reaches an ancestor; a most-
/// specific-non-null membership at any present level authorizes the resource.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ScopeChain {
    pub org: Option<Uuid>,
    pub team: Option<Uuid>,
    pub project: Option<Uuid>,
}

impl ScopeChain {
    /// an org-scoped resource (providers, the org itself, org-scoped budgets)
    pub(crate) fn org(org_id: Uuid) -> Self {
        Self {
            org: Some(org_id),
            team: None,
            project: None,
        }
    }

    /// walk a team up to its owning org
    pub(crate) async fn from_team(pool: &PgPool, team_id: Uuid) -> ApiResult<Self> {
        let team = TeamRepo(pool).get(team_id).await?;
        Ok(Self {
            org: Some(team.org_id),
            team: Some(team_id),
            project: None,
        })
    }

    /// walk a project up to its team and owning org
    pub(crate) async fn from_project(pool: &PgPool, project_id: Uuid) -> ApiResult<Self> {
        let project = ProjectRepo(pool).get(project_id).await?;
        let team = TeamRepo(pool).get(project.team_id).await?;
        Ok(Self {
            org: Some(team.org_id),
            team: Some(project.team_id),
            project: Some(project_id),
        })
    }

    /// The chain a budget or rate limit is authorized against, which for a
    /// `user` cap is decided by where it is narrowed to (#1830): a person
    /// belongs to no org, so the cap is the org's (or project's) to manage only
    /// through its `within`. A user cap with no `within` counts the person's keys
    /// everywhere, so it reaches no org and only a superadmin passes.
    pub(crate) async fn from_cap(
        pool: &PgPool,
        scope_type: &str,
        scope_id: Uuid,
        within: rolter_store::postgres::repo::Within,
    ) -> ApiResult<Self> {
        if scope_type != "user" {
            return Self::from_scope(pool, scope_type, scope_id).await;
        }
        // the person has to exist: the cap's id names nothing else
        UserRepo(pool).get(scope_id).await?;
        match (within.project, within.org) {
            (Some(project), _) => Self::from_project(pool, project).await,
            (None, Some(org)) => {
                OrgRepo(pool).get(org).await?;
                Ok(Self::org(org))
            }
            (None, None) => Ok(Self::default()),
        }
    }

    /// resolve the chain for a budget/rate-limit `{scope_type, scope_id}` body
    pub(crate) async fn from_scope(
        pool: &PgPool,
        scope_type: &str,
        scope_id: Uuid,
    ) -> ApiResult<Self> {
        match scope_type {
            "org" => Ok(Self::org(scope_id)),
            "team" => Self::from_team(pool, scope_id).await,
            "project" => Self::from_project(pool, scope_id).await,
            "virtual_key" => {
                let vk = VirtualKeyRepo(pool).get(scope_id).await?;
                Self::from_project(pool, vk.project_id).await
            }
            "business_unit" => {
                let unit = BusinessUnitRepo(pool).get(scope_id).await?;
                Ok(Self::org(unit.org_id))
            }
            "customer" => {
                let customer = CustomerRepo(pool).get(scope_id).await?;
                Ok(Self::org(customer.org_id))
            }
            other => Err(ApiError::Core(rolter_core::Error::Config(format!(
                "unknown scope_type '{other}'"
            )))),
        }
    }
}

/// [`authorize`] for a budget or rate limit, whose chain may be empty (#1830).
///
/// A `user` cap that is not narrowed to an org or a project counts the person's
/// keys everywhere, so no membership reaches it and the matrix has no scope to
/// resolve a role in. It is the deployment's to manage: only a superadmin
/// passes.
pub(crate) async fn authorize_cap(
    state: &ControlState,
    principal: &Principal,
    chain: ScopeChain,
    requirement: Requirement,
) -> ApiResult<()> {
    if chain.org.is_none() && chain.team.is_none() && chain.project.is_none() {
        // an api token never holds a deployment authority, so it is refused
        // here as a member is
        return match principal {
            Principal::Superadmin { .. } => Ok(()),
            Principal::User(_) | Principal::Token(_) => Err(ApiError::Forbidden),
        };
    }
    authorize(state, principal, chain, requirement).await
}

/// every built-in role, weakest first. The one place the set is enumerated, so
/// the capability matrix and the guard cannot disagree about what exists.
pub(crate) const ROLES: [Role; 3] = [Role::Viewer, Role::Member, Role::Admin];

/// total order over roles: viewer < member < admin
pub(crate) fn role_rank(role: Role) -> u8 {
    match role {
        Role::Viewer => 0,
        Role::Member => 1,
        Role::Admin => 2,
    }
}

pub(crate) fn parse_role(role: &str) -> Option<Role> {
    match role {
        "admin" => Some(Role::Admin),
        "member" => Some(Role::Member),
        "viewer" => Some(Role::Viewer),
        _ => None,
    }
}

/// how specifically a membership matches a scope chain: `3` project, `2` team,
/// `1` org, `None` if the membership does not apply to the chain. A membership
/// is interpreted by its most-specific non-null scope id (matching the schema's
/// invariant), so a project-scoped grant only matches the chain's project, a
/// team-scoped grant only its team, and an org-scoped grant only its org.
fn membership_specificity(
    m: &Membership,
    org: Option<Uuid>,
    team: Option<Uuid>,
    project: Option<Uuid>,
) -> Option<u8> {
    scope_specificity(
        m.org_id,
        m.team_id,
        m.project_id,
        ScopeChain { org, team, project },
    )
}

/// How specifically a scope triple `(org, team, project)` matches `chain`, by
/// its most-specific non-null id. Shared by memberships and by the access
/// profiles of #534, so a custom role composed at team scope inherits down to a
/// project exactly the way a team membership does.
fn scope_specificity(
    org: Option<Uuid>,
    team: Option<Uuid>,
    project: Option<Uuid>,
    chain: ScopeChain,
) -> Option<u8> {
    if let Some(p) = project {
        return (Some(p) == chain.project).then_some(3);
    }
    if let Some(t) = team {
        return (Some(t) == chain.team).then_some(2);
    }
    if let Some(o) = org {
        return (Some(o) == chain.org).then_some(1);
    }
    None
}

/// Whether a profile composition reaches `chain` at all.
pub(crate) fn grant_applies(grant: &EffectiveGrant, chain: ScopeChain) -> bool {
    scope_specificity(grant.org_id, grant.team_id, grant.project_id, chain).is_some()
}

/// The highest built-in role the applicable custom roles confer at `chain`,
/// via their `base_role`. Purely additive: it can raise the membership answer
/// and never lower it, which is what keeps a deployment with no custom roles
/// behaving exactly as it did before.
pub(crate) fn custom_base_role(grants: &[EffectiveGrant], chain: ScopeChain) -> Option<Role> {
    grants
        .iter()
        .filter(|grant| grant_applies(grant, chain))
        .filter_map(|grant| parse_role(&grant.base_role))
        .max_by_key(|&role| role_rank(role))
}

/// Whether an applicable custom role names this exact `(resource, action)`.
///
/// A superadmin requirement is never satisfiable this way: deployment-wide
/// settings have no org for an org admin to define a role in, so letting a
/// custom grant reach them would be a privilege-escalation path out of the
/// tenant.
pub(crate) fn custom_grants_allow(
    grants: &[EffectiveGrant],
    chain: ScopeChain,
    requirement: Requirement,
) -> bool {
    let (Authority::Role(required) | Authority::RoleOrDeployment(required, _)) =
        requirement.authority
    else {
        return false;
    };
    let action = action_key(requirement.action);
    grants.iter().any(|grant| {
        grant_applies(grant, chain)
            && (parse_role(&grant.base_role).is_some_and(|r| role_rank(r) >= role_rank(required))
                || (grant.resource.as_deref() == Some(requirement.resource)
                    && grant.action.as_deref() == Some(action)))
    })
}

/// Resolve the effective role for a user's memberships at a scope chain.
///
/// Most-specific membership wins; ties break to the highest role. Returns
/// `None` when no membership applies to the chain. Pure and DB-free so the
/// precedence rules can be unit-tested exhaustively.
pub(crate) fn resolve_role(
    memberships: &[Membership],
    org: Option<Uuid>,
    team: Option<Uuid>,
    project: Option<Uuid>,
) -> Option<Role> {
    let mut best: Option<(u8, Role)> = None;
    for m in memberships {
        let Some(specificity) = membership_specificity(m, org, team, project) else {
            continue;
        };
        let Some(role) = parse_role(&m.role) else {
            continue;
        };
        let candidate = (specificity, role);
        best = match best {
            // higher specificity always wins; equal specificity → higher role
            Some((bs, br))
                if bs > specificity || (bs == specificity && role_rank(br) >= role_rank(role)) =>
            {
                Some((bs, br))
            }
            _ => Some(candidate),
        };
    }
    best.map(|(_, role)| role)
}

/// Pure authorization decision for a `User` principal: do `memberships` grant
/// at least `required` at `chain`? Default-deny — a chain the memberships do not
/// reach (or too low a role) is `false`. DB-free so the whole `(role, scope,
/// action)` matrix can be exhaustively unit-tested; [`authorize`] wraps this
/// with the superadmin short-circuit and the membership fetch.
pub(crate) fn user_authorized(
    memberships: &[Membership],
    chain: ScopeChain,
    required: Role,
) -> bool {
    match resolve_role(memberships, chain.org, chain.team, chain.project) {
        Some(role) => role_rank(role) >= role_rank(required),
        None => false,
    }
}

/// Require `principal` to satisfy `requirement` at `chain`.
///
/// `requirement` comes from the capability table via [`crate::rbac_matrix::cap`]
/// — never a literal — so the rule the guard enforces is the rule
/// `GET /api/v1/rbac/matrix` publishes. Superadmin (and therefore open mode)
/// always passes; a `User` is checked via [`user_authorized`] against their
/// memberships. Maps insufficient authority to `403`.
pub(crate) async fn authorize(
    state: &ControlState,
    principal: &Principal,
    chain: ScopeChain,
    requirement: Requirement,
) -> ApiResult<()> {
    #[cfg(debug_assertions)]
    if let Some(problem) = crate::rbac_matrix::guard_shape_mismatch(requirement, chain) {
        panic!("{problem}");
    }
    let user = match principal {
        Principal::Superadmin { .. } => return Ok(()),
        Principal::User(user) => user,
        Principal::Token(held) => return authorize_token(held, chain, requirement),
    };
    let required = match requirement.authority {
        Authority::Role(required) => required,
        // a deployment role passes at every scope; without one the scoped role
        // is checked below, exactly as for `Authority::Role` (#3362)
        Authority::RoleOrDeployment(required, roles) => {
            if holds_deployment_role(principal, roles) {
                return Ok(());
            }
            required
        }
        // holding a `Principal` at all means authentication already succeeded,
        // so there is nothing further to check
        Authority::Authenticated => return Ok(()),
        // a superadmin-only capability has no scope a membership could satisfy,
        // so a plain user is refused without touching the database
        Authority::Superadmin => return Err(ApiError::Forbidden),
        // nor does one a deployment role reaches: the role is on the account,
        // read from the row the extractor already loaded
        Authority::Deployment(roles) => {
            return if holds_deployment_role(principal, roles) {
                Ok(())
            } else {
                Err(ApiError::Forbidden)
            };
        }
    };
    let memberships = MembershipRepo(pool(state)).list_for_user(user.id).await?;
    if user_authorized(&memberships, chain, required) {
        return Ok(());
    }
    // only now the configurable half (#534). Custom grants are strictly
    // additive, so this second query runs exactly on the path that was already
    // about to answer 403 — a deployment with no custom roles pays one empty
    // lookup on requests it was going to refuse anyway, and nothing at all on
    // the ones it allows.
    let grants = AccessProfileRepo(pool(state))
        .effective_grants_for_user(user.id)
        .await?;
    if custom_grants_allow(&grants, chain, requirement) {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// What an API token never changes, whatever role it holds: who may sign in,
/// and with which roles. A token that could grant a membership or a custom
/// role, send an invitation, reset someone's password or register an identity
/// provider would leave access behind that outlives its own revocation, so
/// these writes stay with people. Reading them is unaffected.
pub(crate) const TOKEN_REFUSED_WRITES: &[&str] = &[
    "access_profile_assignment",
    "api_token",
    "custom_role",
    "invitation",
    "ldap_directory",
    "ldap_group_mapping",
    "membership",
    "org_auth_policy",
    "password_reset",
    "scim_group_mapping",
    "scim_token",
    "sso_group_mapping",
    "sso_provider",
    "user",
];

/// [`authorize`] for an API token: the same rules as a user holding its role,
/// over the memberships and grants it carries, with no query. A deployment-wide
/// capability, superadmin-only or reached by a deployment role, is never a
/// token's: neither its membership nor a custom role can express it.
/// Nor is a write to anything in [`TOKEN_REFUSED_WRITES`].
fn authorize_token(
    held: &TokenPrincipal,
    chain: ScopeChain,
    requirement: Requirement,
) -> ApiResult<()> {
    if requirement.action != Action::Read && TOKEN_REFUSED_WRITES.contains(&requirement.resource) {
        return Err(ApiError::Forbidden);
    }
    let required = match requirement.authority {
        // a token holds no deployment role, so the scoped half is all it has
        Authority::Role(required) | Authority::RoleOrDeployment(required, _) => required,
        Authority::Authenticated => return Ok(()),
        Authority::Superadmin | Authority::Deployment(_) => return Err(ApiError::Forbidden),
    };
    if user_authorized(&held.memberships, chain, required)
        || custom_grants_allow(&held.grants, chain, requirement)
    {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

impl Principal {
    /// The API token behind this principal, when the request carried one, so a
    /// record of who acted can name it (#1836)
    pub(crate) fn api_token(&self) -> Option<&ApiToken> {
        match self {
            Principal::Token(held) => Some(&held.token),
            Principal::Superadmin { .. } | Principal::User(_) => None,
        }
    }

    /// The principal a signed-in account acts as: the superadmin bit makes it
    /// [`Principal::Superadmin`], exactly as the extractor decides for every
    /// other route. Handlers that take a `CurrentUser` and then authorize must
    /// build their principal here, or a superadmin with no membership is
    /// refused what any org admin may do (#1847).
    pub(crate) fn for_user(user: User) -> Self {
        if user.is_superadmin {
            Principal::Superadmin {
                account: Some(user.id),
            }
        } else {
            Principal::User(user)
        }
    }

    /// The signed-in account behind this principal, for a record of who did
    /// something: a plain user's own id, and a superadmin's when the request
    /// carried their session. `None` means the admin token or open mode, where
    /// no person is signed in to name.
    ///
    /// Matching `Principal::User` alone loses a superadmin session, because
    /// [`Principal::for_user`] turns that account into
    /// [`Principal::Superadmin`] and the account holds no membership to find it
    /// by (#2813). Every audit row and `created_by`-style column is filled from
    /// here (#2844), so there is one rule for who acted rather than a `match`
    /// per handler.
    pub(crate) fn account_id(&self) -> Option<Uuid> {
        match self {
            Principal::Superadmin { account } => *account,
            Principal::User(user) => Some(user.id),
            // a token is a credential, not a person: its records name it
            // through [`Principal::api_token`]
            Principal::Token(_) => None,
        }
    }

    /// The deployment role a plain account holds (#1834). A superadmin holds
    /// none because it needs none: it passes every deployment capability. An
    /// unrecognised stored value reads as no role, never as a wider one.
    pub(crate) fn deployment_role(&self) -> Option<DeploymentRole> {
        match self {
            Principal::Superadmin { .. } | Principal::Token(_) => None,
            Principal::User(user) => user
                .deployment_role
                .as_deref()
                .and_then(DeploymentRole::parse),
        }
    }
}

/// One caller's authority, checked against many scopes, with the memberships
/// and grants fetched once.
///
/// A listing answers with the rows the caller may read instead of refusing
/// the whole list because the caller holds no role at its parent (#1846,
/// #1850). A project member lists their own team and project; a team admin
/// lists their own team's people. Each row is decided by exactly the rule
/// [`authorize`] would apply to it alone.
pub(crate) struct ScopeFilter {
    superadmin: bool,
    deployment_role: Option<DeploymentRole>,
    memberships: Vec<Membership>,
    grants: Vec<EffectiveGrant>,
    requirement: Requirement,
}

impl ScopeFilter {
    pub(crate) async fn load(
        state: &ControlState,
        principal: &Principal,
        requirement: Requirement,
    ) -> ApiResult<Self> {
        let (superadmin, memberships, grants) = match principal {
            Principal::Superadmin { .. } => (true, Vec::new(), Vec::new()),
            Principal::Token(held) => (false, held.memberships.clone(), held.grants.clone()),
            Principal::User(user) => (
                false,
                MembershipRepo(pool(state)).list_for_user(user.id).await?,
                AccessProfileRepo(pool(state))
                    .effective_grants_for_user(user.id)
                    .await?,
            ),
        };
        Ok(Self {
            superadmin,
            deployment_role: principal.deployment_role(),
            memberships,
            grants,
            requirement,
        })
    }

    /// The verdict [`authorize`] would give at `chain`.
    pub(crate) fn allows(&self, chain: ScopeChain) -> bool {
        self.allows_as(chain, self.requirement)
    }

    /// [`Self::allows`] against another capability, reusing the memberships
    /// and grants already loaded, for a caller that asks about several kinds
    /// of resource in one request
    pub(crate) fn allows_as(&self, chain: ScopeChain, requirement: Requirement) -> bool {
        #[cfg(debug_assertions)]
        if let Some(problem) = crate::rbac_matrix::guard_shape_mismatch(requirement, chain) {
            panic!("{problem}");
        }
        if self.superadmin {
            return true;
        }
        match requirement.authority {
            Authority::Authenticated => true,
            Authority::Superadmin => false,
            Authority::Deployment(roles) => self
                .deployment_role
                .is_some_and(|held| roles.contains(&held)),
            Authority::RoleOrDeployment(required, roles) => {
                self.deployment_role
                    .is_some_and(|held| roles.contains(&held))
                    || user_authorized(&self.memberships, chain, required)
                    || custom_grants_allow(&self.grants, chain, requirement)
            }
            Authority::Role(required) => {
                user_authorized(&self.memberships, chain, required)
                    || custom_grants_allow(&self.grants, chain, requirement)
            }
        }
    }

    pub(crate) fn is_superadmin(&self) -> bool {
        self.superadmin
    }

    /// The full org → team → project chain of every scope the caller holds a
    /// membership or a profile role at: what they can navigate to, even where
    /// the role sits below the level being listed.
    pub(crate) async fn reach(&self, pool: &PgPool) -> ApiResult<Vec<ScopeChain>> {
        let scopes = self
            .memberships
            .iter()
            .map(|m| (m.org_id, m.team_id, m.project_id))
            .chain(
                self.grants
                    .iter()
                    .map(|g| (g.org_id, g.team_id, g.project_id)),
            );
        let mut reach = Vec::new();
        for (org, team, project) in scopes {
            reach.push(match (org, team, project) {
                (_, _, Some(project)) => ScopeChain::from_project(pool, project).await?,
                (_, Some(team), None) => ScopeChain::from_team(pool, team).await?,
                (Some(org), None, None) => ScopeChain::org(org),
                (None, None, None) => continue,
            });
        }
        Ok(reach)
    }
}

/// Whether anything in `reach` sits inside `org`.
pub(crate) fn reaches_org(reach: &[ScopeChain], org: Uuid) -> bool {
    reach.iter().any(|chain| chain.org == Some(org))
}

/// The built-in role a user effectively holds at `chain`: the best of their
/// memberships and the `base_role` of any custom role a profile confers there.
pub(crate) async fn effective_role(
    state: &ControlState,
    user: &User,
    chain: ScopeChain,
) -> ApiResult<Option<Role>> {
    let memberships = MembershipRepo(pool(state)).list_for_user(user.id).await?;
    let grants = AccessProfileRepo(pool(state))
        .effective_grants_for_user(user.id)
        .await?;
    Ok(effective_role_from(&memberships, &grants, chain))
}

/// [`effective_role`] over data the caller already holds.
///
/// Split out so a caller that has already fetched a user's memberships and
/// grants does not re-fetch them. `policy_allows` did exactly that — it loaded
/// memberships, then called `effective_role`, which loaded the identical set
/// again and threw the first away (#1048). Keeping the decision itself in one
/// pure function is what makes the two entry points impossible to drift apart.
pub(crate) fn effective_role_from(
    memberships: &[Membership],
    grants: &[EffectiveGrant],
    chain: ScopeChain,
) -> Option<Role> {
    best_role(
        resolve_role(memberships, chain.org, chain.team, chain.project),
        custom_base_role(grants, chain),
    )
}

/// The higher of two optional roles.
pub(crate) fn best_role(a: Option<Role>, b: Option<Role>) -> Option<Role> {
    match (a, b) {
        (Some(a), Some(b)) if role_rank(b) > role_rank(a) => Some(b),
        (Some(a), _) => Some(a),
        (None, b) => b,
    }
}

/// Whether `principal` holds admin at `chain`, as a question rather than a
/// guard. Used where the answer selects *what* a caller sees (their own rows
/// vs. the whole org) after a weaker guard has already run.
pub(crate) async fn holds_admin(
    state: &ControlState,
    principal: &Principal,
    chain: ScopeChain,
) -> ApiResult<bool> {
    let user = match principal {
        Principal::Superadmin { .. } => return Ok(true),
        Principal::User(user) => user,
        Principal::Token(held) => {
            return Ok(effective_role_from(&held.memberships, &held.grants, chain)
                .is_some_and(|role| role_rank(role) >= role_rank(Role::Admin)))
        }
    };
    Ok(effective_role(state, user, chain)
        .await?
        .is_some_and(|role| role_rank(role) >= role_rank(Role::Admin)))
}

/// The caller's authorization data for resource-level access profiles in one
/// org, fetched once per request (#1098).
///
/// A list filtered by access profile used to call [`policy_allows`] once per
/// row, and every call re-fetched the same caller's memberships and grants:
/// 40 skills cost 80 identical queries. Building this once and asking
/// [`PolicyContext::allows`] per row makes the filter pure over data already in
/// hand. It is deliberately not a cache: it lives for one request, so an
/// authorization change still takes effect on the next one.
pub(crate) enum PolicyContext {
    /// sees every policy-bound resource
    Superadmin,
    User {
        /// the built-in role the caller effectively holds at the org, from
        /// memberships and any custom role a profile confers there
        role: Option<Role>,
        /// teams the caller belongs to directly, or through a project
        /// membership's team
        team_ids: Vec<Uuid>,
    },
}

impl PolicyContext {
    /// Fetch what [`Self::allows`] needs: memberships and grants once, plus one
    /// batched project-to-team lookup when the caller has project memberships.
    pub(crate) async fn load(
        state: &ControlState,
        principal: &Principal,
        org_id: Uuid,
    ) -> ApiResult<Self> {
        let (memberships, grants) = match principal {
            Principal::Superadmin { .. } => return Ok(Self::Superadmin),
            Principal::Token(held) => (held.memberships.clone(), held.grants.clone()),
            Principal::User(user) => (
                MembershipRepo(pool(state)).list_for_user(user.id).await?,
                // a custom role a profile confers at the org counts here too, so an
                // operator can widen resource visibility without minting a membership
                AccessProfileRepo(pool(state))
                    .effective_grants_for_user(user.id)
                    .await?,
            ),
        };
        let role = effective_role_from(&memberships, &grants, ScopeChain::org(org_id));
        let mut team_ids: Vec<Uuid> = memberships.iter().filter_map(|m| m.team_id).collect();
        // a role below every requirement never reaches the team scan, so the
        // project lookup is skipped for it
        let project_ids: Vec<Uuid> = memberships.iter().filter_map(|m| m.project_id).collect();
        if role.is_some_and(|role| role != Role::Admin) && !project_ids.is_empty() {
            let resolved = ProjectRepo(pool(state)).team_ids_for(&project_ids).await?;
            team_ids.extend(resolved.into_iter().map(|(_, team_id)| team_id));
        }
        Ok(Self::User { role, team_ids })
    }

    /// Whether a resource bound to `allowed_team_ids` and `minimum_role` is
    /// visible. Org admins and superadmins can always inspect policy-bound
    /// resources; an empty team list restricts by role alone.
    pub(crate) fn allows(&self, allowed_team_ids: &[Uuid], minimum_role: &str) -> bool {
        let (role, team_ids) = match self {
            Self::Superadmin => return true,
            Self::User { role, team_ids } => (role, team_ids),
        };
        let Some(role) = *role else {
            return false;
        };
        let Some(required_role) = parse_role(minimum_role) else {
            return false;
        };
        if role_rank(role) < role_rank(required_role) {
            return false;
        }
        if role == Role::Admin || allowed_team_ids.is_empty() {
            return true;
        }
        team_ids.iter().any(|id| allowed_team_ids.contains(id))
    }
}

/// Check a resource-level access profile after ordinary org authorization,
/// for a single resource. A list builds one [`PolicyContext`] and asks it per
/// row instead.
pub(crate) async fn policy_allows(
    state: &ControlState,
    principal: &Principal,
    org_id: Uuid,
    allowed_team_ids: &[Uuid],
    minimum_role: &str,
) -> ApiResult<bool> {
    Ok(PolicyContext::load(state, principal, org_id)
        .await?
        .allows(allowed_team_ids, minimum_role))
}

/// Require `principal` to be a superadmin. Used for global resources with no
/// org/team/project scope (the model-pricing catalog, cross-project model
/// deletion). Not called from a handler directly — handlers go through
/// [`authorize_superadmin`] so their requirement comes from the capability
/// table.
fn require_superadmin(principal: &Principal) -> ApiResult<()> {
    match principal {
        Principal::Superadmin { .. } => Ok(()),
        Principal::User(_) | Principal::Token(_) => Err(ApiError::Forbidden),
    }
}

/// Whether `principal` is a superadmin or holds one of `roles` (#1834).
fn holds_deployment_role(principal: &Principal, roles: &[DeploymentRole]) -> bool {
    matches!(principal, Principal::Superadmin { .. })
        || principal
            .deployment_role()
            .is_some_and(|held| roles.contains(&held))
}

/// Whether `principal` is a superadmin or a deployment operator (#1834): the
/// callers who run every alert rule and channel, whichever org owns it, since
/// alerting was the operator's before it was delegated to scoped admins (#1829).
pub(crate) fn is_deployment_operator(principal: &Principal) -> bool {
    holds_deployment_role(principal, &[DeploymentRole::Operator])
}

/// Guard for a capability with no tenancy scope to resolve a role in.
///
/// `requirement` comes from [`crate::rbac_matrix::superadmin_cap`], which is a
/// compile error unless the table records the pair as superadmin-only; the
/// scoped arm is therefore unreachable and denies rather than widening.
pub(crate) fn authorize_superadmin(
    principal: &Principal,
    requirement: Requirement,
) -> ApiResult<()> {
    match requirement.authority {
        Authority::Superadmin => require_superadmin(principal),
        Authority::Deployment(roles) if holds_deployment_role(principal, roles) => Ok(()),
        Authority::Deployment(_)
        | Authority::Role(_)
        | Authority::RoleOrDeployment(..)
        | Authority::Authenticated => Err(ApiError::Forbidden),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn membership(
        org: Option<Uuid>,
        team: Option<Uuid>,
        project: Option<Uuid>,
        role: &str,
    ) -> Membership {
        Membership {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            org_id: org,
            team_id: team,
            project_id: project,
            role: role.to_string(),
            source: "manual".into(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn a_policy_context_filters_by_role_then_team() {
        let team = Uuid::new_v4();
        let other = Uuid::new_v4();
        let member = PolicyContext::User {
            role: Some(Role::Member),
            team_ids: vec![team],
        };
        assert!(member.allows(&[], "member"));
        assert!(member.allows(&[team], "member"));
        assert!(!member.allows(&[other], "member"));
        assert!(!member.allows(&[], "admin"));
        assert!(!member.allows(&[], "not-a-role"));

        let admin = PolicyContext::User {
            role: Some(Role::Admin),
            team_ids: vec![],
        };
        assert!(admin.allows(&[other], "member"));

        let outsider = PolicyContext::User {
            role: None,
            team_ids: vec![team],
        };
        assert!(!outsider.allows(&[team], "member"));
        assert!(PolicyContext::Superadmin.allows(&[other], "admin"));
    }

    #[test]
    fn no_matching_membership_resolves_to_none() {
        let org = Uuid::new_v4();
        let other_org = Uuid::new_v4();
        let ms = vec![membership(Some(other_org), None, None, "admin")];
        assert_eq!(resolve_role(&ms, Some(org), None, None), None);
        // empty set is None too
        assert_eq!(resolve_role(&[], Some(org), None, None), None);
    }

    #[test]
    fn org_membership_authorizes_org_scope() {
        let org = Uuid::new_v4();
        let ms = vec![membership(Some(org), None, None, "member")];
        assert_eq!(resolve_role(&ms, Some(org), None, None), Some(Role::Member));
    }

    #[test]
    fn project_beats_org_even_when_org_role_is_higher() {
        let org = Uuid::new_v4();
        let team = Uuid::new_v4();
        let project = Uuid::new_v4();
        // org admin, but only viewer on the specific project → most-specific wins
        let ms = vec![
            membership(Some(org), None, None, "admin"),
            membership(None, None, Some(project), "viewer"),
        ];
        assert_eq!(
            resolve_role(&ms, Some(org), Some(team), Some(project)),
            Some(Role::Viewer)
        );
    }

    #[test]
    fn team_beats_org() {
        let org = Uuid::new_v4();
        let team = Uuid::new_v4();
        let ms = vec![
            membership(Some(org), None, None, "admin"),
            membership(None, Some(team), None, "member"),
        ];
        assert_eq!(
            resolve_role(&ms, Some(org), Some(team), None),
            Some(Role::Member)
        );
    }

    #[test]
    fn project_beats_team() {
        let team = Uuid::new_v4();
        let project = Uuid::new_v4();
        let ms = vec![
            membership(None, Some(team), None, "admin"),
            membership(None, None, Some(project), "member"),
        ];
        assert_eq!(
            resolve_role(&ms, None, Some(team), Some(project)),
            Some(Role::Member)
        );
    }

    #[test]
    fn higher_role_wins_within_same_specificity() {
        let org = Uuid::new_v4();
        // two org-level grants for the same user; the highest wins regardless
        // of ordering
        let ms = vec![
            membership(Some(org), None, None, "viewer"),
            membership(Some(org), None, None, "admin"),
            membership(Some(org), None, None, "member"),
        ];
        assert_eq!(resolve_role(&ms, Some(org), None, None), Some(Role::Admin));

        let ms_rev = vec![
            membership(Some(org), None, None, "admin"),
            membership(Some(org), None, None, "viewer"),
        ];
        assert_eq!(
            resolve_role(&ms_rev, Some(org), None, None),
            Some(Role::Admin)
        );
    }

    #[test]
    fn membership_for_other_scope_does_not_leak() {
        let org = Uuid::new_v4();
        let team = Uuid::new_v4();
        let other_team = Uuid::new_v4();
        // a team-scoped grant on a *different* team must not authorize this team
        let ms = vec![membership(None, Some(other_team), None, "admin")];
        assert_eq!(resolve_role(&ms, Some(org), Some(team), None), None);
    }

    #[test]
    fn org_grant_does_not_match_when_chain_has_no_org() {
        let org = Uuid::new_v4();
        let project = Uuid::new_v4();
        // org-scoped membership, but the resource chain carries only a project
        // (no org resolved) → no match
        let ms = vec![membership(Some(org), None, None, "admin")];
        assert_eq!(resolve_role(&ms, None, None, Some(project)), None);
    }

    #[test]
    fn unknown_role_string_is_ignored() {
        let org = Uuid::new_v4();
        let ms = vec![membership(Some(org), None, None, "superuser")];
        assert_eq!(resolve_role(&ms, Some(org), None, None), None);
    }

    #[test]
    fn role_rank_is_ordered() {
        assert!(role_rank(Role::Admin) > role_rank(Role::Member));
        assert!(role_rank(Role::Member) > role_rank(Role::Viewer));
    }

    // ------------------------------------------------------------------- //
    // exhaustive authorization matrix (#621): every (granted, required)    //
    // pair at every scope, with the accent on the *deny* cases            //
    // ------------------------------------------------------------------- //

    const ROLES: [Role; 3] = [Role::Viewer, Role::Member, Role::Admin];

    fn role_str(r: Role) -> &'static str {
        match r {
            Role::Viewer => "viewer",
            Role::Member => "member",
            Role::Admin => "admin",
        }
    }

    /// Core invariant: a user is authorized **iff** their granted role at the
    /// scope ranks at least as high as the required role. Checked for every
    /// (granted, required) pair at org, team, and project scope — 3×3×3 = 27
    /// cases, of which the sub-rank pairs are the security-critical denials.
    #[test]
    fn grant_authorizes_iff_rank_is_sufficient() {
        let org = Uuid::new_v4();
        let team = Uuid::new_v4();
        let project = Uuid::new_v4();
        let scopes: [(&str, Membership, ScopeChain); 3] = [
            (
                "org",
                membership(Some(org), None, None, "placeholder"),
                ScopeChain::org(org),
            ),
            (
                "team",
                membership(Some(org), Some(team), None, "placeholder"),
                ScopeChain {
                    org: Some(org),
                    team: Some(team),
                    project: None,
                },
            ),
            (
                "project",
                membership(Some(org), Some(team), Some(project), "placeholder"),
                ScopeChain {
                    org: Some(org),
                    team: Some(team),
                    project: Some(project),
                },
            ),
        ];

        for (scope_name, template, chain) in scopes {
            for granted in ROLES {
                let mut m = template.clone();
                m.role = role_str(granted).to_string();
                for required in ROLES {
                    let allowed = user_authorized(std::slice::from_ref(&m), chain, required);
                    let expected = role_rank(granted) >= role_rank(required);
                    assert_eq!(
                        allowed, expected,
                        "{scope_name}: granted={granted:?} required={required:?} \
                         → {allowed} (expected {expected})",
                    );
                }
            }
        }
    }

    /// Default-deny: with no membership at all, every required role at every
    /// scope is refused. A regression to default-*allow* would flip these.
    #[test]
    fn no_membership_denies_everything() {
        let org = Uuid::new_v4();
        let team = Uuid::new_v4();
        let project = Uuid::new_v4();
        let chains = [
            ScopeChain::org(org),
            ScopeChain {
                org: Some(org),
                team: Some(team),
                project: None,
            },
            ScopeChain {
                org: Some(org),
                team: Some(team),
                project: Some(project),
            },
        ];
        for chain in chains {
            for required in ROLES {
                assert!(
                    !user_authorized(&[], chain, required),
                    "empty memberships must deny required={required:?}",
                );
            }
        }
    }

    /// Property: no membership whose effective role ranks *below* the required
    /// role is ever authorized, for any combination of scope grants a user
    /// might plausibly hold. This is the anti-privilege-escalation guard —
    /// `resolve_role` must never round a lower grant up.
    #[test]
    fn lower_grant_never_escalates() {
        let org = Uuid::new_v4();
        let team = Uuid::new_v4();
        let project = Uuid::new_v4();
        let chain = ScopeChain {
            org: Some(org),
            team: Some(team),
            project: Some(project),
        };
        // enumerate a grant at each of the three levels independently
        for level in 0..3 {
            for granted in ROLES {
                let m = match level {
                    0 => membership(Some(org), None, None, role_str(granted)),
                    1 => membership(Some(org), Some(team), None, role_str(granted)),
                    _ => membership(Some(org), Some(team), Some(project), role_str(granted)),
                };
                for required in ROLES {
                    if role_rank(granted) < role_rank(required) {
                        assert!(
                            !user_authorized(std::slice::from_ref(&m), chain, required),
                            "escalation: level={level} granted={granted:?} \
                             required={required:?} was allowed",
                        );
                    }
                }
            }
        }
    }

    /// Sibling isolation: an admin grant on team A / project A never authorizes
    /// team B / project B at any required role.
    #[test]
    fn sibling_scopes_are_isolated() {
        let org = Uuid::new_v4();
        let team_a = Uuid::new_v4();
        let team_b = Uuid::new_v4();
        let project_a = Uuid::new_v4();
        let project_b = Uuid::new_v4();

        // team-A admin cannot touch team-B
        let team_grant = vec![membership(Some(org), Some(team_a), None, "admin")];
        let team_b_chain = ScopeChain {
            org: Some(org),
            team: Some(team_b),
            project: None,
        };
        for required in ROLES {
            assert!(
                !user_authorized(&team_grant, team_b_chain, required),
                "team-A admin leaked to team-B for required={required:?}",
            );
        }

        // project-A admin cannot touch project-B
        let proj_grant = vec![membership(
            Some(org),
            Some(team_a),
            Some(project_a),
            "admin",
        )];
        let project_b_chain = ScopeChain {
            org: Some(org),
            team: Some(team_b),
            project: Some(project_b),
        };
        for required in ROLES {
            assert!(
                !user_authorized(&proj_grant, project_b_chain, required),
                "project-A admin leaked to project-B for required={required:?}",
            );
        }
    }

    /// Most-specific downgrade actually *reduces* authority: a user who is org
    /// Admin but only project Viewer can still perform org-Admin actions on the
    /// org, yet is held to Viewer on that project (no Admin/Member write there).
    #[test]
    fn specific_downgrade_reduces_authority_at_that_scope() {
        let org = Uuid::new_v4();
        let team = Uuid::new_v4();
        let project = Uuid::new_v4();
        let ms = vec![
            membership(Some(org), None, None, "admin"),
            membership(Some(org), Some(team), Some(project), "viewer"),
        ];
        let org_chain = ScopeChain::org(org);
        let project_chain = ScopeChain {
            org: Some(org),
            team: Some(team),
            project: Some(project),
        };
        // org scope: still Admin
        assert!(user_authorized(&ms, org_chain, Role::Admin));
        // project scope: only Viewer — Member/Admin actions denied
        assert!(user_authorized(&ms, project_chain, Role::Viewer));
        assert!(!user_authorized(&ms, project_chain, Role::Member));
        assert!(!user_authorized(&ms, project_chain, Role::Admin));
    }

    // ------------------------------------------------------------------- //
    // require_superadmin: global (unscoped) resources                      //
    // ------------------------------------------------------------------- //

    fn user_with_superadmin(flag: bool) -> User {
        User {
            id: Uuid::new_v4(),
            email: "u@e2e.test".into(),
            password_hash: None,
            is_superadmin: flag,
            deactivated_at: None,
            created_at: Utc::now(),
            display_name: None,
            bio: None,
            deployment_role: None,
        }
    }

    /// A superadmin session keeps the account it came from, so a record of who
    /// acted can name a superadmin who holds no membership (#2813); the admin
    /// token and open mode are credentials, not people, and name nobody.
    #[test]
    fn account_id_names_every_signed_in_account_and_no_token() {
        let superadmin = user_with_superadmin(true);
        let superadmin_id = superadmin.id;
        assert_eq!(
            Principal::for_user(superadmin).account_id(),
            Some(superadmin_id)
        );

        let member = user_with_superadmin(false);
        let member_id = member.id;
        assert_eq!(Principal::for_user(member).account_id(), Some(member_id));

        assert_eq!(Principal::Superadmin { account: None }.account_id(), None);
    }

    fn user_with_deployment_role(role: Option<&str>) -> Principal {
        let mut user = user_with_superadmin(false);
        user.deployment_role = role.map(str::to_string);
        Principal::for_user(user)
    }

    /// a deployment role opens exactly the capabilities that name it, through
    /// the same guard every deployment handler already calls (#1834)
    #[test]
    fn a_deployment_role_passes_only_the_capabilities_that_name_it() {
        use crate::rbac_matrix::superadmin_cap;
        let operator = user_with_deployment_role(Some("operator"));
        let auditor = user_with_deployment_role(Some("security_auditor"));
        let plain = user_with_deployment_role(None);

        let drain = superadmin_cap!("cluster_node", Update);
        let read_security = superadmin_cap!("security_settings", Read);
        let write_security = superadmin_cap!("security_settings", Update);
        let grant = superadmin_cap!("deployment_role", Update);

        assert!(authorize_superadmin(&operator, drain).is_ok());
        assert!(authorize_superadmin(&operator, read_security).is_err());
        assert!(authorize_superadmin(&auditor, read_security).is_ok());
        assert!(authorize_superadmin(&auditor, write_security).is_err());
        assert!(authorize_superadmin(&auditor, drain).is_err());
        assert!(authorize_superadmin(&plain, drain).is_err());
        // neither role can grant one
        assert!(authorize_superadmin(&operator, grant).is_err());
        assert!(authorize_superadmin(&auditor, grant).is_err());
        // the admin token and a superadmin account pass every one of them
        let token = Principal::Superadmin { account: None };
        assert!(authorize_superadmin(&token, drain).is_ok());
        assert!(authorize_superadmin(&token, grant).is_ok());
    }

    /// a token holds one role at one scope: nothing deployment-wide, whatever
    /// the role, and nothing outside its own chain (#1836)
    #[test]
    fn a_token_reaches_neither_a_deployment_capability_nor_another_scope() {
        use crate::rbac_matrix::{cap, superadmin_cap};
        let (org, team, project, other) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let now = Utc::now();
        let held = TokenPrincipal {
            token: ApiToken {
                id: Uuid::new_v4(),
                org_id: org,
                team_id: Some(team),
                project_id: Some(project),
                name: "ci".into(),
                role: Some("admin".into()),
                custom_role_id: None,
                token_hash: String::new(),
                token_prefix: String::new(),
                expires_at: None,
                created_by: None,
                created_at: now,
                rotated_at: None,
                last_used_at: None,
                revoked_at: None,
            },
            memberships: vec![Membership {
                id: Uuid::new_v4(),
                user_id: Uuid::new_v4(),
                org_id: Some(org),
                team_id: Some(team),
                project_id: Some(project),
                role: "admin".into(),
                source: "api_token".into(),
                created_at: now,
            }],
            grants: Vec::new(),
        };
        let principal = Principal::Token(Box::new(held));
        let at = |project| ScopeChain {
            org: Some(org),
            team: Some(team),
            project: Some(project),
        };
        // an admin token passes a project guard at its own project only
        let guard = |p: &Principal, project| match p {
            Principal::Token(held) => authorize_token(held, at(project), cap!("route", Create)),
            _ => unreachable!(),
        };
        assert!(guard(&principal, project).is_ok());
        assert!(guard(&principal, other).is_err());
        // admin is the top role, and still no deployment or superadmin row
        assert!(
            authorize_superadmin(&principal, superadmin_cap!("feature_flags", Update)).is_err()
        );
        assert!(
            authorize_superadmin(&principal, superadmin_cap!("security_settings", Read)).is_err()
        );
        assert!(authorize_superadmin(&principal, superadmin_cap!("user", Update)).is_err());
        assert_eq!(principal.deployment_role(), None);
        assert_eq!(principal.account_id(), None);
        assert!(principal.api_token().is_some());
        // nor does it grant access to anyone, though it may read who has it
        let Principal::Token(held) = &principal else {
            unreachable!()
        };
        // (a person who is this project's admin may grant in it, #3074)
        for write in [cap!("membership", Create), cap!("invitation", Create)] {
            assert!(
                authorize_token(held, at(project), write).is_err(),
                "{write:?}"
            );
        }
        assert!(authorize_token(held, at(project), cap!("membership", Read)).is_ok());
    }

    #[test]
    fn an_unrecognised_stored_deployment_role_holds_nothing() {
        let stale = user_with_deployment_role(Some("superadmin"));
        assert_eq!(stale.deployment_role(), None);
        assert!(authorize_superadmin(
            &stale,
            crate::rbac_matrix::superadmin_cap!("cluster_node", Update)
        )
        .is_err());
        // a superadmin holds no deployment role and needs none
        assert_eq!(
            Principal::Superadmin { account: None }.deployment_role(),
            None
        );
    }

    #[test]
    fn require_superadmin_allows_only_superadmin() {
        assert!(require_superadmin(&Principal::Superadmin { account: None }).is_ok());
        // a plain user — even one flagged is_superadmin at the row level is a
        // `Principal::Superadmin`, so the `User` variant is always non-super
        let user = Principal::User(user_with_superadmin(false));
        assert!(matches!(
            require_superadmin(&user),
            Err(ApiError::Forbidden)
        ));
    }

    /// A scoped requirement built by hand, which `superadmin_cap!` refuses at
    /// compile time — only the deployment guard's defensive branch needs one.
    fn scoped_viewer_requirement() -> crate::rbac_matrix::Requirement {
        crate::rbac_matrix::Requirement {
            resource: "provider",
            action: crate::rbac_matrix::Action::Read,
            authority: crate::rbac_matrix::Authority::Role(Role::Viewer),
        }
    }

    /// The deployment guard passes a superadmin through and refuses everyone
    /// else — including, defensively, a scoped requirement that `superadmin_cap!`
    /// would have rejected at compile time.
    #[test]
    fn authorize_superadmin_denies_users_and_scoped_requirements() {
        let user = Principal::User(user_with_superadmin(false));
        assert!(authorize_superadmin(
            &Principal::Superadmin { account: None },
            Requirement::unscoped_superadmin()
        )
        .is_ok());
        assert!(matches!(
            authorize_superadmin(&user, Requirement::unscoped_superadmin()),
            Err(ApiError::Forbidden)
        ));
        assert!(matches!(
            authorize_superadmin(
                &Principal::Superadmin { account: None },
                scoped_viewer_requirement()
            ),
            Err(ApiError::Forbidden)
        ));
    }

    // ------------------------------------------------------------------- //
    // error shape: a denial is 403 + the OpenAI-style JSON envelope        //
    // ------------------------------------------------------------------- //

    #[tokio::test]
    async fn forbidden_renders_403_openai_envelope() {
        use axum::body::to_bytes;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;

        let resp = ApiError::Forbidden.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"error": {"message": "insufficient role for this resource"}}),
        );
    }
}
