//! Budgets: spend limits and the live spend read.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use rolter_core::{BudgetConfig, BudgetPeriod, BudgetScope};
use rolter_store::postgres::models::Budget;
use rolter_store::postgres::repo::{BudgetRepo, Within};

use super::{
    explicit_null, invalid_field, log_audit, pool, publish_config_change, ApiError, ApiResult,
    Query, SafeJson,
};
use crate::rbac::{authorize, authorize_cap, reaches_org, Principal, ScopeChain, ScopeFilter};
use crate::rbac_matrix::{cap, Requirement};
use crate::ControlState;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/budgets", get(list_budgets).post(create_budget))
        .route("/api/v1/budgets/spend", get(budget_spend))
        .route("/api/v1/me/budgets", get(my_budgets))
        .route("/api/v1/orgs/{org_id}/budgets", get(list_org_budgets))
        .route("/api/v1/orgs/{org_id}/budgets/spend", get(org_budget_spend))
        .route(
            "/api/v1/budgets/{id}",
            patch(update_budget).delete(delete_budget),
        )
}

// --- budgets ---

#[derive(Deserialize)]
pub(super) struct ScopeQuery {
    pub(super) scope_type: String,
    pub(super) scope_id: Uuid,
    /// a `user` scope only (#1830): read the caps narrowed to this org
    #[serde(default)]
    pub(super) within_org_id: Option<Uuid>,
    /// a `user` scope only: read the caps narrowed to this project
    #[serde(default)]
    pub(super) within_project_id: Option<Uuid>,
}

impl ScopeQuery {
    pub(super) fn within(&self) -> Within {
        Within {
            org: self.within_org_id,
            project: self.within_project_id,
        }
    }
}

const SCOPE_TYPES: [&str; 7] = [
    "org",
    "team",
    "project",
    "virtual_key",
    "business_unit",
    "customer",
    "user",
];

/// A narrowing means something only on a `user` cap, and names one place.
pub(super) fn validate_within(scope_type: &str, within: Within) -> ApiResult<()> {
    if within.org.is_some() && within.project.is_some() {
        return Err(invalid_field(
            "within_project_id",
            "a cap is narrowed to an org or to a project, not both",
        ));
    }
    if scope_type != "user" && (within.org.is_some() || within.project.is_some()) {
        return Err(invalid_field(
            "within_org_id",
            "only a user cap can be narrowed to an org or a project",
        ));
    }
    Ok(())
}

pub(super) fn validate_scope(scope_type: &str) -> ApiResult<()> {
    if !SCOPE_TYPES.contains(&scope_type) {
        return Err(invalid_field(
            "scope_type",
            format!("scope_type must be one of {SCOPE_TYPES:?}"),
        ));
    }
    Ok(())
}

/// Authorize reading the caps set at one scope.
///
/// A cap applies to everything beneath the scope it is set on, so the caller
/// may read the rows at their own scope and at the team and org above it, which
/// is what throttles their keys (#2527). Holding the role at the scope itself
/// passes as always. Otherwise the scope must be the team or org of a place the
/// caller holds the role at; another project's rows, a sibling team's, a
/// customer's or a business unit's stay refused, and writes never come through
/// here.
pub(super) async fn authorize_scope_read(
    state: &ControlState,
    principal: &Principal,
    scope: &ScopeQuery,
    chain: ScopeChain,
    requirement: Requirement,
) -> ApiResult<()> {
    match authorize_cap(state, principal, chain, requirement).await {
        Err(ApiError::Forbidden) if matches!(scope.scope_type.as_str(), "org" | "team") => {}
        other => return other,
    }
    let filter = ScopeFilter::load(state, principal, requirement).await?;
    for held in filter.reach(pool(state)).await? {
        let above = match scope.scope_type.as_str() {
            "org" => held.org == Some(scope.scope_id),
            _ => held.team == Some(scope.scope_id),
        };
        if above && filter.allows(held) {
            return Ok(());
        }
    }
    Err(ApiError::Forbidden)
}

async fn list_budgets(
    principal: Principal,
    State(state): State<ControlState>,
    Query(scope): Query<ScopeQuery>,
) -> ApiResult<Json<Vec<Budget>>> {
    validate_scope(&scope.scope_type)?;
    let chain = ScopeChain::from_cap(
        pool(&state),
        &scope.scope_type,
        scope.scope_id,
        scope.within(),
    )
    .await?;
    authorize_scope_read(&state, &principal, &scope, chain, cap!("budget", Read)).await?;
    Ok(Json(budgets_on(&state, &scope).await?))
}

/// The budgets on the scope a query names; for a `user`, the ones narrowed to
/// exactly the place it names, since the place is what the caller was
/// authorized for.
async fn budgets_on(state: &ControlState, scope: &ScopeQuery) -> ApiResult<Vec<Budget>> {
    let repo = BudgetRepo(pool(state));
    Ok(if scope.scope_type == "user" {
        repo.list_for_user(scope.scope_id, scope.within()).await?
    } else {
        repo.list_for_scope(&scope.scope_type, scope.scope_id)
            .await?
    })
}

/// The caps of an org-wide list the caller may read, each decided by the rule
/// [`authorize_scope_read`] applies to its scope alone: the role at the row's
/// own scope, or the team or org above a place the caller holds the role at
/// (#2527). A caller with no role anywhere in the org is refused; one with a
/// role below it gets the rows they may read, which can be none.
pub(super) async fn readable_caps<T>(
    state: &ControlState,
    principal: &Principal,
    requirement: Requirement,
    org_id: Uuid,
    rows: Vec<T>,
    scope_of: impl Fn(&T) -> (&str, Uuid, Within),
) -> ApiResult<Vec<T>> {
    let filter = ScopeFilter::load(state, principal, requirement).await?;
    if filter.allows(ScopeChain::org(org_id)) {
        return Ok(rows);
    }
    let reach = filter.reach(pool(state)).await?;
    if !reaches_org(&reach, org_id) {
        return Err(ApiError::Forbidden);
    }
    let mut chains: HashMap<(String, Uuid, Within), ScopeChain> = HashMap::new();
    let mut visible = Vec::new();
    for row in rows {
        let (scope_type, scope_id, within) = scope_of(&row);
        let key = (scope_type.to_string(), scope_id, within);
        let chain = match chains.get(&key) {
            Some(chain) => *chain,
            None => {
                let chain = ScopeChain::from_cap(pool(state), scope_type, scope_id, within).await?;
                chains.insert(key, chain);
                chain
            }
        };
        let above = || {
            reach.iter().any(|held| {
                let sits_under = match scope_type {
                    "org" => held.org == Some(scope_id),
                    "team" => held.team == Some(scope_id),
                    _ => false,
                };
                sits_under && filter.allows(*held)
            })
        };
        if filter.allows(chain) || above() {
            visible.push(row);
        }
    }
    Ok(visible)
}

/// The budgets of an org the caller may read.
async fn readable_org_budgets(
    state: &ControlState,
    principal: &Principal,
    org_id: Uuid,
) -> ApiResult<Vec<Budget>> {
    let rows = BudgetRepo(pool(state)).list_in_org(org_id).await?;
    readable_caps(state, principal, cap!("budget", Read), org_id, rows, |b| {
        (
            b.scope_type.as_str(),
            b.scope_id,
            Within {
                org: b.within_org_id,
                project: b.within_project_id,
            },
        )
    })
    .await
}

/// A budget of an org-wide list, with the name of the virtual key it caps when
/// that is its scope (#3128). Every other scope is named by the dashboard from
/// the lists it already reads; keys have no org-wide list, so without this a
/// key's budget could only be told apart by the start of its id.
#[derive(Serialize)]
struct OrgBudget {
    #[serde(flatten)]
    row: Budget,
    /// the key's name, or its prefix when it has none; `null` on any other scope
    scope_name: Option<String>,
}

/// The names of the keys among `scopes`, by id (#3128).
pub(super) async fn key_names<'a>(
    state: &ControlState,
    scopes: impl Iterator<Item = (&'a str, Uuid)>,
) -> ApiResult<HashMap<Uuid, String>> {
    let ids: Vec<Uuid> = scopes
        .filter(|(scope_type, _)| *scope_type == "virtual_key")
        .map(|(_, id)| id)
        .collect();
    Ok(rolter_store::postgres::repo::VirtualKeyRepo(pool(state))
        .names(&ids)
        .await?)
}

/// Every budget the caller may read across one org, so "which caps exist here"
/// is one request instead of one per scope.
async fn list_org_budgets(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<OrgBudget>>> {
    let rows = readable_org_budgets(&state, &principal, org_id).await?;
    let names = key_names(
        &state,
        rows.iter().map(|b| (b.scope_type.as_str(), b.scope_id)),
    )
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| OrgBudget {
                scope_name: (row.scope_type == "virtual_key")
                    .then(|| names.get(&row.scope_id).cloned())
                    .flatten(),
                row,
            })
            .collect(),
    ))
}

/// Spend so far against every budget of an org the caller may read, as the
/// gateway counts it (#3128): the batch counterpart of `/budgets/spend`, which
/// reads one scope. Same rows as the org-wide list, same `status` and `null`
/// rules, one Redis round trip however many budgets there are.
async fn org_budget_spend(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<BudgetSpendResponse>> {
    let rows = readable_org_budgets(&state, &principal, org_id).await?;
    Ok(Json(spend_of(&state, &rows).await))
}

/// Spend so far against one budget, as the gateway counts it.
#[derive(Serialize, Debug, PartialEq)]
struct BudgetSpend {
    /// the budget row's id
    id: Uuid,
    /// the counter in the settlement currency, four places like `limit_usd`;
    /// `null` when the counter could not be read, never zero
    spent_usd: Option<String>,
    /// start of the next window; `null` for a budget that never resets
    resets_at: Option<String>,
}

#[derive(Serialize)]
struct BudgetSpendResponse {
    /// `ok`, `not_configured` (no Redis URL on the control plane) or
    /// `unavailable` (Redis did not answer)
    status: &'static str,
    budgets: Vec<BudgetSpend>,
}

/// How long the dashboard waits on Redis before it shows the spend as unknown.
const SPEND_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The place a `user` budget row is narrowed to, as the gateway keys its
/// counter (#1830).
fn row_within(row: &Budget) -> Option<rolter_core::ScopeWithin> {
    match (row.within_org_id, row.within_project_id) {
        (Some(org), None) => Some(rolter_core::ScopeWithin::Org(org.to_string())),
        (None, Some(project)) => Some(rolter_core::ScopeWithin::Project(project.to_string())),
        _ => None,
    }
}

/// The Redis keys of a scope's budgets, in row order, with each row's reset.
fn spend_plan(rows: &[Budget], now: DateTime<Utc>) -> Vec<(Uuid, Option<String>, String)> {
    rows.iter()
        .filter_map(|row| {
            let scope = BudgetScope::from_scope_type(&row.scope_type)?;
            // the same fallback the snapshot loader applies to a period it
            // does not recognise, so the figure is the one being enforced
            let period = BudgetPeriod::parse(&row.period).unwrap_or(BudgetPeriod::Monthly);
            let budget = BudgetConfig {
                scope,
                id: row.scope_id.to_string(),
                limit_usd: Default::default(),
                period,
                unpriced_policy: None,
                within: row_within(row),
            };
            let reset = period
                .next_reset(now)
                .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true));
            Some((row.id, reset, budget.spend_key(now)))
        })
        .collect()
}

async fn read_spend(client: &redis::Client, keys: &[String]) -> Option<Vec<Option<String>>> {
    let read = async {
        let mut conn = client.get_multiplexed_async_connection().await.ok()?;
        redis::cmd("MGET")
            .arg(keys)
            .query_async::<Vec<Option<String>>>(&mut conn)
            .await
            .ok()
    };
    tokio::time::timeout(SPEND_READ_TIMEOUT, read).await.ok()?
}

/// Spend so far against each budget on a scope (#2091).
///
/// Reads the counters the gateway enforces from Redis, under the keys and
/// windows [`BudgetConfig::spend_key`] defines, so the figure is the one a 402
/// is decided on. A counter that cannot be read comes back as `null` with a
/// `status` saying why; reporting it as zero would show a spent budget as
/// untouched.
async fn budget_spend(
    principal: Principal,
    State(state): State<ControlState>,
    Query(scope): Query<ScopeQuery>,
) -> ApiResult<Json<BudgetSpendResponse>> {
    validate_scope(&scope.scope_type)?;
    let chain = ScopeChain::from_cap(
        pool(&state),
        &scope.scope_type,
        scope.scope_id,
        scope.within(),
    )
    .await?;
    authorize_scope_read(&state, &principal, &scope, chain, cap!("budget", Read)).await?;
    let rows = budgets_on(&state, &scope).await?;
    Ok(Json(spend_of(&state, &rows).await))
}

/// The counters behind `rows`, read in one round trip. A counter that cannot
/// be read comes back `null` with a `status` saying why, never zero.
async fn spend_of(state: &ControlState, rows: &[Budget]) -> BudgetSpendResponse {
    let plan = spend_plan(rows, Utc::now());
    let unknown = |status: &'static str| BudgetSpendResponse {
        status,
        budgets: plan
            .iter()
            .map(|(id, reset, _)| BudgetSpend {
                id: *id,
                spent_usd: None,
                resets_at: reset.clone(),
            })
            .collect(),
    };
    let Some(client) = state.redis.as_ref() else {
        return unknown("not_configured");
    };
    if plan.is_empty() {
        return BudgetSpendResponse {
            status: "ok",
            budgets: Vec::new(),
        };
    }
    let keys: Vec<String> = plan.iter().map(|(_, _, key)| key.clone()).collect();
    let Some(values) = read_spend(client, &keys).await else {
        return unknown("unavailable");
    };
    let mut budgets = Vec::with_capacity(plan.len());
    for ((id, reset, _), value) in plan.iter().zip(values) {
        // an absent key is a window nothing has been spent in yet
        let spent = match value {
            None => Some(0.0),
            Some(text) => text.parse::<f64>().ok().filter(|v| v.is_finite()),
        };
        budgets.push(BudgetSpend {
            id: *id,
            spent_usd: spent.map(|v| format!("{v:.4}")),
            resets_at: reset.clone(),
        });
    }
    BudgetSpendResponse {
        status: "ok",
        budgets,
    }
}

/// One cap that counts the caller's own keys, with what is left of it (#1830).
#[derive(Serialize, Debug)]
struct MyBudget {
    id: Uuid,
    limit_usd: String,
    period: String,
    /// the org or project the cap is narrowed to; both null counts the keys
    /// the caller minted anywhere
    within_org_id: Option<Uuid>,
    within_project_id: Option<Uuid>,
    /// the counter as the gateway holds it; `null` when it could not be read,
    /// never zero
    spent_usd: Option<String>,
    resets_at: Option<String>,
}

#[derive(Serialize, Debug)]
struct MyRateLimit {
    id: Uuid,
    rpm: Option<i32>,
    tpm: Option<i32>,
    within_org_id: Option<Uuid>,
    within_project_id: Option<Uuid>,
}

#[derive(Serialize)]
struct MyBudgetsResponse {
    /// `ok`, `not_configured` or `unavailable`, as for `/api/v1/budgets/spend`
    status: &'static str,
    budgets: Vec<MyBudget>,
    rate_limits: Vec<MyRateLimit>,
}

/// The per-user caps that count the calling account's keys, and what is left of
/// them (#1830). Rows are selected by the session's own user id, never by a
/// parameter, so there is nothing to ask for another person's.
async fn my_budgets(
    current: crate::auth::CurrentUser,
    State(state): State<ControlState>,
) -> ApiResult<Json<MyBudgetsResponse>> {
    authorize(
        &state,
        &Principal::for_user(current.user.clone()),
        ScopeChain::default(),
        cap!("my_budget", Read),
    )
    .await?;
    let rows = BudgetRepo(pool(&state))
        .list_all_for_user(current.user.id)
        .await?;
    let limits = rolter_store::postgres::repo::RateLimitRepo(pool(&state))
        .list_all_for_user(current.user.id)
        .await?;
    let plan = spend_plan(&rows, Utc::now());
    let (status, values): (&'static str, Vec<Option<String>>) = match state.redis.as_ref() {
        None => ("not_configured", vec![None; plan.len()]),
        Some(_) if plan.is_empty() => ("ok", Vec::new()),
        Some(client) => {
            let keys: Vec<String> = plan.iter().map(|(_, _, key)| key.clone()).collect();
            match read_spend(client, &keys).await {
                // an absent counter is a window nothing was spent in
                Some(values) => (
                    "ok",
                    values
                        .into_iter()
                        .map(|v| match v {
                            None => Some("0.0000".to_string()),
                            Some(text) => text
                                .parse::<f64>()
                                .ok()
                                .filter(|v| v.is_finite())
                                .map(|v| format!("{v:.4}")),
                        })
                        .collect(),
                ),
                None => ("unavailable", vec![None; plan.len()]),
            }
        }
    };
    let mut budgets = Vec::with_capacity(rows.len());
    for (row, ((_, reset, _), spent)) in rows.iter().zip(plan.iter().zip(values)) {
        budgets.push(MyBudget {
            id: row.id,
            limit_usd: row.limit_usd.clone(),
            period: row.period.clone(),
            within_org_id: row.within_org_id,
            within_project_id: row.within_project_id,
            spent_usd: spent,
            resets_at: reset.clone(),
        });
    }
    Ok(Json(MyBudgetsResponse {
        status,
        budgets,
        rate_limits: limits
            .into_iter()
            .map(|l| MyRateLimit {
                id: l.id,
                rpm: l.rpm,
                tpm: l.tpm,
                within_org_id: l.within_org_id,
                within_project_id: l.within_project_id,
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBudget {
    scope_type: String,
    scope_id: Uuid,
    /// a `user` budget only: count only the keys the person minted in this
    /// org (#1830). Omit both for every key they minted, which only a
    /// superadmin may set
    #[serde(default)]
    within_org_id: Option<Uuid>,
    /// a `user` budget only: count only the keys the person minted in this
    /// project. Not together with `within_org_id`
    #[serde(default)]
    within_project_id: Option<Uuid>,
    limit_usd: String,
    #[serde(default = "default_period")]
    period: String,
    /// this budget's own answer to unpriced traffic, overriding the
    /// deployment-wide setting; omitted or null inherits it (#996)
    #[serde(default)]
    unpriced_policy: Option<String>,
}

fn default_period() -> String {
    "30d".to_string()
}

const UNPRICED_POLICIES: [&str; 3] = ["ignore", "warn", "block"];

/// Validate the optional per-budget unpriced-policy override.
///
/// The column carries the same check constraint, but a database error surfaces
/// as a 500; a caller who sends `blocked` deserves a 400 that names the three
/// values. Absent means "inherit the deployment-wide setting" and is always
/// allowed.
fn validate_unpriced_policy(value: Option<&str>) -> ApiResult<()> {
    match value {
        None => Ok(()),
        Some(policy) if UNPRICED_POLICIES.contains(&policy) => Ok(()),
        Some(_) => Err(invalid_field(
            "unpriced_policy",
            format!("unpriced_policy must be one of {UNPRICED_POLICIES:?}"),
        )),
    }
}

async fn create_budget(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<CreateBudget>,
) -> ApiResult<Json<Budget>> {
    validate_scope(&body.scope_type)?;
    let within = Within {
        org: body.within_org_id,
        project: body.within_project_id,
    };
    validate_within(&body.scope_type, within)?;
    let chain = ScopeChain::from_cap(pool(&state), &body.scope_type, body.scope_id, within).await?;
    let org_id = chain.org;
    authorize_cap(&state, &principal, chain, cap!("budget", Create)).await?;
    // the same checks an edit gets (#1903): a cap the gateway would read as
    // no cap, or as already spent, is refused here rather than stored
    validate_limit_usd(&body.limit_usd)?;
    validate_period(&body.period)?;
    validate_unpriced_policy(body.unpriced_policy.as_deref())?;
    let row = BudgetRepo(pool(&state))
        .create(
            &body.scope_type,
            body.scope_id,
            body.limit_usd.trim(),
            body.period.trim(),
            body.unpriced_policy.as_deref(),
            within,
        )
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "budget.create",
        "budget",
        row.id,
        serde_json::json!({
            "scope_type": body.scope_type,
            "scope_id": body.scope_id,
            "within_org_id": body.within_org_id,
            "within_project_id": body.within_project_id,
            "limit_usd": body.limit_usd,
            // an override changes what the gateway will serve, so the audit
            // row has to say when one was set (#996)
            "unpriced_policy": body.unpriced_policy,
        }),
    )
    .await;
    Ok(Json(row))
}

/// The caps a key is minted with (#1171), shared by the operator mint
/// (`CreateVirtualKey`) and the self-service one (`MintKey`) so the two cannot
/// disagree about what a cap is. Each is the body of the budget or rate limit
/// the key would otherwise need a second screen for; both are optional and a
/// key may carry either, both or neither.
///
/// The two bodies carry `budget` and `rate_limit` as plain fields, not through
/// `#[serde(flatten)]`, which cannot be combined with `deny_unknown_fields`.
#[derive(Default)]
pub(crate) struct KeyCapsBody {
    /// a spend cap over a window
    pub(crate) budget: Option<KeyBudgetBody>,
    /// a request and/or token cap per minute
    pub(crate) rate_limit: Option<KeyRateLimitBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeyBudgetBody {
    limit_usd: String,
    #[serde(default = "default_period")]
    period: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeyRateLimitBody {
    #[serde(default)]
    rpm: Option<i32>,
    #[serde(default)]
    tpm: Option<i32>,
}

impl KeyCapsBody {
    /// Refuse a cap the gateway would misread, before a key exists: the key's
    /// plaintext is shown once, so a mint that fails halfway cannot be retried
    /// by looking at what was made.
    pub(crate) fn validate(&self) -> ApiResult<()> {
        if let Some(budget) = &self.budget {
            validate_limit_usd(&budget.limit_usd)?;
            validate_period(&budget.period)?;
        }
        if let Some(limit) = &self.rate_limit {
            super::rate_limits::validate_rate_limit_caps(limit.rpm, limit.tpm)?;
            if limit.rpm.is_none() && limit.tpm.is_none() {
                return Err(invalid_field(
                    "rate_limit",
                    "a rate limit needs an rpm cap, a tpm cap or both",
                ));
            }
        }
        Ok(())
    }

    /// The store's view of the caps, borrowing from `self`.
    pub(crate) fn as_new(&self) -> rolter_store::postgres::repo::KeyCaps<'_> {
        use rolter_store::postgres::repo::{KeyCaps, NewKeyBudget, NewKeyRateLimit};
        KeyCaps {
            budget: self.budget.as_ref().map(|b| NewKeyBudget {
                limit_usd: b.limit_usd.trim(),
                period: b.period.trim(),
            }),
            rate_limit: self.rate_limit.as_ref().map(|l| NewKeyRateLimit {
                rpm: l.rpm,
                tpm: l.tpm,
            }),
            inherit_from: None,
        }
    }

    /// What the audit row says about the caps.
    pub(crate) fn audit(&self) -> serde_json::Value {
        serde_json::json!({
            "limit_usd": self.budget.as_ref().map(|b| b.limit_usd.trim()),
            "period": self.budget.as_ref().map(|b| b.period.trim()),
            "rpm": self.rate_limit.as_ref().and_then(|l| l.rpm),
            "tpm": self.rate_limit.as_ref().and_then(|l| l.tpm),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBudget {
    /// omit to leave the cap as it is
    #[serde(default)]
    limit_usd: Option<String>,
    /// omit to leave the window as it is
    #[serde(default)]
    period: Option<String>,
    // doubly optional so a PATCH can tell "leave the override alone" (absent)
    // from "drop it and inherit the deployment setting again" (null)
    #[serde(default, deserialize_with = "explicit_null")]
    unpriced_policy: Option<Option<String>>,
}

/// The smallest `limit_usd` the `numeric(12,4)` column cannot hold.
///
/// The column tops out at 99999999.9999, and Postgres rounds a value to four
/// places before it checks the precision, so everything from 99999999.99995
/// up overflows. Parsing text to `f64` is monotonic, so refusing any value
/// that parses to this float or above refuses every one of those. The only
/// cost is a sliver just below the boundary that parses to the same float.
const LIMIT_USD_CEILING: f64 = 99_999_999.999_95;

/// Validate a budget cap, on create and on update.
///
/// Stricter than a bare `f64` parse: `NaN` parses as a float and is a valid
/// `numeric`, but the gateway reads it back as no cap at all, and a negative
/// cap refuses every request as already exhausted. A cap
/// too large for the column would pass a parse and then fail in the store as
/// a 500 carrying the database's own message, so it is refused here as a 400
/// naming the range. A cap of zero stays legal, since it is how a scope is
/// frozen without deleting anything.
fn validate_limit_usd(value: &str) -> ApiResult<()> {
    match value.trim().parse::<f64>() {
        // a range check refuses NaN and both infinities along with the rest
        Ok(limit) if (0.0..LIMIT_USD_CEILING).contains(&limit) => Ok(()),
        _ => Err(invalid_field(
            "limit_usd",
            "limit_usd must be a finite number from 0 to 99999999.9999",
        )),
    }
}

/// Validate a budget period, on create and on update (#1902).
///
/// The column is free text, and the snapshot loader reads anything it does not
/// recognise as monthly, so a `7d` budget, which the dashboard itself used to
/// suggest, was enforced as a calendar-month cap without a word. Refusing the
/// value here, with every spelling the gateway does recognise, is what stops a
/// new row from being misread. Rows stored before this check are reported by
/// `GET /api/v1/config/problems` instead of being rewritten.
fn validate_period(value: &str) -> ApiResult<()> {
    if BudgetPeriod::parse(value).is_some() {
        return Ok(());
    }
    let accepted: Vec<&str> = BudgetPeriod::SPELLINGS
        .iter()
        .map(|(spelling, _)| *spelling)
        .collect();
    Err(invalid_field(
        "period",
        format!("period must be one of {accepted:?}; there are no rolling windows such as 7d"),
    ))
}

/// What an edit actually moved, field by field, for the audit row.
///
/// Read off the row before and the row the update returned rather than off
/// the request, so a field sent with the value it already had is not reported
/// as a change and `limit_usd` compares in the column's own `numeric(12,4)`
/// spelling instead of whatever the caller typed.
fn budget_changes(before: &Budget, after: &Budget) -> serde_json::Value {
    let mut changes = serde_json::Map::new();
    if before.limit_usd != after.limit_usd {
        changes.insert(
            "limit_usd".into(),
            serde_json::json!({"from": before.limit_usd, "to": after.limit_usd}),
        );
    }
    if before.period != after.period {
        changes.insert(
            "period".into(),
            serde_json::json!({"from": before.period, "to": after.period}),
        );
    }
    if before.unpriced_policy != after.unpriced_policy {
        changes.insert(
            "unpriced_policy".into(),
            serde_json::json!({"from": before.unpriced_policy, "to": after.unpriced_policy}),
        );
    }
    serde_json::Value::Object(changes)
}

/// Change a budget in place (#1285).
///
/// Deleting and recreating a budget to change it left the scope with no cap
/// between the two calls, and each call bumped `config_version`, so a polling
/// gateway could load exactly that gap. One update is one statement and one
/// bump. The scope is not editable: a budget moved to another scope is a
/// different budget, and the authorization below is checked against the scope
/// it already has.
async fn update_budget(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateBudget>,
) -> ApiResult<Json<Budget>> {
    let existing = BudgetRepo(pool(&state)).get(id).await?;
    let chain = ScopeChain::from_cap(
        pool(&state),
        &existing.scope_type,
        existing.scope_id,
        Within {
            org: existing.within_org_id,
            project: existing.within_project_id,
        },
    )
    .await?;
    let org_id = chain.org;
    authorize_cap(&state, &principal, chain, cap!("budget", Update)).await?;
    if let Some(limit) = &body.limit_usd {
        validate_limit_usd(limit)?;
    }
    if let Some(period) = &body.period {
        validate_period(period)?;
    }
    validate_unpriced_policy(body.unpriced_policy.as_ref().and_then(|p| p.as_deref()))?;
    if body.limit_usd.is_none() && body.period.is_none() && body.unpriced_policy.is_none() {
        // an empty patch changes nothing, so it costs no write, no config bump
        // and no audit row
        return Ok(Json(existing));
    }
    let edit = BudgetRepo(pool(&state))
        .update(
            id,
            body.limit_usd.as_deref().map(str::trim),
            body.period.as_deref().map(str::trim),
            body.unpriced_policy.as_ref().map(|p| p.as_deref()),
        )
        .await?;
    if !edit.changed {
        // every field sent already held that value, so the store wrote
        // nothing and there is no bump to announce or edit to audit
        return Ok(Json(edit.after));
    }
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "budget.update",
        "budget",
        id,
        serde_json::json!({
            "scope_type": edit.after.scope_type,
            "scope_id": edit.after.scope_id,
            "changes": budget_changes(&edit.before, &edit.after),
        }),
    )
    .await;
    Ok(Json(edit.after))
}

async fn delete_budget(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let budget = BudgetRepo(pool(&state)).get(id).await?;
    let chain = ScopeChain::from_cap(
        pool(&state),
        &budget.scope_type,
        budget.scope_id,
        Within {
            org: budget.within_org_id,
            project: budget.within_project_id,
        },
    )
    .await?;
    let org_id = chain.org;
    authorize_cap(&state, &principal, chain, cap!("budget", Delete)).await?;
    BudgetRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "budget.delete",
        "budget",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod cap_edit_tests {
    use super::super::rate_limits::{validate_rate_limit_caps, CreateRateLimit, UpdateRateLimit};
    use super::*;

    fn refused<T: std::fmt::Debug>(res: ApiResult<T>) -> bool {
        matches!(res, Err(ApiError::InvalidField { .. }))
    }

    #[test]
    fn a_limit_is_refused_outside_what_numeric_12_4_holds() {
        for ok in [
            "0",
            "0.0001",
            "500",
            " 750.5 ",
            "99999999.9999",
            "99999999.99994",
        ] {
            assert!(validate_limit_usd(ok).is_ok(), "{ok} should be accepted");
        }
        // 99999999.99995 rounds up to 100000000.0000, one digit too many for
        // the column, so it has to be refused with the rest of the overflow
        for bad in [
            "99999999.99995",
            "100000000",
            "1e9",
            "-1",
            "-0.0001",
            "NaN",
            "inf",
            "lots",
            "",
        ] {
            assert!(refused(validate_limit_usd(bad)), "{bad} should be refused");
        }
    }

    #[test]
    fn a_cap_a_patch_sets_has_to_be_positive() {
        let patch = |body: serde_json::Value| -> UpdateRateLimit {
            serde_json::from_value(body).expect("patch body")
        };
        let check = |patch: UpdateRateLimit| {
            validate_rate_limit_caps(patch.rpm.flatten(), patch.tpm.flatten())
        };
        for ok in [
            serde_json::json!({}),
            serde_json::json!({"rpm": 1}),
            serde_json::json!({"rpm": null, "tpm": 10}),
        ] {
            assert!(check(patch(ok.clone())).is_ok(), "{ok}");
        }
        for bad in [
            serde_json::json!({"rpm": 0}),
            serde_json::json!({"tpm": -5}),
            serde_json::json!({"rpm": 10, "tpm": 0}),
        ] {
            assert!(refused(check(patch(bad.clone()))), "{bad}");
        }
    }

    /// #1903: a create is held to the per-cap rule an edit already was. The
    /// "at least one cap" half is the store's, and is covered there.
    #[test]
    fn a_cap_a_create_sets_has_to_be_positive() {
        let body = |caps: serde_json::Value| -> CreateRateLimit {
            let mut body = serde_json::json!({
                "scope_type": "org",
                "scope_id": "00000000-0000-0000-0000-000000000001",
            });
            body.as_object_mut()
                .expect("object")
                .extend(caps.as_object().expect("object").clone());
            serde_json::from_value(body).expect("create body")
        };
        let check = |create: CreateRateLimit| validate_rate_limit_caps(create.rpm, create.tpm);
        for ok in [
            serde_json::json!({"rpm": 1}),
            serde_json::json!({"tpm": 1000}),
            serde_json::json!({"rpm": null, "tpm": 10}),
        ] {
            assert!(check(body(ok.clone())).is_ok(), "{ok}");
        }
        for bad in [
            serde_json::json!({"rpm": 0}),
            serde_json::json!({"tpm": -5}),
            serde_json::json!({"rpm": 60, "tpm": 0}),
        ] {
            assert!(refused(check(body(bad.clone()))), "{bad}");
        }
    }

    /// #1902: `7d` used to be stored and enforced as monthly. A period is
    /// accepted only when the gateway reads it as the window it names, and the
    /// refusal lists every spelling that would have been accepted.
    #[test]
    fn a_period_is_one_the_gateway_recognises() {
        for ok in [
            "daily", "1d", "24h", "monthly", "30d", "total", "lifetime", "all", " Daily ",
        ] {
            assert!(validate_period(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in ["7d", "weekly", "dialy", "", "  ", "1w", "month"] {
            match validate_period(bad) {
                Err(ApiError::InvalidField { message, .. }) => {
                    for (spelling, _) in BudgetPeriod::SPELLINGS {
                        assert!(
                            message.contains(spelling),
                            "{message} should name {spelling}"
                        );
                    }
                }
                other => panic!("{bad:?} should be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_explicit_null_is_told_apart_from_an_absent_cap() {
        let lifted: UpdateRateLimit =
            serde_json::from_value(serde_json::json!({"rpm": null})).expect("patch body");
        assert_eq!(lifted.rpm, Some(None), "null lifts the cap");
        assert_eq!(lifted.tpm, None, "absent leaves it alone");
    }
}
