//! Who may read the ClickHouse-backed analytics and health rollups, and which
//! rows and bodies they see (#1820).
//!
//! These routes used to be merged onto the open router with no guard of their
//! own, so anyone who could reach the port read every tenant's request logs —
//! the captured prompts and completions included — even on a deployment whose
//! CRUD API was enforcing RBAC. [`AnalyticsAccess`] is the extractor that closes
//! that. It authenticates a caller the way the CRUD API does and reduces what
//! they may see to a filter every analytics and health query binds.
//!
//! # What a caller sees
//!
//! * **Everything** — the admin token, a superadmin's session, and open mode (no
//!   admin token at all, which `open_mode` confines to loopback). These routes
//!   follow the CRUD API there rather than being stricter than the resources
//!   they summarise.
//! * **Their own tenancy** — any other signed-in user sees a request-log row
//!   when a membership or custom role they hold reaches the row's org, team or
//!   project. Viewer is the lowest role there is, so a narrower membership never
//!   takes away what a wider one showed. A row the gateway logged with no org at
//!   all is visible only to the first group.
//! * **Bodies** — the captured request and response bodies need the
//!   `request_payload` floor at the row's own scope. The role there is resolved
//!   most-specific membership first, exactly as `rbac::resolve_role` does, and
//!   raised by any custom role that reaches it, as `rbac::custom_base_role`
//!   does. A project whose admin set `payload_min_role = viewer` shows its
//!   bodies to its viewers as well. Anyone else gets the row with both bodies
//!   blanked and `payload_withheld` set, so the dashboard can say why they are
//!   missing instead of claiming payload capture is off.
//! * **Provider health** — those rows name a provider and carry no org, so a
//!   user sees the providers of the orgs where they may read providers.
//!
//! # Why a filter rather than a guard
//!
//! Every other guarded route authorizes the one scope its path names. These
//! answer across scopes — the dashboard asks for "the logs I can see", not for
//! one project's — so the question is not *may this caller read scope X* but
//! *which rows may they read*, and the answer has to reach the database as a
//! predicate. It is bound as ClickHouse query parameters and never spliced into
//! SQL: the ids are uuids from our own store, the ranks are small integers, and
//! the fragments that consume them are the constants below.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::ControlState;

/// A rank no role reaches: the floor of a capability nobody holds, and the
/// "no role here" answer the SQL computes for a row.
const NO_RANK: i8 = -1;

/// A caller's reach over the analytics tables, as the parameters the SQL
/// fragments in this module consume.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AnalyticsAccess {
    /// the admin token, a superadmin, or open mode: no filter at all
    unrestricted: bool,
    /// each membership as `(scope id, rank)`, filed under its most specific
    /// non-null scope. Most specific wins when the SQL resolves a row
    member_orgs: Vec<(String, i8)>,
    member_teams: Vec<(String, i8)>,
    member_projects: Vec<(String, i8)>,
    /// the highest rank any custom-role grant confers at each scope id. Grants
    /// only ever raise the membership answer
    granted_orgs: Vec<(String, i8)>,
    granted_teams: Vec<(String, i8)>,
    granted_projects: Vec<(String, i8)>,
    /// projects in reach whose admin shows captured bodies to viewers
    viewer_payload_projects: Vec<String>,
    /// the ranks the `analytics` and `request_payload` floors require
    read_rank: i8,
    payload_rank: i8,
    /// provider names whose health rows the caller may read
    providers: Vec<String>,
}

/// The caller's rank at a request-log row: the most specific membership that
/// reaches the row wins, as in `rbac::resolve_role`, and any custom-role grant
/// that reaches it may raise the answer, as `rbac::custom_base_role` does.
/// `-1` means no role reaches the row.
macro_rules! row_rank {
    () => {
        "greatest(\
            if(has({member_project_ids:Array(String)}, project_id), \
               {member_project_ranks:Array(Int8)}[indexOf({member_project_ids:Array(String)}, project_id)], \
            if(has({member_team_ids:Array(String)}, team_id), \
               {member_team_ranks:Array(Int8)}[indexOf({member_team_ids:Array(String)}, team_id)], \
            if(has({member_org_ids:Array(String)}, org_id), \
               {member_org_ranks:Array(Int8)}[indexOf({member_org_ids:Array(String)}, org_id)], \
               toInt8(-1)))), \
            if(has({granted_project_ids:Array(String)}, project_id), \
               {granted_project_ranks:Array(Int8)}[indexOf({granted_project_ids:Array(String)}, project_id)], \
               toInt8(-1)), \
            if(has({granted_team_ids:Array(String)}, team_id), \
               {granted_team_ranks:Array(Int8)}[indexOf({granted_team_ids:Array(String)}, team_id)], \
               toInt8(-1)), \
            if(has({granted_org_ids:Array(String)}, org_id), \
               {granted_org_ranks:Array(Int8)}[indexOf({granted_org_ids:Array(String)}, org_id)], \
               toInt8(-1)))"
    };
}

/// Whether the caller may see a `request_logs` row at all. Rows logged with no
/// org belong to no tenant, so only an unrestricted caller sees them.
pub(crate) const ROW_VISIBLE: &str = concat!(
    "({unrestricted:UInt8} = 1 or (org_id != '' and ",
    row_rank!(),
    " >= {read_rank:Int8}))"
);

/// Whether the caller may read the captured bodies of a visible row: the
/// `request_payload` floor at the row's scope, or the `analytics` floor on a
/// project that shows its bodies to viewers.
pub(crate) const PAYLOAD_VISIBLE: &str = concat!(
    "({unrestricted:UInt8} = 1 or ",
    row_rank!(),
    " >= {payload_rank:Int8} or (",
    row_rank!(),
    " >= {read_rank:Int8} and has({viewer_payload_projects:Array(String)}, project_id)))"
);

/// Whether the caller may see a `provider_health_events` row.
pub(crate) const PROVIDER_VISIBLE: &str =
    "({unrestricted:UInt8} = 1 or has({providers:Array(String)}, provider))";

impl AnalyticsAccess {
    /// No filter: the admin token, a superadmin, and open mode.
    pub(crate) fn unrestricted() -> Self {
        Self {
            unrestricted: true,
            read_rank: NO_RANK,
            payload_rank: NO_RANK,
            ..Self::default()
        }
    }

    /// The ClickHouse `param_*` bindings for [`ROW_VISIBLE`],
    /// [`PAYLOAD_VISIBLE`] and [`PROVIDER_VISIBLE`]. Every binding is always
    /// present, so a query may use any of the fragments.
    pub(crate) fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![param(
            "unrestricted",
            if self.unrestricted { "1" } else { "0" }.to_string(),
        )];
        for (name, entries) in [
            ("member_org", &self.member_orgs),
            ("member_team", &self.member_teams),
            ("member_project", &self.member_projects),
            ("granted_org", &self.granted_orgs),
            ("granted_team", &self.granted_teams),
            ("granted_project", &self.granted_projects),
        ] {
            let ids: Vec<&str> = entries.iter().map(|(id, _)| id.as_str()).collect();
            let ranks: Vec<i8> = entries.iter().map(|(_, rank)| *rank).collect();
            params.push(param(&format!("{name}_ids"), string_array(&ids)));
            params.push(param(&format!("{name}_ranks"), int_array(&ranks)));
        }
        let viewer_projects: Vec<&str> = self
            .viewer_payload_projects
            .iter()
            .map(String::as_str)
            .collect();
        params.push(param(
            "viewer_payload_projects",
            string_array(&viewer_projects),
        ));
        params.push(param("read_rank", self.read_rank.to_string()));
        params.push(param("payload_rank", self.payload_rank.to_string()));
        let providers: Vec<&str> = self.providers.iter().map(String::as_str).collect();
        params.push(param("providers", string_array(&providers)));
        params
    }
}

fn param(name: &str, value: String) -> (String, String) {
    (format!("param_{name}"), value)
}

/// A ClickHouse `Array(String)` literal in the text format a query parameter
/// is parsed with. Backslash and quote are escaped, so no value can end the
/// string early, although every value here is a uuid or a provider name from
/// our own store.
fn string_array(values: &[&str]) -> String {
    let quoted: Vec<String> = values
        .iter()
        .map(|value| format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'")))
        .collect();
    format!("[{}]", quoted.join(","))
}

fn int_array(values: &[i8]) -> String {
    let rendered: Vec<String> = values.iter().map(i8::to_string).collect();
    format!("[{}]", rendered.join(","))
}

/// The 401 every other guarded route answers with, in the same shape.
fn unauthenticated() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": {"message": "missing or invalid credentials"}})),
    )
        .into_response()
}

/// Whether the request presents the configured admin token, compared in
/// constant time so it cannot be recovered byte by byte.
fn presents_admin_token(parts: &Parts, expected: &str) -> bool {
    let presented = parts
        .headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    !presented.is_empty()
        && bool::from(subtle::ConstantTimeEq::ct_eq(
            presented.as_bytes(),
            expected.as_bytes(),
        ))
}

impl FromRequestParts<ControlState> for AnalyticsAccess {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ControlState,
    ) -> Result<Self, Self::Rejection> {
        // open mode: the CRUD API treats every caller as superadmin, and a
        // rollup is no more sensitive than the rows it summarises
        let Some(expected) = state.admin_token.as_deref() else {
            return Ok(Self::unrestricted());
        };
        // with a database, a session resolves exactly as it does for the CRUD
        // API — the admin token and superadmin sessions included
        #[cfg(feature = "postgres")]
        if state.pool.is_some() {
            use crate::rbac::Principal;
            return match Principal::from_request_parts(parts, state).await {
                Ok(Principal::Superadmin) => Ok(Self::unrestricted()),
                Ok(Principal::User(user)) => scoped::for_user(state, &user)
                    .await
                    .map_err(IntoResponse::into_response),
                Err(error) => Err(error.into_response()),
            };
        }
        // without one there are no sessions, and the admin token is the only
        // credential that exists
        if presents_admin_token(parts, expected) {
            Ok(Self::unrestricted())
        } else {
            Err(unauthenticated())
        }
    }
}

/// Turning a signed-in user's memberships into an [`AnalyticsAccess`]. Only a
/// postgres build has users.
#[cfg(feature = "postgres")]
mod scoped {
    use std::collections::HashMap;

    use rolter_store::postgres::models::{EffectiveGrant, Membership, User};
    use rolter_store::postgres::repo::{
        AccessProfileRepo, MembershipRepo, ProjectRepo, ProviderRepo,
    };
    use uuid::Uuid;

    use super::{AnalyticsAccess, NO_RANK};
    use crate::crud::{pool, ApiResult};
    use crate::rbac::{parse_role, role_rank};
    use crate::rbac_matrix::{action_key, cap, Action, Authority, Requirement};
    use crate::ControlState;

    /// The rank a requirement's floor sits at. A superadmin-only floor is
    /// [`NO_RANK`]'s opposite: no membership reaches it.
    fn floor(requirement: Requirement) -> i8 {
        match requirement.authority {
            Authority::Role(role) => role_rank(role) as i8,
            Authority::Authenticated => 0,
            Authority::Superadmin => i8::MAX,
        }
    }

    fn rank_of(role: &str) -> i8 {
        parse_role(role).map_or(NO_RANK, |role| role_rank(role) as i8)
    }

    /// Which level a scope triple is filed under: its most specific non-null
    /// id, the same reading `rbac` gives memberships and grants.
    enum Level {
        Org(Uuid),
        Team(Uuid),
        Project(Uuid),
    }

    fn level(org: Option<Uuid>, team: Option<Uuid>, project: Option<Uuid>) -> Option<Level> {
        project
            .map(Level::Project)
            .or(team.map(Level::Team))
            .or(org.map(Level::Org))
    }

    /// Per-level `id → highest rank` maps, flattened into sorted vectors so the
    /// bindings are deterministic.
    #[derive(Default)]
    struct Ranks {
        orgs: HashMap<Uuid, i8>,
        teams: HashMap<Uuid, i8>,
        projects: HashMap<Uuid, i8>,
    }

    impl Ranks {
        fn raise(&mut self, level: Level, rank: i8) {
            let (map, id) = match level {
                Level::Org(id) => (&mut self.orgs, id),
                Level::Team(id) => (&mut self.teams, id),
                Level::Project(id) => (&mut self.projects, id),
            };
            let entry = map.entry(id).or_insert(rank);
            *entry = (*entry).max(rank);
        }

        fn flatten(map: &HashMap<Uuid, i8>) -> Vec<(String, i8)> {
            let mut entries: Vec<(String, i8)> = map
                .iter()
                .map(|(id, rank)| (id.to_string(), *rank))
                .collect();
            entries.sort();
            entries
        }
    }

    /// The pure half of [`for_user`]: memberships and grants in, reach out.
    /// DB-free so the resolution rules can be tested without a database.
    pub(super) fn from_roles(
        memberships: &[Membership],
        grants: &[EffectiveGrant],
        read_rank: i8,
        payload_rank: i8,
    ) -> AnalyticsAccess {
        let mut members = Ranks::default();
        for membership in memberships {
            let rank = rank_of(&membership.role);
            if rank == NO_RANK {
                continue;
            }
            if let Some(at) = level(membership.org_id, membership.team_id, membership.project_id) {
                members.raise(at, rank);
            }
        }
        let mut granted = Ranks::default();
        let read = action_key(Action::Read);
        for grant in grants {
            // a custom role's base role, raised by an explicit grant of either
            // capability these routes are floored on — `custom_grants_allow`
            // honours both kinds of grant, so this does too
            let mut rank = rank_of(&grant.base_role);
            if grant.action.as_deref() == Some(read) {
                match grant.resource.as_deref() {
                    Some("analytics") => rank = rank.max(read_rank),
                    Some("request_payload") => rank = rank.max(payload_rank),
                    _ => {}
                }
            }
            if rank == NO_RANK {
                continue;
            }
            if let Some(at) = level(grant.org_id, grant.team_id, grant.project_id) {
                granted.raise(at, rank);
            }
        }
        AnalyticsAccess {
            unrestricted: false,
            member_orgs: Ranks::flatten(&members.orgs),
            member_teams: Ranks::flatten(&members.teams),
            member_projects: Ranks::flatten(&members.projects),
            granted_orgs: Ranks::flatten(&granted.orgs),
            granted_teams: Ranks::flatten(&granted.teams),
            granted_projects: Ranks::flatten(&granted.projects),
            viewer_payload_projects: Vec::new(),
            read_rank,
            payload_rank,
            providers: Vec::new(),
        }
    }

    /// Every scope id a membership or grant files under, per level: the reach
    /// the "shows bodies to viewers" lookup is bounded by.
    fn reach(access: &AnalyticsAccess) -> (Vec<Uuid>, Vec<Uuid>, Vec<Uuid>) {
        let ids = |a: &[(String, i8)], b: &[(String, i8)]| -> Vec<Uuid> {
            let mut ids: Vec<Uuid> = a
                .iter()
                .chain(b)
                .filter_map(|(id, _)| id.parse().ok())
                .collect();
            ids.sort();
            ids.dedup();
            ids
        };
        (
            ids(&access.member_orgs, &access.granted_orgs),
            ids(&access.member_teams, &access.granted_teams),
            ids(&access.member_projects, &access.granted_projects),
        )
    }

    /// The orgs whose providers' health the caller may read: the same org-level
    /// read `provider_health` takes through `authorize`, where only an org-level
    /// membership or grant reaches an org-scoped chain.
    pub(super) fn health_orgs(
        memberships: &[Membership],
        grants: &[EffectiveGrant],
        health_rank: i8,
    ) -> Vec<Uuid> {
        let read = action_key(Action::Read);
        let mut orgs: Vec<Uuid> = memberships
            .iter()
            .filter(|m| m.team_id.is_none() && m.project_id.is_none())
            .filter(|m| rank_of(&m.role) >= health_rank)
            .filter_map(|m| m.org_id)
            .chain(
                grants
                    .iter()
                    .filter(|g| g.team_id.is_none() && g.project_id.is_none())
                    .filter(|g| {
                        rank_of(&g.base_role) >= health_rank
                            || (g.resource.as_deref() == Some("provider_health")
                                && g.action.as_deref() == Some(read))
                    })
                    .filter_map(|g| g.org_id),
            )
            .collect();
        orgs.sort();
        orgs.dedup();
        orgs
    }

    /// A signed-in, non-superadmin user's reach, read from their memberships,
    /// their custom roles and the project settings in that reach.
    pub(super) async fn for_user(state: &ControlState, user: &User) -> ApiResult<AnalyticsAccess> {
        let pool = pool(state);
        let memberships = MembershipRepo(pool).list_for_user(user.id).await?;
        let grants = AccessProfileRepo(pool)
            .effective_grants_for_user(user.id)
            .await?;
        let mut access = from_roles(
            &memberships,
            &grants,
            floor(cap!("analytics", Read)),
            floor(cap!("request_payload", Read)),
        );
        let (orgs, teams, projects) = reach(&access);
        access.viewer_payload_projects = ProjectRepo(pool)
            .viewer_payload_projects(&orgs, &teams, &projects)
            .await?
            .into_iter()
            .map(|id| id.to_string())
            .collect();
        access.viewer_payload_projects.sort();
        let health = health_orgs(&memberships, &grants, floor(cap!("provider_health", Read)));
        access.providers = ProviderRepo(pool).names_in_orgs(&health).await?;
        access.providers.sort();
        access.providers.dedup();
        Ok(access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrestricted_caller_binds_the_flag_and_empty_sets() {
        let params = AnalyticsAccess::unrestricted().params();
        let get = |name: &str| {
            params
                .iter()
                .find(|(key, _)| key == &format!("param_{name}"))
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(get("unrestricted"), Some("1"));
        assert_eq!(get("member_org_ids"), Some("[]"));
        assert_eq!(get("member_org_ranks"), Some("[]"));
        assert_eq!(get("viewer_payload_projects"), Some("[]"));
        assert_eq!(get("providers"), Some("[]"));
    }

    #[test]
    fn every_parameter_the_fragments_name_is_bound() {
        let bound: Vec<String> = AnalyticsAccess::default()
            .params()
            .into_iter()
            .map(|(key, _)| key.trim_start_matches("param_").to_string())
            .collect();
        for fragment in [ROW_VISIBLE, PAYLOAD_VISIBLE, PROVIDER_VISIBLE] {
            let mut rest = fragment;
            while let Some(open) = rest.find('{') {
                let close = rest[open..].find(':').expect("typed parameter") + open;
                let name = &rest[open + 1..close];
                assert!(bound.iter().any(|b| b == name), "{name} is never bound");
                rest = &rest[close..];
            }
        }
    }

    #[test]
    fn a_string_array_cannot_be_closed_early() {
        assert_eq!(string_array(&["a", "b"]), "['a','b']");
        assert_eq!(string_array(&["it's"]), r"['it\'s']");
        assert_eq!(string_array(&[r"a\"]), r"['a\\']");
        assert_eq!(string_array(&[]), "[]");
        assert_eq!(int_array(&[0, -1, 2]), "[0,-1,2]");
    }

    #[cfg(feature = "postgres")]
    mod resolution {
        use chrono::Utc;
        use rolter_store::postgres::models::{EffectiveGrant, Membership};
        use uuid::Uuid;

        use super::super::scoped::{from_roles, health_orgs};

        const READ: i8 = 0;
        const PAYLOAD: i8 = 1;

        fn membership(
            org: Option<Uuid>,
            team: Option<Uuid>,
            project: Option<Uuid>,
            role: &str,
        ) -> Membership {
            Membership {
                id: Uuid::new_v4(),
                user_id: Uuid::nil(),
                org_id: org,
                team_id: team,
                project_id: project,
                role: role.to_string(),
                source: "manual".to_string(),
                created_at: Utc::now(),
            }
        }

        fn grant(
            org: Option<Uuid>,
            project: Option<Uuid>,
            base_role: &str,
            resource: Option<&str>,
        ) -> EffectiveGrant {
            EffectiveGrant {
                profile_id: Uuid::new_v4(),
                role_id: Uuid::new_v4(),
                role_slug: "custom".to_string(),
                base_role: base_role.to_string(),
                org_id: org,
                team_id: None,
                project_id: project,
                resource: resource.map(str::to_string),
                action: resource.map(|_| "read".to_string()),
            }
        }

        #[test]
        fn memberships_file_under_their_most_specific_scope() {
            let (org, team, project) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
            let access = from_roles(
                &[
                    membership(Some(org), None, None, "admin"),
                    membership(Some(org), Some(team), None, "member"),
                    membership(Some(org), Some(team), Some(project), "viewer"),
                ],
                &[],
                READ,
                PAYLOAD,
            );
            assert_eq!(access.member_orgs, vec![(org.to_string(), 2)]);
            assert_eq!(access.member_teams, vec![(team.to_string(), 1)]);
            assert_eq!(access.member_projects, vec![(project.to_string(), 0)]);
            assert!(!access.unrestricted);
        }

        #[test]
        fn two_memberships_at_one_scope_keep_the_higher_role() {
            let org = Uuid::new_v4();
            let access = from_roles(
                &[
                    membership(Some(org), None, None, "viewer"),
                    membership(Some(org), None, None, "member"),
                ],
                &[],
                READ,
                PAYLOAD,
            );
            assert_eq!(access.member_orgs, vec![(org.to_string(), 1)]);
        }

        #[test]
        fn an_explicit_payload_grant_raises_its_scope_to_the_payload_floor() {
            let (org, project) = (Uuid::new_v4(), Uuid::new_v4());
            let access = from_roles(
                &[],
                &[
                    grant(Some(org), None, "viewer", None),
                    grant(None, Some(project), "viewer", Some("request_payload")),
                    // a grant of something unrelated confers only its base role
                    grant(None, Some(project), "viewer", Some("provider")),
                ],
                READ,
                PAYLOAD,
            );
            assert_eq!(access.granted_orgs, vec![(org.to_string(), 0)]);
            assert_eq!(access.granted_projects, vec![(project.to_string(), 1)]);
            assert!(access.member_orgs.is_empty());
        }

        #[test]
        fn an_unknown_role_confers_nothing() {
            let org = Uuid::new_v4();
            let access = from_roles(
                &[membership(Some(org), None, None, "owner")],
                &[grant(Some(org), None, "owner", None)],
                READ,
                PAYLOAD,
            );
            assert!(access.member_orgs.is_empty());
            assert!(access.granted_orgs.is_empty());
        }

        #[test]
        fn provider_health_reaches_only_org_level_roles() {
            let (org, other, team) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
            let orgs = health_orgs(
                &[
                    membership(Some(org), None, None, "viewer"),
                    // a team membership does not reach an org-scoped provider
                    membership(Some(other), Some(team), None, "admin"),
                ],
                &[],
                READ,
            );
            assert_eq!(orgs, vec![org]);
        }
    }
}
