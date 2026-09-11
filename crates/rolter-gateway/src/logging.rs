//! Asynchronous, batched request-log writer for ClickHouse.
//!
//! The request handler builds a [`RequestLog`] and hands it to [`LogSink::log`],
//! which only does a non-blocking `try_send` onto a bounded channel — the hot
//! path never awaits ClickHouse. A background task accumulates records and
//! flushes them in batches (on size or a timer) to the ClickHouse HTTP interface
//! using `JSONEachRow`. When the queue is full records are dropped and counted,
//! never blocked on. Token and cost fields are captured in a later phase; this
//! writer establishes the plumbing and the record shape.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use crossbeam_queue::ArrayQueue;
use futures_util::Stream;
use rust_decimal::prelude::ToPrimitive;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::metrics::Metrics;

/// ClickHouse setting appended to every insert URL so a `DateTime64(3)` column
/// accepts the RFC 3339 literal [`clickhouse_ts`] writes. The default `basic`
/// parser only reads `YYYY-MM-DD hh:mm:ss`, so without this the insert fails
/// outright rather than falling back to the column default (#1210)
pub(crate) const BEST_EFFORT_DATES: &str = "&date_time_input_format=best_effort";

/// Serialize a timestamp the way ClickHouse's `best_effort` parser reads it
/// into a `DateTime64(3)`: RFC 3339, UTC, truncated to milliseconds.
///
/// Milliseconds are the column's own precision, so nothing is sent that the
/// column would silently round away.
pub(crate) mod clickhouse_ts {
    use super::{DateTime, SecondsFormat, Utc};
    use serde::Serializer;

    pub(crate) fn serialize<S>(ts: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&ts.to_rfc3339_opts(SecondsFormat::Millis, true))
    }
}

/// The wall-clock instant a request began, reconstructed from its monotonic
/// start.
///
/// An `Instant` has no calendar value, and threading a second wall-clock field
/// down every forwarding path would only add state to keep in sync, so the
/// start is derived by walking back the elapsed monotonic time. This is what
/// makes `request_logs.ts` the request's own time rather than the time its
/// batch happened to be flushed (#1210).
pub fn started_at(started: Instant) -> DateTime<Utc> {
    let now = Utc::now();
    TimeDelta::from_std(started.elapsed())
        .ok()
        .and_then(|elapsed| now.checked_sub_signed(elapsed))
        .unwrap_or(now)
}

/// Bounded, lock-free reuse for the response bytes retained only long enough
/// to extract token usage. Oversized buffers are deliberately not retained so
/// one unusually large completion cannot inflate the steady-state footprint.
#[derive(Clone)]
struct UsageBufferPool {
    buffers: Arc<ArrayQueue<Vec<u8>>>,
}

impl Default for UsageBufferPool {
    fn default() -> Self {
        Self {
            buffers: Arc::new(ArrayQueue::new(128)),
        }
    }
}

fn should_sample_request(request_id: &str, sample_rate: f64) -> bool {
    if sample_rate >= 1.0 {
        return true;
    }
    if sample_rate <= 0.0 {
        return false;
    }
    let mut hash = 1469598103934665603u64;
    for byte in request_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    let bucket = (hash % 10_000) as f64 / 10_000.0;
    bucket < sample_rate
}

impl UsageBufferPool {
    const MAX_RETAINED_BYTES: usize = 1024 * 1024;

    fn take(&self) -> Vec<u8> {
        self.buffers
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(4096))
    }

    fn recycle(&self, mut buffer: Vec<u8>) {
        if buffer.capacity() > Self::MAX_RETAINED_BYTES {
            return;
        }
        buffer.clear();
        let _ = self.buffers.push(buffer);
    }
}

/// One row of the ClickHouse `request_logs` table. Field names match the column
/// names so the struct serializes directly as a `JSONEachRow` line.
#[derive(Debug, Clone, Serialize)]
pub struct RequestLog {
    /// when the request began, not when its batch was flushed. the column still
    /// carries `default now64(3)` so a gateway older than #1210 keeps writing,
    /// but every row this writer emits stamps its own time — otherwise a whole
    /// batch lands on one millisecond and ordering within it is lost
    #[serde(serialize_with = "clickhouse_ts::serialize")]
    pub ts: DateTime<Utc>,
    pub request_id: String,
    /// inbound distributed-trace id (W3C traceparent / B3), empty when the caller
    /// sent none — lets logs join a caller's trace across services
    pub trace_id: String,
    pub org_id: String,
    pub team_id: String,
    pub project_id: String,
    pub virtual_key_id: String,
    /// governance attribution carried by the key; empty when unattributed (#539)
    pub business_unit_id: String,
    pub customer_id: String,
    pub model: String,
    pub provider: String,
    pub target: String,
    /// chosen variant name for A/B attribution; empty on the classic single-pool
    /// path (a route with no variants)
    pub variant: String,
    pub status: u16,
    pub stream: u8,
    pub cache_hit: u8,
    /// provider-native prompt-cache input tokens reused by the upstream; this
    /// is distinct from `cache_hit`, which means a Rolter response-cache hit
    pub cache_read_tokens: u32,
    /// provider-native prompt-cache tokens written/created for this request
    pub cache_write_tokens: u32,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cost_usd: f64,
    /// 1 when this request ran against a model with **no price row**, so
    /// `cost_usd` is not a cost at all — it is the absence of one (#969).
    ///
    /// Without this, "this traffic cost nothing" and "we do not know what this
    /// traffic cost" are the same zero, and an operator can run a fleet for a
    /// month, see $0.00, and conclude spend is under control.
    pub unpriced: u8,
    pub latency_ms: u32,
    pub ttft_ms: u32,
    pub error: String,
    /// raw bodies are persisted in the short-retention `request_payloads`
    /// table, never in the primary metadata table
    #[serde(skip)]
    pub request_payload: String,
    #[serde(skip)]
    pub response_payload: String,
    #[serde(skip)]
    pub capture_payloads: bool,
    #[serde(skip)]
    pub payload_max_bytes: usize,
    #[serde(skip)]
    pub payload_redact_fields: Vec<String>,
    #[serde(skip)]
    pub sample_rate: f64,
}

impl Default for RequestLog {
    fn default() -> Self {
        Self {
            // now, not the epoch: a row that forgets to stamp its start is only
            // slightly late, where a zero would land in a 1970 partition and
            // fall straight past the table's ttl
            ts: Utc::now(),
            request_id: String::new(),
            trace_id: String::new(),
            org_id: String::new(),
            team_id: String::new(),
            project_id: String::new(),
            virtual_key_id: String::new(),
            business_unit_id: String::new(),
            customer_id: String::new(),
            model: String::new(),
            provider: String::new(),
            target: String::new(),
            variant: String::new(),
            status: 0,
            stream: 0,
            cache_hit: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cost_usd: 0.0,
            unpriced: 0,
            latency_ms: 0,
            ttft_ms: 0,
            error: String::new(),
            request_payload: String::new(),
            response_payload: String::new(),
            capture_payloads: false,
            payload_max_bytes: 0,
            payload_redact_fields: Vec::new(),
            sample_rate: 1.0,
        }
    }
}

/// Serialize a payload safely for the short-retention capture store. JSON is
/// redacted before truncation so a large secret can never escape at the edge of
/// the retained prefix; non-JSON bodies are retained as lossy UTF-8 text.
pub fn capture_payload(body: &[u8], max_bytes: usize, redact_fields: &[String]) -> String {
    if max_bytes == 0 || body.is_empty() {
        return String::new();
    }
    let mut rendered = match serde_json::from_slice::<Value>(body) {
        Ok(mut json) => {
            redact_json(&mut json, redact_fields);
            serde_json::to_string(&json).unwrap_or_else(|_| String::new())
        }
        Err(_) => String::from_utf8_lossy(body).into_owned(),
    };
    if rendered.len() > max_bytes {
        let end = rendered.floor_char_boundary(max_bytes);
        rendered.truncate(end);
        rendered.push_str("…[truncated]");
    }
    rendered
}

fn redact_json(value: &mut Value, redact_fields: &[String]) {
    match value {
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                if redact_fields
                    .iter()
                    .any(|field| field.eq_ignore_ascii_case(key))
                {
                    *value = Value::String("[REDACTED]".to_string());
                } else {
                    redact_json(value, redact_fields);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_json(value, redact_fields);
            }
        }
        _ => {}
    }
}

/// The `model` the upstream reported on its response, for
/// `gen_ai.response.model` (#808).
///
/// Reads the same buffer `parse_usage` walks. For SSE the first frame carrying
/// a `model` wins: every frame of one completion reports the same model, so
/// scanning further would only cost time. Anthropic nests it under `message` on
/// `message_start`, which is handled alongside the top-level OpenAI shape.
pub fn parse_response_model(is_sse: bool, buf: &[u8]) -> Option<String> {
    fn model_of(value: &serde_json::Value) -> Option<String> {
        value
            .get("model")
            .or_else(|| value.pointer("/message/model"))
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty())
            .map(str::to_string)
    }

    if !is_sse {
        return model_of(&serde_json::from_slice::<serde_json::Value>(buf).ok()?);
    }
    for line in buf.split(|&b| b == b'\n') {
        let line = trim_ascii(line);
        let Some(rest) = line.strip_prefix(b"data:") else {
            continue;
        };
        let rest = trim_ascii(rest);
        if rest == b"[DONE]" {
            break;
        }
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(rest) {
            if let Some(model) = model_of(&value) {
                return Some(model);
            }
        }
    }
    None
}

/// The provider's own id for the call, for `gen_ai.response.id` (#846).
///
/// This is the join key between a rolter span and the provider's record of the
/// same request — the thing that makes a provider-side support ticket a
/// reference rather than a description.
///
/// The dialects agree more than they usually do: OpenAI puts it at `id` on
/// every response and every SSE chunk, and Anthropic puts it at `id` on the
/// non-streaming message and at `message.id` inside the `message_start` event.
/// Both are read from the one buffer `parse_usage` already walks.
pub fn parse_response_id(is_sse: bool, buf: &[u8]) -> Option<String> {
    fn id_of(value: &serde_json::Value) -> Option<String> {
        value
            .get("id")
            .or_else(|| value.pointer("/message/id"))
            .and_then(|id| id.as_str())
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    }

    if !is_sse {
        return id_of(&serde_json::from_slice::<serde_json::Value>(buf).ok()?);
    }
    for line in buf.split(|&b| b == b'\n') {
        let line = trim_ascii(line);
        let Some(rest) = line.strip_prefix(b"data:") else {
            continue;
        };
        let rest = trim_ascii(rest);
        if rest == b"[DONE]" {
            break;
        }
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(rest) {
            if let Some(id) = id_of(&value) {
                return Some(id);
            }
        }
    }
    None
}

/// The width of the vectors an embeddings response returned, for
/// `gen_ai.embeddings.dimension.count` (#846).
///
/// Read from the first vector rather than declared anywhere: the request may
/// not carry `dimensions` at all, and when it does the provider is free to
/// ignore it — what the span should report is what actually came back.
///
/// `base64` encoding yields a string rather than an array, and there is no
/// honest dimension count to give without decoding it, so that case reports
/// nothing.
pub fn parse_embedding_dimensions(buf: &[u8]) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_slice(buf).ok()?;
    let len = value.pointer("/data/0/embedding")?.as_array()?.len();
    (len > 0).then_some(len as u64)
}

/// Why the model stopped generating, for `gen_ai.response.finish_reasons`
/// (#835).
///
/// This is the attribute that separates "the model finished" from "we cut it
/// off at the token limit" or "a guardrail stopped it" — a distinction the
/// other GenAI attributes cannot express at all, since a truncated completion
/// and a complete one look identical in latency and token counts.
///
/// Each dialect spells it differently, so all three are read from the one
/// buffer `parse_usage` already walks:
///
/// - OpenAI chat and completions: `choices[].finish_reason`, one per choice,
///   `null` on every streamed chunk but the last
/// - Anthropic messages: `stop_reason`, top level when buffered, on
///   `message_delta`'s `delta` when streamed (and present-but-null under
///   `message` on `message_start`)
/// - Responses API: `incomplete_details.reason` when the response stopped
///   short, otherwise the terminal `status`; nested under `response` in its SSE
///   events
///
/// Values stay provider-native — see `genai::RESPONSE_FINISH_REASONS` for why.
/// Returns empty when the response carries no reason, which is the honest
/// answer for a request that failed before generation or a route (embeddings,
/// images) that has no such concept.
pub fn parse_finish_reasons(is_sse: bool, buf: &[u8]) -> Vec<String> {
    // OpenAI reports one reason per choice, so they are collected by choice
    // index: streaming repeats the index across frames, and keying on it stops
    // an n>1 response from recording the same reason several times. the other
    // dialects describe a single generation and have no index at all
    let mut by_choice: BTreeMap<u64, String> = BTreeMap::new();
    let mut single: Option<String> = None;

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
                merge_finish_reasons(&mut by_choice, &mut single, &value);
            }
        }
    } else if let Ok(value) = serde_json::from_slice::<Value>(buf) {
        merge_finish_reasons(&mut by_choice, &mut single, &value);
    }

    if by_choice.is_empty() {
        return single.into_iter().collect();
    }
    by_choice.into_values().collect()
}

/// Merge any finish reason found in `value`, whichever dialect wrote it.
fn merge_finish_reasons(
    by_choice: &mut BTreeMap<u64, String>,
    single: &mut Option<String>,
    value: &Value,
) {
    fn non_empty(value: Option<&Value>) -> Option<&str> {
        value.and_then(Value::as_str).filter(|s| !s.is_empty())
    }

    // openai: choices[].finish_reason, null until the final chunk
    if let Some(choices) = value.get("choices").and_then(Value::as_array) {
        for (position, choice) in choices.iter().enumerate() {
            let index = choice
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(position as u64);
            if let Some(reason) = non_empty(choice.get("finish_reason")) {
                by_choice.insert(index, reason.to_string());
            }
        }
    }

    // anthropic: top level when buffered, under `delta` on message_delta. the
    // last writer wins because a stream only ever resolves the reason once, on
    // the final event — earlier occurrences are null and filtered out above
    for candidate in [
        value.get("stop_reason"),
        value.pointer("/delta/stop_reason"),
        value.pointer("/message/stop_reason"),
    ] {
        if let Some(reason) = non_empty(candidate) {
            *single = Some(reason.to_string());
        }
    }

    // responses api: the object is the payload when buffered and sits under
    // `response` in every SSE event
    for response in [Some(value), value.get("response")].into_iter().flatten() {
        // only a terminal status is a finish reason; `response.created` and
        // every delta event carry `in_progress`, which describes nothing
        let terminal = non_empty(response.get("status"))
            .filter(|s| matches!(*s, "completed" | "incomplete" | "failed" | "cancelled"));
        let Some(status) = terminal else { continue };
        // a stop-short reason is the specific answer; the status is the
        // fallback that at least says generation ended normally
        *single = Some(
            non_empty(response.pointer("/incomplete_details/reason"))
                .unwrap_or(status)
                .to_string(),
        );
    }
}

/// Token usage extracted from an upstream response.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub prompt: u32,
    pub completion: u32,
    pub total: u32,
    pub cache_read: u32,
    pub cache_write: u32,
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
        if let Some(read) = u32_field(u, "cache_read_input_tokens").or_else(|| {
            u.pointer("/prompt_tokens_details/cached_tokens")
                .and_then(Value::as_u64)
                .map(|n| n as u32)
        }) {
            usage.cache_read = usage.cache_read.max(read);
        }
        if let Some(write) = u32_field(u, "cache_creation_input_tokens") {
            usage.cache_write = usage.cache_write.max(write);
        }
    }
}

fn u32_field(value: &Value, key: &str) -> Option<u32> {
    value.get(key).and_then(|v| v.as_u64()).map(|n| n as u32)
}

pub type CompletionObserver = Box<dyn FnOnce(&[u8]) + Send>;

/// Status recorded for a request whose client went away before the response
/// finished. 499 is nginx's `client closed request`; it is not a real HTTP
/// status, which is the point — no status was ever sent to anyone. Using it
/// keeps a cancelled request distinguishable from the 200 it would otherwise be
/// logged as, without adding a column (#1083).
pub const CLIENT_DISCONNECT_STATUS: u16 = 499;

/// `error` text on a cancelled request's log row.
pub const CLIENT_DISCONNECT_ERROR: &str = "client disconnected";

/// Response body stream that forwards each chunk to the client unchanged while
/// buffering the whole body, then on end-of-stream parses token usage, stamps
/// latency/ttft and emits the completed [`RequestLog`] exactly once.
pub struct UsageLoggingStream {
    inner: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    buf: Vec<u8>,
    buffer_pool: UsageBufferPool,
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
    // optional response-body observer invoked once before the buffer is recycled
    completion_observer: Option<CompletionObserver>,
    /// upstream span held open so `gen_ai.usage.*` can be recorded once the
    /// body has been consumed (#808)
    genai_span: Option<tracing::Span>,
    /// set when the upstream stream ran to its end. Left false when the stream
    /// is dropped early, which for a response body means the client hung up
    /// mid-answer — the tokens are still billed, so the row is kept and marked
    /// rather than discarded (#1083)
    completed: bool,
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
        let buffer_pool = sink.usage_buffers.clone();
        Self {
            inner,
            buf: buffer_pool.take(),
            buffer_pool,
            is_sse,
            started,
            ttft_ms: None,
            sink,
            price,
            recorder,
            token_recorder,
            _inflight_guard: inflight_guard,
            pending: Some(log),
            completion_observer: None,
            genai_span: None,
            completed: false,
        }
    }

    pub fn with_completion_observer(mut self, observer: Option<CompletionObserver>) -> Self {
        self.completion_observer = observer;
        self
    }

    /// Attach the upstream span so token usage can be recorded on it (#808).
    ///
    /// Usage is only known once the response body has been consumed, which is
    /// after the upstream request itself returned. Holding a clone of the span
    /// keeps it open until `finalize`, so `gen_ai.usage.*` lands on the span the
    /// conventions expect rather than on nothing. `None` when no OTLP pipeline
    /// is installed, in which case this costs a moved `Option`.
    pub fn with_genai_span(mut self, span: Option<tracing::Span>) -> Self {
        self.genai_span = span;
        self
    }

    fn finalize(&mut self) {
        let Some(mut log) = self.pending.take() else {
            return;
        };
        // the client hung up before the body ended. everything generated so far
        // was still produced (and billed) upstream, so the row keeps its tokens
        // and cost and is marked instead of dropped — abandoned spend has to be
        // countable, not invisible (#1083)
        if !self.completed {
            log.status = CLIENT_DISCONNECT_STATUS;
            if log.error.is_empty() {
                log.error = CLIENT_DISCONNECT_ERROR.to_string();
            }
            self.sink
                .metrics()
                .client_disconnects_total
                .fetch_add(1, Relaxed);
        }
        let usage = parse_usage(self.is_sse, &self.buf);
        // the conventions want token counts on the inference span, and this is
        // the first moment they are known (#808)
        if let Some(span) = self.genai_span.take() {
            span.record(crate::genai::USAGE_INPUT_TOKENS, usage.prompt);
            span.record(crate::genai::USAGE_OUTPUT_TOKENS, usage.completion);
            // the model the provider says it actually served, which is not
            // always the one asked for: an alias, a dated snapshot, or a
            // provider-side substitution all show up here and nowhere else
            if let Some(model) = parse_response_model(self.is_sse, &self.buf) {
                span.record(crate::genai::RESPONSE_MODEL, model.as_str());
            }
            // the provider's own id for this call: the join key between this
            // span and the provider's record of it (#846)
            if let Some(id) = parse_response_id(self.is_sse, &self.buf) {
                span.record(crate::genai::RESPONSE_ID, id.as_str());
            }
            // embeddings spans otherwise say nothing about the vectors, and
            // dimensionality is what an index has to agree with. read from the
            // response, since the provider may ignore a requested `dimensions`
            if !self.is_sse {
                if let Some(dimensions) = parse_embedding_dimensions(&self.buf) {
                    span.record(crate::genai::EMBEDDINGS_DIMENSION_COUNT, dimensions);
                }
            }
            // whether the model stopped on its own or was cut short; joined
            // because `tracing` has no array field type (#835)
            let reasons = parse_finish_reasons(self.is_sse, &self.buf);
            if !reasons.is_empty() {
                span.record(
                    crate::genai::RESPONSE_FINISH_REASONS,
                    reasons.join(",").as_str(),
                );
            }
        }
        if let Some(observer) = self.completion_observer.take() {
            observer(&self.buf);
        }
        if log.capture_payloads {
            log.response_payload =
                capture_payload(&self.buf, log.payload_max_bytes, &log.payload_redact_fields);
        }
        self.buffer_pool.recycle(std::mem::take(&mut self.buf));
        log.prompt_tokens = usage.prompt;
        log.completion_tokens = usage.completion;
        log.total_tokens = usage.total;
        log.cache_read_tokens = usage.cache_read;
        log.cache_write_tokens = usage.cache_write;
        // cache_hit accounting arrives with the response-cache phase; price the
        // full prompt as fresh input for now
        // the price arrives already denominated in the deployment's base
        // currency (converted once when the snapshot is assembled, see
        // `state::to_base_currency`), so this is base-currency cost — the field
        // name predates non-USD support (#650)
        // a missing price is recorded as such rather than collapsing to a zero
        // that reads like a free request (#969)
        log.unpriced = u8::from(self.price.is_none());
        // the cost is computed exactly (#967) and then narrowed to `f64` here,
        // because `request_logs.cost_usd` is a ClickHouse `Float64` and the
        // budget counter behind `record` below is a Redis `INCRBYFLOAT`.
        // Those two sinks are the remaining inexact links in the chain and each
        // has its own follow-up; narrowing in one named place keeps them
        // findable rather than scattering `as f64` through the hot path
        let cost = self
            .price
            .as_ref()
            .map(|p| p.cost(usage.prompt, usage.completion, usage.cache_read))
            .unwrap_or(rust_decimal::Decimal::ZERO);
        log.cost_usd = cost.to_f64().unwrap_or(0.0);
        log.latency_ms = self.started.elapsed().as_millis() as u32;
        log.ttft_ms = self.ttft_ms.unwrap_or(log.latency_ms);
        // add this request's cost to its budget counters and its tokens to its
        // rate-limit windows. Both write to redis, so both go onto a bounded
        // queue rather than running inline — `finalize` stays sync and never
        // blocks the response path. Unlike the detached task this replaces, the
        // queue has a depth: when the counter store stalls, records are dropped
        // and counted instead of piling up without limit (#1051)
        if let Some(recorder) = self.recorder.take() {
            let cost = log.cost_usd;
            if cost > 0.0 {
                self.sink
                    .usage_recorders()
                    .record(crate::usage_recording::UsageRecord::Spend { recorder, cost });
            }
        }
        // uses total tokens so a single big request counts against tpm
        if let Some(recorder) = self.token_recorder.take() {
            let tokens = log.total_tokens as u64;
            if tokens > 0 {
                self.sink
                    .usage_recorders()
                    .record(crate::usage_recording::UsageRecord::Tokens { recorder, tokens });
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
                self.completed = true;
                self.finalize();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for UsageLoggingStream {
    // if the client disconnects mid-stream the stream is dropped without a final
    // None poll; still emit what we have, marked as a disconnect, so the request
    // is neither lost nor logged as if it had been delivered
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
        // the request's own instant, so an uptime rollup and the request row it
        // was derived from agree on when the observation happened
        ts: record.ts,
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
    /// reusable response-accounting buffers shared by all request streams
    usage_buffers: UsageBufferPool,
    // the passive funnel also feeds provider health events (ROL-197); disabled
    // when no clickhouse url is set
    health_events: crate::health_events::HealthEventSink,
    /// bounded sink for post-response budget/rate-limit recording; inert until
    /// [`LogSink::with_usage_recorders`] attaches one
    usage_recorders: crate::usage_recording::UsageRecorderSink,
}

impl LogSink {
    /// A sink that discards everything (logging disabled / used in tests).
    pub fn disabled(metrics: Arc<Metrics>) -> Self {
        Self {
            tx: None,
            health_events: crate::health_events::HealthEventSink::disabled(metrics.clone()),
            metrics,
            usage_buffers: UsageBufferPool::default(),
            usage_recorders: crate::usage_recording::UsageRecorderSink::default(),
        }
    }

    /// Attach the health-event sink fed by the passive request funnel. Returns
    /// `self` so it composes with the constructors.
    pub fn with_health_events(mut self, sink: crate::health_events::HealthEventSink) -> Self {
        self.health_events = sink;
        self
    }

    /// Attach the bounded sink that carries budget and rate-limit recording off
    /// the response path. Composes with the constructors like the above; a sink
    /// that is never attached leaves usage recording inert, which is what tests
    /// and embedders get.
    pub fn with_usage_recorders(mut self, sink: crate::usage_recording::UsageRecorderSink) -> Self {
        self.usage_recorders = sink;
        self
    }

    /// The usage-recording sink, for the response path and for metrics.
    pub fn usage_recorders(&self) -> &crate::usage_recording::UsageRecorderSink {
        &self.usage_recorders
    }

    /// The metrics registry this sink reports into. A disabled sink still has
    /// one, so counters stay correct with logging switched off.
    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
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
                "{}/?query=INSERT%20INTO%20request_logs%20FORMAT%20JSONEachRow{BEST_EFFORT_DATES}",
                clickhouse_url.trim_end_matches('/')
            ),
            payload_url: format!(
                "{}/?query=INSERT%20INTO%20request_payloads%20FORMAT%20JSONEachRow{BEST_EFFORT_DATES}",
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
            usage_buffers: UsageBufferPool::default(),
            usage_recorders: crate::usage_recording::UsageRecorderSink::default(),
        }
    }

    /// Enqueue a record without blocking. Drops (and counts) the record if the
    /// queue is full or the writer has stopped.
    pub fn log(&self, record: RequestLog) {
        // observe latency/ttft histograms + passive per-target outcome for every
        // completed request, even when clickhouse logging is disabled (metrics
        // are always present)
        self.metrics.observe_request(
            &record.provider,
            &record.model,
            record.latency_ms,
            record.ttft_ms,
            record.completion_tokens,
        );
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
        if !should_sample_request(&record.request_id, record.sample_rate) {
            return;
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
    payload_url: String,
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
        // Heuristic: ~1024 bytes per log line for pre-allocation
        let mut body = Vec::with_capacity(batch.len() * 1024);
        for record in batch.iter() {
            let start_len = body.len();
            match serde_json::to_writer(&mut body, record) {
                Ok(_) => {
                    body.push(b'\n');
                }
                Err(err) => {
                    body.truncate(start_len);
                    tracing::warn!(%err, "failed to serialize request log");
                }
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
        // Heuristic: ~1024 bytes per payload line for pre-allocation
        let mut payloads = Vec::with_capacity(batch.len() * 1024);
        for record in batch.iter().filter(|record| {
            !record.request_payload.is_empty() || !record.response_payload.is_empty()
        }) {
            let payload = PayloadLog::from(record);
            let start_len = payloads.len();
            match serde_json::to_writer(&mut payloads, &payload) {
                Ok(_) => {
                    payloads.push(b'\n');
                }
                Err(err) => {
                    payloads.truncate(start_len);
                    tracing::warn!(%err, "failed to serialize request payload");
                }
            }
        }
        if !payloads.is_empty() {
            match self
                .client
                .post(&self.payload_url)
                .body(payloads)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) => {
                    let status = resp.status();
                    let detail = resp.text().await.unwrap_or_default();
                    tracing::warn!(%status, detail, "clickhouse rejected payload batch");
                }
                Err(err) => tracing::warn!(%err, "failed to write payload batch to clickhouse"),
            }
        }
        batch.clear();
    }
}

/// One short-retention raw payload row keyed by the corresponding metadata log.
#[derive(Serialize)]
struct PayloadLog<'a> {
    /// the same instant as the metadata row, so a payload and the request it
    /// belongs to sit in the same partition and sort together
    #[serde(serialize_with = "clickhouse_ts::serialize")]
    ts: DateTime<Utc>,
    request_id: &'a str,
    request_payload: &'a str,
    response_payload: &'a str,
}

impl<'a> From<&'a RequestLog> for PayloadLog<'a> {
    fn from(log: &'a RequestLog) -> Self {
        Self {
            ts: log.ts,
            request_id: &log.request_id,
            request_payload: &log.request_payload,
            response_payload: &log.response_payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decimal literal for tests. `rust_decimal`'s `dec!` macro would read
    /// slightly better, but its `macros` feature pulls `rust_decimal_macros`,
    /// `proc-macro-crate`, `toml_edit` and `borsh` into the dependency graph in
    /// production position, which is a poor trade for test ergonomics (#967).
    fn d(literal: &str) -> rust_decimal::Decimal {
        literal.parse().expect("a valid decimal literal")
    }

    #[test]
    fn usage_buffer_pool_reuses_small_buffers() {
        let pool = UsageBufferPool::default();
        let mut buffer = pool.take();
        buffer.extend_from_slice(b"usage");
        let ptr = buffer.as_ptr();
        pool.recycle(buffer);
        let reused = pool.take();
        assert_eq!(reused.as_ptr(), ptr);
    }

    #[test]
    fn payload_capture_redacts_before_truncation() {
        let fields = vec!["token".to_string(), "password".to_string()];
        let captured = capture_payload(
            br#"{"token":"secret-value","nested":{"password":"also-secret"},"text":"hello"}"#,
            64,
            &fields,
        );
        assert!(!captured.contains("secret-value"));
        assert!(!captured.contains("also-secret"));
        assert!(captured.contains("[REDACTED]"));
    }

    #[test]
    fn request_log_serializes_ts_clickhouse_accepts() {
        let rec = RequestLog {
            ts: DateTime::from_timestamp_millis(1_757_030_542_061).expect("timestamp is in range"),
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
        // rfc 3339 at exactly the column's millisecond precision, which the
        // best_effort parser reads into DateTime64(3)
        assert_eq!(value["ts"], "2025-09-05T00:02:22.061Z");
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
                total: 33,
                ..Usage::default()
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
                total: 12,
                ..Usage::default()
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
                total: 12,
                ..Usage::default()
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
                total: 65,
                ..Usage::default()
            }
        );
    }

    #[test]
    fn missing_usage_is_zero() {
        assert_eq!(parse_usage(false, b"{\"id\":\"x\"}"), Usage::default());
    }

    #[test]
    fn parses_openai_non_stream_finish_reason() {
        let body = br#"{"id":"x","choices":[{"index":0,"finish_reason":"stop"}]}"#;
        assert_eq!(parse_finish_reasons(false, body), vec!["stop"]);
    }

    /// The case the attribute exists for: the completion did not finish, it ran
    /// out of room. Nothing else on the span says so.
    #[test]
    fn parses_openai_length_truncation() {
        let body = br#"{"choices":[{"index":0,"finish_reason":"length"}]}"#;
        assert_eq!(parse_finish_reasons(false, body), vec!["length"]);
    }

    /// `n>1` yields one reason per choice, in choice order rather than the
    /// order the frames happened to arrive in.
    #[test]
    fn parses_one_reason_per_choice_in_index_order() {
        let body = br#"{"choices":[
            {"index":1,"finish_reason":"length"},
            {"index":0,"finish_reason":"stop"}
        ]}"#;
        assert_eq!(parse_finish_reasons(false, body), vec!["stop", "length"]);
    }

    /// Every streamed chunk carries `finish_reason: null` until the last one,
    /// so a naive scan would record nothing and a repeated scan would record
    /// the same reason many times.
    #[test]
    fn parses_openai_sse_finish_reason_from_the_final_chunk_only() {
        let sse = b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
data: [DONE]\n\n";
        assert_eq!(parse_finish_reasons(true, sse), vec!["stop"]);
    }

    #[test]
    fn parses_anthropic_non_stream_stop_reason() {
        let body = br#"{"type":"message","stop_reason":"end_turn"}"#;
        assert_eq!(parse_finish_reasons(false, body), vec!["end_turn"]);
    }

    /// Anthropic resolves the reason on `message_delta`; `message_start`
    /// carries the key with a null value, which must not win.
    #[test]
    fn parses_anthropic_sse_stop_reason_from_message_delta() {
        let sse = b"event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"stop_reason\":null}}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n";
        assert_eq!(parse_finish_reasons(true, sse), vec!["max_tokens"]);
    }

    /// The Responses API has no `finish_reason` key at all: a response that
    /// stopped short says so through `incomplete_details`.
    #[test]
    fn parses_responses_api_incomplete_reason() {
        let body = br#"{"object":"response","status":"incomplete",
            "incomplete_details":{"reason":"max_output_tokens"}}"#;
        assert_eq!(parse_finish_reasons(false, body), vec!["max_output_tokens"]);
    }

    #[test]
    fn parses_responses_api_completed_status() {
        let body = br#"{"object":"response","status":"completed"}"#;
        assert_eq!(parse_finish_reasons(false, body), vec!["completed"]);
    }

    /// `in_progress` describes nothing, and the Responses stream emits it on
    /// every event before the last. Only a terminal status is a finish reason.
    #[test]
    fn ignores_non_terminal_responses_api_status() {
        let sse =
            b"data: {\"type\":\"response.created\",\"response\":{\"status\":\"in_progress\"}}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";
        assert_eq!(parse_finish_reasons(true, sse), vec!["completed"]);
    }

    /// Embeddings, images and any request that failed before generation have
    /// no finish reason; the attribute is then absent rather than invented.
    #[test]
    fn missing_finish_reason_is_empty() {
        assert!(parse_finish_reasons(false, br#"{"id":"x"}"#).is_empty());
        assert!(parse_finish_reasons(false, br#"{"data":[{"embedding":[0.1]}]}"#).is_empty());
    }

    // ── gen_ai.response.id (#846) ───────────────────────────────────────────
    // the join key between a rolter span and the provider's own record of the
    // call, so each dialect's spelling is pinned rather than assumed

    #[test]
    fn openai_response_id_is_read_from_the_body() {
        let body = br#"{"id":"chatcmpl-9x","object":"chat.completion","model":"gpt-4o"}"#;
        assert_eq!(
            parse_response_id(false, body).as_deref(),
            Some("chatcmpl-9x")
        );
    }

    #[test]
    fn openai_response_id_is_read_from_the_first_sse_chunk() {
        let sse = b"data: {\"id\":\"chatcmpl-9x\",\"choices\":[]}\n\ndata: [DONE]\n\n";
        assert_eq!(parse_response_id(true, sse).as_deref(), Some("chatcmpl-9x"));
    }

    /// Anthropic nests it under `message` in the `message_start` event, which
    /// is exactly the shape `parse_response_model` already has to handle.
    #[test]
    fn anthropic_response_id_is_read_from_message_start() {
        let sse = b"data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\"}}\n\n";
        assert_eq!(parse_response_id(true, sse).as_deref(), Some("msg_01"));
        let body = br#"{"id":"msg_02","type":"message","role":"assistant"}"#;
        assert_eq!(parse_response_id(false, body).as_deref(), Some("msg_02"));
    }

    /// An absent or empty id is reported as absent. A span carrying an empty
    /// join key is worse than one carrying none: it looks answerable.
    #[test]
    fn a_missing_response_id_is_none() {
        assert_eq!(parse_response_id(false, br#"{"model":"gpt-4o"}"#), None);
        assert_eq!(parse_response_id(false, br#"{"id":""}"#), None);
        assert_eq!(parse_response_id(false, b"not json at all"), None);
        assert_eq!(parse_response_id(true, b"data: [DONE]\n\n"), None);
    }

    // ── gen_ai.embeddings.dimension.count (#846) ────────────────────────────

    /// Read from the vector that actually came back, not from a requested
    /// `dimensions` the provider is free to ignore.
    #[test]
    fn embedding_dimensions_come_from_the_returned_vector() {
        let body = br#"{"data":[{"embedding":[0.1,0.2,0.3,0.4]}],"model":"text-embedding-3"}"#;
        assert_eq!(parse_embedding_dimensions(body), Some(4));
    }

    /// `encoding_format: "base64"` returns a string, and there is no honest
    /// count to give without decoding it, so nothing is reported.
    #[test]
    fn a_base64_embedding_reports_no_dimension_count() {
        let body = br#"{"data":[{"embedding":"eyJhIjoxfQ=="}]}"#;
        assert_eq!(parse_embedding_dimensions(body), None);
    }

    #[test]
    fn a_non_embeddings_response_reports_no_dimension_count() {
        assert_eq!(parse_embedding_dimensions(br#"{"id":"chatcmpl-9x"}"#), None);
        assert_eq!(parse_embedding_dimensions(br#"{"data":[]}"#), None);
        assert_eq!(
            parse_embedding_dimensions(br#"{"data":[{"embedding":[]}]}"#),
            None
        );
    }

    /// Collects every field recorded onto a span after creation, so the GenAI
    /// attributes are asserted as *recorded* rather than as merely parsed. The
    /// same shape `trace.rs` uses for the tenant attributes (#836).
    #[derive(Clone, Default)]
    struct Recorded(Arc<parking_lot::Mutex<Vec<(String, String)>>>);

    impl Recorded {
        fn get(&self, key: &str) -> Option<String> {
            let seen = self.0.lock();
            seen.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        }
    }

    impl tracing::field::Visit for Recorded {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0
                .lock()
                .push((field.name().to_string(), value.to_string()));
        }

        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            self.0
                .lock()
                .push((field.name().to_string(), value.to_string()));
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .lock()
                .push((field.name().to_string(), format!("{value:?}")));
        }
    }

    impl<S> tracing_subscriber::Layer<S> for Recorded
    where
        S: tracing::Subscriber,
    {
        fn on_record(
            &self,
            _span: &tracing::span::Id,
            values: &tracing::span::Record<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            values.record(&mut self.clone());
        }
    }

    /// End of the #846 path, driven through `UsageLoggingStream` rather than by
    /// calling the parsers: an embeddings response must leave the span carrying
    /// the provider's id *and* the width of the vectors, neither of which any
    /// other attribute expresses. A parser that is never wired into `finalize`
    /// fails here, which is the failure mode worth testing for.
    #[tokio::test]
    async fn an_embeddings_response_records_its_id_and_dimension_count() {
        use futures_util::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tracing_subscriber::layer::SubscriberExt as _;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await;
        });

        let recorded = Recorded::default();
        let subscriber = tracing_subscriber::registry().with(recorded.clone());
        let _default = tracing::subscriber::set_default(subscriber);
        let span = tracing::info_span!(
            "upstream.request",
            gen_ai.response.id = tracing::field::Empty,
            gen_ai.embeddings.dimension.count = tracing::field::Empty,
        );

        let body = br#"{"id":"embd-77","data":[{"embedding":[0.1,0.2,0.3]}],"usage":{"prompt_tokens":2,"total_tokens":2}}"#;
        let inner = futures_util::stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(
            body.to_vec(),
        ))]);
        let mut wrapped = UsageLoggingStream::new(
            Box::pin(inner),
            false,
            Instant::now(),
            LogSink::spawn(
                format!("http://{addr}"),
                10,
                Duration::from_millis(50),
                100,
                Arc::new(Metrics::default()),
            ),
            None,
            RequestLog {
                request_id: "req-embed".to_string(),
                model: "text-embedding-3".to_string(),
                ..Default::default()
            },
            None,
            None,
            None,
        )
        .with_genai_span(Some(span));

        while wrapped.next().await.is_some() {}
        drop(wrapped); // finalize runs here

        assert_eq!(
            recorded.get(crate::genai::RESPONSE_ID).as_deref(),
            Some("embd-77"),
            "the provider id is the join key to its own record of the call"
        );
        assert_eq!(
            recorded
                .get(crate::genai::EMBEDDINGS_DIMENSION_COUNT)
                .as_deref(),
            Some("3"),
        );
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
        // the derived event is stamped with the request's instant, not "now"
        assert_eq!(ok.ts, base.ts);
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

    #[test]
    fn started_at_walks_back_the_monotonic_elapsed_time() {
        let now = Instant::now();
        let a_minute_ago = now.checked_sub(Duration::from_secs(60)).unwrap_or(now);
        let stamped = started_at(a_minute_ago);
        let gap = (Utc::now() - stamped).num_seconds();
        assert!((55..=65).contains(&gap), "expected ~60s back, got {gap}s");
    }

    #[test]
    fn sampling_rate_boundaries_are_respected() {
        assert!(should_sample_request("req-1", 1.0));
        assert!(should_sample_request("req-1", 2.0));
        assert!(!should_sample_request("req-1", 0.0));
        assert!(!should_sample_request("req-1", -0.1));
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
    async fn each_row_in_a_batch_keeps_its_own_request_time() {
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
        let sink = LogSink::spawn(
            format!("http://{addr}"),
            10,
            Duration::from_millis(50),
            100,
            metrics.clone(),
        );

        // two requests that began a minute apart but complete close enough
        // together to share one flush
        let now = Instant::now();
        let older = now.checked_sub(Duration::from_secs(60)).unwrap_or(now);
        let flushed_after = Utc::now();
        for (request_id, started) in [("req-old", older), ("req-new", now)] {
            sink.log(RequestLog {
                ts: started_at(started),
                request_id: request_id.to_string(),
                ..Default::default()
            });
        }

        let req = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        // clickhouse only reads an rfc 3339 literal into DateTime64(3) when the
        // insert asks for the best_effort parser
        assert!(req.contains("date_time_input_format=best_effort"));

        let body = req.split("\r\n\r\n").nth(1).expect("request has a body");
        let rows: Vec<serde_json::Value> = body
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("each line is one json row"))
            .collect();
        assert_eq!(rows.len(), 2);

        let mut stamps = Vec::new();
        for row in &rows {
            let raw = row["ts"].as_str().expect("ts is serialized");
            stamps.push(
                DateTime::parse_from_rfc3339(raw)
                    .expect("ts is rfc 3339")
                    .with_timezone(&Utc),
            );
        }
        // the whole point of #1210: one batch, two rows, two different times
        assert_ne!(stamps[0], stamps[1]);
        let gap = (stamps[1] - stamps[0]).num_seconds();
        assert!(
            (55..=65).contains(&gap),
            "rows should be ~60s apart, were {gap}s"
        );
        // and both predate the flush, so neither borrowed the writer's clock
        assert!(stamps[1] <= flushed_after);
    }

    #[tokio::test]
    async fn a_burst_inside_one_flush_keeps_millisecond_resolution() {
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
        let sink = LogSink::spawn(
            format!("http://{addr}"),
            10,
            Duration::from_millis(50),
            100,
            metrics.clone(),
        );

        // a burst: four requests a few milliseconds apart, all landing in one
        // flush. the minute-apart case above proves the writer's clock is not
        // borrowed; this proves the stamp survives at the resolution a burst
        // actually happens at, which is what the logs screen renders (#1344)
        let now = Instant::now();
        let gap_ms = 25u64;
        for (index, request_id) in ["req-a", "req-b", "req-c", "req-d"].iter().enumerate() {
            let offset = Duration::from_millis(gap_ms * (3 - index as u64));
            let started = now.checked_sub(offset).unwrap_or(now);
            sink.log(RequestLog {
                ts: started_at(started),
                request_id: (*request_id).to_string(),
                ..Default::default()
            });
        }

        let req = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        let body = req.split("\r\n\r\n").nth(1).expect("request has a body");
        let stamps: Vec<DateTime<Utc>> = body
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let row: serde_json::Value =
                    serde_json::from_str(line).expect("each line is one json row");
                let raw = row["ts"].as_str().expect("ts is serialized").to_string();
                DateTime::parse_from_rfc3339(&raw)
                    .expect("ts is rfc 3339")
                    .with_timezone(&Utc)
            })
            .collect();
        assert_eq!(stamps.len(), 4);

        let mut distinct = stamps.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            4,
            "burst collapsed onto one stamp: {stamps:?}"
        );

        // and the rows stay in the order the requests arrived, spaced by the
        // gap between them rather than by the flush
        for pair in stamps.windows(2) {
            let delta = (pair[1] - pair[0]).num_milliseconds();
            assert!(
                (gap_ms as i64 - 5..=gap_ms as i64 + 5).contains(&delta),
                "rows should be ~{gap_ms}ms apart, were {delta}ms: {stamps:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_client_that_leaves_mid_stream_is_marked_and_keeps_its_tokens() {
        use futures_util::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 16384];
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

        // two frames of a streamed answer: the usage frame the provider sends
        // before the caller gives up, then a frame that is never read
        let frames = vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from_static(
                b"data: {\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":6,\"total_tokens\":10}}\n\n",
            )),
            Ok(Bytes::from_static(b"data: [DONE]\n\n")),
        ];
        let mut wrapped = UsageLoggingStream::new(
            Box::pin(futures_util::stream::iter(frames)),
            true,
            Instant::now(),
            sink,
            None,
            RequestLog {
                request_id: "req-abandoned".to_string(),
                model: "gpt-4o".to_string(),
                status: 200,
                stream: 1,
                ..Default::default()
            },
            None,
            None,
            None,
        );

        // the client reads one frame and hangs up: axum drops the body stream
        let _ = wrapped.next().await.unwrap().unwrap();
        drop(wrapped);

        let req = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        // the row survives, marked as a disconnect rather than as the 200 it
        // was on its way to being
        assert!(req.contains("\"request_id\":\"req-abandoned\""), "{req}");
        assert!(req.contains("\"status\":499"), "{req}");
        assert!(req.contains("client disconnected"), "{req}");
        // and it keeps what the provider already generated — that spend
        // happened whether or not anyone read it
        assert!(req.contains("\"total_tokens\":10"), "{req}");
        assert_eq!(metrics.client_disconnects_total.load(Relaxed), 1);
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
            input_per_mtok: d("1000000.0"), // 1 usd per token, for an exact assert
            output_per_mtok: d("1000000.0"),
            cached_input_per_mtok: None,
            currency: "USD".to_string(),
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
        // a priced request is not flagged
        assert!(req.contains("\"unpriced\":0"), "{req}");
    }

    /// #969: a model with no price row was billed at zero and reported as
    /// zero, so "this cost nothing" and "we do not know what this cost" were
    /// the same number. The record has to tell them apart.
    #[tokio::test]
    async fn a_model_with_no_price_is_recorded_as_unpriced_not_as_zero_cost() {
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
        let mut wrapped = UsageLoggingStream::new(
            Box::pin(inner),
            false,
            Instant::now(),
            sink,
            // the whole point: no price row for this model
            None,
            RequestLog {
                request_id: "req-unpriced".to_string(),
                model: "brand-new-model".to_string(),
                ..Default::default()
            },
            None,
            None,
            None,
        );

        let mut forwarded = Vec::new();
        while let Some(chunk) = wrapped.next().await {
            forwarded.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(forwarded, upstream);
        drop(wrapped);

        let req = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        // the tokens were really served, so this is not an empty request
        assert!(req.contains("\"total_tokens\":10"), "{req}");
        // and it is marked unpriced, which is what makes the zero readable as
        // "unknown" rather than "free"
        assert!(req.contains("\"unpriced\":1"), "{req}");
        assert!(req.contains("\"cost_usd\":0.0"), "{req}");
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
