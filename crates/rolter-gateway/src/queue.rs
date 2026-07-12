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

use crate::metrics::Metrics;

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
    },
    Raw {
        provider: ProviderConfig,
        path: String,
        body: Bytes,
        content_type: String,
        api_key: Option<String>,
        trace_headers: Vec<(String, String)>,
        reply: oneshot::Sender<Result<reqwest::Response>>,
    },
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
        let sender = spawn_queue(config.clone(), self.forwarder.clone());
        self.queues.insert(
            provider.to_string(),
            QueueEntry {
                config: config.clone(),
                sender: sender.clone(),
            },
        );
        sender
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
            while let Some(job) = { receiver.lock().await.recv().await } {
                run_job(&forwarder, job).await;
            }
        });
    }
    sender
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
        } => {
            let headers = borrowed_headers(&trace_headers);
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
        } => {
            let headers = borrowed_headers(&trace_headers);
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
}
