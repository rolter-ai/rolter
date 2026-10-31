//! Which tenant-owned rows of the effective config a caller may see (#3094).
//!
//! `GET /api/v1/models` and `GET /api/v1/config` list the merged store, which
//! holds every organization's providers, routes and groups. Both used to answer
//! any signed-in caller with all of it, so a member of one org could read
//! another's model names, route targets and (once the models list carried
//! them) `org_id` and `project_id`.
//!
//! The rule is the one [`visible_in_scope`](crate::crud) applies to the org
//! listings: a row with no tenancy belongs to the deployment (it came from the
//! bootstrap file) and everyone sees it; an org-wide row is seen by whoever
//! holds a role anywhere in that org; a project-scoped row needs a role that
//! reaches the project. A superadmin, the admin token and open mode see all.
//!
//! Budgets and rate limits (#3100) belong to a scope instead of a tenancy
//! (org, team, project, key, business unit, customer). Each is resolved to the
//! chain it sits under and read with the rule `GET /api/v1/budgets` applies to
//! that scope. An id that is not a uuid came from `rolter.toml`, has no tenant
//! and stays visible, like any other row without tenancy.

use std::collections::HashMap;

use rolter_core::{BudgetScope, Tenancy};
use rolter_store::{ConfigProblem, ProblemOwner};
use uuid::Uuid;

use crate::crud::{pool, ApiResult};
use crate::rbac::{reaches_org, Principal, ScopeChain, ScopeFilter};
use crate::rbac_matrix::{cap, Requirement};
use crate::ControlState;

/// What one caller may see of tenant-owned rows, loaded once per request.
pub(crate) enum TenantView {
    All,
    Scoped {
        filter: Box<ScopeFilter>,
        reach: Vec<ScopeChain>,
        // a project resolves to its chain once however many rows name it;
        // `None` is a project that no longer exists
        projects: HashMap<Uuid, Option<ScopeChain>>,
        // the same for a budget's or rate limit's scope; `None` is a scope
        // whose row no longer exists
        scopes: HashMap<(&'static str, Uuid), Option<ScopeChain>>,
    },
}

impl TenantView {
    /// `principal` is `None` where no identity exists to scope by (open mode,
    /// the admin token without a database), which sees everything.
    pub(crate) async fn load(
        state: &ControlState,
        principal: Option<&Principal>,
    ) -> ApiResult<Self> {
        let Some(principal @ (Principal::User(_) | Principal::Token(_))) = principal else {
            return Ok(Self::All);
        };
        let filter = ScopeFilter::load(state, principal, cap!("route", Read)).await?;
        let reach = filter.reach(pool(state)).await?;
        Ok(Self::Scoped {
            filter: Box::new(filter),
            reach,
            projects: HashMap::new(),
            scopes: HashMap::new(),
        })
    }

    pub(crate) fn sees_everything(&self) -> bool {
        matches!(self, Self::All)
    }

    /// Whether the caller may see a row owned by `tenancy`. A row whose owner
    /// cannot be resolved (unparsable id, deleted project) is hidden rather
    /// than failing the whole list.
    pub(crate) async fn sees(&mut self, state: &ControlState, tenancy: Option<&Tenancy>) -> bool {
        let Self::Scoped {
            filter,
            reach,
            projects,
            ..
        } = self
        else {
            return true;
        };
        let Some(tenancy) = tenancy else {
            return true;
        };
        let Ok(org) = tenancy.org_id.parse::<Uuid>() else {
            return false;
        };
        let Some(project) = tenancy.project_id.as_deref() else {
            return reaches_org(reach, org);
        };
        let Ok(project) = project.parse::<Uuid>() else {
            return false;
        };
        if let std::collections::hash_map::Entry::Vacant(slot) = projects.entry(project) {
            slot.insert(ScopeChain::from_project(pool(state), project).await.ok());
        }
        match projects.get(&project) {
            Some(Some(chain)) => chain.org == Some(org) && filter.allows(*chain),
            _ => false,
        }
    }

    /// Whether the caller may read the budget or rate limit on `scope`/`id`
    /// under `requirement`, by the rule the listing endpoints apply: the role
    /// at the scope, or for an org or team, a role held at or below it.
    pub(crate) async fn sees_scope(
        &mut self,
        state: &ControlState,
        scope: BudgetScope,
        id: &str,
        within: Option<&rolter_core::ScopeWithin>,
        requirement: Requirement,
    ) -> bool {
        let Self::Scoped {
            filter,
            reach,
            scopes,
            ..
        } = self
        else {
            return true;
        };
        let Ok(id) = id.parse::<Uuid>() else {
            return true;
        };
        if scope == BudgetScope::User {
            // a person belongs to no org: a cap on one is the caller's to see
            // only through the place it is narrowed to (#1830), and one with
            // no such place is a deployment-level row only a superadmin sees
            let narrowed = match within {
                Some(rolter_core::ScopeWithin::Org(org)) => org
                    .parse::<Uuid>()
                    .ok()
                    .map(rolter_store::postgres::repo::Within::org),
                Some(rolter_core::ScopeWithin::Project(project)) => project
                    .parse::<Uuid>()
                    .ok()
                    .map(rolter_store::postgres::repo::Within::project),
                None => None,
            };
            let Some(narrowed) = narrowed else {
                return false;
            };
            return match ScopeChain::from_cap(pool(state), "user", id, narrowed).await {
                Ok(chain) => filter.allows_as(chain, requirement),
                Err(_) => false,
            };
        }
        let scope_type = scope_type(scope);
        if let std::collections::hash_map::Entry::Vacant(slot) = scopes.entry((scope_type, id)) {
            slot.insert(
                ScopeChain::from_scope(pool(state), scope_type, id)
                    .await
                    .ok(),
            );
        }
        let Some(Some(chain)) = scopes.get(&(scope_type, id)) else {
            return false;
        };
        if filter.allows_as(*chain, requirement) {
            return true;
        }
        if !matches!(scope, BudgetScope::Org | BudgetScope::Team) {
            return false;
        }
        reach.iter().any(|held| {
            let above = match scope {
                BudgetScope::Org => held.org == Some(id),
                _ => held.team == Some(id),
            };
            above && filter.allows_as(*held, requirement)
        })
    }

    /// The lines of the store's problem list the caller may read: those about
    /// the deployment, and those about a row it can see.
    pub(crate) async fn visible_problems(
        &mut self,
        state: &ControlState,
        records: Vec<ConfigProblem>,
    ) -> Vec<String> {
        if self.sees_everything() {
            return records.into_iter().map(|p| p.message).collect();
        }
        let mut kept = Vec::with_capacity(records.len());
        for problem in records {
            let seen = match &problem.owner {
                ProblemOwner::Deployment => true,
                ProblemOwner::Tenant(tenancy) => self.sees(state, Some(tenancy)).await,
                ProblemOwner::Scope { scope, id } => {
                    self.sees_scope(state, *scope, id, None, cap!("budget", Read))
                        .await
                }
            };
            if seen {
                kept.push(problem.message);
            }
        }
        kept
    }
}

/// The `scope_type` a stored budget or rate limit names this scope by.
fn scope_type(scope: BudgetScope) -> &'static str {
    match scope {
        BudgetScope::Org => "org",
        BudgetScope::Team => "team",
        BudgetScope::Project => "project",
        BudgetScope::Key => "virtual_key",
        BudgetScope::BusinessUnit => "business_unit",
        BudgetScope::Customer => "customer",
        BudgetScope::User => "user",
        // a scope a newer release stored (#3347); no row is named this way
        BudgetScope::Unknown => "unknown",
    }
}

/// Which loaded rows of the effective config a caller sees, by position.
///
/// Positions rather than a filtered copy because `GET /api/v1/config/problems`
/// points at a row by its place in the loaded list, and the dashboard joins the
/// two reads on it: hiding a row has to renumber the records that follow.
pub(crate) struct ConfigVisibility {
    all: bool,
    providers: Vec<bool>,
    routes: Vec<bool>,
    groups: Vec<bool>,
    budgets: Vec<bool>,
    rate_limits: Vec<bool>,
}

impl ConfigVisibility {
    pub(crate) async fn compute(
        state: &ControlState,
        view: &mut TenantView,
        config: &rolter_core::GatewayConfig,
    ) -> Self {
        let mut providers = Vec::with_capacity(config.providers.len());
        for p in &config.providers {
            providers.push(view.sees(state, p.tenancy.as_ref()).await);
        }
        let mut routes = Vec::with_capacity(config.routes.len());
        for r in &config.routes {
            routes.push(view.sees(state, r.tenancy.as_ref()).await);
        }
        let mut groups = Vec::with_capacity(config.provider_groups.len());
        for g in &config.provider_groups {
            groups.push(view.sees(state, g.tenancy.as_ref()).await);
        }
        let mut budgets = Vec::with_capacity(config.budgets.len());
        for b in &config.budgets {
            budgets.push(
                view.sees_scope(
                    state,
                    b.scope,
                    &b.id,
                    b.within.as_ref(),
                    cap!("budget", Read),
                )
                .await,
            );
        }
        let mut rate_limits = Vec::with_capacity(config.rate_limits.len());
        for l in &config.rate_limits {
            rate_limits.push(
                view.sees_scope(
                    state,
                    l.scope,
                    &l.id,
                    l.within.as_ref(),
                    cap!("rate_limit", Read),
                )
                .await,
            );
        }
        Self {
            all: view.sees_everything(),
            providers,
            routes,
            groups,
            budgets,
            rate_limits,
        }
    }

    /// Drop the providers, routes, groups, budgets and rate limits the caller may not see.
    pub(crate) fn retain(&self, config: &mut rolter_core::GatewayConfig) {
        fn keep<T>(rows: &mut Vec<T>, seen: &[bool]) {
            let mut seen = seen.iter();
            rows.retain(|_| seen.next().copied().unwrap_or(false));
        }
        if self.all {
            return;
        }
        keep(&mut config.providers, &self.providers);
        keep(&mut config.routes, &self.routes);
        keep(&mut config.provider_groups, &self.groups);
        keep(&mut config.budgets, &self.budgets);
        keep(&mut config.rate_limits, &self.rate_limits);
    }

    /// The omitted-entry records of the rows the caller sees, renumbered to
    /// their place in the list [`Self::retain`] leaves. File keys are the
    /// deployment's and keep their numbers.
    pub(crate) fn renumber(
        &self,
        omitted: Vec<rolter_core::OmittedEntry>,
    ) -> Vec<rolter_core::OmittedEntry> {
        use rolter_core::OmittedKind;
        if self.all {
            return omitted;
        }
        omitted
            .into_iter()
            .filter_map(|mut entry| {
                let seen = match entry.kind {
                    OmittedKind::Provider => &self.providers,
                    OmittedKind::Route => &self.routes,
                    _ => return Some(entry),
                };
                if !seen.get(entry.index).copied().unwrap_or(false) {
                    return None;
                }
                entry.index = seen[..entry.index].iter().filter(|s| **s).count();
                Some(entry)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::{OmittedEntry, OmittedKind};

    fn entry(kind: OmittedKind, index: usize) -> OmittedEntry {
        OmittedEntry {
            kind,
            name: format!("row-{index}"),
            reason: String::new(),
            index,
        }
    }

    #[test]
    fn hiding_a_row_renumbers_the_records_after_it() {
        let seen = ConfigVisibility {
            all: false,
            providers: vec![false, true, true],
            routes: vec![true, false],
            groups: Vec::new(),
            budgets: Vec::new(),
            rate_limits: Vec::new(),
        };
        let kept = seen.renumber(vec![
            entry(OmittedKind::Provider, 0),
            entry(OmittedKind::Provider, 2),
            entry(OmittedKind::Route, 1),
            entry(OmittedKind::Route, 0),
            entry(OmittedKind::VirtualKey, 5),
        ]);
        let shape: Vec<(OmittedKind, usize)> = kept.iter().map(|e| (e.kind, e.index)).collect();
        // the hidden provider and route are gone, the visible provider moves up
        // one place and a file key keeps its number
        assert_eq!(
            shape,
            vec![
                (OmittedKind::Provider, 1),
                (OmittedKind::Route, 0),
                (OmittedKind::VirtualKey, 5)
            ]
        );
    }

    #[test]
    fn a_caller_who_sees_everything_keeps_every_record() {
        let seen = ConfigVisibility {
            all: true,
            providers: Vec::new(),
            routes: Vec::new(),
            groups: Vec::new(),
            budgets: Vec::new(),
            rate_limits: Vec::new(),
        };
        assert_eq!(seen.renumber(vec![entry(OmittedKind::Route, 4)]).len(), 1);
    }
}
