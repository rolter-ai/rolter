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
//! rate-limit and budget enforcers. A lost connection is re-established on its
//! own once Redis is reachable again (see [`crate::redis_conn`]).

// only the test-only in-memory backend shares state through an Arc
#[cfg(test)]
use std::sync::Arc;

use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::redis_conn::ReconnectingRedis;

/// A stored upstream response: enough to reconstruct the client reply byte-for-byte.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    pub status: u16,
    pub content_type: String,
    /// raw response body bytes
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SemanticEntry {
    embedding: Vec<f32>,
    response: CachedResponse,
}

/// Exact-match response cache. Cheap to clone (shared connection). Disabled when
/// no Redis url is configured; disabled instances treat every request as a miss.
#[derive(Clone)]
pub struct ResponseCache {
    redis: Option<ReconnectingRedis>,
    /// in-process stand-in for Redis so HTTP-level cache tests run where no
    /// Redis exists, CI included
    #[cfg(test)]
    memory: Option<Arc<parking_lot::Mutex<memory::Store>>>,
}

impl ResponseCache {
    /// A disabled cache: every lookup misses and stores are dropped.
    pub fn disabled() -> Self {
        Self {
            redis: None,
            #[cfg(test)]
            memory: None,
        }
    }

    /// A cache held in process memory, for tests. Entries never expire.
    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self {
            redis: None,
            memory: Some(Arc::default()),
        }
    }

    /// Build a cache against `redis_url`. An invalid url disables it.
    pub fn new(redis_url: &str) -> Self {
        match ReconnectingRedis::new(redis_url, "response cache") {
            Ok(redis) => Self {
                redis: Some(redis),
                #[cfg(test)]
                memory: None,
            },
            Err(err) => {
                tracing::warn!(error = %err, "invalid redis url; response cache disabled");
                Self::disabled()
            }
        }
    }

    /// Whether this cache can store/serve anything (has a Redis client).
    pub fn is_enabled(&self) -> bool {
        #[cfg(test)]
        if self.memory.is_some() {
            return true;
        }
        self.redis.is_some()
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
        out.push_str(&rolter_auth::hex::encode(&digest));
        out
    }

    /// Stable list key for the bounded semantic candidate window of one route,
    /// optional virtual-key scope and compatibility partition.
    ///
    /// `partition` is the digest of every request field that must match exactly
    /// before two requests may share a reply (see `crate::semantic`), so a
    /// similarity scan never sees a candidate from another partition. The
    /// layout version sits in the namespace, which is what retires entries
    /// written under older rules (#1476).
    pub fn semantic_index_key(
        namespace: &str,
        path: &str,
        route: &str,
        per_key_scope: &str,
        partition: &str,
    ) -> String {
        let mut identity = Vec::with_capacity(route.len() + 1 + partition.len());
        identity.extend_from_slice(route.as_bytes());
        identity.push(0x1f);
        identity.extend_from_slice(partition.as_bytes());
        Self::make_key(
            &format!(
                "{namespace}:semantic:{}",
                crate::semantic::SEMANTIC_LAYOUT_VERSION
            ),
            path,
            per_key_scope,
            &identity,
        )
    }

    /// Look up a cached response for `key`. Returns `None` on a miss, when
    /// disabled, when Redis is down, or when the stored blob fails to decode.
    pub async fn get(&self, key: &str) -> Option<CachedResponse> {
        #[cfg(test)]
        if let Some(memory) = &self.memory {
            let raw = memory.lock().values.get(key).cloned()?;
            return serde_json::from_slice(&raw).ok();
        }
        let mut conn = self.redis.as_ref()?.get().await?;
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
        let blob = match serde_json::to_vec(resp) {
            Ok(blob) => blob,
            Err(err) => {
                tracing::warn!(error = %err, "failed to encode response for cache");
                return;
            }
        };
        #[cfg(test)]
        if let Some(memory) = &self.memory {
            memory.lock().values.insert(key.to_string(), blob);
            return;
        }
        let Some(redis) = self.redis.as_ref() else {
            return;
        };
        let Some(mut conn) = redis.get().await else {
            return;
        };
        let res: redis::RedisResult<()> = conn.set_ex(key, blob, ttl_secs).await;
        if let Err(err) = res {
            tracing::warn!(error = %err, key, "failed to store cached response");
        }
    }

    /// Find the nearest response in a bounded recent-candidate window. Any
    /// Redis or decode failure is a miss so semantic caching remains fail-open.
    pub async fn semantic_get(
        &self,
        index_key: &str,
        embedding: &[f32],
        threshold: f32,
        max_candidates: usize,
    ) -> Option<CachedResponse> {
        if embedding.is_empty() || max_candidates == 0 {
            return None;
        }
        #[cfg(test)]
        if let Some(memory) = &self.memory {
            let memory = memory.lock();
            let blobs = memory
                .lists
                .get(index_key)
                .into_iter()
                .flatten()
                .take(max_candidates)
                .map(|id| {
                    memory
                        .values
                        .get(&format!("{index_key}:entry:{id}"))
                        .cloned()
                });
            return nearest(blobs, embedding, threshold);
        }
        let mut conn = self.redis.as_ref()?.get().await?;
        let ids: Vec<String> = conn
            .lrange(index_key, 0, max_candidates.saturating_sub(1) as isize)
            .await
            .unwrap_or_default();
        if ids.is_empty() {
            return None;
        }
        let keys: Vec<String> = ids
            .iter()
            .map(|id| format!("{index_key}:entry:{id}"))
            .collect();
        let blobs: Vec<Option<Vec<u8>>> = redis::cmd("MGET")
            .arg(&keys)
            .query_async(&mut conn)
            .await
            .unwrap_or_default();
        nearest(blobs, embedding, threshold)
    }

    /// Add or refresh one semantic candidate and trim the route's index to a
    /// fixed size. `entry_id` is normally the exact-cache key for the request.
    pub async fn semantic_put(
        &self,
        index_key: &str,
        entry_id: &str,
        embedding: Vec<f32>,
        response: &CachedResponse,
        ttl_secs: u64,
        max_candidates: usize,
    ) {
        if embedding.is_empty() || ttl_secs == 0 || max_candidates == 0 {
            return;
        }
        let Ok(blob) = serde_json::to_vec(&SemanticEntry {
            embedding,
            response: response.clone(),
        }) else {
            return;
        };
        let entry_key = format!("{index_key}:entry:{entry_id}");
        #[cfg(test)]
        if let Some(memory) = &self.memory {
            let mut memory = memory.lock();
            memory.values.insert(entry_key, blob);
            let list = memory.lists.entry(index_key.to_string()).or_default();
            list.retain(|id| id != entry_id);
            list.insert(0, entry_id.to_string());
            list.truncate(max_candidates);
            return;
        }
        let Some(redis) = self.redis.as_ref() else {
            return;
        };
        let Some(mut conn) = redis.get().await else {
            return;
        };
        let result: redis::RedisResult<()> = redis::pipe()
            .atomic()
            .cmd("SETEX")
            .arg(&entry_key)
            .arg(ttl_secs)
            .arg(blob)
            .ignore()
            .cmd("LREM")
            .arg(index_key)
            .arg(0)
            .arg(entry_id)
            .ignore()
            .cmd("LPUSH")
            .arg(index_key)
            .arg(entry_id)
            .ignore()
            .cmd("LTRIM")
            .arg(index_key)
            .arg(0)
            .arg(max_candidates.saturating_sub(1))
            .ignore()
            .cmd("EXPIRE")
            .arg(index_key)
            .arg(ttl_secs)
            .ignore()
            .query_async(&mut conn)
            .await;
        if let Err(error) = result {
            tracing::warn!(%error, "failed to store semantic cache entry");
        }
    }
}

/// The stored response whose embedding is most similar to `embedding`, if any
/// clears `threshold`. Missing and undecodable blobs are skipped.
fn nearest(
    blobs: impl IntoIterator<Item = Option<Vec<u8>>>,
    embedding: &[f32],
    threshold: f32,
) -> Option<CachedResponse> {
    blobs
        .into_iter()
        .flatten()
        .filter_map(|blob| serde_json::from_slice::<SemanticEntry>(&blob).ok())
        .filter_map(|entry| {
            cosine_similarity(embedding, &entry.embedding)
                .filter(|score| *score >= threshold)
                .map(|score| (score, entry.response))
        })
        .max_by(|(a, _), (b, _)| a.total_cmp(b))
        .map(|(_, response)| response)
}

#[cfg(test)]
mod memory {
    use std::collections::HashMap;

    /// The two Redis shapes the cache uses: plain values and the semantic
    /// candidate lists (newest first).
    #[derive(Default)]
    pub(super) struct Store {
        pub(super) values: HashMap<String, Vec<u8>>,
        pub(super) lists: HashMap<String, Vec<String>>,
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let (mut dot, mut norm_a, mut norm_b) = (0.0f64, 0.0f64, 0.0f64);
    for (&x, &y) in a.iter().zip(b) {
        let (x, y) = (x as f64, y as f64);
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    (denom > 0.0).then(|| (dot / denom).clamp(-1.0, 1.0) as f32)
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

    #[test]
    fn sse_body_round_trips_through_stored_encoding() {
        // a buffered streaming response is stored and served via the same
        // serde_json encode/decode put/get use; the SSE frames (incl. the
        // terminal [DONE] and the final usage chunk) must survive verbatim so
        // replay still parses token usage. this is the storage contract ROL-235
        // relies on for streaming cache hits.
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"po\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"ng\"}}]}\n\n",
            "data: {\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );
        let original = CachedResponse {
            status: 200,
            content_type: "text/event-stream".to_string(),
            body: sse.as_bytes().to_vec(),
        };
        let blob = serde_json::to_vec(&original).unwrap();
        let decoded: CachedResponse = serde_json::from_slice(&blob).unwrap();
        assert_eq!(decoded.status, 200);
        assert_eq!(decoded.content_type, "text/event-stream");
        // byte-for-byte identical framing so UsageLoggingStream sees the same
        // events it would on a live miss
        assert_eq!(decoded.body, original.body);
        assert_eq!(String::from_utf8(decoded.body).unwrap(), sse);
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

    #[test]
    fn cosine_similarity_handles_matches_misses_and_bad_shapes() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), Some(1.0));
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), Some(0.0));
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 0.0]), None);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[0.0, 0.0]), None);
    }

    /// #1483, for the response cache: after Redis closes the cache's
    /// connection the same instance serves the entry stored before the drop,
    /// and stores new ones through the new connection.
    #[tokio::test]
    async fn an_existing_cache_recovers_after_redis_drops_its_connection() {
        use crate::redis_conn::testing::{self, db};

        let Some(url) = testing::url(db::CACHE) else {
            return;
        };
        let cache = ResponseCache::new(&url);
        let response = |body: &str| CachedResponse {
            status: 200,
            content_type: "application/json".to_string(),
            body: body.as_bytes().to_vec(),
        };
        let before = testing::unique("rolter:test:cache:before");
        cache.put(&before, &response("{\"n\":1}"), 60).await;
        assert!(cache.get(&before).await.is_some());

        let killed = testing::kill_clients_on_db(db::CACHE).await;
        assert!(killed >= 1, "the cache's connection was closed");

        let hit = cache
            .get(&before)
            .await
            .expect("a healthy redis must restore the existing cache");
        assert_eq!(hit.body, b"{\"n\":1}");

        let after = testing::unique("rolter:test:cache:after");
        cache.put(&after, &response("{\"n\":2}"), 60).await;
        assert!(cache.get(&after).await.is_some());
    }
}
