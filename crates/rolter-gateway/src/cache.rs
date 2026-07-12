//! Exact-match response cache backed by Redis (ROL-56).
//!
//! When a route opts in (and the global `[cache]` switch is on), a non-streaming
//! successful response is stored under a key derived from the exact request:
//! `sha256(path ⋄ per-key-segment ⋄ forward_body)`, namespaced. A later identical
//! request is served from Redis verbatim — no upstream call, no token spend —
//! and the `x-rolter-cache` header flips to `HIT` (ROL-58).
//!
//! Entries live in Redis so the cache is shared across gateway replicas. With no
//! Redis url — or when Redis is unreachable — the cache is inert: every request
//! is a miss and the data plane is unaffected (fail open), exactly like the
//! rate-limit and budget enforcers.

use std::sync::Arc;

use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

/// A stored upstream response: enough to reconstruct the client reply byte-for-byte.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    pub status: u16,
    pub content_type: String,
    /// raw response body bytes
    pub body: Vec<u8>,
}

/// Exact-match response cache. Cheap to clone (shared connection). Disabled when
/// no Redis url is configured; disabled instances treat every request as a miss.
#[derive(Clone)]
pub struct ResponseCache {
    inner: Option<Arc<Inner>>,
}

struct Inner {
    client: redis::Client,
    conn: OnceCell<redis::aio::MultiplexedConnection>,
}

impl ResponseCache {
    /// A disabled cache: every lookup misses and stores are dropped.
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Build a cache against `redis_url`. An invalid url disables it.
    pub fn new(redis_url: &str) -> Self {
        match redis::Client::open(redis_url) {
            Ok(client) => Self {
                inner: Some(Arc::new(Inner {
                    client,
                    conn: OnceCell::new(),
                })),
            },
            Err(err) => {
                tracing::warn!(error = %err, "invalid redis url; response cache disabled");
                Self::disabled()
            }
        }
    }

    /// Whether this cache can store/serve anything (has a Redis client).
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Derive the Redis key for a request. `per_key_scope` is the virtual-key id
    /// mixed in when the route isolates entries per key (empty otherwise), so
    /// callers of a shared route collide and callers of an isolated route don't.
    pub fn make_key(namespace: &str, path: &str, per_key_scope: &str, body: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(path.as_bytes());
        hasher.update([0x1f]); // domain separator
        hasher.update(per_key_scope.as_bytes());
        hasher.update([0x1f]);
        hasher.update(body);
        let digest = hasher.finalize();
        let mut out = String::with_capacity(namespace.len() + 1 + digest.len() * 2);
        out.push_str(namespace);
        out.push(':');
        for byte in digest {
            out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
            out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap());
        }
        out
    }

    async fn connection(inner: &Inner) -> Option<redis::aio::MultiplexedConnection> {
        match inner
            .conn
            .get_or_try_init(|| inner.client.get_multiplexed_async_connection())
            .await
        {
            Ok(conn) => Some(conn.clone()),
            Err(err) => {
                tracing::warn!(error = %err, "redis unavailable; response cache misses");
                None
            }
        }
    }

    /// Look up a cached response for `key`. Returns `None` on a miss, when
    /// disabled, when Redis is down, or when the stored blob fails to decode.
    pub async fn get(&self, key: &str) -> Option<CachedResponse> {
        let inner = self.inner.as_ref()?;
        let mut conn = Self::connection(inner).await?;
        let raw: Option<Vec<u8>> = conn.get(key).await.unwrap_or(None);
        let raw = raw?;
        match serde_json::from_slice(&raw) {
            Ok(resp) => Some(resp),
            Err(err) => {
                tracing::warn!(error = %err, key, "failed to decode cached response");
                None
            }
        }
    }

    /// Store `resp` under `key` with a `ttl_secs` expiry. No-op when disabled,
    /// Redis is down, or the TTL is zero; failures are logged, never propagated.
    pub async fn put(&self, key: &str, resp: &CachedResponse, ttl_secs: u64) {
        if ttl_secs == 0 {
            return;
        }
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        let Some(mut conn) = Self::connection(inner).await else {
            return;
        };
        let blob = match serde_json::to_vec(resp) {
            Ok(blob) => blob,
            Err(err) => {
                tracing::warn!(error = %err, "failed to encode response for cache");
                return;
            }
        };
        let res: redis::RedisResult<()> = conn.set_ex(key, blob, ttl_secs).await;
        if let Err(err) = res {
            tracing::warn!(error = %err, key, "failed to store cached response");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_deterministic_and_namespaced() {
        let a = ResponseCache::make_key("ns", "/v1/chat/completions", "", b"body");
        let b = ResponseCache::make_key("ns", "/v1/chat/completions", "", b"body");
        assert_eq!(a, b);
        assert!(a.starts_with("ns:"));
        // sha-256 hex is 64 chars after the "ns:" prefix
        assert_eq!(a.len(), "ns:".len() + 64);
    }

    #[test]
    fn key_varies_with_every_input() {
        let base = ResponseCache::make_key("ns", "/p", "", b"body");
        assert_ne!(base, ResponseCache::make_key("ns", "/p2", "", b"body"));
        assert_ne!(base, ResponseCache::make_key("ns", "/p", "vk-1", b"body"));
        assert_ne!(base, ResponseCache::make_key("ns", "/p", "", b"body2"));
        // the namespace prefixes but doesn't change the digest input
        let other_ns = ResponseCache::make_key("other", "/p", "", b"body");
        assert_eq!(base["ns".len()..], other_ns["other".len()..]);
    }

    #[test]
    fn per_key_scope_isolates_entries() {
        let shared_a = ResponseCache::make_key("ns", "/p", "", b"body");
        let key_1 = ResponseCache::make_key("ns", "/p", "vk-1", b"body");
        let key_2 = ResponseCache::make_key("ns", "/p", "vk-2", b"body");
        assert_ne!(key_1, key_2);
        assert_ne!(shared_a, key_1);
    }

    #[tokio::test]
    async fn disabled_cache_always_misses() {
        let cache = ResponseCache::disabled();
        assert!(!cache.is_enabled());
        assert!(cache.get("any").await.is_none());
        // storing is a no-op and must not panic
        cache
            .put(
                "any",
                &CachedResponse {
                    status: 200,
                    content_type: "application/json".to_string(),
                    body: b"{}".to_vec(),
                },
                60,
            )
            .await;
    }
}
