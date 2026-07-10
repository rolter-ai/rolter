//! Asynchronous, batched request-log writer for ClickHouse.
//!
//! The request handler builds a [`RequestLog`] and hands it to [`LogSink::log`],
//! which only does a non-blocking `try_send` onto a bounded channel — the hot
//! path never awaits ClickHouse. A background task accumulates records and
//! flushes them in batches (on size or a timer) to the ClickHouse HTTP interface
//! using `JSONEachRow`. When the queue is full records are dropped and counted,
//! never blocked on. Token and cost fields are captured in a later phase; this
//! writer establishes the plumbing and the record shape.

use std::pin::Pin;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::Stream;
use serde::Serialize;
use serde_json::Value;
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
    /// chosen variant name for A/B attribution; empty on the classic single-pool
    /// path (a route with no variants)
    pub variant: String,
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
            variant: String::new(),
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

/// Token usage extracted from an upstream response.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub prompt: u32,
    pub completion: u32,
    pub total: u32,
}

/// Extract token usage from a fully-buffered upstream response body.
///
/// Handles both OpenAI (`prompt_tokens`/`completion_tokens`/`total_tokens`) and
/// Anthropic (`input_tokens`/`output_tokens`, top-level or under `message`) key
/// styles, for non-streamed JSON and SSE. For SSE every `data:` object is
/// scanned and the largest values are kept, since streamed usage is cumulative
/// or reported once at the end (OpenAI final chunk, Anthropic
/// `message_start`/`message_delta`). `total` falls back to `prompt + completion`
/// when the upstream does not report it.
pub fn parse_usage(is_sse: bool, buf: &[u8]) -> Usage {
    let mut usage = Usage::default();
    if is_sse {
        for line in buf.split(|&b| b == b'\n') {
            let line = trim_ascii(line);
            let Some(rest) = line.strip_prefix(b"data:") else {
                continue;
            };
            let rest = trim_ascii(rest);
            if rest == b"[DONE]" {
                continue;
            }
            if let Ok(value) = serde_json::from_slice::<Value>(rest) {
                merge_usage(&mut usage, &value);
            }
        }
    } else if let Ok(value) = serde_json::from_slice::<Value>(buf) {
        merge_usage(&mut usage, &value);
    }
    if usage.total == 0 {
        usage.total = usage.prompt.saturating_add(usage.completion);
    }
    usage
}

fn trim_ascii(mut b: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = b {
        if first.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = b {
        if last.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    b
}

/// Merge any usage numbers found in `value` into `usage`, keeping the max of
/// each field (streamed usage is cumulative or final-only).
fn merge_usage(usage: &mut Usage, value: &Value) {
    // usage can sit at the top level (openai, anthropic non-stream / message_delta)
    // or under `message` (anthropic message_start event)
    for holder in [value.get("usage"), value.pointer("/message/usage")] {
        let Some(u) = holder else { continue };
        let prompt = u32_field(u, "prompt_tokens").or_else(|| u32_field(u, "input_tokens"));
        let completion =
            u32_field(u, "completion_tokens").or_else(|| u32_field(u, "output_tokens"));
        if let Some(p) = prompt {
            usage.prompt = usage.prompt.max(p);
        }
        if let Some(c) = completion {
            usage.completion = usage.completion.max(c);
        }
        if let Some(t) = u32_field(u, "total_tokens") {
            usage.total = usage.total.max(t);
        }
    }
}

fn u32_field(value: &Value, key: &str) -> Option<u32> {
    value.get(key).and_then(|v| v.as_u64()).map(|n| n as u32)
}

/// Response body stream that forwards each chunk to the client unchanged while
/// buffering the whole body, then on end-of-stream parses token usage, stamps
/// latency/ttft and emits the completed [`RequestLog`] exactly once.
pub struct UsageLoggingStream {
    inner: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    buf: Vec<u8>,
    is_sse: bool,
    started: Instant,
    ttft_ms: Option<u32>,
    sink: LogSink,
    price: Option<rolter_core::ModelPriceConfig>,
    // records the request's cost against its budgets once cost_usd is known
    recorder: Option<crate::budgets::SpendRecorder>,
    // records the request's tokens against its rate limits once usage is known
    token_recorder: Option<crate::rate_limits::TokenRecorder>,
    // held for the stream's lifetime; decrements the target's in-flight count on
    // drop (stream end or client disconnect)
    _inflight_guard: Option<crate::load::LoadGuard>,
    // taken and emitted once the stream ends
    pending: Option<RequestLog>,
}

impl UsageLoggingStream {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inner: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
        is_sse: bool,
        started: Instant,
        sink: LogSink,
        price: Option<rolter_core::ModelPriceConfig>,
        log: RequestLog,
        recorder: Option<crate::budgets::SpendRecorder>,
        token_recorder: Option<crate::rate_limits::TokenRecorder>,
        inflight_guard: Option<crate::load::LoadGuard>,
    ) -> Self {
        Self {
            inner,
            buf: Vec::new(),
            is_sse,
            started,
            ttft_ms: None,
            sink,
            price,
            recorder,
            token_recorder,
            _inflight_guard: inflight_guard,
            pending: Some(log),
        }
    }

    fn finalize(&mut self) {
        let Some(mut log) = self.pending.take() else {
            return;
        };
        let usage = parse_usage(self.is_sse, &self.buf);
        log.prompt_tokens = usage.prompt;
        log.completion_tokens = usage.completion;
        log.total_tokens = usage.total;
        // cache_hit accounting arrives with the response-cache phase; price the
        // full prompt as fresh input for now
        log.cost_usd = self
            .price
            .as_ref()
            .map(|p| p.cost_usd(usage.prompt, usage.completion, 0))
            .unwrap_or(0.0);
        log.latency_ms = self.started.elapsed().as_millis() as u32;
        log.ttft_ms = self.ttft_ms.unwrap_or(log.latency_ms);
        // add this request's cost to its budget counters (async, fire-and-forget
        // so finalize stays sync and never blocks the response path)
        if let Some(recorder) = self.recorder.take() {
            let cost = log.cost_usd;
            if cost > 0.0 {
                tokio::spawn(async move { recorder.record(cost).await });
            }
        }
        // add this request's tokens to its rate-limit windows (async, same as
        // above); uses total tokens so a single big request counts against tpm
        if let Some(token_recorder) = self.token_recorder.take() {
            let tokens = log.total_tokens as u64;
            if tokens > 0 {
                tokio::spawn(async move { token_recorder.record(tokens).await });
            }
        }
        self.sink.log(log);
    }
}

impl Stream for UsageLoggingStream {
    type Item = reqwest::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if self.ttft_ms.is_none() {
                    self.ttft_ms = Some(self.started.elapsed().as_millis() as u32);
                }
                self.buf.extend_from_slice(&chunk);
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err))),
            Poll::Ready(None) => {
                self.finalize();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for UsageLoggingStream {
    // if the client disconnects mid-stream the stream is dropped without a final
    // None poll; still emit what we have so the request is not lost
    fn drop(&mut self) {
        self.finalize();
    }
}

/// Derive a passive [`HealthEvent`](crate::health_events::HealthEvent) from a
/// completed request. A 2xx is `ok`; a timed-out upstream is `timeout`; anything
/// else is `error`. `status` 0 means the request never reached the upstream
/// (connect failure), so no status code is reported.
fn passive_health_event(record: &RequestLog) -> crate::health_events::HealthEvent {
    use crate::health_events::{HealthEvent, HealthOutcome, HealthSource};
    let ok = (200..300).contains(&record.status);
    let timed_out = record.error.contains("timed out") || record.error.contains("timeout");
    let outcome = if ok {
        HealthOutcome::Ok
    } else if timed_out {
        HealthOutcome::Timeout
    } else {
        HealthOutcome::Error
    };
    let error_kind = match outcome {
        HealthOutcome::Ok => None,
        HealthOutcome::Timeout => Some("timeout".to_string()),
        HealthOutcome::Error => Some(if record.status == 429 {
            "rate_limited".to_string()
        } else if record.status >= 500 {
            "upstream_error".to_string()
        } else if record.status == 0 {
            "connect_error".to_string()
        } else {
            "error".to_string()
        }),
    };
    HealthEvent {
        target_id: record.target.clone(),
        provider: record.provider.clone(),
        source: HealthSource::Passive,
        outcome,
        status_code: (record.status > 0).then_some(record.status),
        latency_ms: record.latency_ms,
        error_kind,
    }
}

/// Handle used by request handlers to emit logs. Cheap to clone.
#[derive(Clone)]
pub struct LogSink {
    tx: Option<mpsc::Sender<RequestLog>>,
    metrics: Arc<Metrics>,
    // the passive funnel also feeds provider health events (ROL-197); disabled
    // when no clickhouse url is set
    health_events: crate::health_events::HealthEventSink,
}

impl LogSink {
    /// A sink that discards everything (logging disabled / used in tests).
    pub fn disabled(metrics: Arc<Metrics>) -> Self {
        Self {
            tx: None,
            health_events: crate::health_events::HealthEventSink::disabled(metrics.clone()),
            metrics,
        }
    }

    /// Attach the health-event sink fed by the passive request funnel. Returns
    /// `self` so it composes with the constructors.
    pub fn with_health_events(mut self, sink: crate::health_events::HealthEventSink) -> Self {
        self.health_events = sink;
        self
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
            health_events: crate::health_events::HealthEventSink::disabled(metrics.clone()),
            metrics,
        }
    }

    /// Enqueue a record without blocking. Drops (and counts) the record if the
    /// queue is full or the writer has stopped.
    pub fn log(&self, record: RequestLog) {
        // observe latency/ttft histograms + passive per-target outcome for every
        // completed request, even when clickhouse logging is disabled (metrics
        // are always present)
        self.metrics
            .observe_request(&record.model, record.latency_ms, record.ttft_ms);
        self.metrics.observe_target(
            &record.provider,
            &record.target,
            (200..300).contains(&record.status),
        );
        self.metrics.observe_variant(&record.model, &record.variant);
        // funnel a passive health event for every real upstream target (skip the
        // builtin fake-llm and any row without a provider/target)
        if !record.provider.is_empty() && !record.target.is_empty() {
            self.health_events.emit(passive_health_event(&record));
        }
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
    fn parses_openai_non_stream_usage() {
        let body =
            br#"{"id":"x","usage":{"prompt_tokens":11,"completion_tokens":22,"total_tokens":33}}"#;
        assert_eq!(
            parse_usage(false, body),
            Usage {
                prompt: 11,
                completion: 22,
                total: 33
            }
        );
    }

    #[test]
    fn parses_anthropic_non_stream_usage_and_derives_total() {
        let body = br#"{"type":"message","usage":{"input_tokens":7,"output_tokens":5}}"#;
        // anthropic omits total; it is derived as prompt + completion
        assert_eq!(
            parse_usage(false, body),
            Usage {
                prompt: 7,
                completion: 5,
                total: 12
            }
        );
    }

    #[test]
    fn parses_openai_sse_final_chunk_usage() {
        let sse = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":9,\"total_tokens\":12}}\n\n\
data: [DONE]\n\n";
        assert_eq!(
            parse_usage(true, sse),
            Usage {
                prompt: 3,
                completion: 9,
                total: 12
            }
        );
    }

    #[test]
    fn parses_anthropic_sse_message_start_and_delta() {
        let sse = b"event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":40,\"output_tokens\":1}}}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":25}}\n\n";
        // input from message_start, output is the larger (final) delta value
        assert_eq!(
            parse_usage(true, sse),
            Usage {
                prompt: 40,
                completion: 25,
                total: 65
            }
        );
    }

    #[test]
    fn missing_usage_is_zero() {
        assert_eq!(parse_usage(false, b"{\"id\":\"x\"}"), Usage::default());
    }

    #[test]
    fn passive_event_maps_status_to_outcome() {
        use crate::health_events::{HealthOutcome, HealthSource};
        let base = RequestLog {
            provider: "openai".to_string(),
            target: "openai/gpt-4o".to_string(),
            latency_ms: 15,
            ..Default::default()
        };

        let ok = passive_health_event(&RequestLog {
            status: 200,
            ..base.clone()
        });
        assert_eq!(ok.source, HealthSource::Passive);
        assert_eq!(ok.outcome, HealthOutcome::Ok);
        assert_eq!(ok.status_code, Some(200));
        assert!(ok.error_kind.is_none());

        let rl = passive_health_event(&RequestLog {
            status: 429,
            ..base.clone()
        });
        assert_eq!(rl.outcome, HealthOutcome::Error);
        assert_eq!(rl.error_kind.as_deref(), Some("rate_limited"));

        let up = passive_health_event(&RequestLog {
            status: 503,
            ..base.clone()
        });
        assert_eq!(up.error_kind.as_deref(), Some("upstream_error"));

        // never reached upstream: status 0, no status code, timeout error text
        let to = passive_health_event(&RequestLog {
            status: 0,
            error: "upstream request timed out after 30s".to_string(),
            ..base.clone()
        });
        assert_eq!(to.outcome, HealthOutcome::Timeout);
        assert_eq!(to.status_code, None);
        assert_eq!(to.error_kind.as_deref(), Some("timeout"));

        // connect failure: status 0, non-timeout error
        let ce = passive_health_event(&RequestLog {
            status: 0,
            error: "connection refused".to_string(),
            ..base
        });
        assert_eq!(ce.outcome, HealthOutcome::Error);
        assert_eq!(ce.error_kind.as_deref(), Some("connect_error"));
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
    async fn stream_wrapper_forwards_bytes_and_logs_usage() {
        use futures_util::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        });

        let metrics = Arc::new(Metrics::default());
        let sink = LogSink::spawn(
            format!("http://{addr}"),
            10,
            Duration::from_millis(50),
            100,
            metrics.clone(),
        );

        let upstream = br#"{"usage":{"prompt_tokens":4,"completion_tokens":6,"total_tokens":10}}"#;
        let inner = futures_util::stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(
            upstream.to_vec(),
        ))]);
        let price = Some(rolter_core::ModelPriceConfig {
            model: "gpt-4o".to_string(),
            input_per_mtok: 1_000_000.0, // 1 usd per token, for an exact assert
            output_per_mtok: 1_000_000.0,
            cached_input_per_mtok: None,
        });
        let mut wrapped = UsageLoggingStream::new(
            Box::pin(inner),
            false,
            Instant::now(),
            sink,
            price,
            RequestLog {
                request_id: "req-stream".to_string(),
                model: "gpt-4o".to_string(),
                ..Default::default()
            },
            None,
            None,
            None,
        );

        // draining the wrapper forwards the body unchanged to the client
        let mut forwarded = Vec::new();
        while let Some(chunk) = wrapped.next().await {
            forwarded.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(forwarded, upstream);
        drop(wrapped); // ensure finalize ran

        let req = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        assert!(req.contains("\"request_id\":\"req-stream\""));
        assert!(req.contains("\"prompt_tokens\":4"));
        assert!(req.contains("\"completion_tokens\":6"));
        assert!(req.contains("\"total_tokens\":10"));
        // 1 usd/token * (4 + 6) tokens = 10.0
        assert!(req.contains("\"cost_usd\":10.0"));
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
