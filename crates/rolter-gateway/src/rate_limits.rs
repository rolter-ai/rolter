//! Request/token throughput limits backed by a Redis sliding window.
//!
//! Each [`RateLimitConfig`] caps a scope (org/team/project/virtual-key) to at
//! most `rpm` requests and/or `tpm` tokens over a rolling one-minute window.
//! Before forwarding, the gateway checks every applicable limit and rejects with
//! 429 (+ `retry-after`) when any one is at capacity — most-restrictive-wins
//! across the scope chain. Request counts are incremented on admission; token
//! counts are added after the response, when the usage is known (so `tpm` is
//! enforced against the trailing window, like a leaky bucket).
//!
//! The window is a sliding-window *counter*: two adjacent fixed one-minute
//! buckets are read and the previous bucket is weighted by how much of it still
//! falls inside the trailing 60s. This is O(1) per check (no per-request sorted
//! sets) and smooths the burst that a plain fixed window allows at its edges.
//!
//! Counters live in Redis so limits are shared across gateway replicas. With no
//! Redis url — or when Redis is unreachable — enforcement fails open so a
//! counter-store outage never takes the data plane down.

use std::sync::Arc;

use chrono::Utc;
use redis::AsyncCommands;
use rolter_core::{BudgetScope, RateLimitConfig};
use tokio::sync::OnceCell;

use crate::budgets::ScopeIds;

/// window length in seconds; `rpm`/`tpm` are per this window
const WINDOW_SECS: i64 = 60;
/// generous TTL so idle buckets self-clean (two windows of headroom)
const BUCKET_TTL_SECS: u64 = (WINDOW_SECS as u64) * 2;

/// A limit that a request would breach, with the seconds a client should wait.
#[derive(Debug, Clone)]
pub struct RateLimitHit {
    pub scope: BudgetScope,
    pub id: String,
    /// which cap tripped: `"rpm"` or `"tpm"`
    pub kind: &'static str,
    pub limit: u32,
    /// seconds until the trailing window frees capacity (`Retry-After`)
    pub retry_after: u64,
}

impl ScopeIds {
    /// The rate limits in `all` that apply to this request's scope chain.
    fn applicable_limits<'a>(&self, all: &'a [RateLimitConfig]) -> Vec<&'a RateLimitConfig> {
        all.iter()
            .filter(|l| {
                let id = self.id_for(l.scope);
                !id.is_empty() && id == l.id
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

/// Redis key of the fixed bucket a limit's `kind` counter lives in.
fn bucket_key(limit: &RateLimitConfig, kind: &str, bucket: i64) -> String {
    format!(
        "rolter:rl:{}:{}:{}:{}",
        scope_str(limit.scope),
        limit.id,
        kind,
        bucket
    )
}

/// Enforces throughput caps against Redis. Cheap to clone (shared connection).
#[derive(Clone)]
pub struct RateLimiter {
    inner: Option<Arc<Inner>>,
}

struct Inner {
    client: redis::Client,
    conn: OnceCell<redis::aio::MultiplexedConnection>,
}

impl RateLimiter {
    /// A disabled limiter: every check passes and nothing is recorded.
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Build a limiter against `redis_url`. An invalid url disables it.
    pub fn new(redis_url: &str) -> Self {
        match redis::Client::open(redis_url) {
            Ok(client) => Self {
                inner: Some(Arc::new(Inner {
                    client,
                    conn: OnceCell::new(),
                })),
            },
            Err(err) => {
                tracing::warn!(error = %err, "invalid redis url; rate limiting disabled");
                Self::disabled()
            }
        }
    }

    async fn connection(inner: &Inner) -> Option<redis::aio::MultiplexedConnection> {
        match inner
            .conn
            .get_or_try_init(|| inner.client.get_multiplexed_async_connection())
            .await
        {
            Ok(conn) => Some(conn.clone()),
            Err(err) => {
                tracing::warn!(error = %err, "redis unavailable; rate limits fail open");
                None
            }
        }
    }

    /// Sliding-window estimate of a `kind` counter for `limit` at second `now`.
    /// Reads the current and previous fixed buckets and weights the previous one
    /// by the fraction still inside the trailing window.
    async fn windowed_count(
        conn: &mut redis::aio::MultiplexedConnection,
        limit: &RateLimitConfig,
        kind: &str,
        now: i64,
    ) -> f64 {
        let bucket = now / WINDOW_SECS;
        let curr_key = bucket_key(limit, kind, bucket);
        let prev_key = bucket_key(limit, kind, bucket - 1);
        let counts: redis::RedisResult<(Option<u64>, Option<u64>)> =
            conn.mget(&[curr_key, prev_key]).await;
        let (curr, prev) = counts.unwrap_or((None, None));
        let curr = curr.unwrap_or(0) as f64;
        let prev = prev.unwrap_or(0) as f64;
        // how much of the previous fixed window still lies within the trailing
        // WINDOW_SECS: 1.0 at a bucket boundary, →0.0 as the current bucket fills
        let elapsed = (now % WINDOW_SECS) as f64;
        let prev_weight = (WINDOW_SECS as f64 - elapsed) / WINDOW_SECS as f64;
        curr + prev * prev_weight
    }

    /// Return the first applicable limit this request would breach, or `None`
    /// when admitted (also when disabled or Redis is down). On admission the
    /// request counter is incremented for every applicable `rpm` limit; token
    /// counts are recorded later via [`record_tokens`](Self::record_tokens).
    pub async fn check(
        &self,
        limits: &[RateLimitConfig],
        scope: &ScopeIds,
    ) -> Option<RateLimitHit> {
        let inner = self.inner.as_ref()?;
        let applicable = scope.applicable_limits(limits);
        if applicable.is_empty() {
            return None;
        }
        let mut conn = Self::connection(inner).await?;
        let now = Utc::now().timestamp();
        let retry_after = (WINDOW_SECS - (now % WINDOW_SECS)) as u64;

        // evaluate all caps before mutating any counter, so a request rejected
        // on one limit is not counted against another
        for limit in &applicable {
            if let Some(rpm) = limit.rpm {
                let est = Self::windowed_count(&mut conn, limit, "req", now).await;
                if est + 1.0 > rpm as f64 {
                    return Some(RateLimitHit {
                        scope: limit.scope,
                        id: limit.id.clone(),
                        kind: "rpm",
                        limit: rpm,
                        retry_after,
                    });
                }
            }
            if let Some(tpm) = limit.tpm {
                let est = Self::windowed_count(&mut conn, limit, "tok", now).await;
                // reactive: block once the trailing window is already at the cap
                if est >= tpm as f64 {
                    return Some(RateLimitHit {
                        scope: limit.scope,
                        id: limit.id.clone(),
                        kind: "tpm",
                        limit: tpm,
                        retry_after,
                    });
                }
            }
        }

        // admitted: count this request against every rpm-capped limit
        let bucket = now / WINDOW_SECS;
        for limit in &applicable {
            if limit.rpm.is_some() {
                Self::incr_bucket(&mut conn, &bucket_key(limit, "req", bucket), 1).await;
            }
        }
        None
    }

    /// Add `tokens` to the current window for every applicable `tpm` limit.
    /// No-op when disabled, Redis is down, `tokens` is zero, or nothing applies.
    pub async fn record_tokens(&self, limits: &[RateLimitConfig], scope: &ScopeIds, tokens: u64) {
        if tokens == 0 {
            return;
        }
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        let applicable = scope.applicable_limits(limits);
        if applicable.is_empty() {
            return;
        }
        let Some(mut conn) = Self::connection(inner).await else {
            return;
        };
        let bucket = Utc::now().timestamp() / WINDOW_SECS;
        for limit in applicable {
            if limit.tpm.is_some() {
                Self::incr_bucket(&mut conn, &bucket_key(limit, "tok", bucket), tokens).await;
            }
        }
    }

    /// INCRBY a bucket counter and refresh its TTL, logging (not failing) on err.
    async fn incr_bucket(conn: &mut redis::aio::MultiplexedConnection, key: &str, by: u64) {
        let incr: redis::RedisResult<u64> = conn.incr(key, by).await;
        match incr {
            Ok(_) => {
                let _: redis::RedisResult<()> = conn.expire(key, BUCKET_TTL_SECS as i64).await;
            }
            Err(err) => tracing::warn!(error = %err, key, "failed to record rate-limit count"),
        }
    }
}

/// A prepared handle that adds a single request's token usage to its applicable
/// `tpm` limits. Built on the request path, fired once from the response stream
/// after the total token count is known.
#[derive(Clone)]
pub struct TokenRecorder {
    limiter: RateLimiter,
    limits: Arc<Vec<RateLimitConfig>>,
    scope: ScopeIds,
}

impl TokenRecorder {
    pub fn new(limiter: RateLimiter, limits: Arc<Vec<RateLimitConfig>>, scope: ScopeIds) -> Self {
        Self {
            limiter,
            limits,
            scope,
        }
    }

    /// Record `tokens` against this request's rate limits.
    pub async fn record(&self, tokens: u64) {
        self.limiter
            .record_tokens(&self.limits, &self.scope, tokens)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(scope: BudgetScope, id: &str, rpm: Option<u32>, tpm: Option<u32>) -> RateLimitConfig {
        RateLimitConfig {
            scope,
            id: id.to_string(),
            rpm,
            tpm,
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
            limit(BudgetScope::Org, "org-1", Some(60), None),
            limit(BudgetScope::Org, "org-2", Some(60), None), // wrong id
            limit(BudgetScope::Team, "team-1", None, Some(1000)),
            limit(BudgetScope::Project, "p-1", Some(10), None), // scope empty
            limit(BudgetScope::Key, "vk-1", Some(5), Some(500)),
        ];
        assert_eq!(scope.applicable_limits(&all).len(), 3);
    }

    #[test]
    fn bucket_key_partitions_by_scope_kind_and_window() {
        let l = limit(BudgetScope::Key, "vk-9", Some(60), None);
        assert_eq!(bucket_key(&l, "req", 12345), "rolter:rl:key:vk-9:req:12345");
    }

    #[tokio::test]
    async fn disabled_limiter_never_blocks() {
        let limiter = RateLimiter::disabled();
        let scope = ScopeIds {
            org: "org-1".to_string(),
            ..Default::default()
        };
        let limits = vec![limit(BudgetScope::Org, "org-1", Some(1), Some(1))];
        assert!(limiter.check(&limits, &scope).await.is_none());
        limiter.record_tokens(&limits, &scope, 100).await; // no panic
    }
}
