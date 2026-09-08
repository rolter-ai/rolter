//! Spend-cap enforcement backed by Redis-tracked cumulative cost.
//!
//! Each configured [`BudgetConfig`] caps the spend of a scope (org/team/project/
//! virtual-key, or the business unit / customer the key is attributed to) over a rolling [`BudgetPeriod`]. Before forwarding, the gateway
//! sums the request's applicable budgets and blocks when any one has reached its
//! limit — most-restrictive-wins across the scope chain. After the response, the
//! request's `cost_usd` is added to every applicable counter.
//!
//! Counters live in Redis so enforcement is shared across gateway replicas. When
//! no Redis url is configured — or Redis is unreachable — enforcement fails open
//! (requests pass, spend is not recorded) so a counter store outage never takes
//! the data plane down.

use std::sync::Arc;

use chrono::Utc;
use rolter_core::{BudgetConfig, BudgetScope, UnpricedPolicy};
use tokio::sync::OnceCell;

/// Scope identity of a request, taken from its virtual key. An empty string
/// means "no id at this level" and never matches a budget.
#[derive(Debug, Default, Clone)]
pub struct ScopeIds {
    pub org: String,
    pub team: String,
    pub project: String,
    pub key: String,
    /// governance dimensions the key is attributed to; empty when unattributed
    pub business_unit: String,
    pub customer: String,
}

impl ScopeIds {
    pub(crate) fn id_for(&self, scope: BudgetScope) -> &str {
        match scope {
            BudgetScope::Org => &self.org,
            BudgetScope::Team => &self.team,
            BudgetScope::Project => &self.project,
            BudgetScope::Key => &self.key,
            BudgetScope::BusinessUnit => &self.business_unit,
            BudgetScope::Customer => &self.customer,
        }
    }

    /// The budgets in `all` that apply to this request's scope chain.
    fn applicable<'a>(&self, all: &'a [BudgetConfig]) -> Vec<&'a BudgetConfig> {
        all.iter()
            .filter(|b| {
                let id = self.id_for(b.scope);
                !id.is_empty() && id == b.id
            })
            .collect()
    }
}

fn scope_str(scope: BudgetScope) -> &'static str {
    match scope {
        BudgetScope::Org => "org",
        BudgetScope::Team => "team",
        BudgetScope::Project => "project",
        BudgetScope::Key => "key",
        BudgetScope::BusinessUnit => "business_unit",
        BudgetScope::Customer => "customer",
    }
}

fn spend_key(budget: &BudgetConfig, now: chrono::DateTime<Utc>) -> String {
    format!(
        "rolter:budget:{}:{}:{}",
        scope_str(budget.scope),
        budget.id,
        budget.period.bucket(now)
    )
}

/// Enforces spend caps against Redis. Cheap to clone (shared connection).
#[derive(Clone)]
pub struct BudgetEnforcer {
    inner: Option<Arc<Inner>>,
}

struct Inner {
    client: redis::Client,
    // a shared multiplexed connection, lazily established on first use
    conn: OnceCell<redis::aio::MultiplexedConnection>,
}

impl BudgetEnforcer {
    /// A disabled enforcer: every check passes and spend is never recorded.
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Build an enforcer against `redis_url`. An invalid url disables it.
    pub fn new(redis_url: &str) -> Self {
        match redis::Client::open(redis_url) {
            Ok(client) => Self {
                inner: Some(Arc::new(Inner {
                    client,
                    conn: OnceCell::new(),
                })),
            },
            Err(err) => {
                tracing::warn!(error = %err, "invalid redis url; budget enforcement disabled");
                Self::disabled()
            }
        }
    }

    async fn connection(inner: &Inner) -> Option<redis::aio::MultiplexedConnection> {
        let conn = inner
            .conn
            .get_or_try_init(|| inner.client.get_multiplexed_async_connection())
            .await;
        match conn {
            Ok(conn) => Some(conn.clone()),
            Err(err) => {
                tracing::warn!(error = %err, "redis unavailable; budgets fail open");
                None
            }
        }
    }

    /// Return the first budget that is already at or over its limit, or `None`
    /// when the request is within budget (also when disabled or Redis is down).
    pub async fn exceeded(
        &self,
        budgets: &[BudgetConfig],
        scope: &ScopeIds,
    ) -> Option<BudgetConfig> {
        let inner = self.inner.as_ref()?;
        let applicable = scope.applicable(budgets);
        if applicable.is_empty() {
            return None;
        }
        let mut conn = Self::connection(inner).await?;
        let now = Utc::now();

        let keys: Vec<String> = applicable.iter().map(|b| spend_key(b, now)).collect();
        let spents: Vec<Option<String>> = redis::cmd("MGET")
            .arg(&keys)
            .query_async(&mut conn)
            .await
            .unwrap_or_else(|_| vec![None; applicable.len()]);
        for (budget, spent) in applicable.into_iter().zip(spents) {
            let spent = spent.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            if spent >= budget.limit_usd {
                return Some((*budget).clone());
            }
        }

        None
    }

    /// Add `cost` USD to every applicable budget counter. No-op when disabled,
    /// Redis is down, `cost` is non-positive, or no budget applies.
    pub async fn record(&self, budgets: &[BudgetConfig], scope: &ScopeIds, cost: f64) {
        if cost <= 0.0 {
            return;
        }
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        let applicable = scope.applicable(budgets);
        if applicable.is_empty() {
            return;
        }
        let Some(mut conn) = Self::connection(inner).await else {
            return;
        };
        let now = Utc::now();
        for budget in applicable {
            let key = spend_key(budget, now);
            // INCRBYFLOAT creates the key at `cost` when absent
            let incr: redis::RedisResult<f64> = redis::cmd("INCRBYFLOAT")
                .arg(&key)
                .arg(cost)
                .query_async(&mut conn)
                .await;
            if let Err(err) = incr {
                tracing::warn!(error = %err, key, "failed to record budget spend");
                continue;
            }
            if let Some(ttl) = budget.period.ttl_secs() {
                let _: redis::RedisResult<()> = redis::cmd("EXPIRE")
                    .arg(&key)
                    .arg(ttl)
                    .query_async(&mut conn)
                    .await;
            }
        }
    }
}

/// How long a `warn` policy stays quiet about a model it has already named.
///
/// The point of `warn` is to tell an operator that a budget is unenforceable,
/// which is a fact about configuration, not about this request. Logging it per
/// request would bury that fact in the volume of the traffic reporting it.
const UNPRICED_WARN_WINDOW: std::time::Duration = std::time::Duration::from_secs(3600);

/// Once-per-model-per-window dedup for the `warn` unpriced policy (#974).
#[derive(Default)]
pub struct UnpricedWarnLog {
    // a plain mutex over a small map: entries are bounded by the number of
    // distinct unpriced models, and the lock is only taken on unpriced traffic.
    // parking_lot rather than std so a panic under the lock cannot poison it
    // and silence the warning it guards
    last: parking_lot::Mutex<std::collections::HashMap<String, std::time::Instant>>,
}

impl UnpricedWarnLog {
    /// Whether `model` should be logged now, recording that it was.
    pub fn should_log(&self, model: &str) -> bool {
        let now = std::time::Instant::now();
        let mut last = self.last.lock();
        match last.get(model) {
            Some(at) if now.duration_since(*at) < UNPRICED_WARN_WINDOW => false,
            _ => {
                last.insert(model.to_string(), now);
                true
            }
        }
    }
}

/// What the gateway should do with a request whose model has no price row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnpricedDecision {
    /// forward it; its spend will be missing from every applicable budget
    Serve,
    /// refuse at admission — no upstream tokens are spent
    Refuse,
}

/// Decide what to do about an unpriced request, emitting the `warn` log as a
/// side effect (#974).
///
/// Called only when the model genuinely has no price, so `ignore` reaching
/// `Serve` without touching the warn log is the pre-#974 behaviour exactly.
pub fn decide_unpriced(
    policy: UnpricedPolicy,
    model: &str,
    warn_log: &UnpricedWarnLog,
) -> UnpricedDecision {
    match policy {
        UnpricedPolicy::Ignore => UnpricedDecision::Serve,
        UnpricedPolicy::Warn => {
            if warn_log.should_log(model) {
                tracing::warn!(
                    model,
                    "serving a model with no price row: its spend is missing from every \
                     applicable budget, so those budgets are not a complete accounting. \
                     Set a price for this model, or set unpriced_policy = \"block\"."
                );
            }
            UnpricedDecision::Serve
        }
        UnpricedPolicy::Block => UnpricedDecision::Refuse,
    }
}

/// The unpriced-traffic policy in force for one request (#996).
///
/// `deployment` is the deployment-wide setting from #974; each budget matching
/// the request's scope chain may carry its own override. The answer is the
/// **most restrictive** of all of them, which is a `max` because
/// [`UnpricedPolicy`] derives `Ord` as `Ignore < Warn < Block`.
///
/// Two consequences are deliberate:
///
/// - A budget can only tighten. A project budget set to `ignore` cannot undo an
///   org budget set to `block`, the same way it cannot raise the org's cap. A
///   cost control that a narrower scope could loosen would not be a control.
/// - The deployment setting is a floor, not merely a fallback. An operator who
///   set the deployment to `block` refuses unaccountable traffic everywhere;
///   a tenant cannot opt back into serving it.
///
/// A request that matches no budget resolves to `deployment` unchanged, which
/// is what makes this a strict extension of #974.
pub fn resolve_unpriced_policy(
    deployment: UnpricedPolicy,
    budgets: &[BudgetConfig],
    scope: &ScopeIds,
) -> UnpricedPolicy {
    scope
        .applicable(budgets)
        .into_iter()
        .filter_map(|budget| budget.unpriced_policy)
        .fold(deployment, UnpricedPolicy::max)
}

/// A prepared handle that adds a single request's cost to its applicable
/// budgets. Built on the request path (which knows the scope + snapshot), then
/// fired once from the response stream after `cost_usd` is known.
#[derive(Clone)]
pub struct SpendRecorder {
    enforcer: BudgetEnforcer,
    budgets: Arc<Vec<BudgetConfig>>,
    scope: ScopeIds,
}

impl SpendRecorder {
    pub fn new(enforcer: BudgetEnforcer, budgets: Arc<Vec<BudgetConfig>>, scope: ScopeIds) -> Self {
        Self {
            enforcer,
            budgets,
            scope,
        }
    }

    /// Record `cost` against this request's budgets.
    pub async fn record(&self, cost: f64) {
        self.enforcer.record(&self.budgets, &self.scope, cost).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::BudgetPeriod;

    fn budget(scope: BudgetScope, id: &str) -> BudgetConfig {
        BudgetConfig {
            scope,
            id: id.to_string(),
            limit_usd: 10.0,
            period: BudgetPeriod::Monthly,
            unpriced_policy: None,
        }
    }

    fn budget_with(scope: BudgetScope, id: &str, policy: UnpricedPolicy) -> BudgetConfig {
        BudgetConfig {
            unpriced_policy: Some(policy),
            ..budget(scope, id)
        }
    }

    #[test]
    fn applicable_matches_scope_chain_by_id() {
        let scope = ScopeIds {
            org: "org-1".to_string(),
            team: "team-1".to_string(),
            project: String::new(),
            key: "vk-1".to_string(),
            business_unit: "bu-1".to_string(),
            customer: String::new(),
        };
        let all = vec![
            budget(BudgetScope::Org, "org-1"),         // matches
            budget(BudgetScope::Org, "org-2"),         // wrong id
            budget(BudgetScope::BusinessUnit, "bu-1"), // matches
            budget(BudgetScope::Customer, "cust-1"),   // key is unattributed here
            budget(BudgetScope::Team, "team-1"),       // matches
            budget(BudgetScope::Project, "p-1"),       // scope empty on request
            budget(BudgetScope::Key, "vk-1"),          // matches
        ];
        let got = scope.applicable(&all);
        assert_eq!(got.len(), 4);
    }

    #[tokio::test]
    async fn disabled_enforcer_never_blocks() {
        let enforcer = BudgetEnforcer::disabled();
        let scope = ScopeIds {
            org: "org-1".to_string(),
            ..Default::default()
        };
        let budgets = vec![budget(BudgetScope::Org, "org-1")];
        // exceeded() short-circuits to None without touching Redis
        assert!(enforcer.exceeded(&budgets, &scope).await.is_none());
        enforcer.record(&budgets, &scope, 5.0).await; // no panic
    }

    // ── per-budget unpriced-policy override (#996) ─────────────────────────

    fn scope_ids() -> ScopeIds {
        ScopeIds {
            org: "org-1".into(),
            team: "team-1".into(),
            project: "proj-1".into(),
            key: "vk-1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_budget_with_no_override_inherits_the_deployment_policy() {
        let budgets = vec![budget(BudgetScope::Org, "org-1")];
        for deployment in [
            UnpricedPolicy::Ignore,
            UnpricedPolicy::Warn,
            UnpricedPolicy::Block,
        ] {
            assert_eq!(
                resolve_unpriced_policy(deployment, &budgets, &scope_ids()),
                deployment
            );
        }
    }

    #[test]
    fn a_request_matching_no_budget_gets_the_deployment_policy() {
        // a budget for someone else's org, and a scope chain that matches none
        let budgets = vec![budget_with(
            BudgetScope::Org,
            "someone-else",
            UnpricedPolicy::Block,
        )];
        assert_eq!(
            resolve_unpriced_policy(UnpricedPolicy::Warn, &budgets, &scope_ids()),
            UnpricedPolicy::Warn
        );
    }

    #[test]
    fn a_project_budget_cannot_loosen_what_the_org_blocked() {
        // the whole point of most-restrictive-wins: a narrower scope tightening
        // is honoured, a narrower scope loosening is not
        let budgets = vec![
            budget_with(BudgetScope::Org, "org-1", UnpricedPolicy::Block),
            budget_with(BudgetScope::Project, "proj-1", UnpricedPolicy::Ignore),
        ];
        assert_eq!(
            resolve_unpriced_policy(UnpricedPolicy::Ignore, &budgets, &scope_ids()),
            UnpricedPolicy::Block
        );
    }

    #[test]
    fn a_single_budget_may_tighten_the_deployment_policy() {
        let budgets = vec![
            budget_with(BudgetScope::Team, "team-1", UnpricedPolicy::Block),
            budget(BudgetScope::Org, "org-1"),
        ];
        assert_eq!(
            resolve_unpriced_policy(UnpricedPolicy::Ignore, &budgets, &scope_ids()),
            UnpricedPolicy::Block
        );
    }

    #[test]
    fn the_deployment_setting_is_a_floor_no_budget_can_lower() {
        let budgets = vec![
            budget_with(BudgetScope::Org, "org-1", UnpricedPolicy::Ignore),
            budget_with(BudgetScope::Key, "vk-1", UnpricedPolicy::Ignore),
        ];
        assert_eq!(
            resolve_unpriced_policy(UnpricedPolicy::Block, &budgets, &scope_ids()),
            UnpricedPolicy::Block
        );
    }

    /// `warn` sits between the two, so a `warn` override neither undoes a
    /// deployment `block` nor is undone by an `ignore` sibling.
    #[test]
    fn warn_orders_between_ignore_and_block() {
        let warn_on_team = vec![budget_with(
            BudgetScope::Team,
            "team-1",
            UnpricedPolicy::Warn,
        )];
        assert_eq!(
            resolve_unpriced_policy(UnpricedPolicy::Ignore, &warn_on_team, &scope_ids()),
            UnpricedPolicy::Warn
        );
        assert_eq!(
            resolve_unpriced_policy(UnpricedPolicy::Block, &warn_on_team, &scope_ids()),
            UnpricedPolicy::Block
        );
    }

    // ── unpriced-traffic policy (#974) ──────────────────────────────────────

    /// `ignore` must reproduce the pre-#974 behaviour exactly: serve, and do
    /// not even consult the warn log, so nothing is allocated or logged.
    #[test]
    fn ignore_serves_unpriced_traffic_and_stays_silent() {
        let log = UnpricedWarnLog::default();
        for _ in 0..3 {
            assert_eq!(
                decide_unpriced(UnpricedPolicy::Ignore, "gpt-4o", &log),
                UnpricedDecision::Serve
            );
        }
        assert!(
            log.last.lock().is_empty(),
            "ignore must not touch the warn log"
        );
    }

    #[test]
    fn warn_serves_unpriced_traffic() {
        let log = UnpricedWarnLog::default();
        assert_eq!(
            decide_unpriced(UnpricedPolicy::Warn, "gpt-4o", &log),
            UnpricedDecision::Serve
        );
    }

    /// The point of `warn` is to report an unenforceable budget, which is a
    /// fact about configuration. Repeating it per request would bury it in the
    /// volume of the traffic reporting it.
    #[test]
    fn warn_names_each_model_once_per_window() {
        let log = UnpricedWarnLog::default();
        assert!(log.should_log("gpt-4o"));
        assert!(!log.should_log("gpt-4o"));
        assert!(!log.should_log("gpt-4o"));
        // a different unpriced model is a different fact, and is still named
        assert!(log.should_log("deepseek-r1"));
        assert!(!log.should_log("deepseek-r1"));
    }

    /// A window that has elapsed lets the model be named again, so a gap that
    /// was never fixed keeps reporting itself.
    #[test]
    fn warn_speaks_again_once_the_window_elapses() {
        let log = UnpricedWarnLog::default();
        assert!(log.should_log("gpt-4o"));
        // rewind the recorded instant past the window rather than sleeping
        log.last.lock().insert(
            "gpt-4o".to_string(),
            std::time::Instant::now() - UNPRICED_WARN_WINDOW - std::time::Duration::from_secs(1),
        );
        assert!(log.should_log("gpt-4o"));
    }

    #[test]
    fn block_refuses_unpriced_traffic() {
        let log = UnpricedWarnLog::default();
        assert_eq!(
            decide_unpriced(UnpricedPolicy::Block, "gpt-4o", &log),
            UnpricedDecision::Refuse
        );
    }

    /// `ignore` is the default precisely so an upgrade does not start refusing
    /// traffic for the many deployments #969 measured as fully unpriced.
    #[test]
    fn the_default_policy_preserves_todays_behaviour() {
        assert_eq!(UnpricedPolicy::default(), UnpricedPolicy::Ignore);
        let log = UnpricedWarnLog::default();
        assert_eq!(
            decide_unpriced(UnpricedPolicy::default(), "anything", &log),
            UnpricedDecision::Serve
        );
    }

    #[test]
    fn spend_key_partitions_by_scope_id_and_window() {
        let b = budget(BudgetScope::Team, "team-9");
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-09T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(spend_key(&b, now), "rolter:budget:team:team-9:202607");
    }
}
