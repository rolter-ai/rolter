//! Isolated bounded dispatch queues for upstream providers.
//!
//! A request occupies a worker only until the upstream response headers arrive.
//! The response body then streams directly to the client, preserving the
//! gateway's existing SSE behaviour while preventing a provider that stalls at
//! admission from consuming every request task.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use dashmap::DashMap;
use rolter_core::{BackpressurePolicy, Error, ProviderConfig, QueueConfig, Result};
use rolter_proxy::Forwarder;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::Instrument;

use crate::metrics::{InflightGuard, Metrics, QueuedGuard};

#[derive(Clone)]
pub struct ProviderQueues {
    forwarder: Arc<Forwarder>,
    metrics: Arc<Metrics>,
    queues: Arc<DashMap<String, QueueEntry>>,
}

#[derive(Clone)]
struct QueueEntry {
    config: QueueConfig,
    sender: mpsc::Sender<Job>,
}

enum Job {
    Json {
        provider: ProviderConfig,
        path: String,
        body: Bytes,
        api_key: Option<String>,
        upstream_model: Option<String>,
        trace_headers: Vec<(String, String)>,
        reply: oneshot::Sender<Result<reqwest::Response>>,
        wait: tracing::Span,
        parent: tracing::Span,
        /// counts the job in its provider's queue depth until a worker takes it
        queued: QueuedGuard,
    },
    Raw {
        provider: ProviderConfig,
        path: String,
        body: Bytes,
        content_type: String,
        api_key: Option<String>,
        trace_headers: Vec<(String, String)>,
        reply: oneshot::Sender<Result<reqwest::Response>>,
        wait: tracing::Span,
        parent: tracing::Span,
        /// counts the job in its provider's queue depth until a worker takes it
        queued: QueuedGuard,
    },
}

/// Span covering the time a job sits in the provider queue (#805).
///
/// Created on the caller's task inside its `upstream.request` span and dropped
/// by the worker the moment it picks the job up, so its duration is the queue
/// wait itself — not the wait plus the upstream call, which is what wrapping
/// the whole dispatch would have measured. Disabled (and free) when no OTLP
/// pipeline is installed, and never created at all when queueing is off.
fn queue_wait_span(provider: &str) -> tracing::Span {
    crate::trace::stage_span!("queue.wait", provider = %provider)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueError {
    Dropped,
    Full,
    Timeout,
    Closed,
}

impl QueueError {
    fn message(self) -> &'static str {
        match self {
            Self::Dropped => "provider queue request dropped",
            Self::Full => "provider queue full",
            Self::Timeout => "provider queue wait timed out",
            Self::Closed => "provider queue worker stopped",
        }
    }
}

impl ProviderQueues {
    pub fn new(forwarder: Arc<Forwarder>, metrics: Arc<Metrics>) -> Self {
        Self {
            forwarder,
            metrics,
            queues: Arc::new(DashMap::new()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn forward_json(
        &self,
        config: &QueueConfig,
        provider: &ProviderConfig,
        path: &str,
        body: Bytes,
        api_key: Option<&str>,
        upstream_model: Option<&str>,
        trace_headers: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        if !config.enabled {
            let _inflight = InflightGuard::new(self.metrics.provider_load(&provider.name));
            return self
                .forwarder
                .forward_json(provider, path, body, api_key, upstream_model, trace_headers)
                .await;
        }
        let (reply, result) = oneshot::channel();
        let job = Job::Json {
            provider: provider.clone(),
            path: path.to_string(),
            body,
            api_key: api_key.map(str::to_string),
            upstream_model: upstream_model.map(str::to_string),
            trace_headers: owned_headers(trace_headers),
            reply,
            wait: queue_wait_span(&provider.name),
            // the worker runs on its own task, where nothing is in scope: carry
            // the caller's span across so the forwarder's own stages land under
            // `upstream.request` instead of becoming orphan roots (#805)
            parent: tracing::Span::current(),
            queued: QueuedGuard::new(self.metrics.provider_load(&provider.name)),
        };
        self.dispatch(config, &provider.name, job, result).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn forward_raw(
        &self,
        config: &QueueConfig,
        provider: &ProviderConfig,
        path: &str,
        body: Bytes,
        content_type: &str,
        api_key: Option<&str>,
        trace_headers: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        if !config.enabled {
            let _inflight = InflightGuard::new(self.metrics.provider_load(&provider.name));
            return self
                .forwarder
                .forward_raw(provider, path, body, content_type, api_key, trace_headers)
                .await;
        }
        let (reply, result) = oneshot::channel();
        let job = Job::Raw {
            provider: provider.clone(),
            path: path.to_string(),
            body,
            content_type: content_type.to_string(),
            api_key: api_key.map(str::to_string),
            trace_headers: owned_headers(trace_headers),
            reply,
            wait: queue_wait_span(&provider.name),
            // the worker runs on its own task, where nothing is in scope: carry
            // the caller's span across so the forwarder's own stages land under
            // `upstream.request` instead of becoming orphan roots (#805)
            parent: tracing::Span::current(),
            queued: QueuedGuard::new(self.metrics.provider_load(&provider.name)),
        };
        self.dispatch(config, &provider.name, job, result).await
    }

    async fn dispatch(
        &self,
        config: &QueueConfig,
        provider: &str,
        job: Job,
        result: oneshot::Receiver<Result<reqwest::Response>>,
    ) -> Result<reqwest::Response> {
        let sender = self.sender_for(provider, config);
        if let Err(err) = enqueue(&sender, job, config).await {
            self.record_rejection(err);
            return Err(Error::Upstream(err.message().to_string()));
        }
        result
            .await
            .map_err(|_| Error::Upstream(QueueError::Closed.message().to_string()))?
    }

    fn sender_for(&self, provider: &str, config: &QueueConfig) -> mpsc::Sender<Job> {
        if let Some(entry) = self.queues.get(provider) {
            if entry.config == *config {
                return entry.sender.clone();
            }
        }
        // decided under the entry's lock, so a burst of first calls to a
        // provider — or of calls straddling a config change — shares one new
        // queue. Checking and then inserting let each caller in the burst
        // spawn a queue of its own, and a provider with `workers = 2` took
        // five calls at once (#1815). Spawning never blocks, so holding the
        // shard across it is brief
        let mut entry = self
            .queues
            .entry(provider.to_string())
            .or_insert_with(|| QueueEntry {
                config: config.clone(),
                sender: spawn_queue(config.clone(), self.forwarder.clone()),
            });
        if entry.config != *config {
            *entry = QueueEntry {
                config: config.clone(),
                sender: spawn_queue(config.clone(), self.forwarder.clone()),
            };
        }
        entry.sender.clone()
    }

    fn record_rejection(&self, err: QueueError) {
        match err {
            QueueError::Dropped | QueueError::Full | QueueError::Closed => {
                self.metrics
                    .provider_queue_rejections_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            QueueError::Timeout => {
                self.metrics
                    .provider_queue_timeouts_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

fn spawn_queue(config: QueueConfig, forwarder: Arc<Forwarder>) -> mpsc::Sender<Job> {
    let (sender, receiver) = mpsc::channel(config.capacity);
    let receiver = Arc::new(Mutex::new(receiver));
    for _ in 0..config.workers {
        let receiver = receiver.clone();
        let forwarder = forwarder.clone();
        tokio::spawn(async move {
            while let Some(job) = next_job(&receiver).await {
                run_job(&forwarder, job).await;
            }
        });
    }
    sender
}

/// Take the next job off a receiver the provider's workers share.
///
/// The lock is held while this worker waits for a job and released before the
/// job runs, so each worker carries its own upstream call (#1815). It is a
/// function of its own because the inline spelling — `while let Some(job) =
/// receiver.lock().await.recv().await { … }` — keeps the guard alive until the
/// end of the loop body: under edition 2021 a temporary in the `while let`
/// scrutinee lives that long. Every worker then held the receiver across its
/// whole upstream call, and a provider with eight workers served one request
/// at a time.
async fn next_job(receiver: &Mutex<mpsc::Receiver<Job>>) -> Option<Job> {
    receiver.lock().await.recv().await
}

async fn run_job(forwarder: &Forwarder, job: Job) {
    match job {
        Job::Json {
            provider,
            path,
            body,
            api_key,
            upstream_model,
            trace_headers,
            reply,
            wait,
            parent,
            queued,
        } => {
            // the job is off the queue: close the wait span before doing any work,
            // and move it from the queue depth to the provider's in-flight count
            drop(wait);
            let _inflight = queued.picked();
            let headers = borrowed_headers(&trace_headers);
            // `.instrument`, not `.enter()`: an entered guard is `!Send` and this
            // future is spawned onto the worker task
            let _ = reply.send(
                forwarder
                    .forward_json(
                        &provider,
                        &path,
                        body,
                        api_key.as_deref(),
                        upstream_model.as_deref(),
                        &headers,
                    )
                    .instrument(parent)
                    .await,
            );
        }
        Job::Raw {
            provider,
            path,
            body,
            content_type,
            api_key,
            trace_headers,
            reply,
            wait,
            parent,
            queued,
        } => {
            // the job is off the queue: close the wait span before doing any work,
            // and move it from the queue depth to the provider's in-flight count
            drop(wait);
            let _inflight = queued.picked();
            let headers = borrowed_headers(&trace_headers);
            // `.instrument`, not `.enter()`: an entered guard is `!Send` and this
            // future is spawned onto the worker task
            let _ = reply.send(
                forwarder
                    .forward_raw(
                        &provider,
                        &path,
                        body,
                        &content_type,
                        api_key.as_deref(),
                        &headers,
                    )
                    .instrument(parent)
                    .await,
            );
        }
    }
}

fn owned_headers(headers: &[(&str, &str)]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

fn borrowed_headers(headers: &[(String, String)]) -> Vec<(&str, &str)> {
    headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

async fn enqueue<T>(
    sender: &mpsc::Sender<T>,
    item: T,
    config: &QueueConfig,
) -> std::result::Result<(), QueueError> {
    match config.backpressure {
        BackpressurePolicy::Drop => sender.try_send(item).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => QueueError::Dropped,
            mpsc::error::TrySendError::Closed(_) => QueueError::Closed,
        }),
        BackpressurePolicy::Error => sender.try_send(item).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => QueueError::Full,
            mpsc::error::TrySendError::Closed(_) => QueueError::Closed,
        }),
        BackpressurePolicy::Block => match tokio::time::timeout(
            Duration::from_millis(config.block_timeout_ms),
            sender.send(item),
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(QueueError::Closed),
            Err(_) => Err(QueueError::Timeout),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn config(policy: BackpressurePolicy) -> QueueConfig {
        QueueConfig {
            capacity: 1,
            workers: 1,
            backpressure: policy,
            block_timeout_ms: 5,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn error_policy_rejects_a_full_queue_without_waiting() {
        let (sender, _receiver) = mpsc::channel(1);
        sender.send(()).await.unwrap();
        assert_eq!(
            enqueue(&sender, (), &config(BackpressurePolicy::Error)).await,
            Err(QueueError::Full)
        );
    }

    #[tokio::test]
    async fn drop_policy_marks_a_full_queue_as_shed() {
        let (sender, _receiver) = mpsc::channel(1);
        sender.send(()).await.unwrap();
        assert_eq!(
            enqueue(&sender, (), &config(BackpressurePolicy::Drop)).await,
            Err(QueueError::Dropped)
        );
    }

    #[tokio::test]
    async fn block_policy_times_out_when_a_queue_stays_full() {
        let (sender, _receiver) = mpsc::channel(1);
        sender.send(()).await.unwrap();
        assert_eq!(
            enqueue(&sender, (), &config(BackpressurePolicy::Block)).await,
            Err(QueueError::Timeout)
        );
    }

    /// An upstream that holds every call for a while and records the most it
    /// ever held at once.
    #[derive(Clone, Default)]
    struct Held {
        now: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    async fn held_upstream(held: Held) -> String {
        async fn slow(axum::extract::State(held): axum::extract::State<Held>) -> &'static str {
            let now = held.now.fetch_add(1, Ordering::SeqCst) + 1;
            held.peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(300)).await;
            held.now.fetch_sub(1, Ordering::SeqCst);
            "{}"
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new()
            .route("/v1/chat/completions", axum::routing::post(slow))
            .with_state(held);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    /// `workers = N` puts N calls in flight to one provider at once (#1815).
    /// Each worker used to hold the shared receiver across its whole upstream
    /// call, so a provider's calls ran one at a time however many workers it
    /// had, and latency grew linearly with concurrency.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn every_worker_carries_its_own_upstream_call() {
        let held = Held::default();
        let provider = ProviderConfig {
            name: "slow".into(),
            api_base: held_upstream(held.clone()).await,
            ..Default::default()
        };
        let config = QueueConfig {
            enabled: true,
            capacity: 16,
            workers: 4,
            backpressure: BackpressurePolicy::Block,
            block_timeout_ms: 5_000,
        };
        let metrics = Arc::new(Metrics::default());
        let queues = ProviderQueues::new(Arc::new(Forwarder::new()), metrics.clone());
        // one call first, so the burst below lands on one established queue
        // and nothing but its workers decides how many calls run at once
        assert_eq!(call(&queues, &config, &provider).await, Some(200));
        held.peak.store(0, Ordering::SeqCst);

        let started = std::time::Instant::now();
        let calls: Vec<_> = (0..4)
            .map(|_| {
                let (queues, config, provider) = (queues.clone(), config.clone(), provider.clone());
                tokio::spawn(async move { call(&queues, &config, &provider).await })
            })
            .collect();
        for call in calls {
            assert_eq!(call.await.unwrap(), Some(200));
        }
        assert_eq!(
            held.peak.load(Ordering::SeqCst),
            4,
            "four workers must hold four calls at once, not one after another"
        );
        // one after another would take four holds, 1.2s
        assert!(
            started.elapsed() < Duration::from_millis(1_000),
            "{:?}",
            started.elapsed()
        );
        let out = metrics.render();
        assert!(out.contains("rolter_provider_queue_wait_ms_count{provider=\"slow\"} 5"));
        assert!(out.contains("rolter_provider_inflight{provider=\"slow\"} 0"));
    }

    /// One chat call through `queues`; the upstream status, or `None` when
    /// the call failed.
    async fn call(
        queues: &ProviderQueues,
        config: &QueueConfig,
        provider: &ProviderConfig,
    ) -> Option<u16> {
        queues
            .forward_json(
                config,
                provider,
                "/v1/chat/completions",
                Bytes::from_static(br#"{"model":"m","messages":[]}"#),
                None,
                None,
                &[],
            )
            .await
            .ok()
            .map(|response| response.status().as_u16())
    }

    /// The workers bound the calls too: past their number a call waits for a
    /// free worker instead of going upstream alongside the others. Six calls
    /// arriving together are the first a provider sees, which used to spawn a
    /// queue per caller and let five through at once (#1815).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn calls_beyond_the_worker_count_wait_their_turn() {
        let held = Held::default();
        let provider = ProviderConfig {
            name: "slow".into(),
            api_base: held_upstream(held.clone()).await,
            ..Default::default()
        };
        let config = QueueConfig {
            enabled: true,
            capacity: 16,
            workers: 2,
            backpressure: BackpressurePolicy::Block,
            block_timeout_ms: 5_000,
        };
        let queues = ProviderQueues::new(Arc::new(Forwarder::new()), Arc::new(Metrics::default()));
        let calls: Vec<_> = (0..6)
            .map(|_| {
                let (queues, config, provider) = (queues.clone(), config.clone(), provider.clone());
                tokio::spawn(async move { call(&queues, &config, &provider).await })
            })
            .collect();
        for call in calls {
            assert_eq!(call.await.unwrap(), Some(200));
        }
        assert_eq!(held.peak.load(Ordering::SeqCst), 2);
    }
}
