//! Spend-cap enforcement backed by Redis-tracked cumulative cost.
//!
//! Each configured [`BudgetConfig`] caps the spend of a scope (org/team/project/
//! virtual-key) over a rolling [`BudgetPeriod`]. Before forwarding, the gateway
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
use redis::AsyncCommands;
use rolter_core::{BudgetConfig, BudgetScope};
use tokio::sync::OnceCell;

/// Scope identity of a request, taken from its virtual key. An empty string
/// means "no id at this level" and never matches a budget.
#[derive(Debug, Default, Clone)]
pub struct ScopeIds {
    pub org: String,
    pub team: String,
    pub project: String,
    pub key: String,
}

impl ScopeIds {
    fn id_for(&self, scope: BudgetScope) -> &str {
        match scope {
            BudgetScope::Org => &self.org,
            BudgetScope::Team => &self.team,
            BudgetScope::Project => &self.project,
            BudgetScope::Key => &self.key,
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
        for budget in applicable {
            let key = spend_key(budget, now);
            let spent: Option<String> = conn.get(&key).await.ok().flatten();
            let spent = spent.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            if spent >= budget.limit_usd {
                return Some(budget.clone());
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
        }
    }

    #[test]
    fn applicable_matches_scope_chain_by_id() {
        let scope = ScopeIds {
            org: "org-1".to_string(),
            team: "team-1".to_string(),
            project: String::new(),
            key: "vk-1".to_string(),
        };
        let all = vec![
            budget(BudgetScope::Org, "org-1"),   // matches
            budget(BudgetScope::Org, "org-2"),   // wrong id
            budget(BudgetScope::Team, "team-1"), // matches
            budget(BudgetScope::Project, "p-1"), // scope empty on request
            budget(BudgetScope::Key, "vk-1"),    // matches
        ];
        let got = scope.applicable(&all);
        assert_eq!(got.len(), 3);
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

    #[test]
    fn spend_key_partitions_by_scope_id_and_window() {
        let b = budget(BudgetScope::Team, "team-9");
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-09T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(spend_key(&b, now), "rolter:budget:team:team-9:202607");
    }
}
