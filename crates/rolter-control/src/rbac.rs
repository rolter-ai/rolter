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
use rolter_store::postgres::models::{Membership, User};
use rolter_store::postgres::repo::{
    MembershipRepo, ProjectRepo, SessionRepo, TeamRepo, UserRepo, VirtualKeyRepo,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{bearer_token, session_pepper};
use crate::crud::{pool, ApiError, ApiResult};
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
        if user.is_superadmin {
            Ok(Principal::Superadmin)
        } else {
            Ok(Principal::User(user))
        }
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
            other => Err(ApiError::Core(rolter_core::Error::Config(format!(
                "unknown scope_type '{other}'"
            )))),
        }
    }
}

/// total order over roles: viewer < member < admin
fn role_rank(role: Role) -> u8 {
    match role {
        Role::Viewer => 0,
        Role::Member => 1,
        Role::Admin => 2,
    }
}

fn parse_role(role: &str) -> Option<Role> {
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
    if let Some(p) = m.project_id {
        return (Some(p) == project).then_some(3);
    }
    if let Some(t) = m.team_id {
        return (Some(t) == team).then_some(2);
    }
    if let Some(o) = m.org_id {
        return (Some(o) == org).then_some(1);
    }
    None
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

/// Require `principal` to hold at least `required` at `chain`. Superadmin (and
/// therefore open mode) always passes; a `User` is checked via
/// [`user_authorized`] against their memberships. Maps insufficient authority to
/// `403`.
pub(crate) async fn authorize(
    state: &ControlState,
    principal: &Principal,
    chain: ScopeChain,
    required: Role,
) -> ApiResult<()> {
    let user = match principal {
        Principal::Superadmin => return Ok(()),
        Principal::User(user) => user,
    };
    let memberships = MembershipRepo(pool(state)).list_for_user(user.id).await?;
    if user_authorized(&memberships, chain, required) {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// Require `principal` to be a superadmin. Used for global resources with no
/// org/team/project scope (the model-pricing catalog, cross-project model
/// deletion).
pub(crate) fn require_superadmin(principal: &Principal) -> ApiResult<()> {
    match principal {
        Principal::Superadmin => Ok(()),
        Principal::User(_) => Err(ApiError::Forbidden),
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
