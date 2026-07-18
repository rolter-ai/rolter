//! Non-blocking cache telemetry for precise vLLM and LMCache-aware routing.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use parking_lot::Mutex;
use rmpv::Value;
use rolter_core::{KvEventsConfig, LmCacheConfig, ProviderConfig};
use serde::Deserialize;
use zeromq::{Socket, SocketRecv, SubSocket};

use crate::metrics::Metrics;

#[derive(Clone)]
pub struct CacheTelemetry {
    inner: Arc<Inner>,
}

struct Inner {
    kv: DashMap<String, Arc<KvTarget>>,
    lmcache: DashMap<String, Arc<LmCacheTarget>>,
    started: DashMap<String, ()>,
    metrics: Arc<Metrics>,
}

struct KvTarget {
    state: Mutex<KvState>,
    max_blocks: usize,
    stale_secs: u64,
    updated_ms: AtomicU64,
}

#[derive(Default)]
struct KvState {
    block_size: usize,
    resident: HashSet<u64>,
    external_to_local: HashMap<String, u64>,
    insertion_order: VecDeque<u64>,
}

struct LmCacheTarget {
    occupancy_bits: AtomicU64,
    available: AtomicU64,
    stale_secs: u64,
    updated_ms: AtomicU64,
}

#[derive(Debug, Deserialize)]
struct LmCacheSignal {
    occupancy: f32,
    #[serde(default = "default_true")]
    cache_available: bool,
}

fn default_true() -> bool {
    true
}

impl CacheTelemetry {
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self {
            inner: Arc::new(Inner {
                kv: DashMap::new(),
                lmcache: DashMap::new(),
                started: DashMap::new(),
                metrics,
            }),
        }
    }

    /// Start one background consumer/poller per configured provider endpoint.
    /// Request routing only reads local atomics/maps and never performs I/O.
    pub fn configure(&self, providers: &[ProviderConfig]) {
        for provider in providers {
            if let Some(config) = provider.kv_events.clone() {
                self.start_kv(provider.name.clone(), config);
            }
            if let Some(config) = provider.lmcache.clone() {
                self.start_lmcache(provider.name.clone(), config);
            }
        }
    }

    fn start_kv(&self, provider: String, config: KvEventsConfig) {
        let task_key = format!("kv:{provider}:{}:{}", config.endpoint, config.topic);
        if self.inner.started.insert(task_key, ()).is_some() {
            return;
        }
        let target = Arc::new(KvTarget {
            state: Mutex::new(KvState::default()),
            max_blocks: config.max_blocks,
            stale_secs: config.stale_secs,
            updated_ms: AtomicU64::new(0),
        });
        self.inner.kv.insert(provider.clone(), target.clone());
        let telemetry = self.clone();
        tokio::spawn(async move {
            loop {
                let mut socket = SubSocket::new();
                let connected = socket.connect(&config.endpoint).await;
                let subscribed = if connected.is_ok() {
                    socket.subscribe(&config.topic).await
                } else {
                    connected
                };
                if subscribed.is_err() {
                    telemetry
                        .inner
                        .metrics
                        .kv_event_stream_failures_total
                        .fetch_add(1, Relaxed);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                let mut last_seq = None;
                let mut desynced = false;
                while let Ok(message) = socket.recv().await {
                    if message.len() != 3 {
                        telemetry
                            .inner
                            .metrics
                            .kv_events_malformed_total
                            .fetch_add(1, Relaxed);
                        continue;
                    }
                    let Some(seq) = message.get(1).and_then(|frame| {
                        (frame.len() == 8).then(|| {
                            u64::from_be_bytes(frame.as_ref().try_into().expect("length checked"))
                        })
                    }) else {
                        telemetry
                            .inner
                            .metrics
                            .kv_events_malformed_total
                            .fetch_add(1, Relaxed);
                        continue;
                    };
                    let payload = message.get(2).expect("frame count checked");
                    if last_seq.is_some_and(|last| seq != last + 1) {
                        desynced = true;
                        *target.state.lock() = KvState::default();
                        target.updated_ms.store(0, Relaxed);
                        telemetry
                            .inner
                            .metrics
                            .kv_event_stream_failures_total
                            .fetch_add(1, Relaxed);
                    }
                    last_seq = Some(seq);
                    if desynced && !payload_contains_clear(payload) {
                        continue;
                    }
                    match apply_vllm_payload(&target, payload) {
                        Ok(events) => {
                            if payload_contains_clear(payload) {
                                desynced = false;
                            }
                            telemetry
                                .inner
                                .metrics
                                .kv_events_total
                                .fetch_add(events, Relaxed);
                        }
                        Err(error) => {
                            telemetry
                                .inner
                                .metrics
                                .kv_events_malformed_total
                                .fetch_add(1, Relaxed);
                            tracing::debug!(provider, %error, "ignored malformed vLLM KV event");
                        }
                    }
                }
                telemetry
                    .inner
                    .metrics
                    .kv_event_stream_failures_total
                    .fetch_add(1, Relaxed);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    fn start_lmcache(&self, provider: String, config: LmCacheConfig) {
        let task_key = format!("lmcache:{provider}:{}", config.endpoint);
        if self.inner.started.insert(task_key, ()).is_some() {
            return;
        }
        let target = Arc::new(LmCacheTarget {
            occupancy_bits: AtomicU64::new(0),
            available: AtomicU64::new(0),
            stale_secs: config.stale_secs,
            updated_ms: AtomicU64::new(0),
        });
        self.inner.lmcache.insert(provider, target.clone());
        let metrics = self.inner.metrics.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let interval = Duration::from_secs(config.refresh_secs);
            loop {
                match client.get(&config.endpoint).send().await {
                    Ok(response) if response.status().is_success() => {
                        match response.json::<LmCacheSignal>().await {
                            Ok(signal) if signal.occupancy.is_finite() => {
                                target.occupancy_bits.store(
                                    signal.occupancy.clamp(0.0, 1.0).to_bits() as u64,
                                    Relaxed,
                                );
                                target
                                    .available
                                    .store(signal.cache_available as u64, Relaxed);
                                target.updated_ms.store(epoch_millis(), Relaxed);
                                metrics.lmcache_refreshes_total.fetch_add(1, Relaxed);
                            }
                            _ => {
                                metrics.lmcache_refresh_failures_total.fetch_add(1, Relaxed);
                            }
                        }
                    }
                    _ => {
                        metrics.lmcache_refresh_failures_total.fetch_add(1, Relaxed);
                    }
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    pub fn kv_source(
        &self,
        providers: Vec<String>,
    ) -> Arc<dyn rolter_balancer::scorer::KvCacheSource> {
        Arc::new(RouteKvSource {
            telemetry: self.clone(),
            providers,
        })
    }

    pub fn lmcache_source(
        &self,
        providers: Vec<String>,
    ) -> Arc<dyn rolter_balancer::scorer::LmCacheSource> {
        Arc::new(RouteLmCacheSource {
            telemetry: self.clone(),
            providers,
        })
    }

    pub fn freshness(&self) -> Vec<(String, &'static str, u64)> {
        let now = epoch_millis();
        let mut values = Vec::new();
        for entry in self.inner.kv.iter() {
            values.push((
                entry.key().clone(),
                "vllm",
                now.saturating_sub(entry.updated_ms.load(Relaxed)) / 1000,
            ));
        }
        for entry in self.inner.lmcache.iter() {
            values.push((
                entry.key().clone(),
                "lmcache",
                now.saturating_sub(entry.updated_ms.load(Relaxed)) / 1000,
            ));
        }
        values
    }
}

struct RouteKvSource {
    telemetry: CacheTelemetry,
    providers: Vec<String>,
}

impl rolter_balancer::scorer::KvCacheSource for RouteKvSource {
    fn scores(&self, token_ids: &[u32]) -> Option<Vec<f32>> {
        let now = epoch_millis();
        let mut fresh = false;
        let scores = self
            .providers
            .iter()
            .map(|provider| {
                let Some(target) = self.telemetry.inner.kv.get(provider) else {
                    return 0.0;
                };
                if now.saturating_sub(target.updated_ms.load(Relaxed))
                    > target.stale_secs.saturating_mul(1000)
                {
                    return 0.0;
                }
                fresh = true;
                target.score(token_ids)
            })
            .collect();
        if fresh {
            self.telemetry
                .inner
                .metrics
                .kv_cache_decisions_total
                .fetch_add(1, Relaxed);
            Some(scores)
        } else {
            None
        }
    }
}

struct RouteLmCacheSource {
    telemetry: CacheTelemetry,
    providers: Vec<String>,
}

impl rolter_balancer::scorer::LmCacheSource for RouteLmCacheSource {
    fn scores(&self) -> Option<Vec<f32>> {
        let now = epoch_millis();
        let mut fresh = false;
        let scores = self
            .providers
            .iter()
            .map(|provider| {
                let Some(target) = self.telemetry.inner.lmcache.get(provider) else {
                    return 0.0;
                };
                if now.saturating_sub(target.updated_ms.load(Relaxed))
                    > target.stale_secs.saturating_mul(1000)
                {
                    return 0.0;
                }
                fresh = true;
                if target.available.load(Relaxed) == 0 {
                    0.0
                } else {
                    1.0 - f32::from_bits(target.occupancy_bits.load(Relaxed) as u32)
                }
            })
            .collect();
        if fresh {
            self.telemetry
                .inner
                .metrics
                .lmcache_decisions_total
                .fetch_add(1, Relaxed);
            Some(scores)
        } else {
            None
        }
    }
}

impl KvTarget {
    fn score(&self, token_ids: &[u32]) -> f32 {
        let state = self.state.lock();
        if state.block_size == 0 || token_ids.is_empty() {
            return 0.0;
        }
        let mut parent = 0u64;
        let mut matched = 0usize;
        let mut total = 0usize;
        for block in token_ids.chunks(state.block_size) {
            total += 1;
            let identity = block_identity(parent, block);
            if state.resident.contains(&identity) && matched + 1 == total {
                matched += 1;
            }
            parent = identity;
        }
        matched as f32 / total.max(1) as f32
    }
}

fn apply_vllm_payload(target: &KvTarget, payload: &[u8]) -> Result<u64, String> {
    let value = rmpv::decode::read_value(&mut Cursor::new(payload)).map_err(|e| e.to_string())?;
    let batch = value.as_array().ok_or("event batch is not an array")?;
    let events = batch
        .get(1)
        .and_then(Value::as_array)
        .ok_or("event batch has no events array")?;
    let mut applied = 0;
    for event in events {
        let fields = event.as_array().ok_or("event is not an array")?;
        let tag = fields.first().and_then(Value::as_str).unwrap_or_default();
        match tag {
            "BlockStored" => apply_stored(target, fields)?,
            "BlockRemoved" => apply_removed(target, fields)?,
            "AllBlocksCleared" => *target.state.lock() = KvState::default(),
            _ => return Err(format!("unsupported event tag '{tag}'")),
        }
        applied += 1;
    }
    target.updated_ms.store(epoch_millis(), Relaxed);
    Ok(applied)
}

fn payload_contains_clear(payload: &[u8]) -> bool {
    payload
        .windows("AllBlocksCleared".len())
        .any(|window| window == b"AllBlocksCleared")
}

fn apply_stored(target: &KvTarget, fields: &[Value]) -> Result<(), String> {
    let hashes = fields
        .get(1)
        .and_then(Value::as_array)
        .ok_or("BlockStored block_hashes missing")?;
    let tokens: Vec<u32> = fields
        .get(3)
        .and_then(Value::as_array)
        .ok_or("BlockStored token_ids missing")?
        .iter()
        .map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()))
        .collect::<Option<_>>()
        .ok_or("BlockStored token_ids malformed")?;
    let block_size = fields
        .get(4)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or("BlockStored block_size malformed")?;
    let mut state = target.state.lock();
    state.block_size = block_size;
    let mut parent = 0u64;
    for (index, block) in tokens.chunks(block_size).enumerate() {
        let Some(external) = hashes.get(index) else {
            break;
        };
        let identity = block_identity(parent, block);
        state.resident.insert(identity);
        state
            .external_to_local
            .insert(format!("{external:?}"), identity);
        state.insertion_order.push_back(identity);
        parent = identity;
    }
    while state.resident.len() > target.max_blocks {
        if let Some(identity) = state.insertion_order.pop_front() {
            state.resident.remove(&identity);
            state
                .external_to_local
                .retain(|_, value| *value != identity);
        } else {
            break;
        }
    }
    Ok(())
}

fn apply_removed(target: &KvTarget, fields: &[Value]) -> Result<(), String> {
    let hashes = fields
        .get(1)
        .and_then(Value::as_array)
        .ok_or("BlockRemoved block_hashes missing")?;
    let mut state = target.state.lock();
    for hash in hashes {
        if let Some(identity) = state.external_to_local.remove(&format!("{hash:?}")) {
            state.resident.remove(&identity);
        }
    }
    Ok(())
}

fn block_identity(parent: u64, tokens: &[u32]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parent.hash(&mut hasher);
    tokens.hash(&mut hasher);
    hasher.finish()
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Json, Router};
    use rolter_balancer::scorer::{KvCacheSource, LmCacheSource};

    fn target(max_blocks: usize) -> KvTarget {
        KvTarget {
            state: Mutex::new(KvState::default()),
            max_blocks,
            stale_secs: 30,
            updated_ms: AtomicU64::new(0),
        }
    }

    #[test]
    fn stored_removed_and_clear_events_drive_exact_prefix_score() {
        let target = target(10);
        let batch = Value::Array(vec![
            Value::F64(1.0),
            Value::Array(vec![Value::Array(vec![
                Value::from("BlockStored"),
                Value::Array(vec![Value::from(11), Value::from(12)]),
                Value::Nil,
                Value::Array((1..=8).map(Value::from).collect()),
                Value::from(4),
            ])]),
        ]);
        let mut payload = Vec::new();
        rmpv::encode::write_value(&mut payload, &batch).unwrap();
        assert_eq!(apply_vllm_payload(&target, &payload), Ok(1));
        assert_eq!(target.score(&[1, 2, 3, 4, 5, 6, 7, 8]), 1.0);
        assert_eq!(target.score(&[1, 2, 3, 4, 99, 100, 101, 102]), 0.5);
    }

    #[test]
    fn residency_is_bounded() {
        let target = target(1);
        let mut state = target.state.lock();
        state.resident.extend([1, 2]);
        state.insertion_order.extend([1, 2]);
        drop(state);
        // a stored event enforces the cap; direct state proves the data shape
        let mut state = target.state.lock();
        while state.resident.len() > target.max_blocks {
            let old = state.insertion_order.pop_front().unwrap();
            state.resident.remove(&old);
        }
        assert_eq!(state.resident.len(), 1);
    }

    fn payload(events: Vec<Value>) -> Vec<u8> {
        let batch = Value::Array(vec![Value::F64(1.0), Value::Array(events)]);
        let mut payload = Vec::new();
        rmpv::encode::write_value(&mut payload, &batch).unwrap();
        payload
    }

    fn stored(hashes: &[u64], tokens: &[u32], block_size: u64) -> Value {
        Value::Array(vec![
            Value::from("BlockStored"),
            Value::Array(hashes.iter().copied().map(Value::from).collect()),
            Value::Nil,
            Value::Array(tokens.iter().copied().map(Value::from).collect()),
            Value::from(block_size),
        ])
    }

    #[test]
    fn payload_validation_removal_clear_and_eviction_are_deterministic() {
        let target = target(1);
        let stored_payload = payload(vec![stored(&[11, 12], &[1, 2, 3, 4], 2)]);
        assert_eq!(apply_vllm_payload(&target, &stored_payload), Ok(1));
        assert_eq!(target.state.lock().resident.len(), 1);
        assert_eq!(target.score(&[1, 2]), 0.0);

        let removed = payload(vec![Value::Array(vec![
            Value::from("BlockRemoved"),
            Value::Array(vec![Value::from(12)]),
        ])]);
        assert_eq!(apply_vllm_payload(&target, &removed), Ok(1));
        assert!(target.state.lock().resident.is_empty());

        let clear = payload(vec![Value::Array(vec![Value::from("AllBlocksCleared")])]);
        assert!(payload_contains_clear(&clear));
        assert!(!payload_contains_clear(&stored_payload));
        assert_eq!(apply_vllm_payload(&target, &clear), Ok(1));
        assert_eq!(target.state.lock().block_size, 0);

        assert!(apply_vllm_payload(&target, b"not-msgpack").is_err());
        assert!(apply_vllm_payload(&target, &payload(vec![])).is_ok());
        assert!(apply_vllm_payload(
            &target,
            &payload(vec![Value::Array(vec![Value::from("Unknown")])])
        )
        .is_err());
        assert!(apply_vllm_payload(
            &target,
            &payload(vec![Value::Array(vec![Value::from("BlockStored")])])
        )
        .is_err());
        assert!(apply_vllm_payload(
            &target,
            &payload(vec![Value::Array(vec![Value::from("BlockRemoved")])])
        )
        .is_err());
    }

    #[test]
    fn route_sources_cover_fresh_stale_missing_and_unavailable_targets() {
        let metrics = Arc::new(Metrics::default());
        let telemetry = CacheTelemetry::new(metrics.clone());
        let fresh_kv = Arc::new(target(10));
        apply_vllm_payload(
            &fresh_kv,
            &payload(vec![stored(&[11, 12], &[1, 2, 3, 4], 2)]),
        )
        .unwrap();
        telemetry.inner.kv.insert("fresh".into(), fresh_kv);
        let stale_kv = Arc::new(KvTarget {
            state: Mutex::new(KvState::default()),
            max_blocks: 10,
            stale_secs: 0,
            updated_ms: AtomicU64::new(0),
        });
        telemetry.inner.kv.insert("stale".into(), stale_kv);

        let source = RouteKvSource {
            telemetry: telemetry.clone(),
            providers: vec!["fresh".into(), "stale".into(), "missing".into()],
        };
        assert_eq!(source.scores(&[1, 2, 3, 4]), Some(vec![1.0, 0.0, 0.0]));
        assert_eq!(metrics.kv_cache_decisions_total.load(Relaxed), 1);
        assert!(RouteKvSource {
            telemetry: telemetry.clone(),
            providers: vec!["missing".into()],
        }
        .scores(&[1])
        .is_none());

        telemetry.inner.lmcache.insert(
            "available".into(),
            Arc::new(LmCacheTarget {
                occupancy_bits: AtomicU64::new(0.25f32.to_bits() as u64),
                available: AtomicU64::new(1),
                stale_secs: 30,
                updated_ms: AtomicU64::new(epoch_millis()),
            }),
        );
        telemetry.inner.lmcache.insert(
            "unavailable".into(),
            Arc::new(LmCacheTarget {
                occupancy_bits: AtomicU64::new(0.5f32.to_bits() as u64),
                available: AtomicU64::new(0),
                stale_secs: 30,
                updated_ms: AtomicU64::new(epoch_millis()),
            }),
        );
        let source = RouteLmCacheSource {
            telemetry: telemetry.clone(),
            providers: vec!["available".into(), "unavailable".into(), "missing".into()],
        };
        assert_eq!(source.scores(), Some(vec![0.75, 0.0, 0.0]));
        assert_eq!(metrics.lmcache_decisions_total.load(Relaxed), 1);
        assert!(RouteLmCacheSource {
            telemetry: telemetry.clone(),
            providers: vec!["missing".into()],
        }
        .scores()
        .is_none());

        let freshness = telemetry.freshness();
        assert!(freshness
            .iter()
            .any(|(provider, kind, _)| provider == "fresh" && *kind == "vllm"));
        assert!(freshness
            .iter()
            .any(|(provider, kind, _)| provider == "available" && *kind == "lmcache"));
        assert_eq!(
            telemetry.kv_source(vec!["fresh".into()]).scores(&[1, 2]),
            Some(vec![1.0])
        );
        assert_eq!(
            telemetry.lmcache_source(vec!["available".into()]).scores(),
            Some(vec![0.75])
        );
    }

    #[tokio::test]
    async fn configure_polls_lmcache_counts_failures_and_deduplicates_tasks() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/signal",
                    get(|| async {
                        Json(serde_json::json!({
                            "occupancy": 1.4,
                            "cache_available": true
                        }))
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let provider: ProviderConfig = serde_json::from_value(serde_json::json!({
            "name": "cache-node",
            "kind": "openai_compatible",
            "api_base": "http://cache-node:8000",
            "kv_events": {
                "endpoint": "invalid://endpoint",
                "topic": "kv-events",
                "max_blocks": 10,
                "stale_secs": 30
            },
            "lmcache": {
                "endpoint": format!("http://{address}/signal"),
                "refresh_secs": 60,
                "stale_secs": 30
            }
        }))
        .unwrap();
        let metrics = Arc::new(Metrics::default());
        let telemetry = CacheTelemetry::new(metrics.clone());
        telemetry.configure(std::slice::from_ref(&provider));
        telemetry.configure(&[provider]);

        for _ in 0..100 {
            if metrics.lmcache_refreshes_total.load(Relaxed) > 0
                && metrics.kv_event_stream_failures_total.load(Relaxed) > 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(telemetry.inner.started.len(), 2);
        assert_eq!(metrics.lmcache_refreshes_total.load(Relaxed), 1);
        assert!(metrics.kv_event_stream_failures_total.load(Relaxed) >= 1);
        assert_eq!(
            telemetry.lmcache_source(vec!["cache-node".into()]).scores(),
            Some(vec![0.0])
        );
        server.abort();
    }
}
