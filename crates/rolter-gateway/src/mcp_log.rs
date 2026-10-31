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

// spec-ref: json-rpc
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

use crate::logging::{clickhouse_ts, INSERT_SETTINGS};
use crate::metrics::Metrics;
use crate::sink_drain::SinkTasks;

/// How much of a response body is kept to find the JSON-RPC reply and read its
/// `isError`. A larger body is passed through untouched and recorded with the
/// status the HTTP layer gave and no result.
const MAX_OBSERVED_BYTES: usize = 1 << 20;

/// The most `tools/call`s of one request that become log rows. A body is
/// bounded only by the request size limit (32 MiB by default), and every call
/// recorded is held as a pending row for the whole wait on the upstream, so an
/// unbounded batch would be a memory amplifier. The calls past this are still
/// forwarded and judged; they are counted in `rolter_mcp_events_dropped_total`
/// and have no row.
pub(crate) const MAX_CALLS_PER_REQUEST: usize = 256;

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
    /// the scopes an upstream's `insufficient_scope` challenge asked for,
    /// validated and bounded (#3375). Left out of the row when empty, so a
    /// gateway writing to a ClickHouse that has not applied migration `042`
    /// sends nothing it would have to drop, and `input_format_skip_unknown_fields`
    /// covers the one that does
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub error_scopes: Vec<String>,
    /// the call was a JSON-RPC notification, so it has no `id` and no reply
    /// was awaited: `status` then says only what the HTTP layer answered, never
    /// what the tool did (#3466). Left out of the row while `false`, for the
    /// same reason as `error_scopes`: a ClickHouse that has not applied
    /// migration `047` drops it under `input_format_skip_unknown_fields`
    #[serde(skip_serializing_if = "is_false", serialize_with = "as_u8")]
    pub notification: bool,
    /// what refines `status` for a call whose result nothing judged, one of
    /// [`rolter_core::mcp_log::MCP_OUTCOMES`], or empty (#3529). `status` beside
    /// it is the HTTP layer's `success`, which is how a table that has not
    /// applied migration `051` reads the row; the field is left out while
    /// empty for the same reason as `error_scopes`
    #[serde(skip_serializing_if = "str::is_empty")]
    pub outcome: &'static str,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// the column is a `UInt8`, so the flag is written as the number the table
/// stores rather than as a JSON boolean
fn as_u8<S: serde::Serializer>(value: &bool, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_u8(u8::from(*value))
}

/// Handle the proxy uses to record tool calls. Cheap to clone.
#[derive(Clone)]
pub struct McpEventSink {
    tx: Option<mpsc::Sender<McpEvent>>,
    metrics: Arc<Metrics>,
    tasks: Option<Arc<SinkTasks>>,
}

impl McpEventSink {
    /// A sink that hands its events to a channel instead of ClickHouse, for the
    /// proxy's tests to read back what was recorded.
    #[cfg(test)]
    pub(crate) fn capturing(metrics: Arc<Metrics>) -> (Self, mpsc::Receiver<McpEvent>) {
        Self::capturing_with(metrics, 16)
    }

    /// [`Self::capturing`] with room for `capacity` events.
    #[cfg(test)]
    pub(crate) fn capturing_with(
        metrics: Arc<Metrics>,
        capacity: usize,
    ) -> (Self, mpsc::Receiver<McpEvent>) {
        let (tx, rx) = mpsc::channel(capacity);
        (
            Self {
                tx: Some(tx),
                metrics,
                tasks: None,
            },
            rx,
        )
    }

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
                "{}/?query=INSERT%20INTO%20mcp_tool_call_logs%20FORMAT%20JSONEachRow{INSERT_SETTINGS}",
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

    /// Count `n` calls that were not recorded, as a full queue would have
    /// dropped them.
    pub(crate) fn count_dropped(&self, n: usize) {
        if n > 0 && self.tx.is_some() {
            self.metrics
                .mcp_events_dropped_total
                .fetch_add(n as u64, Relaxed);
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
                // stop first: with a backlog both arms are ready and an
                // unbiased pick keeps taking from an open queue, so a send
                // racing the drain would be written instead of counted dropped
                biased;
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
/// content itself, which [`PendingCall::from_request`] takes from the request.
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
    /// the compiled `capture` redaction policy (#3277)
    pub redactor: &'a Arc<rolter_core::payload_redaction::PayloadRedactor>,
}

/// The `error` of a row refused for an insufficient OAuth scope, which tells it
/// from the other `auth_denied` causes (no session, an expired one, a scope the
/// server requires and the session lacks).
pub(crate) const INSUFFICIENT_SCOPE_ERROR: &str = "insufficient scope";

/// What a finished call amounts to, before it is rendered into a row.
struct Outcome {
    status: &'static str,
    /// the call was a notification: nothing answered it, so the status is the
    /// HTTP layer's
    notification: bool,
    /// what refines `status` for a call whose result nothing judged
    /// (`accepted`), or empty
    refined: &'static str,
    /// a fixed category that replaces the one `status` implies
    error: Option<&'static str>,
    /// the JSON-RPC `result` or `error` object, when one was seen
    payload: Option<Value>,
}

/// A `tools/call` that has been forwarded and has not yet been answered.
pub(crate) struct PendingCall {
    sink: McpEventSink,
    started: Instant,
    ts: DateTime<Utc>,
    /// `Null` for a notification
    rpc_id: Value,
    /// the row stands for a whole body the relay forwarded unread, not for one
    /// call (#3528)
    unread: bool,
    arguments: Option<Value>,
    event: McpEvent,
    capture_enabled: bool,
    capture_max: usize,
    redactor: Arc<rolter_core::payload_redaction::PayloadRedactor>,
}

impl PendingCall {
    /// A pending call for one already-parsed `tools/call` request, whether or
    /// not it has an id. The proxy records an allowed single request with an id
    /// from the body the policy read ([`crate::mcp_policy::Read`], #3399), and a
    /// call the policy refused (#1403), including one inside a batch or sent as
    /// a notification, which has no reply to wait for.
    pub(crate) fn from_request(
        sink: &McpEventSink,
        mut request: serde_json::Map<String, Value>,
        context: &CallContext<'_>,
    ) -> Option<Self> {
        if !sink.enabled() {
            return None;
        }
        let rpc_id = request.remove("id").unwrap_or(Value::Null);
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
            error_scopes: Vec::new(),
            notification: false,
            outcome: "",
        };
        let capturing = context.capture.enabled && context.capture.max_bytes > 0;
        Some(Self {
            sink: sink.clone(),
            started: Instant::now(),
            ts,
            rpc_id,
            unread: false,
            arguments: if capturing { arguments } else { None },
            event,
            capture_enabled: context.capture.enabled,
            capture_max: context.capture.max_bytes,
            redactor: if capturing {
                Arc::clone(context.redactor)
            } else {
                Arc::default()
            },
        })
    }

    /// A pending row for a body the relay forwards to an ungoverned server
    /// without reading it, because it is encoded or is not JSON (#3528). The
    /// calls it holds cannot be itemised, so the one row has no tool, and once
    /// the server has answered `2xx` it is `unread`: nothing says what any tool
    /// did. A failure the HTTP layer reports is recorded as that failure.
    pub(crate) fn unread(sink: &McpEventSink, context: &CallContext<'_>) -> Option<Self> {
        let mut request = serde_json::Map::new();
        request.insert("params".to_string(), serde_json::json!({ "name": "" }));
        let mut call = Self::from_request(sink, request, context)?;
        call.unread = true;
        Some(call)
    }

    /// Record a call that never produced a response: a refusal before
    /// forwarding, or an upstream that could not be reached.
    pub(crate) fn fail(self, status: &'static str) {
        self.finish(Outcome {
            status,
            notification: false,
            refined: "",
            error: None,
            payload: None,
        });
    }

    /// Record a call the upstream refused for want of an OAuth scope (#3375): an
    /// `auth_denied` row that says so, with the scopes the challenge named, so
    /// the dashboard can offer the step-up for exactly those.
    pub(crate) fn fail_insufficient_scope(mut self, scopes: Vec<String>) {
        self.event.error_scopes = scopes;
        self.finish(Outcome {
            status: "auth_denied",
            notification: false,
            refined: "",
            error: Some(INSUFFICIENT_SCOPE_ERROR),
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

    /// Record a call whose reply the relay did not read, because it rewrote
    /// the reply (a batch holding a `tools/list` is narrowed): the status is the
    /// HTTP layer's and no result is kept.
    pub(crate) fn finish_unobserved(self, http_status: u16) {
        let outcome = self.by_status_alone(http_status);
        self.finish(outcome);
    }

    fn notification(&self) -> bool {
        self.rpc_id.is_null() && !self.unread
    }

    /// The outcome of a call that has no reply to be read, by the HTTP status
    /// alone: a notification, or a body forwarded unread
    fn by_status_alone(&self, http_status: u16) -> Outcome {
        let outcome = by_http_status(http_status, true, self.notification());
        if self.unread && outcome.status == "success" {
            Outcome {
                refined: "unread",
                ..outcome
            }
        } else {
            outcome
        }
    }

    /// No reply in the body answers this call
    fn awaits_no_reply(&self) -> bool {
        self.notification() || self.unread
    }

    fn finish(mut self, outcome: Outcome) {
        let Outcome {
            status,
            notification,
            refined,
            error,
            payload,
        } = outcome;
        self.event.notification = notification;
        self.event.outcome = refined;
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
        self.event.error = error.map_or_else(|| safe_error(status, detail), str::to_string);
        self.event.arguments = capture(
            self.arguments.take(),
            self.capture_enabled,
            self.capture_max,
            &self.redactor,
        );
        self.event.result = capture(
            payload,
            self.capture_enabled,
            self.capture_max,
            &self.redactor,
        );
        self.sink.emit(self.event);
    }
}

/// The JSON-RPC replies in a buffered response body, parsed once for every call
/// the request held, each handed to the call it answers by `id`. Indexed by the
/// id's JSON text (ids compare as JSON values, so `1` is not `1.0`), so handing
/// out a reply costs the same however many a batch holds.
struct Replies(std::collections::HashMap<String, std::collections::VecDeque<Value>>);

impl Replies {
    fn empty() -> Self {
        Self(Default::default())
    }

    fn of(buf: &[u8], is_sse: bool) -> Self {
        let mut replies = Self::empty();
        let mut collect = |value: Value| match value {
            Value::Array(items) => items.into_iter().for_each(|item| replies.add(item)),
            other => replies.add(other),
        };
        if !is_sse {
            if let Ok(value) = serde_json::from_slice(buf) {
                collect(value);
            }
        } else {
            // an SSE event carries its JSON-RPC message on `data:` lines; the
            // replies are the events answering a call, among any progress
            // notifications
            buf.split(|b| *b == b'\n')
                .filter_map(|line| line.strip_prefix(b"data:"))
                .filter_map(|data| serde_json::from_slice::<Value>(data.trim_ascii()).ok())
                .for_each(collect);
        }
        replies
    }

    fn add(&mut self, reply: Value) {
        let answers = reply.get("id").is_some_and(|id| !id.is_null())
            && (reply.get("result").is_some() || reply.get("error").is_some());
        if answers {
            let key = reply["id"].to_string();
            self.0.entry(key).or_default().push_back(reply);
        }
    }

    /// The reply answering `rpc_id`. Each reply answers one call, so two calls
    /// sharing an id (which JSON-RPC forbids) are not both given the first one.
    fn take(&mut self, rpc_id: &Value) -> Option<Value> {
        self.0.get_mut(&rpc_id.to_string())?.pop_front()
    }
}

/// What the HTTP status alone says about a call. `delivered` is for a call
/// whose reply cannot be read, a body too large to hold, a notification (which
/// has none) or a reply the relay rewrote: a `2xx` is then a call that completed.
fn by_http_status(http_status: u16, delivered: bool, notification: bool) -> Outcome {
    let status = match http_status {
        401 | 403 => "auth_denied",
        200..=299 if delivered => "success",
        408 | 504 => "timeout",
        400..=499 => "error",
        _ => "transport_error",
    };
    Outcome {
        status,
        notification,
        refined: "",
        error: None,
        payload: None,
    }
}

/// What an awaited call amounts to, from the reply found for it, if any.
/// `legacy_sse` is a call to a server on the legacy `sse` transport, which
/// answers a `POST` `202 Accepted` and sends the reply on the stream the client
/// holds open (#3529): a `202` without a reply in its body is `accepted`, while
/// one whose body does carry a reply is judged by it like any other.
fn outcome(
    reply: Option<Value>,
    overflow: bool,
    http_status: u16,
    stream_failed: bool,
    legacy_sse: bool,
) -> Outcome {
    if matches!(http_status, 401 | 403) {
        return by_http_status(http_status, false, false);
    }
    if let Some(mut reply) = reply {
        if let Some(error) = reply.get_mut("error").map(Value::take) {
            return Outcome {
                status: "error",
                notification: false,
                refined: "",
                error: None,
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
            notification: false,
            refined: "",
            error: None,
            payload: result,
        };
    }
    // the relay does not correlate the stream's replies with the calls posted
    // (#3529), so all the row can say is that the server took the call
    if legacy_sse && http_status == 202 && !overflow && !stream_failed {
        return Outcome {
            refined: "accepted",
            ..by_http_status(http_status, true, false)
        };
    }
    // a body too large to hold was still delivered: the call completed
    by_http_status(http_status, overflow, false).tap(stream_failed)
}

impl Outcome {
    /// A body that broke mid-stream without yielding a reply is a transport
    /// failure whatever the headers said.
    fn tap(self, stream_failed: bool) -> Self {
        if stream_failed && self.payload.is_none() && !matches!(self.status, "auth_denied") {
            Self {
                status: "transport_error",
                notification: false,
                refined: "",
                error: None,
                payload: None,
            }
        } else {
            self
        }
    }
}

/// Wrap the upstream body so every call the request held is recorded once the
/// body has been read, without altering a byte of it or waiting on the write.
pub(crate) fn observe<S>(
    calls: Vec<PendingCall>,
    http_status: u16,
    is_sse: bool,
    inner: S,
) -> ObservedBody<S> {
    ObservedBody {
        inner,
        // a notification is answered by nothing, so a body that only they await
        // is not worth buffering
        awaiting: calls.iter().any(|call| !call.awaits_no_reply()),
        calls,
        buf: Vec::new(),
        overflow: false,
        http_status,
        is_sse,
        stream_failed: false,
    }
}

/// An upstream body that is recorded when it ends. Passes every chunk through
/// unchanged; holds a bounded copy only to read the replies from.
pub(crate) struct ObservedBody<S> {
    inner: S,
    calls: Vec<PendingCall>,
    awaiting: bool,
    buf: Vec<u8>,
    overflow: bool,
    http_status: u16,
    is_sse: bool,
    stream_failed: bool,
}

impl<S> ObservedBody<S> {
    fn complete(&mut self) {
        let calls = std::mem::take(&mut self.calls);
        if calls.is_empty() {
            return;
        }
        // the body is parsed once, whatever the number of calls it answers
        let mut replies = if self.awaiting && !self.overflow {
            Replies::of(&self.buf, self.is_sse)
        } else {
            Replies::empty()
        };
        for call in calls {
            let outcome = if call.awaits_no_reply() {
                // headers came back, so the notification (or the unread body)
                // was delivered whatever happened to the body afterwards
                call.by_status_alone(self.http_status)
            } else {
                let reply = replies.take(&call.rpc_id);
                let legacy_sse = call.event.transport == "sse";
                outcome(
                    reply,
                    self.overflow,
                    self.http_status,
                    self.stream_failed,
                    legacy_sse,
                )
            };
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
                if self.awaiting && !self.overflow {
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
        if !self.calls.is_empty() {
            self.stream_failed = true;
            self.complete();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn context<'a>(
        capture: &'a rolter_core::PayloadCaptureConfig,
        redactor: &'a Arc<rolter_core::payload_redaction::PayloadRedactor>,
    ) -> CallContext<'a> {
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
            redactor,
        }
    }

    fn redactor(
        capture: &rolter_core::PayloadCaptureConfig,
    ) -> Arc<rolter_core::payload_redaction::PayloadRedactor> {
        Arc::new(capture.redactor().0)
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

    /// a request body as the proxy hands it to the log: read once with the
    /// policy's reader, then taken as a single call
    fn parse(sink: &McpEventSink, body: &[u8], ctx: &CallContext<'_>) -> Option<PendingCall> {
        let read = crate::mcp_policy::Read::of(body, false, sink.enabled());
        PendingCall::from_request(
            sink,
            read.into_calls(true, MAX_CALLS_PER_REQUEST)
                .kept
                .into_iter()
                .next()?,
            ctx,
        )
    }

    /// the outcome of one awaited call, the way a finished body reads it
    fn outcome_of(
        buf: &[u8],
        overflow: bool,
        is_sse: bool,
        http_status: u16,
        stream_failed: bool,
        id: &Value,
    ) -> Outcome {
        let reply = Replies::of(buf, is_sse).take(id);
        outcome(reply, overflow, http_status, stream_failed, false)
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
    fn the_scopes_of_a_refusal_ride_in_the_row_only_when_there_are_some() {
        let (sink, mut rx) = sink();
        let capture = rolter_core::PayloadCaptureConfig::default();
        let redactor = redactor(&capture);
        let ctx = context(&capture, &redactor);
        let call = parse(&sink, &request("tools/call", Some(1)), &ctx).unwrap();
        call.fail_insufficient_scope(vec!["repo".into(), "read:org".into()]);
        let denied = serde_json::to_value(rx.try_recv().unwrap()).unwrap();
        assert_eq!(denied["status"], "auth_denied");
        assert_eq!(denied["error"], INSUFFICIENT_SCOPE_ERROR);
        assert_eq!(denied["error_scopes"], json!(["repo", "read:org"]));
        // every other row omits the field, so an un-migrated table is not sent
        // a column it would have to drop
        let call = parse(&sink, &request("tools/call", Some(2)), &ctx).unwrap();
        call.fail("auth_denied");
        let plain = serde_json::to_value(rx.try_recv().unwrap()).unwrap();
        assert!(plain.get("error_scopes").is_none(), "{plain}");
        assert_eq!(plain["error"], "authentication denied");
    }

    #[test]
    fn a_call_written_with_other_key_cases_or_escapes_is_recorded() {
        let (sink, _rx) = sink();
        let capture = rolter_core::PayloadCaptureConfig::default();
        let redactor = redactor(&capture);
        let ctx = context(&capture, &redactor);
        // `encoding/json` reads these as `method`, `id`, `params`, `name`
        let cased = br#"{"jsonrpc":"2.0","Method":"tools/call","ID":7,"Params":{"NAME":"search","Arguments":{"q":"x"}}}"#;
        let call = parse(&sink, cased, &ctx).expect("a case-variant key is the same key");
        assert_eq!(call.event.tool, "search");
        assert_eq!(call.rpc_id, json!(7));
        // an escape can spell the method without the bytes `tools/call`
        let escaped =
            br#"{"jsonrpc":"2.0","method":"tools\u002fcall","id":8,"params":{"name":"search"}}"#;
        assert!(parse(&sink, escaped, &ctx).is_some());
        // and a body that never mentions it is not even parsed
        assert!(parse(&sink, br#"{"method":"ping","id":1}"#, &ctx).is_none());
    }

    #[test]
    fn only_a_tools_call_request_is_recorded() {
        let (sink, _rx) = sink();
        let capture = rolter_core::PayloadCaptureConfig::default();
        let redactor = redactor(&capture);
        let ctx = context(&capture, &redactor);
        assert!(parse(&sink, &request("tools/call", Some(1)), &ctx).is_some());
        assert!(parse(&sink, &request("tools/list", Some(1)), &ctx).is_none());
        assert!(parse(&sink, &request("initialize", Some(1)), &ctx).is_none());
        assert!(parse(&sink, b"not json tools/call", &ctx).is_none());
        let disabled = McpEventSink::disabled(Arc::new(Metrics::default()));
        assert!(parse(&disabled, &request("tools/call", Some(1)), &ctx).is_none());
    }

    #[test]
    fn a_json_rpc_error_and_an_is_error_result_are_tool_errors() {
        let id = json!(7);
        let reply = |v: Value| serde_json::to_vec(&v).unwrap();
        let ok = outcome_of(
            &reply(json!({"id": 7, "result": {"content": []}})),
            false,
            false,
            200,
            false,
            &id,
        );
        assert_eq!(ok.status, "success");
        let flagged = outcome_of(
            &reply(json!({"id": 7, "result": {"isError": true}})),
            false,
            false,
            200,
            false,
            &id,
        );
        assert_eq!(flagged.status, "error");
        let rpc = outcome_of(
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
        assert_eq!(
            outcome_of(sse, false, true, 200, false, &id).status,
            "error"
        );
        let other = b"data: {\"id\":9,\"result\":{}}\n\n";
        // a reply to some other call is not this call's reply
        assert_eq!(
            outcome_of(other, false, true, 200, false, &id).status,
            "transport_error"
        );
    }

    #[test]
    fn transport_failures_map_onto_the_closed_status_set() {
        let id = json!(1);
        assert_eq!(
            outcome_of(b"", false, false, 403, false, &id).status,
            "auth_denied"
        );
        assert_eq!(
            outcome_of(b"", false, false, 504, false, &id).status,
            "timeout"
        );
        assert_eq!(
            outcome_of(b"", false, false, 502, false, &id).status,
            "transport_error"
        );
        assert_eq!(
            outcome_of(b"", false, false, 404, false, &id).status,
            "error"
        );
        assert_eq!(
            outcome_of(b"", true, false, 200, false, &id).status,
            "success"
        );
        assert_eq!(
            outcome_of(b"{", false, false, 200, true, &id).status,
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

    /// #3529: on the legacy `sse` transport a `202` with no reply in its body
    /// is `accepted`; one carrying a JSON-RPC error is that error, and a body
    /// that broke off is still a transport failure
    #[test]
    fn a_legacy_sse_202_is_accepted_unless_its_body_answers() {
        let id = json!(4);
        let judged = |buf: &[u8], stream_failed: bool| {
            let reply = Replies::of(buf, false).take(&id);
            outcome(reply, false, 202, stream_failed, true)
        };
        let accepted = judged(b"Accepted", false);
        assert_eq!((accepted.status, accepted.refined), ("success", "accepted"));
        let error = judged(
            br#"{"jsonrpc":"2.0","id":4,"error":{"code":-32601}}"#,
            false,
        );
        assert_eq!((error.status, error.refined), ("error", ""));
        assert_eq!(error.payload, Some(json!({"code": -32601})));
        let broken = judged(b"", true);
        assert_eq!((broken.status, broken.refined), ("transport_error", ""));
        // any other status on the legacy transport is judged as before
        let refused = outcome(None, false, 502, false, true);
        assert_eq!((refused.status, refused.refined), ("transport_error", ""));
        for refined in ["accepted", "unread"] {
            assert!(rolter_core::mcp_log::MCP_OUTCOMES.contains(&refined));
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
        let call = parse(
            &sink,
            &request("tools/call", Some(1)),
            &context(&capture, &redactor(&capture)),
        )
        .unwrap();
        let body = futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(
            br#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#,
        ))]);
        let mut observed = observe(vec![call], 200, false, body);
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

    /// #3277: a credential inside a tool argument or a tool result never reaches
    /// the stored row, whichever key it sits under.
    #[tokio::test]
    async fn a_secret_in_an_argument_or_a_result_never_reaches_the_row() {
        use futures_util::StreamExt;
        let key = format!("sk-ant-{}", "a1B2c3D4".repeat(4));
        let (sink, mut rx) = sink();
        let capture = rolter_core::PayloadCaptureConfig {
            enabled: true,
            ..Default::default()
        };
        let request = serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "search", "arguments": {"query": format!("log in with {key}")}},
        }))
        .unwrap();
        let call = parse(&sink, &request, &context(&capture, &redactor(&capture))).unwrap();
        let reply = serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"content": [{"type": "text", "text": format!("found {key}")}]},
        }))
        .unwrap();
        let body = futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from(reply))]);
        let mut observed = observe(vec![call], 200, false, body);
        while observed.next().await.is_some() {}
        let event = rx.try_recv().unwrap();
        for stored in [&event.arguments, &event.result] {
            assert!(!stored.contains(&key), "{stored}");
            assert!(stored.contains("[redacted:anthropic_key]"), "{stored}");
        }
    }

    /// #3466: the flag rides in the row only for a notification, so a writer
    /// against a table without the column sends nothing it would have to drop.
    #[test]
    fn the_notification_flag_is_sent_only_when_set() {
        let (sink, mut rx) = sink();
        let capture = rolter_core::PayloadCaptureConfig::default();
        let redactor = redactor(&capture);
        let ctx = context(&capture, &redactor);
        let call = parse(&sink, &request("tools/call", None), &ctx).unwrap();
        assert!(call.notification());
        call.finish(by_http_status(202, true, true));
        let note = serde_json::to_value(rx.try_recv().unwrap()).unwrap();
        assert_eq!(
            (&note["status"], &note["notification"]),
            (&json!("success"), &json!(1))
        );
        let call = parse(&sink, &request("tools/call", Some(1)), &ctx).unwrap();
        assert!(!call.notification());
        call.fail("error");
        let plain = serde_json::to_value(rx.try_recv().unwrap()).unwrap();
        assert!(plain.get("notification").is_none(), "{plain}");
    }

    /// a notification the upstream refused is not a success, whatever it awaited
    #[test]
    fn a_notification_ends_with_the_status_the_http_layer_gave() {
        for (http, status) in [
            (202, "success"),
            (200, "success"),
            (401, "auth_denied"),
            (404, "error"),
            (504, "timeout"),
            (502, "transport_error"),
        ] {
            let outcome = by_http_status(http, true, true);
            assert_eq!(
                (outcome.status, outcome.notification),
                (status, true),
                "{http}"
            );
        }
    }

    #[test]
    fn replies_are_handed_out_by_id_once_each() {
        let mut replies = Replies::of(
            br#"[{"id":2,"result":{"n":2}},{"id":1,"error":{"code":1}},{"id":2,"result":{"n":3}},{"method":"x"},{"id":null,"result":{}}]"#,
            false,
        );
        assert_eq!(replies.take(&json!(1)).unwrap()["error"]["code"], 1);
        assert_eq!(replies.take(&json!(2)).unwrap()["result"]["n"], 2);
        // a second call under the same id gets the second reply, not the first again
        assert_eq!(replies.take(&json!(2)).unwrap()["result"]["n"], 3);
        assert!(replies.take(&json!(2)).is_none());
        assert!(replies.take(&json!("1")).is_none());
    }

    #[tokio::test]
    async fn capture_off_keeps_payloads_out_of_the_row() {
        let (sink, mut rx) = sink();
        let capture = rolter_core::PayloadCaptureConfig::default();
        let call = parse(
            &sink,
            &request("tools/call", Some(1)),
            &context(&capture, &redactor(&capture)),
        )
        .unwrap();
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
            if let Some(call) = parse(
                &sink,
                &request("tools/call", Some(1)),
                &context(&capture, &redactor(&capture)),
            ) {
                call.fail("error");
            }
        }
        assert!(metrics.mcp_events_dropped_total.load(Relaxed) > 0);
    }
}
