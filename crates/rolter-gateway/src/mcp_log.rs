//! Direct ClickHouse writer for the `mcp_tool_call_logs` table (#2395).
//!
//! The MCP proxy forwards JSON-RPC to a downstream server and used to write
//! nothing about it, so Observability → MCP Logs was empty on a real
//! deployment: the control plane's ingest route had no caller. A proxied
//! `tools/call` now becomes one [`McpEvent`], handed to [`McpEventSink::emit`],
//! which only does a non-blocking `try_send` onto a bounded channel. A
//! background task batches rows into ClickHouse exactly like the request-log
//! writer and is stopped through the same [`SinkTasks`] at shutdown. A full
//! queue drops the event and counts it; recording never delays or fails a call.
//!
//! Attribution (org, team, project, virtual key, user) is the calling key's and
//! is filled by the proxy, because the control plane scopes MCP log reads by it.
//! Payload capture, the size limit, the redaction list and the error category
//! come from the helpers in `rolter_core::mcp_log`, which the control plane's
//! ingest route applies too.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::Stream;
use rolter_core::mcp_log::{capture, safe_error};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::logging::{clickhouse_ts, BEST_EFFORT_DATES};
use crate::metrics::Metrics;
use crate::sink_drain::SinkTasks;

/// How much of a response body is kept to find the JSON-RPC reply and read its
/// `isError`. A larger body is passed through untouched and recorded with the
/// status the HTTP layer gave and no result.
const MAX_OBSERVED_BYTES: usize = 1 << 20;

/// One row of `mcp_tool_call_logs`; field names match the columns so it
/// serializes directly as a `JSONEachRow` line.
#[derive(Debug, Clone, Serialize)]
pub struct McpEvent {
    #[serde(serialize_with = "clickhouse_ts::serialize")]
    pub ts: DateTime<Utc>,
    pub event_id: String,
    pub server: String,
    pub tool: String,
    pub transport: String,
    pub status: &'static str,
    pub latency_ms: u32,
    pub org_id: String,
    pub team_id: String,
    pub project_id: String,
    pub virtual_key_id: String,
    pub user_id: String,
    pub request_id: String,
    pub trace_id: String,
    pub arguments: String,
    pub result: String,
    pub error: String,
}

/// Handle the proxy uses to record tool calls. Cheap to clone.
#[derive(Clone)]
pub struct McpEventSink {
    tx: Option<mpsc::Sender<McpEvent>>,
    metrics: Arc<Metrics>,
    tasks: Option<Arc<SinkTasks>>,
}

impl McpEventSink {
    /// A sink that discards everything (no ClickHouse configured / tests).
    pub fn disabled(metrics: Arc<Metrics>) -> Self {
        Self {
            tx: None,
            metrics,
            tasks: None,
        }
    }

    /// Build a sink and spawn the batch writer targeting the ClickHouse HTTP
    /// endpoint at `clickhouse_url`. Must be called from within a Tokio runtime.
    pub fn spawn(
        clickhouse_url: String,
        batch_max: usize,
        flush: Duration,
        queue_capacity: usize,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self::spawn_with_client(
            clickhouse_url,
            batch_max,
            flush,
            queue_capacity,
            metrics,
            crate::clickhouse_client::client(),
        )
    }

    /// [`Self::spawn`] with an explicit HTTP client, so tests can bound it tightly.
    pub(crate) fn spawn_with_client(
        clickhouse_url: String,
        batch_max: usize,
        flush: Duration,
        queue_capacity: usize,
        metrics: Arc<Metrics>,
        client: reqwest::Client,
    ) -> Self {
        let (tx, rx) = mpsc::channel(queue_capacity.max(1));
        let writer = BatchWriter {
            url: format!(
                "{}/?query=INSERT%20INTO%20mcp_tool_call_logs%20FORMAT%20JSONEachRow{BEST_EFFORT_DATES}",
                clickhouse_url.trim_end_matches('/')
            ),
            client,
            batch_max: batch_max.max(1),
            flush,
            metrics: metrics.clone(),
        };
        let tasks = Arc::new(SinkTasks::default());
        tasks.track(tokio::spawn(writer.run(rx, tasks.token())));
        Self {
            tx: Some(tx),
            metrics,
            tasks: Some(tasks),
        }
    }

    /// Whether events are being written at all. The proxy checks this first so
    /// a deployment without ClickHouse never parses a request body for it.
    pub fn enabled(&self) -> bool {
        self.tx.is_some()
    }

    /// Flush the batch and queue the writer holds and stop it. Returns once it
    /// has exited; the caller bounds the wait. A no-op on a disabled sink.
    pub async fn shutdown(&self) {
        if let Some(tasks) = &self.tasks {
            tasks.stop().await;
        }
    }

    /// Enqueue an event without blocking. Drops (and counts) it if the queue is
    /// full or the writer has stopped.
    pub fn emit(&self, event: McpEvent) {
        let Some(tx) = &self.tx else {
            return;
        };
        if tx.try_send(event).is_err() {
            self.metrics.mcp_events_dropped_total.fetch_add(1, Relaxed);
        }
    }
}

struct BatchWriter {
    url: String,
    client: reqwest::Client,
    batch_max: usize,
    flush: Duration,
    metrics: Arc<Metrics>,
}

impl BatchWriter {
    async fn run(self, mut rx: mpsc::Receiver<McpEvent>, stop: CancellationToken) {
        let mut batch: Vec<McpEvent> = Vec::with_capacity(self.batch_max);
        let mut ticker = tokio::time::interval(self.flush);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut stopping = false;
        loop {
            tokio::select! {
                // shutdown: closing the receiver keeps what is queued readable
                // and then yields `None`, so the arm below flushes it all
                _ = stop.cancelled(), if !stopping => {
                    stopping = true;
                    rx.close();
                }
                maybe = rx.recv() => match maybe {
                    Some(event) => {
                        batch.push(event);
                        if batch.len() >= self.batch_max {
                            self.flush(&mut batch).await;
                        }
                    }
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

    /// POST the batch as newline-delimited JSON, then clear it. A failure drops
    /// the batch: call accounting must never wedge the writer.
    async fn flush(&self, batch: &mut Vec<McpEvent>) {
        if batch.is_empty() {
            return;
        }
        let mut body = Vec::with_capacity(batch.len() * 512);
        for event in batch.iter() {
            let start = body.len();
            match serde_json::to_writer(&mut body, event) {
                Ok(()) => body.push(b'\n'),
                Err(err) => {
                    body.truncate(start);
                    tracing::warn!(%err, "failed to serialize mcp tool-call event");
                }
            }
        }
        let count = batch.len() as u64;
        match self.client.post(&self.url).body(body).send().await {
            Ok(resp) if resp.status().is_success() => {
                self.metrics
                    .mcp_events_written_total
                    .fetch_add(count, Relaxed);
            }
            Ok(resp) => {
                let status = resp.status();
                let detail = resp.text().await.unwrap_or_default();
                tracing::warn!(%status, detail, "clickhouse rejected mcp tool-call batch");
                self.metrics
                    .mcp_events_dropped_total
                    .fetch_add(count, Relaxed);
            }
            Err(err) => {
                tracing::warn!(timed_out = err.is_timeout(), err = %err.without_url(), "failed to write mcp tool-call batch to clickhouse");
                self.metrics
                    .mcp_events_dropped_total
                    .fetch_add(count, Relaxed);
            }
        }
        batch.clear();
    }
}

/// Who made the call and how payloads are kept; everything but the JSON-RPC
/// content itself, which [`PendingCall::parse`] reads from the request.
pub(crate) struct CallContext<'a> {
    pub server: &'a str,
    pub transport: &'a str,
    pub org_id: &'a str,
    pub team_id: &'a str,
    pub project_id: &'a str,
    pub virtual_key_id: &'a str,
    /// the virtual key's owner, which is the OAuth session owner whenever the
    /// server authenticates per user: the proxy looks the session up by it
    pub user_id: &'a str,
    pub request_id: &'a str,
    pub capture: &'a rolter_core::PayloadCaptureConfig,
}

/// What a finished call amounts to, before it is rendered into a row.
struct Outcome {
    status: &'static str,
    /// the JSON-RPC `result` or `error` object, when one was seen
    payload: Option<Value>,
}

/// A `tools/call` that has been forwarded and has not yet been answered.
pub(crate) struct PendingCall {
    sink: McpEventSink,
    started: Instant,
    ts: DateTime<Utc>,
    rpc_id: Value,
    arguments: Option<Value>,
    event: McpEvent,
    capture_enabled: bool,
    capture_max: usize,
    capture_fields: Vec<String>,
}

impl PendingCall {
    /// Read a request body; `Some` only for a JSON-RPC `tools/call` request
    /// (one with an id, since a notification has no response to record).
    /// `initialize`, `tools/list`, notifications and batches are not recorded.
    pub(crate) fn parse(
        sink: &McpEventSink,
        body: &[u8],
        context: &CallContext<'_>,
    ) -> Option<Self> {
        const NEEDLE: &[u8] = b"tools/call";
        // cheap reject before a full parse: most MCP traffic is not a call
        if !sink.enabled() || !body.windows(NEEDLE.len()).any(|w| w == NEEDLE) {
            return None;
        }
        let Value::Object(mut request) = serde_json::from_slice::<Value>(body).ok()? else {
            return None;
        };
        if request.get("method")?.as_str()? != "tools/call" {
            return None;
        }
        let rpc_id = request.remove("id").filter(|id| !id.is_null())?;
        let Value::Object(mut params) = request.remove("params")? else {
            return None;
        };
        let tool = params.get("name")?.as_str()?.to_string();
        let arguments = params.remove("arguments");
        let ts = Utc::now();
        let event = McpEvent {
            ts,
            event_id: uuid::Uuid::new_v4().to_string(),
            server: context.server.to_string(),
            tool,
            transport: context.transport.to_string(),
            status: "error",
            latency_ms: 0,
            org_id: context.org_id.to_string(),
            team_id: context.team_id.to_string(),
            project_id: context.project_id.to_string(),
            virtual_key_id: context.virtual_key_id.to_string(),
            user_id: context.user_id.to_string(),
            request_id: context.request_id.to_string(),
            trace_id: context.request_id.to_string(),
            arguments: String::new(),
            result: String::new(),
            error: String::new(),
        };
        let capturing = context.capture.enabled && context.capture.max_bytes > 0;
        Some(Self {
            sink: sink.clone(),
            started: Instant::now(),
            ts,
            rpc_id,
            arguments: if capturing { arguments } else { None },
            event,
            capture_enabled: context.capture.enabled,
            capture_max: context.capture.max_bytes,
            capture_fields: if capturing {
                context.capture.redact_fields.clone()
            } else {
                Vec::new()
            },
        })
    }

    /// Record a call that never produced a response: a refusal before
    /// forwarding, or an upstream that could not be reached.
    pub(crate) fn fail(self, status: &'static str) {
        self.finish(Outcome {
            status,
            payload: None,
        });
    }

    /// Record an upstream that could not be reached, telling a timeout from
    /// any other transport failure by the forwarder's message.
    pub(crate) fn fail_upstream(self, message: &str) {
        let status = if message.contains("timed out") {
            "timeout"
        } else {
            "transport_error"
        };
        self.fail(status);
    }

    /// Wrap the upstream body so the call is recorded once it has been read,
    /// without altering a byte of it or waiting on the write.
    pub(crate) fn observe<S>(self, http_status: u16, is_sse: bool, inner: S) -> ObservedBody<S> {
        ObservedBody {
            inner,
            call: Some(self),
            buf: Vec::new(),
            overflow: false,
            http_status,
            is_sse,
            stream_failed: false,
        }
    }

    fn finish(mut self, outcome: Outcome) {
        let Outcome { status, payload } = outcome;
        self.event.ts = self.ts;
        self.event.status = status;
        self.event.latency_ms =
            u32::try_from(self.started.elapsed().as_millis()).unwrap_or(u32::MAX);
        // the fixed category, never the upstream's own words: a tool error
        // message is caller-influenced text and may carry a secret
        let detail = match status {
            "success" => None,
            "timeout" => Some("timeout"),
            "auth_denied" => Some("denied"),
            "transport_error" => Some("transport"),
            _ => Some("tool error"),
        };
        self.event.error = safe_error(status, detail);
        self.event.arguments = capture(
            self.arguments.take(),
            self.capture_enabled,
            self.capture_max,
            &self.capture_fields,
        );
        self.event.result = capture(
            payload,
            self.capture_enabled,
            self.capture_max,
            &self.capture_fields,
        );
        self.sink.emit(self.event);
    }
}

/// The JSON-RPC reply answering `rpc_id` in a buffered response body.
fn find_response(buf: &[u8], is_sse: bool, rpc_id: &Value) -> Option<Value> {
    let answers = |value: &Value| {
        value.get("id") == Some(rpc_id)
            && (value.get("result").is_some() || value.get("error").is_some())
    };
    let matching = |value: Value| match value {
        Value::Array(items) => items.into_iter().find(|item| answers(item)),
        other => answers(&other).then_some(other),
    };
    if !is_sse {
        return serde_json::from_slice(buf).ok().and_then(matching);
    }
    // an SSE event carries its JSON-RPC message on `data:` lines; the reply is
    // the one event answering this call, among any progress notifications
    buf.split(|b| *b == b'\n')
        .filter_map(|line| line.strip_prefix(b"data:"))
        .filter_map(|data| serde_json::from_slice::<Value>(data.trim_ascii()).ok())
        .find_map(matching)
}

fn outcome(
    buf: &[u8],
    overflow: bool,
    is_sse: bool,
    http_status: u16,
    stream_failed: bool,
    rpc_id: &Value,
) -> Outcome {
    let none = |status| Outcome {
        status,
        payload: None,
    };
    if matches!(http_status, 401 | 403) {
        return none("auth_denied");
    }
    if !overflow {
        if let Some(mut reply) = find_response(buf, is_sse, rpc_id) {
            if let Some(error) = reply.get_mut("error").map(Value::take) {
                return Outcome {
                    status: "error",
                    payload: Some(error),
                };
            }
            let result = reply.get_mut("result").map(Value::take);
            let is_error = result
                .as_ref()
                .and_then(|r| r.get("isError"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            return Outcome {
                status: if is_error { "error" } else { "success" },
                payload: result,
            };
        }
    }
    match http_status {
        // a body too large to hold was still delivered: the call completed
        200..=299 if overflow => none("success"),
        408 | 504 => none("timeout"),
        400..=499 => none("error"),
        _ => none("transport_error"),
    }
    .tap(stream_failed)
}

impl Outcome {
    /// A body that broke mid-stream without yielding a reply is a transport
    /// failure whatever the headers said.
    fn tap(self, stream_failed: bool) -> Self {
        if stream_failed && self.payload.is_none() && !matches!(self.status, "auth_denied") {
            Self {
                status: "transport_error",
                payload: None,
            }
        } else {
            self
        }
    }
}

/// An upstream body that is recorded when it ends. Passes every chunk through
/// unchanged; holds a bounded copy only to read the reply from.
pub(crate) struct ObservedBody<S> {
    inner: S,
    call: Option<PendingCall>,
    buf: Vec<u8>,
    overflow: bool,
    http_status: u16,
    is_sse: bool,
    stream_failed: bool,
}

impl<S> ObservedBody<S> {
    fn complete(&mut self) {
        if let Some(call) = self.call.take() {
            let outcome = outcome(
                &self.buf,
                self.overflow,
                self.is_sse,
                self.http_status,
                self.stream_failed,
                &call.rpc_id,
            );
            call.finish(outcome);
        }
    }
}

impl<S, E> Stream for ObservedBody<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    type Item = Result<Bytes, E>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let next = std::pin::Pin::new(&mut self.inner).poll_next(cx);
        match &next {
            Poll::Ready(Some(Ok(chunk))) => {
                if !self.overflow {
                    if self.buf.len() + chunk.len() > MAX_OBSERVED_BYTES {
                        self.overflow = true;
                        self.buf = Vec::new();
                    } else {
                        self.buf.extend_from_slice(chunk);
                    }
                }
            }
            Poll::Ready(Some(Err(_))) => {
                self.stream_failed = true;
                self.complete();
            }
            Poll::Ready(None) => self.complete(),
            _ => {}
        }
        next
    }
}

impl<S> Drop for ObservedBody<S> {
    /// A caller that hangs up mid-response still leaves a row, from whatever
    /// had arrived.
    fn drop(&mut self) {
        if self.call.is_some() {
            self.stream_failed = true;
            self.complete();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn context<'a>(capture: &'a rolter_core::PayloadCaptureConfig) -> CallContext<'a> {
        CallContext {
            server: "docs",
            transport: "streamable_http",
            org_id: "org-1",
            team_id: "team-1",
            project_id: "project-1",
            virtual_key_id: "key-1",
            user_id: "user-1",
            request_id: "req-1",
            capture,
        }
    }

    fn sink() -> (McpEventSink, mpsc::Receiver<McpEvent>) {
        let (tx, rx) = mpsc::channel(8);
        (
            McpEventSink {
                tx: Some(tx),
                metrics: Arc::new(Metrics::default()),
                tasks: None,
            },
            rx,
        )
    }

    fn request(method: &str, id: Option<i64>) -> Vec<u8> {
        let mut value = json!({"jsonrpc": "2.0", "method": method,
            "params": {"name": "search", "arguments": {"q": "x", "token": "s3"}}});
        if let Some(id) = id {
            value["id"] = json!(id);
        }
        serde_json::to_vec(&value).unwrap()
    }

    #[test]
    fn only_a_tools_call_request_with_an_id_is_recorded() {
        let (sink, _rx) = sink();
        let capture = rolter_core::PayloadCaptureConfig::default();
        let ctx = context(&capture);
        assert!(PendingCall::parse(&sink, &request("tools/call", Some(1)), &ctx).is_some());
        assert!(PendingCall::parse(&sink, &request("tools/list", Some(1)), &ctx).is_none());
        assert!(PendingCall::parse(&sink, &request("initialize", Some(1)), &ctx).is_none());
        // a notification has no response to record
        assert!(PendingCall::parse(&sink, &request("tools/call", None), &ctx).is_none());
        assert!(PendingCall::parse(&sink, b"not json tools/call", &ctx).is_none());
        let disabled = McpEventSink::disabled(Arc::new(Metrics::default()));
        assert!(PendingCall::parse(&disabled, &request("tools/call", Some(1)), &ctx).is_none());
    }

    #[test]
    fn a_json_rpc_error_and_an_is_error_result_are_tool_errors() {
        let id = json!(7);
        let reply = |v: Value| serde_json::to_vec(&v).unwrap();
        let ok = outcome(
            &reply(json!({"id": 7, "result": {"content": []}})),
            false,
            false,
            200,
            false,
            &id,
        );
        assert_eq!(ok.status, "success");
        let flagged = outcome(
            &reply(json!({"id": 7, "result": {"isError": true}})),
            false,
            false,
            200,
            false,
            &id,
        );
        assert_eq!(flagged.status, "error");
        let rpc = outcome(
            &reply(json!({"id": 7, "error": {"code": -32602}})),
            false,
            false,
            200,
            false,
            &id,
        );
        assert_eq!(rpc.status, "error");
        assert_eq!(rpc.payload, Some(json!({"code": -32602})));
    }

    #[test]
    fn an_sse_reply_is_found_among_progress_events() {
        let id = json!(3);
        let sse = b"event: message\ndata: {\"method\":\"notifications/progress\"}\n\nevent: message\ndata: {\"id\":3,\"result\":{\"isError\":true}}\n\n";
        assert_eq!(outcome(sse, false, true, 200, false, &id).status, "error");
        let other = b"data: {\"id\":9,\"result\":{}}\n\n";
        // a reply to some other call is not this call's reply
        assert_eq!(
            outcome(other, false, true, 200, false, &id).status,
            "transport_error"
        );
    }

    #[test]
    fn transport_failures_map_onto_the_closed_status_set() {
        let id = json!(1);
        assert_eq!(
            outcome(b"", false, false, 403, false, &id).status,
            "auth_denied"
        );
        assert_eq!(
            outcome(b"", false, false, 504, false, &id).status,
            "timeout"
        );
        assert_eq!(
            outcome(b"", false, false, 502, false, &id).status,
            "transport_error"
        );
        assert_eq!(outcome(b"", false, false, 404, false, &id).status, "error");
        assert_eq!(outcome(b"", true, false, 200, false, &id).status, "success");
        assert_eq!(
            outcome(b"{", false, false, 200, true, &id).status,
            "transport_error"
        );
        for status in [
            "success",
            "timeout",
            "auth_denied",
            "transport_error",
            "error",
        ] {
            assert!(rolter_core::mcp_log::MCP_STATUSES.contains(&status));
        }
    }

    #[tokio::test]
    async fn the_row_carries_attribution_and_redacted_arguments() {
        use futures_util::StreamExt;
        let (sink, mut rx) = sink();
        let capture = rolter_core::PayloadCaptureConfig {
            enabled: true,
            redact_fields: vec!["token".to_string()],
            ..Default::default()
        };
        let call =
            PendingCall::parse(&sink, &request("tools/call", Some(1)), &context(&capture)).unwrap();
        let body = futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(
            br#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#,
        ))]);
        let mut observed = call.observe(200, false, body);
        while observed.next().await.is_some() {}
        let event = rx.try_recv().unwrap();
        assert_eq!(event.status, "success");
        assert_eq!(
            (
                event.org_id.as_str(),
                event.team_id.as_str(),
                event.project_id.as_str(),
                event.user_id.as_str()
            ),
            ("org-1", "team-1", "project-1", "user-1")
        );
        assert_eq!(event.tool, "search");
        assert!(event.arguments.contains("[REDACTED]") && !event.arguments.contains("s3"));
        assert_eq!(event.result, r#"{"content":[]}"#);
    }

    #[tokio::test]
    async fn capture_off_keeps_payloads_out_of_the_row() {
        let (sink, mut rx) = sink();
        let capture = rolter_core::PayloadCaptureConfig::default();
        let call =
            PendingCall::parse(&sink, &request("tools/call", Some(1)), &context(&capture)).unwrap();
        call.fail("auth_denied");
        let event = rx.try_recv().unwrap();
        assert_eq!((event.arguments.as_str(), event.result.as_str()), ("", ""));
        assert_eq!(event.error, "authentication denied");
    }

    #[tokio::test]
    async fn full_queue_drops_and_counts() {
        let metrics = Arc::new(Metrics::default());
        let sink = McpEventSink::spawn(
            "http://127.0.0.1:1".to_string(),
            1000,
            Duration::from_secs(3600),
            1,
            metrics.clone(),
        );
        let capture = rolter_core::PayloadCaptureConfig::default();
        for _ in 0..200 {
            if let Some(call) =
                PendingCall::parse(&sink, &request("tools/call", Some(1)), &context(&capture))
            {
                call.fail("error");
            }
        }
        assert!(metrics.mcp_events_dropped_total.load(Relaxed) > 0);
    }
}
