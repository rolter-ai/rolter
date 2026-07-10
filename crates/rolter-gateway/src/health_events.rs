//! Asynchronous, batched writer for the `provider_health_events` table (ROL-197).
//!
//! Every health signal — active probe sweeps, the passive request funnel, and
//! (later) opt-in llm-call and status-page sources — funnels a [`HealthEvent`]
//! through [`HealthEventSink::emit`], which only does a non-blocking `try_send`
//! onto a bounded channel. A background task accumulates records and flushes them
//! to ClickHouse in batches (on size or a timer) using `JSONEachRow`, exactly
//! like the request-log writer. When the queue is full records are dropped and
//! counted, never blocked on. This feeds the uptime %/MTTR rollups in ROL-198.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::metrics::Metrics;

/// Which signal produced a health observation. Serializes to the string names of
/// the ClickHouse `source` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthSource {
    /// derived from real proxied traffic completing (the request funnel)
    Passive,
    /// an active liveness probe sweep
    Probe,
    /// opt-in tracking of a dedicated llm call (ROL-199)
    LlmCall,
    /// a provider status-page secondary signal (ROL-200)
    StatusPage,
}

/// The observed result of a health signal. Serializes to the string names of the
/// ClickHouse `outcome` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthOutcome {
    Ok,
    Error,
    Timeout,
}

/// One row of the `provider_health_events` table. Field names match the column
/// names so the struct serializes directly as a `JSONEachRow` line. `ts` is
/// omitted deliberately — ClickHouse fills it with `now64(3)` on insert.
#[derive(Debug, Clone, Serialize)]
pub struct HealthEvent {
    pub target_id: String,
    pub provider: String,
    pub source: HealthSource,
    pub outcome: HealthOutcome,
    /// upstream http status when there was one; `None` for connect/timeout errors
    pub status_code: Option<u16>,
    pub latency_ms: u32,
    /// coarse error label (e.g. "timeout", "rate_limited"); `None` on success
    pub error_kind: Option<String>,
}

/// Handle used across the gateway to emit health events. Cheap to clone.
#[derive(Clone)]
pub struct HealthEventSink {
    tx: Option<mpsc::Sender<HealthEvent>>,
    metrics: Arc<Metrics>,
}

impl HealthEventSink {
    /// A sink that discards everything (writing disabled / used in tests).
    pub fn disabled(metrics: Arc<Metrics>) -> Self {
        Self { tx: None, metrics }
    }

    /// Build a sink and spawn the background batch writer targeting the
    /// ClickHouse HTTP endpoint at `clickhouse_url`. Must be called from within a
    /// Tokio runtime.
    pub fn spawn(
        clickhouse_url: String,
        batch_max: usize,
        flush: Duration,
        queue_capacity: usize,
        metrics: Arc<Metrics>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(queue_capacity.max(1));
        let writer = BatchWriter {
            url: format!(
                "{}/?query=INSERT%20INTO%20provider_health_events%20FORMAT%20JSONEachRow",
                clickhouse_url.trim_end_matches('/')
            ),
            client: reqwest::Client::new(),
            batch_max: batch_max.max(1),
            flush,
            metrics: metrics.clone(),
        };
        tokio::spawn(writer.run(rx));
        Self {
            tx: Some(tx),
            metrics,
        }
    }

    /// Enqueue an event without blocking. Drops (and counts) the record if the
    /// queue is full or the writer has stopped. A no-op on a disabled sink.
    pub fn emit(&self, event: HealthEvent) {
        let Some(tx) = &self.tx else {
            return;
        };
        if tx.try_send(event).is_err() {
            self.metrics
                .health_events_dropped_total
                .fetch_add(1, Relaxed);
        }
    }
}

/// Owns the batching loop and the ClickHouse HTTP client.
struct BatchWriter {
    url: String,
    client: reqwest::Client,
    batch_max: usize,
    flush: Duration,
    metrics: Arc<Metrics>,
}

impl BatchWriter {
    async fn run(self, mut rx: mpsc::Receiver<HealthEvent>) {
        let mut batch: Vec<HealthEvent> = Vec::with_capacity(self.batch_max);
        let mut ticker = tokio::time::interval(self.flush);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                maybe = rx.recv() => match maybe {
                    Some(record) => {
                        batch.push(record);
                        if batch.len() >= self.batch_max {
                            self.flush(&mut batch).await;
                        }
                    }
                    // all senders dropped: flush remainder and stop
                    None => {
                        self.flush(&mut batch).await;
                        break;
                    }
                },
                _ = ticker.tick() => {
                    if !batch.is_empty() {
                        self.flush(&mut batch).await;
                    }
                }
            }
        }
    }

    /// POST the current batch as newline-delimited JSON, then clear it. Errors are
    /// logged and the batch dropped — health accounting must never wedge the writer.
    async fn flush(&self, batch: &mut Vec<HealthEvent>) {
        if batch.is_empty() {
            return;
        }
        let mut body = String::new();
        for record in batch.iter() {
            match serde_json::to_string(record) {
                Ok(line) => {
                    body.push_str(&line);
                    body.push('\n');
                }
                Err(err) => tracing::warn!(%err, "failed to serialize health event"),
            }
        }
        let count = batch.len() as u64;
        match self.client.post(&self.url).body(body).send().await {
            Ok(resp) if resp.status().is_success() => {
                self.metrics
                    .health_events_written_total
                    .fetch_add(count, Relaxed);
            }
            Ok(resp) => {
                let status = resp.status();
                let detail = resp.text().await.unwrap_or_default();
                tracing::warn!(%status, detail, "clickhouse rejected health event batch");
                self.metrics
                    .health_events_dropped_total
                    .fetch_add(count, Relaxed);
            }
            Err(err) => {
                tracing::warn!(%err, "failed to write health event batch to clickhouse");
                self.metrics
                    .health_events_dropped_total
                    .fetch_add(count, Relaxed);
            }
        }
        batch.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> HealthEvent {
        HealthEvent {
            target_id: "openai/gpt-4o".to_string(),
            provider: "openai".to_string(),
            source: HealthSource::Probe,
            outcome: HealthOutcome::Ok,
            status_code: Some(200),
            latency_ms: 12,
            error_kind: None,
        }
    }

    #[test]
    fn serializes_without_ts_with_enum_names() {
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&event()).unwrap()).unwrap();
        // ts is filled by clickhouse, never serialized by us
        assert!(value.get("ts").is_none());
        assert_eq!(value["target_id"], "openai/gpt-4o");
        assert_eq!(value["source"], "probe");
        assert_eq!(value["outcome"], "ok");
        assert_eq!(value["status_code"], 200);
        // absent error is null, matching the Nullable column
        assert!(value["error_kind"].is_null());
    }

    #[test]
    fn all_enum_variants_map_to_snake_case() {
        let src = |s: HealthSource| serde_json::to_value(s).unwrap();
        assert_eq!(src(HealthSource::Passive), "passive");
        assert_eq!(src(HealthSource::Probe), "probe");
        assert_eq!(src(HealthSource::LlmCall), "llm_call");
        assert_eq!(src(HealthSource::StatusPage), "status_page");
        let out = |o: HealthOutcome| serde_json::to_value(o).unwrap();
        assert_eq!(out(HealthOutcome::Ok), "ok");
        assert_eq!(out(HealthOutcome::Error), "error");
        assert_eq!(out(HealthOutcome::Timeout), "timeout");
    }

    #[test]
    fn disabled_sink_is_a_noop() {
        let metrics = Arc::new(Metrics::default());
        let sink = HealthEventSink::disabled(metrics.clone());
        sink.emit(event());
        assert_eq!(metrics.health_events_dropped_total.load(Relaxed), 0);
        assert_eq!(metrics.health_events_written_total.load(Relaxed), 0);
    }

    #[tokio::test]
    async fn writes_batch_as_jsoneachrow_over_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            req
        });

        let metrics = Arc::new(Metrics::default());
        let sink = HealthEventSink::spawn(
            format!("http://{addr}"),
            10,
            Duration::from_millis(50),
            100,
            metrics.clone(),
        );
        sink.emit(HealthEvent {
            outcome: HealthOutcome::Error,
            status_code: Some(429),
            error_kind: Some("rate_limited".to_string()),
            ..event()
        });

        let req = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        assert!(req.contains("INSERT%20INTO%20provider_health_events%20FORMAT%20JSONEachRow"));
        assert!(req.contains("\"provider\":\"openai\""));
        assert!(req.contains("\"outcome\":\"error\""));
        assert!(req.contains("\"error_kind\":\"rate_limited\""));

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(metrics.health_events_written_total.load(Relaxed), 1);
    }

    #[tokio::test]
    async fn full_queue_drops_and_counts() {
        let metrics = Arc::new(Metrics::default());
        let sink = HealthEventSink::spawn(
            "http://127.0.0.1:1".to_string(),
            1000,
            Duration::from_secs(3600),
            1,
            metrics.clone(),
        );
        for _ in 0..500 {
            sink.emit(event());
        }
        assert!(metrics.health_events_dropped_total.load(Relaxed) > 0);
    }
}
