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
//! 3. else if the bearer is a valid live session → [`Principal::User`];
//! 4. else → unauthenticated (`401`).
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
use rolter_store::postgres::models::{EffectiveGrant, Membership, User};
use rolter_store::postgres::repo::{
    AccessProfileRepo, BusinessUnitRepo, CustomerRepo, MembershipRepo, ProjectRepo, SessionRepo,
    TeamRepo, UserRepo, VirtualKeyRepo,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{bearer_token, session_pepper};
use crate::crud::{pool, ApiError, ApiResult};
use crate::rbac_matrix::{action_key, Authority, Requirement};
use crate::ControlState;

/// The authenticated caller behind a control-plane mutation.
pub(crate) enum Principal {
    /// full access: the configured admin token, or a session for a
    /// `is_superadmin` user; also the open-mode default (no admin token set)
    Superadmin,
    /// a logged-in local account, authorized per-scope via [`resolve_role`]
    User(User),
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
            return Ok(Principal::Superadmin);
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
            return Ok(Principal::Superadmin);
        }

        // otherwise the bearer must be a live session token
        let token = bearer_token(&parts.headers).ok_or(ApiError::Unauthenticated)?;
        let token_hash = rolter_auth::hash_key(&session_pepper(), token);
        let pool = pool(state);
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
    let Authority::Role(required) = requirement.authority else {
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
fn user_authorized(memberships: &[Membership], chain: ScopeChain, required: Role) -> bool {
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
    let user = match principal {
        Principal::Superadmin => return Ok(()),
        Principal::User(user) => user,
    };
    let required = match requirement.authority {
        Authority::Role(required) => required,
        // holding a `Principal` at all means authentication already succeeded,
        // so there is nothing further to check
        Authority::Authenticated => return Ok(()),
        // a superadmin-only capability has no scope a membership could satisfy,
        // so a plain user is refused without touching the database
        Authority::Superadmin => return Err(ApiError::Forbidden),
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

impl Principal {
    /// The principal a signed-in account acts as: the superadmin bit makes it
    /// [`Principal::Superadmin`], exactly as the extractor decides for every
    /// other route. Handlers that take a `CurrentUser` and then authorize must
    /// build their principal here, or a superadmin with no membership is
    /// refused what any org admin may do (#1847).
    pub(crate) fn for_user(user: User) -> Self {
        if user.is_superadmin {
            Principal::Superadmin
        } else {
            Principal::User(user)
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
            Principal::Superadmin => (true, Vec::new(), Vec::new()),
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
            memberships,
            grants,
            requirement,
        })
    }

    /// The verdict [`authorize`] would give at `chain`.
    pub(crate) fn allows(&self, chain: ScopeChain) -> bool {
        if self.superadmin {
            return true;
        }
        match self.requirement.authority {
            Authority::Authenticated => true,
            Authority::Superadmin => false,
            Authority::Role(required) => {
                user_authorized(&self.memberships, chain, required)
                    || custom_grants_allow(&self.grants, chain, self.requirement)
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
        Principal::Superadmin => return Ok(true),
        Principal::User(user) => user,
    };
    Ok(effective_role(state, user, chain)
        .await?
        .is_some_and(|role| role_rank(role) >= role_rank(Role::Admin)))
}

/// Check a resource-level access profile after ordinary org authorization.
/// Org admins and superadmins can always inspect policy-bound resources.
pub(crate) async fn policy_allows(
    state: &ControlState,
    principal: &Principal,
    org_id: Uuid,
    allowed_team_ids: &[Uuid],
    minimum_role: &str,
) -> ApiResult<bool> {
    let user = match principal {
        Principal::Superadmin => return Ok(true),
        Principal::User(user) => user,
    };
    // one fetch each, reused for both the role decision and the team scan. This
    // used to load memberships, then call `effective_role`, which loaded the
    // identical set again and discarded the first (#1048)
    let memberships = MembershipRepo(pool(state)).list_for_user(user.id).await?;
    // a custom role a profile confers at the org counts here too, so an
    // operator can widen resource visibility without minting a membership
    let grants = AccessProfileRepo(pool(state))
        .effective_grants_for_user(user.id)
        .await?;
    let Some(effective_role) = effective_role_from(&memberships, &grants, ScopeChain::org(org_id))
    else {
        return Ok(false);
    };
    let Some(required_role) = parse_role(minimum_role) else {
        return Ok(false);
    };
    if role_rank(effective_role) < role_rank(required_role) {
        return Ok(false);
    }
    if effective_role == Role::Admin || allowed_team_ids.is_empty() {
        return Ok(true);
    }
    // a direct team membership answers without touching `projects` at all
    if memberships
        .iter()
        .any(|m| m.team_id.is_some_and(|id| allowed_team_ids.contains(&id)))
    {
        return Ok(true);
    }
    // otherwise resolve every project membership's team in one query rather
    // than one `ProjectRepo::get` per membership
    let project_ids: Vec<Uuid> = memberships.iter().filter_map(|m| m.project_id).collect();
    if project_ids.is_empty() {
        return Ok(false);
    }
    let team_ids = ProjectRepo(pool(state)).team_ids_for(&project_ids).await?;
    Ok(team_ids
        .iter()
        .any(|(_, team_id)| allowed_team_ids.contains(team_id)))
}

/// Require `principal` to be a superadmin. Used for global resources with no
/// org/team/project scope (the model-pricing catalog, cross-project model
/// deletion). Not called from a handler directly — handlers go through
/// [`authorize_superadmin`] so their requirement comes from the capability
/// table.
fn require_superadmin(principal: &Principal) -> ApiResult<()> {
    match principal {
        Principal::Superadmin => Ok(()),
        Principal::User(_) => Err(ApiError::Forbidden),
    }
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
        Authority::Role(_) | Authority::Authenticated => Err(ApiError::Forbidden),
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
        }
    }

    #[test]
    fn require_superadmin_allows_only_superadmin() {
        assert!(require_superadmin(&Principal::Superadmin).is_ok());
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
        assert!(
            authorize_superadmin(&Principal::Superadmin, Requirement::unscoped_superadmin())
                .is_ok()
        );
        assert!(matches!(
            authorize_superadmin(&user, Requirement::unscoped_superadmin()),
            Err(ApiError::Forbidden)
        ));
        assert!(matches!(
            authorize_superadmin(&Principal::Superadmin, scoped_viewer_requirement()),
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
