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
//! counter-store outage never takes the data plane down. A lost connection is
//! re-established on its own once Redis is reachable again (see
//! [`crate::redis_conn`]), so fail-open lasts as long as the outage and no
//! longer.

use std::sync::{Arc, LazyLock};

use crate::budgets::ScopeIds;
use crate::metrics::{Metrics, RedisConsumer};
use crate::redis_conn::ReconnectingRedis;
use chrono::Utc;
use rolter_core::{BudgetScope, RateLimitConfig};

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
        BudgetScope::BusinessUnit => "business_unit",
        BudgetScope::Customer => "customer",
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

/// How much of the previous fixed bucket still lies inside the trailing window
/// at unix second `now`: 1.0 on a bucket boundary, falling towards 0.0 as the
/// current bucket fills.
fn previous_bucket_weight(now: i64) -> f64 {
    let elapsed = now.rem_euclid(WINDOW_SECS) as f64;
    (WINDOW_SECS as f64 - elapsed) / WINDOW_SECS as f64
}

/// Admission as one server-side step: evaluate every applicable limit and,
/// only when all of them pass, charge the request to every `rpm` bucket.
///
/// Redis runs a script to completion before serving any other command, so no
/// request can read a counter between another request's read and its charge.
/// That is what the old read (`MGET`), decide, then write (`INCR`) sequence
/// lacked: 32 concurrent requests against `rpm = 1` could all read zero and all
/// be admitted (#1484). Across replicas it is the same Redis, so the same
/// holds.
///
/// Keys come four per limit — request bucket now and before, token bucket now
/// and before — and arguments are the previous bucket's weight, the bucket TTL,
/// then `rpm` and `tpm` per limit with `-1` for "not set". The reply is
/// `{0, 0}` on admission, or the 1-based index of the first limit that refused
/// and `1` for `rpm` or `2` for `tpm`. Limits are checked in scope-chain order
/// and nothing is charged on a refusal, so most-restrictive-wins and "a request
/// rejected by one scope costs no other scope anything" both hold.
///
/// **Replay.** The rule in [`crate::redis_conn`] is that writes are never
/// replayed after a connection dies, because a spend increment could land
/// twice. Admission opts in regardless. If the first attempt never ran (the
/// usual case: the connection was already dead), the replay is exact. If it
/// ran and only the reply was lost, the replay charges one extra request to a
/// bucket that expires within two minutes, and may refuse where the lost reply
/// would have admitted. Erring by one towards *stricter* for one window is the
/// safer failure for a throughput control than the alternative, which is to
/// admit the request unchecked and uncounted.
///
/// **Tokens.** `tpm` is read in the same step, so the decision sees one
/// consistent snapshot, but tokens are only known after the response and are
/// added by [`RateLimiter::record_tokens`]. Requests admitted concurrently can
/// therefore still overshoot a `tpm` cap by their own usage — that is the
/// documented reactive behaviour, not a race this script could close.
static ADMIT: LazyLock<redis::Script> = LazyLock::new(|| {
    redis::Script::new(
        r#"
local weight = tonumber(ARGV[1])
local ttl = tonumber(ARGV[2])
local limits = (#ARGV - 2) / 2

local function windowed(current, previous)
  local now = tonumber(redis.call('GET', current)) or 0
  local before = tonumber(redis.call('GET', previous)) or 0
  return now + before * weight
end

for i = 1, limits do
  local rpm = tonumber(ARGV[1 + 2 * i])
  local tpm = tonumber(ARGV[2 + 2 * i])
  local k = (i - 1) * 4
  if rpm >= 0 and windowed(KEYS[k + 1], KEYS[k + 2]) + 1 > rpm then
    return {i, 1}
  end
  if tpm >= 0 and windowed(KEYS[k + 3], KEYS[k + 4]) >= tpm then
    return {i, 2}
  end
end

for i = 1, limits do
  if tonumber(ARGV[1 + 2 * i]) >= 0 then
    local key = KEYS[(i - 1) * 4 + 1]
    redis.call('INCR', key)
    redis.call('EXPIRE', key, ttl)
  end
end
return {0, 0}
"#,
    )
});

/// Enforces throughput caps against Redis. Cheap to clone (shared connection).
#[derive(Clone)]
pub struct RateLimiter {
    redis: Option<ReconnectingRedis>,
}

impl RateLimiter {
    /// A disabled limiter: every check passes and nothing is recorded.
    pub fn disabled() -> Self {
        Self { redis: None }
    }

    /// Build a limiter against `redis_url`. An invalid url disables it.
    pub fn new(redis_url: &str) -> Self {
        match ReconnectingRedis::new(redis_url, RedisConsumer::RateLimits.label()) {
            Ok(redis) => Self { redis: Some(redis) },
            Err(err) => {
                tracing::warn!(error = %err, "invalid redis url; rate limiting disabled");
                Self::disabled()
            }
        }
    }

    /// Report this limiter's Redis connection, and the admissions it lets
    /// through without Redis, on `/metrics`; and connect now rather than on
    /// the first limited request (#1772). Does nothing when disabled.
    pub fn watch(&self, metrics: &Metrics) {
        if let Some(redis) = &self.redis {
            metrics.watch_redis(RedisConsumer::RateLimits, redis.stats().clone());
            redis.warm_up();
        }
    }

    /// Return the first applicable limit this request would breach, or `None`
    /// when admitted (also when disabled or Redis is down). On admission the
    /// request counter is incremented for every applicable `rpm` limit; token
    /// counts are recorded later via [`record_tokens`](Self::record_tokens).
    ///
    /// Evaluating the limits and charging the admitted request are one atomic
    /// step on the Redis server (see [`ADMIT`]), so concurrent requests — from
    /// one gateway or many — can never all see the same free slot (#1484).
    pub async fn check(
        &self,
        limits: &[RateLimitConfig],
        scope: &ScopeIds,
    ) -> Option<RateLimitHit> {
        self.check_at(limits, scope, Utc::now().timestamp()).await
    }

    /// [`check`](Self::check) at unix second `now`, so tests can place a
    /// request anywhere in the window.
    async fn check_at(
        &self,
        limits: &[RateLimitConfig],
        scope: &ScopeIds,
        now: i64,
    ) -> Option<RateLimitHit> {
        let redis = self.redis.as_ref()?;
        let applicable = scope.applicable_limits(limits);
        if applicable.is_empty() {
            return None;
        }
        let Some(mut conn) = redis.get().await else {
            // unchecked: the request is admitted as if under every limit (#1772)
            redis.stats().on_fail_open();
            return None;
        };
        // the script is safe to replay on a fresh connection when the first
        // one turned out dead: see the note on `ADMIT`
        conn.replay_writes();
        let bucket = now.div_euclid(WINDOW_SECS);

        let mut invocation = ADMIT.prepare_invoke();
        invocation
            .arg(previous_bucket_weight(now))
            .arg(BUCKET_TTL_SECS);
        for limit in &applicable {
            invocation
                .key(bucket_key(limit, "req", bucket))
                .key(bucket_key(limit, "req", bucket - 1))
                .key(bucket_key(limit, "tok", bucket))
                .key(bucket_key(limit, "tok", bucket - 1))
                .arg(limit.rpm.map_or(-1, i64::from))
                .arg(limit.tpm.map_or(-1, i64::from));
        }
        let verdict: (usize, u8) = match invocation.invoke_async(&mut conn).await {
            Ok(verdict) => verdict,
            Err(err) => {
                tracing::warn!(error = %err, "rate-limit admission failed; failing open");
                redis.stats().on_fail_open();
                return None;
            }
        };

        let (index, kind) = verdict;
        let limit = applicable.get(index.checked_sub(1)?)?;
        let retry_after = (WINDOW_SECS - now.rem_euclid(WINDOW_SECS)) as u64;
        let (kind, cap) = match kind {
            1 => ("rpm", limit.rpm?),
            2 => ("tpm", limit.tpm?),
            _ => return None,
        };
        Some(RateLimitHit {
            scope: limit.scope,
            id: limit.id.clone(),
            kind,
            limit: cap,
            retry_after,
        })
    }

    /// Add `tokens` to the current window for every applicable `tpm` limit.
    /// No-op when disabled, Redis is down, `tokens` is zero, or nothing applies.
    pub async fn record_tokens(&self, limits: &[RateLimitConfig], scope: &ScopeIds, tokens: u64) {
        if tokens == 0 {
            return;
        }
        let Some(redis) = self.redis.as_ref() else {
            return;
        };
        let applicable = scope.applicable_limits(limits);
        if applicable.is_empty() {
            return;
        }
        let Some(mut conn) = redis.get().await else {
            return;
        };
        let bucket = Utc::now().timestamp() / WINDOW_SECS;
        let mut has_tok = false;
        let mut pipe = redis::pipe();
        for limit in applicable {
            if limit.tpm.is_some() {
                let key = bucket_key(limit, "tok", bucket);
                pipe.incr(&key, tokens)
                    .ignore()
                    .expire(&key, BUCKET_TTL_SECS as i64)
                    .ignore();
                has_tok = true;
            }
        }
        if has_tok {
            if let Err(err) = pipe.query_async::<()>(&mut conn).await {
                tracing::warn!(error = %err, "failed to record token rate-limit count");
            }
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
    use crate::redis_conn::testing::{self, db};
    use redis::AsyncCommands;

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
            business_unit: "bu-1".to_string(),
            customer: String::new(),
        };
        let all = vec![
            limit(BudgetScope::Org, "org-1", Some(60), None),
            limit(BudgetScope::Org, "org-2", Some(60), None), // wrong id
            limit(BudgetScope::Team, "team-1", None, Some(1000)),
            limit(BudgetScope::Project, "p-1", Some(10), None), // scope empty
            limit(BudgetScope::Key, "vk-1", Some(5), Some(500)),
            limit(BudgetScope::BusinessUnit, "bu-1", Some(30), None),
            limit(BudgetScope::Customer, "cust-1", Some(30), None), // key unattributed
        ];
        assert_eq!(scope.applicable_limits(&all).len(), 4);
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

    /// #1483: after Redis closes the limiter's connection, the same limiter
    /// must reject against the request count that survived the drop and keep
    /// recording tokens, rather than fail open until the gateway restarts.
    #[tokio::test]
    async fn an_existing_limiter_recovers_after_redis_drops_its_connection() {
        let Some(url) = testing::url(db::RATE_LIMITS) else {
            return;
        };
        let scope = ScopeIds {
            org: testing::unique("reconnect-org"),
            ..Default::default()
        };
        let limits = vec![limit(
            BudgetScope::Org,
            &scope.org,
            Some(1),
            Some(1_000_000),
        )];
        let limiter = RateLimiter::new(&url);
        assert!(limiter.check(&limits, &scope).await.is_none());
        assert!(limiter.check(&limits, &scope).await.is_some());

        let killed = testing::kill_clients_on_db(db::RATE_LIMITS).await;
        assert!(killed >= 1, "the limiter's connection was closed");

        let hit = limiter
            .check(&limits, &scope)
            .await
            .expect("a healthy redis must restore the existing limiter");
        assert_eq!(hit.kind, "rpm");

        limiter.record_tokens(&limits, &scope, 500).await;
        let bucket = Utc::now().timestamp() / WINDOW_SECS;
        let keys = [
            bucket_key(&limits[0], "tok", bucket),
            bucket_key(&limits[0], "tok", bucket - 1),
        ];
        let mut conn = redis::Client::open(url.as_str())
            .unwrap()
            .get_multiplexed_async_connection()
            .await
            .unwrap();
        // two buckets, in case the minute turned between recording and reading
        let tokens: Vec<Option<u64>> = conn.mget(&keys).await.unwrap();
        assert_eq!(tokens.into_iter().flatten().sum::<u64>(), 500);
        let _: () = conn.del(&keys).await.unwrap();
    }

    #[test]
    fn the_previous_bucket_weighs_less_as_the_current_one_fills() {
        assert_eq!(previous_bucket_weight(600), 1.0);
        assert_eq!(previous_bucket_weight(630), 0.5);
        assert!((previous_bucket_weight(659) - 1.0 / 60.0).abs() < 1e-9);
    }

    fn org_scope(prefix: &str) -> ScopeIds {
        ScopeIds {
            org: testing::unique(prefix),
            ..Default::default()
        }
    }

    async fn admitted_of(
        limiters: &[RateLimiter],
        limits: &[RateLimitConfig],
        scope: &ScopeIds,
        n: usize,
    ) -> usize {
        let limits = Arc::new(limits.to_vec());
        let scope = Arc::new(scope.clone());
        let mut tasks = tokio::task::JoinSet::new();
        for i in 0..n {
            let limiter = limiters[i % limiters.len()].clone();
            let (limits, scope) = (limits.clone(), scope.clone());
            tasks.spawn(async move { limiter.check(&limits, &scope).await.is_none() });
        }
        let mut admitted = 0;
        while let Some(result) = tasks.join_next().await {
            admitted += usize::from(result.unwrap());
        }
        admitted
    }

    async fn read_count(url: &str, key: &str) -> u64 {
        let mut conn = redis::Client::open(url)
            .unwrap()
            .get_multiplexed_async_connection()
            .await
            .unwrap();
        let count: Option<u64> = conn.get(key).await.unwrap();
        count.unwrap_or(0)
    }

    /// #1484: the reproduction from the issue. 64 requests racing one free
    /// slot admit exactly one, and the counter says one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_requests_cannot_share_one_free_slot() {
        let Some(url) = testing::url(db::RATE_ADMISSION) else {
            return;
        };
        let scope = org_scope("race-org");
        let limits = vec![limit(BudgetScope::Org, &scope.org, Some(1), None)];
        let limiter = RateLimiter::new(&url);
        assert_eq!(admitted_of(&[limiter], &limits, &scope, 64).await, 1);
    }

    /// The same across replicas: separate limiters with separate connections
    /// share the counter, and a cap of five admits five, not five per replica.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn replicas_racing_each_other_admit_exactly_the_cap() {
        let Some(url) = testing::url(db::RATE_ADMISSION) else {
            return;
        };
        let scope = org_scope("replicas-org");
        let limits = vec![limit(BudgetScope::Org, &scope.org, Some(5), None)];
        let replicas = [
            RateLimiter::new(&url),
            RateLimiter::new(&url),
            RateLimiter::new(&url),
        ];
        assert_eq!(admitted_of(&replicas, &limits, &scope, 96).await, 5);
        let bucket = Utc::now().timestamp() / WINDOW_SECS;
        let charged = read_count(&url, &bucket_key(&limits[0], "req", bucket)).await
            + read_count(&url, &bucket_key(&limits[0], "req", bucket - 1)).await;
        assert_eq!(charged, 5, "only admitted requests are counted");
    }

    /// Most-restrictive-wins, and a request refused by one scope costs no
    /// other scope anything — including under concurrency.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn a_refusal_by_one_scope_charges_no_other_scope() {
        let Some(url) = testing::url(db::RATE_ADMISSION) else {
            return;
        };
        let scope = ScopeIds {
            org: testing::unique("chain-org"),
            key: testing::unique("chain-key"),
            ..Default::default()
        };
        let limits = vec![
            limit(BudgetScope::Org, &scope.org, Some(1_000), None),
            limit(BudgetScope::Key, &scope.key, Some(2), None),
        ];
        let limiter = RateLimiter::new(&url);
        assert_eq!(
            admitted_of(std::slice::from_ref(&limiter), &limits, &scope, 40).await,
            2
        );
        let hit = limiter
            .check(&limits, &scope)
            .await
            .expect("the key is full");
        assert_eq!(
            (hit.scope, hit.kind, hit.limit),
            (BudgetScope::Key, "rpm", 2)
        );

        let bucket = Utc::now().timestamp() / WINDOW_SECS;
        let org_charged = read_count(&url, &bucket_key(&limits[0], "req", bucket)).await
            + read_count(&url, &bucket_key(&limits[0], "req", bucket - 1)).await;
        assert_eq!(
            org_charged, 2,
            "the org pays only for what the key admitted"
        );
    }

    /// The sliding window at its edges: a full previous bucket counts in full
    /// on the boundary and half way through counts for half. `check_at` pins
    /// the clock; the buckets are real ones either side of now, so the keys
    /// expire like any other.
    #[tokio::test]
    async fn the_previous_bucket_is_weighted_across_the_boundary() {
        let Some(url) = testing::url(db::RATE_ADMISSION) else {
            return;
        };
        let scope = org_scope("window-org");
        let limits = vec![limit(BudgetScope::Org, &scope.org, Some(2), None)];
        let limiter = RateLimiter::new(&url);
        let start = (Utc::now().timestamp().div_euclid(WINDOW_SECS) - 1) * WINDOW_SECS;

        // fill the previous bucket late in its minute
        assert!(limiter
            .check_at(&limits, &scope, start + 50)
            .await
            .is_none());
        assert!(limiter
            .check_at(&limits, &scope, start + 51)
            .await
            .is_none());
        let refused = limiter.check_at(&limits, &scope, start + 52).await.unwrap();
        assert_eq!(refused.retry_after, 8, "seconds left in that bucket");

        // on the boundary the previous bucket weighs 1.0: 2 + 1 > 2
        let next = start + WINDOW_SECS;
        assert!(limiter.check_at(&limits, &scope, next).await.is_some());
        // half way through it weighs 0.5: 2 * 0.5 + 1 <= 2, admitted
        assert!(limiter.check_at(&limits, &scope, next + 30).await.is_none());
        // and that admission now counts in the current bucket: 1 + 1 + 1 > 2
        assert!(limiter.check_at(&limits, &scope, next + 30).await.is_some());
    }

    /// `tpm` is reactive: once recorded usage reaches the cap the next request
    /// is refused, and a refusal on tokens charges no request slot.
    #[tokio::test]
    async fn a_full_token_window_refuses_without_charging_requests() {
        let Some(url) = testing::url(db::RATE_ADMISSION) else {
            return;
        };
        let scope = org_scope("tpm-org");
        let limits = vec![limit(BudgetScope::Org, &scope.org, Some(100), Some(1_000))];
        let limiter = RateLimiter::new(&url);
        assert!(limiter.check(&limits, &scope).await.is_none());
        limiter.record_tokens(&limits, &scope, 1_000).await;
        for _ in 0..5 {
            let hit = limiter
                .check(&limits, &scope)
                .await
                .expect("tokens exhausted");
            assert_eq!(hit.kind, "tpm");
        }
        let bucket = Utc::now().timestamp() / WINDOW_SECS;
        let charged = read_count(&url, &bucket_key(&limits[0], "req", bucket)).await
            + read_count(&url, &bucket_key(&limits[0], "req", bucket - 1)).await;
        assert_eq!(charged, 1, "refused requests take no rpm slot");
    }
}
