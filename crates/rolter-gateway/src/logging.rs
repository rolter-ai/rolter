//! Asynchronous, batched request-log writer for ClickHouse.
//!
//! The request handler builds a [`RequestLog`] and hands it to [`LogSink::log`],
//! which only does a non-blocking `try_send` onto a bounded channel — the hot
//! path never awaits ClickHouse. A background task accumulates records and
//! flushes them in batches (on size or a timer) to the ClickHouse HTTP interface
//! using `JSONEachRow`. When the queue is full records are dropped and counted,
//! never blocked on. Token and cost fields are captured in a later phase; this
//! writer establishes the plumbing and the record shape.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::metrics::Metrics;

/// One row of the ClickHouse `request_logs` table. Field names match the column
/// names so the struct serializes directly as a `JSONEachRow` line. `ts` is
/// omitted deliberately — ClickHouse fills it with `now64(3)` on insert.
#[derive(Debug, Clone, Serialize)]
pub struct RequestLog {
    pub request_id: String,
    pub org_id: String,
    pub team_id: String,
    pub project_id: String,
    pub virtual_key_id: String,
    pub model: String,
    pub provider: String,
    pub target: String,
    pub status: u16,
    pub stream: u8,
    pub cache_hit: u8,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cost_usd: f64,
    pub latency_ms: u32,
    pub ttft_ms: u32,
    pub error: String,
}

impl Default for RequestLog {
    fn default() -> Self {
        Self {
            request_id: String::new(),
            org_id: String::new(),
            team_id: String::new(),
            project_id: String::new(),
            virtual_key_id: String::new(),
            model: String::new(),
            provider: String::new(),
            target: String::new(),
            status: 0,
            stream: 0,
            cache_hit: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cost_usd: 0.0,
            latency_ms: 0,
            ttft_ms: 0,
            error: String::new(),
        }
    }
}

/// Handle used by request handlers to emit logs. Cheap to clone.
#[derive(Clone)]
pub struct LogSink {
    tx: Option<mpsc::Sender<RequestLog>>,
    metrics: Arc<Metrics>,
}

impl LogSink {
    /// A sink that discards everything (logging disabled / used in tests).
    pub fn disabled(metrics: Arc<Metrics>) -> Self {
        Self { tx: None, metrics }
    }

    /// Build a sink and spawn the background batch writer targeting the
    /// ClickHouse HTTP endpoint at `clickhouse_url`. Must be called from within
    /// a Tokio runtime.
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
                "{}/?query=INSERT%20INTO%20request_logs%20FORMAT%20JSONEachRow",
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

    /// Enqueue a record without blocking. Drops (and counts) the record if the
    /// queue is full or the writer has stopped.
    pub fn log(&self, record: RequestLog) {
        let Some(tx) = &self.tx else {
            return;
        };
        if tx.try_send(record).is_err() {
            self.metrics.logs_dropped_total.fetch_add(1, Relaxed);
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
    async fn run(self, mut rx: mpsc::Receiver<RequestLog>) {
        let mut batch: Vec<RequestLog> = Vec::with_capacity(self.batch_max);
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

    /// POST the current batch as newline-delimited JSON, then clear it. Errors
    /// are logged and the batch is dropped — logging must never wedge the writer.
    async fn flush(&self, batch: &mut Vec<RequestLog>) {
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
                Err(err) => tracing::warn!(%err, "failed to serialize request log"),
            }
        }
        let count = batch.len() as u64;
        match self.client.post(&self.url).body(body).send().await {
            Ok(resp) if resp.status().is_success() => {
                self.metrics.logs_written_total.fetch_add(count, Relaxed);
            }
            Ok(resp) => {
                let status = resp.status();
                let detail = resp.text().await.unwrap_or_default();
                tracing::warn!(%status, detail, "clickhouse rejected log batch");
                self.metrics.logs_dropped_total.fetch_add(count, Relaxed);
            }
            Err(err) => {
                tracing::warn!(%err, "failed to write log batch to clickhouse");
                self.metrics.logs_dropped_total.fetch_add(count, Relaxed);
            }
        }
        batch.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_log_serializes_without_ts() {
        let rec = RequestLog {
            request_id: "req-1".to_string(),
            model: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            status: 200,
            stream: 1,
            latency_ms: 42,
            ..Default::default()
        };
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&rec).unwrap()).unwrap();
        // ts is filled by clickhouse, never serialized by us
        assert!(value.get("ts").is_none());
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["status"], 200);
        assert_eq!(value["stream"], 1);
        assert_eq!(value["latency_ms"], 42);
        // unset numeric fields default to 0, not null
        assert_eq!(value["total_tokens"], 0);
        assert_eq!(value["cost_usd"], 0.0);
    }

    #[test]
    fn disabled_sink_is_a_noop() {
        let metrics = Arc::new(Metrics::default());
        let sink = LogSink::disabled(metrics.clone());
        sink.log(RequestLog::default());
        // no queue, nothing written or dropped
        assert_eq!(metrics.logs_dropped_total.load(Relaxed), 0);
        assert_eq!(metrics.logs_written_total.load(Relaxed), 0);
    }

    #[tokio::test]
    async fn writes_batch_as_jsoneachrow_over_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // minimal one-shot http server standing in for clickhouse
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
        let sink = LogSink::spawn(
            format!("http://{addr}"),
            10,
            Duration::from_millis(50),
            100,
            metrics.clone(),
        );
        sink.log(RequestLog {
            request_id: "req-xyz".to_string(),
            model: "gpt-4o".to_string(),
            ..Default::default()
        });

        let req = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        // the query targets the request_logs table via JSONEachRow
        assert!(req.contains("INSERT%20INTO%20request_logs%20FORMAT%20JSONEachRow"));
        // and the body carries our serialized row
        assert!(req.contains("\"request_id\":\"req-xyz\""));
        assert!(req.contains("\"model\":\"gpt-4o\""));

        // give the writer a moment to record the success
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(metrics.logs_written_total.load(Relaxed), 1);
    }

    #[tokio::test]
    async fn full_queue_drops_and_counts() {
        // capacity 1, tiny flush window; fill past capacity synchronously before
        // the writer can drain (the writer will fail to reach a fake url, but the
        // drop path we assert here is the try_send overflow)
        let metrics = Arc::new(Metrics::default());
        let sink = LogSink::spawn(
            "http://127.0.0.1:1".to_string(),
            1000,
            Duration::from_secs(3600),
            1,
            metrics.clone(),
        );
        for _ in 0..500 {
            sink.log(RequestLog::default());
        }
        assert!(metrics.logs_dropped_total.load(Relaxed) > 0);
    }
}
