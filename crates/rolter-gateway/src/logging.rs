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
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::metrics::Metrics;

/// ClickHouse settings appended to every insert URL.
///
/// `date_time_input_format=best_effort` lets a `DateTime64(3)` column accept
/// the RFC 3339 literal [`clickhouse_ts`] writes. The default `basic` parser
/// only reads `YYYY-MM-DD hh:mm:ss`, so without it the insert fails outright
/// rather than falling back to the column default (#1210).
///
/// `input_format_skip_unknown_fields=1` lets a gateway that writes a column a
/// newer ClickHouse migration adds (for example `log_id`, #1937) keep logging
/// against a ClickHouse that has not applied that migration yet: the unknown
/// field is dropped instead of failing the whole batch, so the order of a
/// rolling upgrade does not matter.
pub(crate) const INSERT_SETTINGS: &str =
    "&date_time_input_format=best_effort&input_format_skip_unknown_fields=1";

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

/// The number of requests one kept row represents at `sample_rate`.
fn sample_weight(sample_rate: f64) -> f64 {
    if sample_rate > 0.0 && sample_rate < 1.0 {
        1.0 / sample_rate
    } else {
        1.0
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

fn is_zero(n: &u32) -> bool {
    *n == 0
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
    /// the gateway's own key for this row, minted when the row is queued. the
    /// caller picks `request_id`, so only this names one request: the control
    /// plane joins a captured body to its row on it (#1937). nil on a row that
    /// was never queued, which is written as the column's empty default
    #[serde(skip_serializing_if = "Uuid::is_nil")]
    pub log_id: Uuid,
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
    /// the part of `cache_write_tokens` that went to the provider's 1 hour
    /// cache, for a provider that reports the split (#2891; Anthropic's
    /// `usage.cache_creation`). `cache_write_tokens` stays the total, so this
    /// is a share of it and the two never add. `0` when the provider reported
    /// no split, which prices every write at the plain write rate. Not
    /// serialized when `0`, so a row that never used the 1 hour cache is byte
    /// for byte what it was before the column, and a ClickHouse that has not
    /// had `clickhouse/017_cache_write_1h.sql` applied keeps accepting it
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_write_1h_tokens: u32,
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
    /// 1 when the upstream produced (and billed) this response but a post-call
    /// policy — an output guardrail or a post-response plugin — refused to
    /// hand it to the caller (#1478).
    ///
    /// `status` is what the caller received (403); the token and cost columns
    /// are what the provider charged. Keeping both on one row is the point:
    /// a refusal is a delivery outcome, not a billing one, and summing
    /// `cost_usd` has to include money spent on answers nobody saw.
    pub withheld: u8,
    /// 1 when the upstream answered successfully but reported no token usage,
    /// so the zero token and cost columns are unknown rather than free (#1478).
    ///
    /// Typical causes are an OpenAI-style stream without
    /// `stream_options.include_usage`, or a provider that omits the usage
    /// object altogether.
    pub usage_unknown: u8,
    pub latency_ms: u32,
    pub ttft_ms: u32,
    pub error: String,
    /// HTTP status of the last upstream attempt that answered, `0` when none
    /// did: a refusal, a cache hit, a built-in model, or a connection that
    /// failed before any status line (#2807).
    ///
    /// `status` is what the caller received. The two differ exactly when the
    /// gateway answered with an error of its own after the upstream failed: a
    /// provider's `429` that ran out of targets is a `429` here and a `429` or
    /// `503` of the gateway's making in `status`. Without it an operator reads
    /// a gateway-made `503` and has to guess which upstream status caused it.
    pub upstream_status: u16,
    /// How many upstream attempts the request made, the one that answered
    /// included; `0` when it never reached an upstream. Saturates at 255.
    pub attempts: u8,
    /// Which call on a stored response this row records: `retrieve`, `delete`,
    /// `cancel`, `input_items`, or one of the two the gateway does not serve
    /// (`compact`, `input_tokens`). Empty for a request that ran a model,
    /// which is every other row (#2836).
    ///
    /// Nothing else on a lifecycle row tells the operations apart: they share
    /// a model and provider, carry no tokens and cost nothing, and a `GET` and
    /// a `DELETE` of the same response differ only in what they did to it.
    /// Not serialized when empty, so a model request's row is byte for byte
    /// what it was before the column existed.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub lifecycle_operation: String,
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
    /// how many real requests this stored row stands for: `1 / sample_rate`
    /// at write time, so analytics can scale counts and sums back to the
    /// traffic that actually ran (#2239). Stamped by the sink after the
    /// sampling decision, never by the request path.
    pub sample_weight: f64,
}

impl Default for RequestLog {
    fn default() -> Self {
        Self {
            // now, not the epoch: a row that forgets to stamp its start is only
            // slightly late, where a zero would land in a 1970 partition and
            // fall straight past the table's ttl
            ts: Utc::now(),
            request_id: String::new(),
            log_id: Uuid::nil(),
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
            cache_write_1h_tokens: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cost_usd: 0.0,
            unpriced: 0,
            withheld: 0,
            usage_unknown: 0,
            latency_ms: 0,
            ttft_ms: 0,
            error: String::new(),
            upstream_status: 0,
            attempts: 0,
            lifecycle_operation: String::new(),
            request_payload: String::new(),
            response_payload: String::new(),
            capture_payloads: false,
            payload_max_bytes: 0,
            payload_redact_fields: Vec::new(),
            sample_rate: 1.0,
            sample_weight: 1.0,
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
    /// the part of `cache_write` the provider says went to its 1 hour cache,
    /// for a provider that bills it apart from the 5 minute one (#2891).
    /// Anthropic's `usage.cache_creation.ephemeral_1h_input_tokens`; `0` when
    /// the body reported no split, and never part of `cache_write` twice
    pub cache_write_1h: u32,
    /// whether the body carried a usage object at all. Without it an all-zero
    /// usage is indistinguishable from an upstream that said nothing (#1478)
    pub reported: bool,
}

/// [`parse_usage_with`] for a provider whose usage is read as stated, which is
/// all of them but one, and so what the tests of the others read.
#[cfg(test)]
pub fn parse_usage(is_sse: bool, buf: &[u8]) -> Usage {
    parse_usage_with(is_sse, buf, rolter_proxy::ThinkingCount::Stated)
}

/// Extract token usage from a fully-buffered upstream response body, for a
/// provider that counts its thinking tokens the way `thinking` says (#2880).
///
/// Handles both OpenAI (`prompt_tokens`/`completion_tokens`/`total_tokens`) and
/// Anthropic (`input_tokens`/`output_tokens`, top-level or under `message`) key
/// styles, for non-streamed JSON and SSE. The Responses API spells the counts
/// like Anthropic but nests them under `response` on its streamed terminal
/// events (`response.completed`, `response.incomplete`, `response.failed`).
/// For SSE every `data:` object is scanned and the largest values are kept,
/// since streamed usage is cumulative or reported once at the end (OpenAI final
/// chunk, Anthropic `message_start`/`message_delta`, Responses terminal event).
/// `total` falls back to `prompt + completion` when the upstream does not
/// report it.
///
/// A provider that states its thinking tokens in `total_tokens` alone (Gemini's
/// OpenAI-compatible endpoint) bills them as output, so they are counted into
/// the completion; for any other provider `thinking` changes nothing.
pub fn parse_usage_with(is_sse: bool, buf: &[u8], thinking: rolter_proxy::ThinkingCount) -> Usage {
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
                merge_usage(&mut usage, &value, thinking);
            }
        }
    } else if let Ok(value) = serde_json::from_slice::<Value>(buf) {
        merge_usage(&mut usage, &value, thinking);
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
fn merge_usage(usage: &mut Usage, value: &Value, thinking: rolter_proxy::ThinkingCount) {
    // usage can sit at the top level (openai, anthropic non-stream / message_delta,
    // a buffered responses api body), under `message` (anthropic message_start
    // event) or under `response` (every streamed responses api event: `created`
    // and `in_progress` carry `usage: null`, the terminal ones the counts)
    for holder in [
        value.get("usage"),
        value.pointer("/message/usage"),
        value.pointer("/response/usage"),
    ] {
        let Some(u) = holder.filter(|u| u.is_object()) else {
            continue;
        };
        usage.reported = true;
        let cache_beside = |key: &str| u32_field(u, key);
        // anthropic's write count: its own total, or the 5 minute and 1 hour
        // breakdown added up for a body that states only that
        let anthropic_write = rolter_proxy::anthropic_cache_written_tokens(u).map(|n| n as u32);
        // `input_tokens` is two things. Anthropic's leaves its cache reads and
        // writes out and reports them beside it, so the prompt is the three
        // added up; the Responses api's has the cached share inside, named in
        // `input_tokens_details`. `ModelPriceConfig::cost` wants the cached
        // share to be part of the prompt it is handed, so an Anthropic-shaped
        // object is folded into that convention here (#2863). That is also how
        // a Messages body translated from a Chat Completions answer reads
        let prompt = u32_field(u, "prompt_tokens").or_else(|| {
            u32_field(u, "input_tokens").map(|input| {
                input
                    .saturating_add(cache_beside("cache_read_input_tokens").unwrap_or(0))
                    .saturating_add(anthropic_write.unwrap_or(0))
            })
        });
        // reasoning tokens are inside the completion except for a provider
        // that states them beside it (xai, whose `total_tokens` is the only
        // figure to add them up), and they are billed as output either way
        // (#2888). gemini's openai-compatible endpoint states them nowhere but
        // in the total (#2880), which `thinking` knows to look at
        let reasoning_beside = thinking.beside_completion(u).map(|n| n as u32).unwrap_or(0);
        let completion = u32_field(u, "completion_tokens")
            .or_else(|| u32_field(u, "output_tokens"))
            .map(|c| c.saturating_add(reasoning_beside));
        if let Some(p) = prompt {
            usage.prompt = usage.prompt.max(p);
        }
        if let Some(c) = completion {
            usage.completion = usage.completion.max(c);
        }
        if let Some(t) = u32_field(u, "total_tokens") {
            usage.total = usage.total.max(t);
        }
        // anthropic's own field, or one of the spellings that count the hit
        // inside the prompt total, which is how `ModelPriceConfig::cost` reads
        // it too: chat completions' and the responses api's details blocks
        // (#2847), deepseek's `prompt_cache_hit_tokens`, kimi's top-level
        // `cached_tokens` and gigachat's `precached_prompt_tokens` (#2877).
        // the translators read the same list, so a body keeps its figure
        // across a dialect hop
        if let Some(read) = cache_beside("cache_read_input_tokens")
            .or_else(|| rolter_proxy::cached_prompt_tokens(u).map(|n| n as u32))
        {
            usage.cache_read = usage.cache_read.max(read);
        }
        // the write count has no OpenAI field; anthropic's own, or one of the
        // spellings that count it inside the prompt total, which the
        // translators read from the same list: the `cache_write_tokens` they
        // (and OpenRouter, Perplexity) put beside `cached_tokens`, qwen's
        // `cache_creation_input_tokens` in the details block (#2879) and
        // vllm's `created_cache_tokens`
        if let Some(write) = anthropic_write
            .or_else(|| rolter_proxy::cache_written_prompt_tokens(u).map(|n| n as u32))
        {
            usage.cache_write = usage.cache_write.max(write);
        }
        // the share of those writes that went to the 1 hour cache, which a
        // provider bills at its own rate (#2891). anthropic states the split
        // on `message_start` only, and its closing `message_delta` repeats the
        // totals without it, so the largest figure seen is kept like the
        // others. the translators name it the same way on the two OpenAI
        // shapes, so a request served across dialects keeps its split
        if let Some(long) = rolter_proxy::cache_written_one_hour_tokens(u) {
            usage.cache_write_1h = usage.cache_write_1h.max(long as u32);
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
    /// usage read from the upstream body before any policy transformed it.
    /// When set it is what gets billed, instead of whatever the delivered bytes
    /// say — a guardrail, plugin or sanitizer may rewrite or drop the usage
    /// object, and spend must follow the provider rather than the policy (#1478)
    billed: Option<Usage>,
    /// how the provider counts its thinking tokens (#2880)
    thinking: rolter_proxy::ThinkingCount,
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
            billed: None,
            thinking: rolter_proxy::ThinkingCount::Stated,
        }
    }

    /// Read the usage of the buffered body the way the provider counts its
    /// thinking tokens (#2880). Gemini's OpenAI-compatible endpoint states them
    /// in `total_tokens` alone, so a passthrough answer's `completion_tokens`
    /// would otherwise leave out tokens the provider billed as output.
    pub fn with_thinking_count(mut self, thinking: rolter_proxy::ThinkingCount) -> Self {
        self.thinking = thinking;
        self
    }

    /// Bill `usage` — read from the upstream body before any post-call policy
    /// touched it — rather than whatever the forwarded bytes report (#1478).
    ///
    /// The buffered path runs output guardrails, post-response plugins and the
    /// PII sanitizer over the body before it reaches this stream. Any of those
    /// can rewrite or remove the usage object, and none of them changes what
    /// the provider charged.
    pub fn with_billed_usage(mut self, usage: Usage) -> Self {
        self.billed = Some(usage);
        self
    }

    /// Account a response the upstream produced but a post-call policy refused
    /// to deliver, then drop the stream without forwarding anything (#1478).
    ///
    /// The row keeps the billed tokens and cost and still feeds the budget and
    /// rate-limit counters — the provider was paid whether or not the caller
    /// saw the answer. `status` is the refusal the caller received and
    /// `reason` names the policy that refused; the rejected content is never
    /// captured, since the body never enters this stream. Pair it with
    /// [`UsageLoggingStream::with_billed_usage`], as there is nothing here to
    /// parse usage from.
    pub fn withhold(mut self, status: u16, reason: String) {
        if let Some(log) = self.pending.as_mut() {
            log.status = status;
            log.error = reason;
            log.withheld = 1;
        }
        // a refusal is a finished exchange, not a caller that hung up
        self.completed = true;
        // the observer records a delivered body (the responses registry);
        // nothing was delivered
        self.completion_observer = None;
        self.sink
            .metrics()
            .withheld_responses_total
            .fetch_add(1, Relaxed);
        // dropping runs `finalize` exactly once
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
        // read before a disconnect rewrites the status: an upstream that
        // answered successfully but reported no usage leaves the zeros below
        // unknown, not free. an upstream error is taken as unbilled (#1478)
        let upstream_answered = log.withheld == 1 || log.status < 400;
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
        let usage = self
            .billed
            .take()
            .unwrap_or_else(|| parse_usage_with(self.is_sse, &self.buf, self.thinking));
        log.usage_unknown = u8::from(upstream_answered && !usage.reported);
        // an upstream error the caller received as it was. the status alone says
        // that it failed, not why; the body already sits in this buffer, so the
        // row can say what the upstream said at no extra cost (#2807)
        if self.completed && log.withheld == 0 && log.error.is_empty() && log.upstream_status >= 400
        {
            log.error =
                crate::upstream_failure::upstream_error_text(log.upstream_status, &self.buf);
        }
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
        // a share of the writes, never more than them
        log.cache_write_1h_tokens = usage.cache_write_1h.min(usage.cache_write);
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
            .map(|p| {
                p.cost(
                    usage.prompt,
                    usage.completion,
                    usage.cache_read,
                    usage.cache_write,
                    usage.cache_write_1h,
                )
            })
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

/// Classify an upstream outcome for the health funnel. A 2xx is `ok`; a
/// timed-out upstream is `timeout`; anything else is `error`. `status` 0 means
/// the attempt never got a response (connect failure).
fn classify_health(
    status: u16,
    error: &str,
) -> (crate::health_events::HealthOutcome, Option<String>) {
    use crate::health_events::HealthOutcome;
    if (200..300).contains(&status) {
        return (HealthOutcome::Ok, None);
    }
    if error.contains("timed out") || error.contains("timeout") {
        return (HealthOutcome::Timeout, Some("timeout".to_string()));
    }
    let kind = if status == 429 {
        "rate_limited"
    } else if status >= 500 {
        "upstream_error"
    } else if status == 0 {
        "connect_error"
    } else {
        "error"
    };
    (HealthOutcome::Error, Some(kind.to_string()))
}

/// Derive a passive [`HealthEvent`](crate::health_events::HealthEvent) from a
/// completed request. Describes the attempt that answered the caller; every
/// attempt before it is funnelled by [`LogSink::record_failed_attempt`].
fn passive_health_event(record: &RequestLog) -> crate::health_events::HealthEvent {
    use crate::health_events::{HealthEvent, HealthSource};
    // a caller handed the upstream's own status is classified by that status.
    // the words in `error` are then the upstream's, not ours to read a verdict
    // from: "timed out" in a 429's message does not make the target a timeout
    // (#2807). a status the gateway made up, such as the 502 for a body that
    // could not be read, still takes its verdict from its own error text
    let error = if record.upstream_status > 0 && record.upstream_status == record.status {
        ""
    } else {
        &record.error
    };
    let (outcome, error_kind) = classify_health(record.status, error);
    HealthEvent {
        // the request's own instant, so an uptime rollup and the request row it
        // was derived from agree on when the observation happened
        ts: record.ts,
        target_id: record.target.clone(),
        provider: record.provider.clone(),
        org_id: String::new(),
        source: HealthSource::Passive,
        outcome,
        status_code: (record.status > 0).then_some(record.status),
        latency_ms: record.latency_ms,
        error_kind,
    }
}

/// One upstream attempt that failed and was superseded by another attempt —
/// a retry against a sibling key, or a failover to another target (#1646).
///
/// The caller never sees these: the gateway recovers and answers 200. That is
/// exactly why they have to be recorded, since a target dropping a quarter of
/// its requests behind a working failover is otherwise indistinguishable from
/// a healthy one on every operator surface.
pub struct FailedAttempt<'a> {
    pub provider: &'a str,
    /// upstream model the attempt asked for, matching `RequestLog::target`
    pub target: &'a str,
    /// upstream status, or `0` when the attempt never got a response
    pub status: u16,
    pub latency_ms: u32,
    pub error: &'a str,
}

/// Build the health event for a superseded attempt. Emitted under the same
/// `passive` source as the request-level one, since both are observations made
/// by real traffic rather than by the prober.
fn failed_attempt_health_event(attempt: &FailedAttempt<'_>) -> crate::health_events::HealthEvent {
    use crate::health_events::{HealthEvent, HealthSource};
    let (outcome, error_kind) = classify_health(attempt.status, attempt.error);
    HealthEvent {
        ts: Utc::now(),
        target_id: attempt.target.to_string(),
        provider: attempt.provider.to_string(),
        org_id: String::new(),
        source: HealthSource::Passive,
        outcome,
        status_code: (attempt.status > 0).then_some(attempt.status),
        latency_ms: attempt.latency_ms,
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
    /// stop handle for the request-log writer; `None` when logging is disabled
    tasks: Option<Arc<crate::sink_drain::SinkTasks>>,
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
            tasks: None,
        }
    }

    /// Flush the batch and queue the request-log writer holds and stop it.
    /// Returns once it has exited; the caller bounds the wait. A no-op on a
    /// disabled sink.
    pub async fn shutdown(&self) {
        if let Some(tasks) = &self.tasks {
            tasks.stop().await;
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
                "{}/?query=INSERT%20INTO%20request_logs%20FORMAT%20JSONEachRow{INSERT_SETTINGS}",
                clickhouse_url.trim_end_matches('/')
            ),
            payload_url: format!(
                "{}/?query=INSERT%20INTO%20request_payloads%20FORMAT%20JSONEachRow{INSERT_SETTINGS}",
                clickhouse_url.trim_end_matches('/')
            ),
            client,
            batch_max: batch_max.max(1),
            flush,
            metrics: metrics.clone(),
        };
        let tasks = Arc::new(crate::sink_drain::SinkTasks::default());
        tasks.track(tokio::spawn(writer.run(rx, tasks.token())));
        Self {
            tx: Some(tx),
            health_events: crate::health_events::HealthEventSink::disabled(metrics.clone()),
            metrics,
            usage_buffers: UsageBufferPool::default(),
            usage_recorders: crate::usage_recording::UsageRecorderSink::default(),
            tasks: Some(tasks),
        }
    }

    /// Record one upstream attempt that failed and was superseded by another
    /// attempt (#1646).
    ///
    /// The request row for the whole exchange is written later and describes
    /// whichever attempt answered the caller, so this is the only place a
    /// recovered failure is attributed to the target that produced it. Cheap
    /// enough for the attempt loop: two relaxed atomics and a bounded-channel
    /// `try_send` that drops rather than blocks.
    pub fn record_failed_attempt(&self, attempt: &FailedAttempt<'_>) {
        if attempt.provider.is_empty() || attempt.target.is_empty() {
            return;
        }
        self.metrics.upstream_errors_total.fetch_add(1, Relaxed);
        self.metrics
            .observe_target(attempt.provider, attempt.target, false);
        self.health_events
            .emit(failed_attempt_health_event(attempt));
    }

    /// Enqueue a record whose attempt was already funnelled by
    /// [`LogSink::record_failed_attempt`], so only the request-level signals
    /// are taken from it. Used when the loop ran out of targets to fail over
    /// to: the row still describes the request, but counting its target again
    /// would double-count the one attempt it made.
    pub fn log_recorded_attempt(&self, record: RequestLog) {
        self.observe(&record, false);
        self.enqueue(record);
    }

    /// Enqueue the row for a request the gateway answered itself, with no
    /// upstream behind it: the built-in `fake-llm` model.
    ///
    /// The latency histograms and the ClickHouse row are written as for any
    /// request. What is skipped is the per-target outcome and the passive
    /// health event, because there is no target to attribute them to and a
    /// health series for a thing that cannot be unhealthy would only be noise.
    pub fn log_builtin(&self, record: RequestLog) {
        self.observe(&record, false);
        self.enqueue(record);
    }

    /// Enqueue the row for a request the gateway refused before it chose a
    /// target: a spent budget, a rate limit, a guardrail, an unknown model
    /// (#2807).
    ///
    /// Only the ClickHouse row is written. The latency histograms are keyed by
    /// the model the caller named, which on a refusal can be any string an
    /// authenticated caller cares to send, so observing it would let one key
    /// mint an unbounded number of metric series. The refusal is already
    /// counted by its own counter (`budget_blocks_total`,
    /// `rate_limit_blocks_total`, and so on), and there is no target to blame.
    pub fn log_refusal(&self, record: RequestLog) {
        self.enqueue(record);
    }

    /// Enqueue the row for a call on a stored response: retrieve, delete,
    /// cancel or list its input items (#2836).
    ///
    /// Only the ClickHouse row is written, for two reasons. The latency and
    /// time-to-first-token histograms describe generating a completion, and a
    /// `GET` that answers in a few milliseconds would drag every model's
    /// percentiles down. And the call is pinned to the provider that holds the
    /// response rather than picked by the balancer, so its outcome is not a
    /// signal about which target to prefer and does not feed the passive
    /// health series. Nothing here touches a budget or a rate-limit window:
    /// those are charged by the spend and token recorders, which a lifecycle
    /// call never builds.
    pub fn log_lifecycle(&self, record: RequestLog) {
        self.enqueue(record);
    }

    /// Enqueue a record without blocking. Drops (and counts) the record if the
    /// queue is full or the writer has stopped.
    pub fn log(&self, record: RequestLog) {
        // observe latency/ttft histograms + passive per-target outcome for every
        // completed request, even when clickhouse logging is disabled (metrics
        // are always present)
        self.observe(&record, true);
        self.enqueue(record);
    }

    /// The metrics half of [`LogSink::log`]. `attribute_target` is false when
    /// the attempt this row describes was already counted against its target,
    /// or when there is no target to count it against.
    fn observe(&self, record: &RequestLog, attribute_target: bool) {
        self.metrics.observe_request(
            &record.provider,
            &record.model,
            record.latency_ms,
            record.ttft_ms,
            record.completion_tokens,
        );
        self.metrics.observe_variant(&record.model, &record.variant);
        if !attribute_target {
            return;
        }
        self.metrics.observe_target(
            &record.provider,
            &record.target,
            (200..300).contains(&record.status),
        );
        // funnel a passive health event for every real upstream target (skip the
        // builtin fake-llm and any row without a provider/target)
        if !record.provider.is_empty() && !record.target.is_empty() {
            self.health_events.emit(passive_health_event(record));
        }
    }

    /// The ClickHouse half of [`LogSink::log`].
    fn enqueue(&self, mut record: RequestLog) {
        if !should_sample_request(&record.request_id, record.sample_rate) {
            return;
        }
        let Some(tx) = &self.tx else {
            return;
        };
        // recorded per row because the configured rate can change between
        // writes; the rate in force when the row was kept is the one that
        // applies to it
        record.sample_weight = sample_weight(record.sample_rate);
        // minted here, after sampling and only for a sink that writes, so a
        // dropped or disabled row costs no entropy
        record.log_id = Uuid::new_v4();
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
    async fn run(self, mut rx: mpsc::Receiver<RequestLog>, stop: CancellationToken) {
        let mut batch: Vec<RequestLog> = Vec::with_capacity(self.batch_max);
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
                tracing::warn!(timed_out = err.is_timeout(), err = %err.without_url(), "failed to write log batch to clickhouse");
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
                Err(err) => {
                    tracing::warn!(timed_out = err.is_timeout(), err = %err.without_url(), "failed to write payload batch to clickhouse")
                }
            }
        }
        batch.clear();
    }
}

/// One short-retention raw payload row keyed by the corresponding metadata log.
#[derive(Serialize)]
struct PayloadLog<'a> {
    /// the same instant as the metadata row, so a payload and the request it
    /// belongs to sit in the same partition and sort together. the control
    /// plane joins a body to a row written before `log_id` existed on
    /// `(request_id, ts)`, so this must stay the row's own `ts` through the
    /// same serializer (#1820)
    #[serde(serialize_with = "clickhouse_ts::serialize")]
    ts: DateTime<Utc>,
    request_id: &'a str,
    /// the log row's own key, which is what the control plane joins on (#1937)
    #[serde(skip_serializing_if = "Uuid::is_nil")]
    log_id: Uuid,
    /// the tenancy of the row, so the join can require it on both sides
    org_id: &'a str,
    project_id: &'a str,
    request_payload: &'a str,
    response_payload: &'a str,
}

impl<'a> From<&'a RequestLog> for PayloadLog<'a> {
    fn from(log: &'a RequestLog) -> Self {
        Self {
            ts: log.ts,
            request_id: &log.request_id,
            log_id: log.log_id,
            org_id: &log.org_id,
            project_id: &log.project_id,
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
    fn a_payload_row_carries_its_log_rows_exact_ts() {
        // the control plane binds a captured body to its log row on
        // (request_id, ts), since request_id is the caller's x-request-id and
        // two tenants can send the same one. a payload stamped any other way
        // would never join, and every body would read as "capture is off"
        let rec = RequestLog {
            ts: DateTime::from_timestamp_millis(1_757_030_542_061).expect("timestamp is in range"),
            request_id: "shared-id".to_string(),
            request_payload: "{\"prompt\":\"p\"}".to_string(),
            response_payload: "{\"answer\":\"a\"}".to_string(),
            ..Default::default()
        };
        let log: serde_json::Value = serde_json::to_value(&rec).unwrap();
        let payload: serde_json::Value = serde_json::to_value(PayloadLog::from(&rec)).unwrap();
        assert_eq!(payload["ts"], log["ts"]);
        assert_eq!(payload["request_id"], log["request_id"]);
    }

    #[test]
    fn a_payload_row_carries_its_log_rows_key_and_tenancy() {
        // the control plane joins a body to its row on log_id, the gateway's
        // own key, because request_id is whatever the caller sent (#1937)
        let rec = RequestLog {
            log_id: Uuid::new_v4(),
            request_id: "shared-id".to_string(),
            org_id: "org-1".to_string(),
            project_id: "project-1".to_string(),
            request_payload: "{}".to_string(),
            ..Default::default()
        };
        let log: serde_json::Value = serde_json::to_value(&rec).unwrap();
        let payload: serde_json::Value = serde_json::to_value(PayloadLog::from(&rec)).unwrap();
        assert_eq!(log["log_id"], rec.log_id.to_string());
        assert_eq!(payload["log_id"], log["log_id"]);
        assert_eq!(payload["org_id"], "org-1");
        assert_eq!(payload["project_id"], "project-1");
    }

    #[test]
    fn an_unkeyed_row_leaves_the_column_to_its_empty_default() {
        // a nil key written as "0000-..." would read as a real key and stop the
        // control plane's fallback to (request_id, ts)
        let rec = RequestLog::default();
        let log: serde_json::Value = serde_json::to_value(&rec).unwrap();
        let payload: serde_json::Value = serde_json::to_value(PayloadLog::from(&rec)).unwrap();
        assert!(log.get("log_id").is_none());
        assert!(payload.get("log_id").is_none());
    }

    #[tokio::test]
    async fn requests_sharing_a_caller_request_id_get_distinct_log_keys() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // one connection per insert, so request_logs and request_payloads
        // arrive as separate bodies
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut seen = Vec::new();
            while seen.len() < 2 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 16384];
                let n = sock.read(&mut buf).await.unwrap();
                seen.push(String::from_utf8_lossy(&buf[..n]).to_string());
                sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await
                    .unwrap();
            }
            seen
        });
        let sink = LogSink::spawn(
            format!("http://{addr}"),
            2,
            Duration::from_millis(50),
            100,
            Arc::new(Metrics::default()),
        );
        for project in ["p-1", "p-2"] {
            sink.log(RequestLog {
                request_id: "constant".to_string(),
                project_id: project.to_string(),
                request_payload: "{}".to_string(),
                ..Default::default()
            });
        }
        let seen = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        let rows = |marker: &str| -> Vec<serde_json::Value> {
            let request = seen.iter().find(|r| r.contains(marker)).unwrap();
            let body = request.split("\r\n\r\n").nth(1).unwrap();
            body.lines()
                .filter(|line| !line.is_empty())
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        };
        let logs = rows("INTO%20request_logs");
        let payloads = rows("INTO%20request_payloads");
        assert_eq!(logs.len(), 2);
        assert_ne!(logs[0]["log_id"], logs[1]["log_id"]);
        assert!(logs[0]["log_id"].as_str().is_some_and(|id| id.len() == 36));
        // each body is keyed to its own row, not to the shared request id
        for (log, payload) in logs.iter().zip(&payloads) {
            assert_eq!(log["log_id"], payload["log_id"]);
            assert_eq!(log["project_id"], payload["project_id"]);
        }
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
                reported: true,
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
                reported: true,
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
                reported: true,
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
                reported: true,
                ..Usage::default()
            }
        );
    }

    #[test]
    fn missing_usage_is_zero() {
        assert_eq!(parse_usage(false, b"{\"id\":\"x\"}"), Usage::default());
    }

    // ── responses api (#2819) ───────────────────────────────────────────────
    // a buffered answer carries `usage` at the top level; a streamed one only
    // on the terminal event, under `response`, so the two shapes are pinned
    // side by side and must yield the same numbers

    /// The streamed terminal event from the issue, behind the events that
    /// precede it: `response.created` and `response.in_progress` carry
    /// `usage: null`, which says nothing and must not be read as a report.
    #[test]
    fn parses_responses_api_sse_completed_usage() {
        let sse = b"event: response.created
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"status\":\"in_progress\",\"usage\":null}}\n\n\
event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"pong\"}\n\n\
event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"usage\":{\"input_tokens\":7,\"output_tokens\":3,\"total_tokens\":10}}}\n\n";
        assert_eq!(
            parse_usage(true, sse),
            Usage {
                prompt: 7,
                completion: 3,
                total: 10,
                reported: true,
                ..Usage::default()
            }
        );
    }

    #[test]
    fn parses_responses_api_non_stream_usage() {
        let body = br#"{"id":"resp_1","object":"response","status":"completed",
            "usage":{"input_tokens":7,"output_tokens":3,"total_tokens":10}}"#;
        assert_eq!(
            parse_usage(false, body),
            Usage {
                prompt: 7,
                completion: 3,
                total: 10,
                reported: true,
                ..Usage::default()
            }
        );
    }

    /// The same answer, buffered and streamed, is billed the same. The detail
    /// objects a real upstream adds are present in both.
    #[test]
    fn a_responses_api_answer_is_read_alike_buffered_and_streamed() {
        let usage = r#"{"input_tokens":120,"input_tokens_details":{"cached_tokens":0},"output_tokens":45,"output_tokens_details":{"reasoning_tokens":16},"total_tokens":165}"#;
        let body = format!(
            r#"{{"id":"resp_1","object":"response","status":"completed","usage":{usage}}}"#
        );
        let sse = format!(
            "event: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{body}}}\n\n"
        );
        let buffered = parse_usage(false, body.as_bytes());
        assert_eq!(buffered, parse_usage(true, sse.as_bytes()));
        assert_eq!(
            buffered,
            Usage {
                prompt: 120,
                completion: 45,
                total: 165,
                reported: true,
                ..Usage::default()
            }
        );
    }

    // ── cached input tokens (#2847) ─────────────────────────────────────────
    // the responses api reports the prompt-cache hit as
    // `usage.input_tokens_details.cached_tokens`, inside `input_tokens`, so a
    // priced model can bill it at `cached_input_per_mtok`

    #[test]
    fn a_buffered_responses_api_body_reports_its_cached_input() {
        let body = br#"{"id":"resp_1","object":"response","status":"completed",
            "usage":{"input_tokens":2006,"input_tokens_details":{"cached_tokens":1920},
            "output_tokens":45,"output_tokens_details":{"reasoning_tokens":16},
            "total_tokens":2051}}"#;
        assert_eq!(
            parse_usage(false, body),
            Usage {
                prompt: 2006,
                completion: 45,
                total: 2051,
                cache_read: 1920,
                reported: true,
                ..Usage::default()
            }
        );
    }

    #[test]
    fn a_streamed_responses_api_terminal_event_reports_its_cached_input() {
        let sse = b"event: response.created\n\
data: {\"type\":\"response.created\",\"response\":{\"status\":\"in_progress\",\"usage\":null}}\n\n\
event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":2006,\"input_tokens_details\":{\"cached_tokens\":1920},\"output_tokens\":45,\"output_tokens_details\":{\"reasoning_tokens\":16},\"total_tokens\":2051}}}\n\n";
        assert_eq!(
            parse_usage(true, sse),
            Usage {
                prompt: 2006,
                completion: 45,
                total: 2051,
                cache_read: 1920,
                reported: true,
                ..Usage::default()
            }
        );
    }

    /// the three spellings of one figure all land in `cache_read`, a
    /// reasoning count next to them is never mistaken for it, and they agree
    /// on the prompt total although Anthropic reports it without the cache
    #[test]
    fn every_dialect_reports_its_cached_input_in_cache_read() {
        for (name, usage) in [
            (
                "anthropic",
                r#"{"input_tokens":10,"cache_read_input_tokens":80,"output_tokens":5}"#,
            ),
            (
                "chat completions",
                r#"{"prompt_tokens":90,"prompt_tokens_details":{"cached_tokens":80},"completion_tokens":5}"#,
            ),
            (
                "responses",
                r#"{"input_tokens":90,"input_tokens_details":{"cached_tokens":80},"output_tokens":5,"output_tokens_details":{"reasoning_tokens":3}}"#,
            ),
        ] {
            let body = format!(r#"{{"usage":{usage}}}"#);
            let usage = parse_usage(false, body.as_bytes());
            assert_eq!(usage.cache_read, 80, "{name}");
            assert_eq!(usage.prompt, 90, "{name}");
        }
    }

    // ── cache hits spelled outside the OpenAI details blocks (#2877) ────────
    // providers behind a Chat Completions-compatible `ProviderKind` that cache
    // their prompts and name the hit somewhere else. each counts it inside the
    // prompt total, so the prompt is read as stated and only `cache_read` moves

    /// DeepSeek: `prompt_tokens` is documented as hit plus miss, and the miss
    /// count is the complement, so it is not read
    #[test]
    fn deepseeks_prompt_cache_hit_tokens_are_the_cache_read() {
        let body = br#"{"usage":{"prompt_tokens":90,"completion_tokens":5,"total_tokens":95,
            "prompt_cache_hit_tokens":80,"prompt_cache_miss_tokens":10}}"#;
        assert_eq!(
            parse_usage(false, body),
            Usage {
                prompt: 90,
                completion: 5,
                total: 95,
                cache_read: 80,
                reported: true,
                ..Usage::default()
            }
        );
    }

    /// Kimi: the hit sits at the top of `usage`, not in a details block
    #[test]
    fn a_top_level_cached_tokens_is_the_cache_read() {
        let body = br#"{"usage":{"prompt_tokens":90,"completion_tokens":5,"total_tokens":95,
            "cached_tokens":80}}"#;
        assert_eq!(
            parse_usage(false, body),
            Usage {
                prompt: 90,
                completion: 5,
                total: 95,
                cache_read: 80,
                reported: true,
                ..Usage::default()
            }
        );
    }

    /// GigaChat: `precached_prompt_tokens`
    #[test]
    fn gigachats_precached_prompt_tokens_are_the_cache_read() {
        let body = br#"{"usage":{"prompt_tokens":90,"completion_tokens":5,"total_tokens":95,
            "precached_prompt_tokens":80}}"#;
        let usage = parse_usage(false, body);
        assert_eq!(usage.cache_read, 80);
        assert_eq!(usage.prompt, 90);
    }

    /// A streamed answer reports the hit on the closing chunk like any other
    /// figure, whichever spelling it uses
    #[test]
    fn a_streamed_answer_reports_a_differently_spelled_hit() {
        let sse = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":90,\"completion_tokens\":5,\"prompt_cache_hit_tokens\":80,\"prompt_cache_miss_tokens\":10}}\n\n\
data: [DONE]\n\n";
        let usage = parse_usage(true, sse);
        assert_eq!(usage.cache_read, 80);
        assert_eq!(usage.prompt, 90);
    }

    /// A provider that spells the hit twice (DeepSeek now carries it in the
    /// details block as well) reports one figure, not the sum
    #[test]
    fn two_spellings_of_one_hit_are_not_added() {
        let body = br#"{"usage":{"prompt_tokens":90,"completion_tokens":5,
            "prompt_tokens_details":{"cached_tokens":80},"prompt_cache_hit_tokens":80}}"#;
        assert_eq!(parse_usage(false, body).cache_read, 80);
    }

    /// An absent figure, or a `null` one, leaves the row at no cache read
    #[test]
    fn an_unreported_hit_is_zero() {
        let body = br#"{"usage":{"prompt_tokens":90,"completion_tokens":5,"cached_tokens":null}}"#;
        assert_eq!(parse_usage(false, body).cache_read, 0);
    }

    /// priced: 10 fresh input and 80 cached at the cached rate, 5 output
    #[test]
    fn a_deepseek_hit_is_priced_at_the_cached_rate() {
        let price: rolter_core::ModelPriceConfig = serde_json::from_value(serde_json::json!({
            "model": "m", "input_per_mtok": 1_000_000, "output_per_mtok": 1_000_000,
            "cached_input_per_mtok": 100_000
        }))
        .unwrap();
        let body = br#"{"usage":{"prompt_tokens":90,"completion_tokens":5,
            "prompt_cache_hit_tokens":80,"prompt_cache_miss_tokens":10}}"#;
        let usage = parse_usage(false, body);
        let cost = price.cost(
            usage.prompt,
            usage.completion,
            usage.cache_read,
            usage.cache_write,
            usage.cache_write_1h,
        );
        assert_eq!(cost, rust_decimal::Decimal::from(23));
    }

    // ── the cache beside `input_tokens` (#2863) ─────────────────────────────
    // anthropic's `input_tokens` leaves the cache reads and writes out, while
    // `ModelPriceConfig::cost` takes the cached share to be part of the prompt
    // it is handed. an anthropic-shaped object is folded into that convention,
    // which is also how a Messages body translated from Chat Completions reads

    #[test]
    fn an_anthropic_body_counts_its_cache_inside_the_prompt() {
        let body = br#"{"id":"msg_1","usage":{"input_tokens":10,
            "cache_creation_input_tokens":30,"cache_read_input_tokens":80,"output_tokens":5}}"#;
        assert_eq!(
            parse_usage(false, body),
            Usage {
                prompt: 120,
                completion: 5,
                total: 125,
                cache_read: 80,
                cache_write: 30,
                reported: true,
                ..Usage::default()
            }
        );
    }

    /// the input side arrives on `message_start`, the output side on
    /// `message_delta`, and newer api versions repeat the cumulative figures
    /// there
    #[test]
    fn an_anthropic_stream_counts_its_cache_inside_the_prompt() {
        let start = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":30,\"cache_read_input_tokens\":80,\"output_tokens\":1}}}\n\n";
        let output_only = "event: message_delta\n\
data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}\n\n";
        let cumulative = "event: message_delta\n\
data: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":30,\"cache_read_input_tokens\":80,\"output_tokens\":5}}\n\n";
        for (name, delta) in [("output only", output_only), ("cumulative", cumulative)] {
            let usage = parse_usage(true, format!("{start}{delta}").as_bytes());
            assert_eq!(usage.prompt, 120, "{name}");
            assert_eq!(usage.completion, 5, "{name}");
            assert_eq!(usage.cache_read, 80, "{name}");
            assert_eq!(usage.cache_write, 30, "{name}");
        }
    }

    /// a Chat Completions object that also carries anthropic's field names
    /// (some gateways add them) already counts the cache in `prompt_tokens`.
    /// Databricks-hosted Claude is one: its reference returns
    /// `cache_read_input_tokens` and `cache_creation_input_tokens` "as a
    /// top-level usage field" beside `prompt_tokens` (#2882)
    #[test]
    fn a_prompt_total_that_includes_the_cache_is_not_added_to_again() {
        let body = br#"{"usage":{"prompt_tokens":90,"completion_tokens":5,
            "cache_read_input_tokens":80,"cache_creation_input_tokens":4}}"#;
        let usage = parse_usage(false, body);
        assert_eq!(usage.prompt, 90);
        assert_eq!(usage.cache_read, 80);
        assert_eq!(usage.cache_write, 4);
    }

    /// the cache write count has no OpenAI field; the translators (and
    /// OpenRouter) name it beside `cached_tokens`, inside the prompt
    #[test]
    fn a_cache_write_count_is_read_from_the_openai_details_blocks() {
        for details in ["prompt_tokens_details", "input_tokens_details"] {
            let key = if details == "prompt_tokens_details" {
                "prompt_tokens"
            } else {
                "input_tokens"
            };
            let body = format!(
                r#"{{"usage":{{"{key}":120,"{details}":{{"cached_tokens":80,"cache_write_tokens":30}}}}}}"#
            );
            let usage = parse_usage(false, body.as_bytes());
            assert_eq!(usage.prompt, 120, "{details}");
            assert_eq!(usage.cache_read, 80, "{details}");
            assert_eq!(usage.cache_write, 30, "{details}");
        }
    }

    /// qwen's explicit cache (#2879) and vllm's prefix cache name the write in
    /// the details block under their own names, and both count it inside the
    /// prompt, so the prompt total is left as it was
    #[test]
    fn a_cache_write_count_is_read_under_qwens_and_vllms_names() {
        for (name, field) in [
            ("qwen", "cache_creation_input_tokens"),
            ("vllm", "created_cache_tokens"),
        ] {
            let usage = format!(
                r#"{{"prompt_tokens":120,"completion_tokens":5,"total_tokens":125,"prompt_tokens_details":{{"cached_tokens":80,"{field}":30}}}}"#
            );
            let body = format!(r#"{{"usage":{usage}}}"#);
            let chunk = format!("data: {{\"choices\":[],\"usage\":{usage}}}\n\ndata: [DONE]\n\n");
            for (shape, parsed) in [
                ("buffered", parse_usage(false, body.as_bytes())),
                ("streamed", parse_usage(true, chunk.as_bytes())),
            ] {
                assert_eq!(parsed.prompt, 120, "{name} {shape}");
                assert_eq!(parsed.cache_read, 80, "{name} {shape}");
                assert_eq!(parsed.cache_write, 30, "{name} {shape}");
            }
        }
    }

    /// the usage block each provider's own reference describes (#2882), as the
    /// log reads it: the hit is inside the prompt total for every one of them
    #[test]
    fn the_documented_cache_hit_of_every_confirmed_provider_is_logged() {
        // (provider, usage, prompt, cache read, cache write)
        let documented = [
            // docs.mistral.ai prompt caching guide
            (
                "mistral",
                r#"{"prompt_tokens":1013,"total_tokens":1043,"completion_tokens":30,"prompt_tokens_details":{"cached_tokens":1008}}"#,
                1013,
                1008,
                0,
            ),
            // docs.together.ai: some models return it flat
            (
                "together",
                r#"{"prompt_tokens":3417,"completion_tokens":64,"total_tokens":3481,"cached_tokens":3327}"#,
                3417,
                3327,
                0,
            ),
            // platform.minimax.io prompt caching guide
            (
                "minimax",
                r#"{"prompt_tokens":1200,"completion_tokens":300,"total_tokens":1500,"prompt_tokens_details":{"cached_tokens":800}}"#,
                1200,
                800,
                0,
            ),
            // the Responses API form of Bedrock's OpenAI models
            (
                "bedrock",
                r#"{"input_tokens":2048,"output_tokens":256,"total_tokens":2304,"input_tokens_details":{"cached_tokens":1920,"cache_write_tokens":0}}"#,
                2048,
                1920,
                0,
            ),
            // docs.perplexity.ai: `prompt_tokens` includes "cache reads and writes"
            (
                "perplexity",
                r#"{"prompt_tokens":4096,"completion_tokens":20,"total_tokens":4116,"prompt_tokens_details":{"cached_tokens":3072,"cache_write_tokens":1024}}"#,
                4096,
                3072,
                1024,
            ),
            // gemini's openai-compatible endpoint, a response posted to the
            // google ai developers forum on 2025-05-13. 116,482 + 44 is not the
            // 117,488 total: the 962 between them are thinking (#2880)
            (
                "gemini (compatible)",
                r#"{"completion_tokens":44,"prompt_tokens":116482,"prompt_tokens_details":{"cached_tokens":114667},"total_tokens":117488}"#,
                116482,
                114667,
                0,
            ),
            // ai.developer.meta.com prompt caching guide
            (
                "meta llama api",
                r#"{"prompt_tokens":1847,"completion_tokens":98,"total_tokens":1945,"prompt_tokens_details":{"cached_tokens":1792}}"#,
                1847,
                1792,
                0,
            ),
        ];
        for (name, usage, prompt, read, write) in documented {
            let body = format!(r#"{{"usage":{usage}}}"#);
            let parsed = parse_usage(false, body.as_bytes());
            assert_eq!(parsed.prompt, prompt, "{name}");
            assert_eq!(parsed.cache_read, read, "{name}");
            assert_eq!(parsed.cache_write, write, "{name}");
        }
    }

    /// xai states its reasoning tokens beside `completion_tokens`, and only
    /// its total adds them up (#2888): 32 + 9 + 94 = 135, as its reference
    /// prints. They are billed as output, so the completion is 9 + 94
    #[test]
    fn reasoning_beside_the_completion_is_logged_as_completion() {
        let usage = r#"{"prompt_tokens":32,"completion_tokens":9,"total_tokens":135,"prompt_tokens_details":{"cached_tokens":6},"completion_tokens_details":{"reasoning_tokens":94}}"#;
        let body = format!(r#"{{"usage":{usage}}}"#);
        let chunk = format!("data: {{\"choices\":[],\"usage\":{usage}}}\n\ndata: [DONE]\n\n");
        for (shape, parsed) in [
            ("buffered", parse_usage(false, body.as_bytes())),
            ("streamed", parse_usage(true, chunk.as_bytes())),
        ] {
            assert_eq!(parsed.prompt, 32, "{shape}");
            assert_eq!(parsed.completion, 103, "{shape}");
            assert_eq!(parsed.total, 135, "{shape}");
            assert_eq!(parsed.cache_read, 6, "{shape}");
        }
    }

    /// openai's convention, which most providers follow: the total is prompt
    /// plus completion, so the reasoning is inside the completion already and
    /// must not be added again
    #[test]
    fn reasoning_inside_the_completion_is_logged_once() {
        let body = br#"{"usage":{"prompt_tokens":32,"completion_tokens":103,"total_tokens":135,
            "completion_tokens_details":{"reasoning_tokens":94}}}"#;
        let usage = parse_usage(false, body);
        assert_eq!(usage.completion, 103);
        assert_eq!(usage.total, 135);
        // the Responses API form
        let body = br#"{"usage":{"input_tokens":32,"output_tokens":103,"total_tokens":135,
            "output_tokens_details":{"reasoning_tokens":94}}}"#;
        let usage = parse_usage(false, body);
        assert_eq!(usage.completion, 103);
        assert_eq!(usage.total, 135);
    }

    /// gemini's openai-compatible endpoint states its thinking tokens in
    /// `total_tokens` alone (#2880). The bodies are responses people posted
    /// from it; the provider bills the thinking as output, so a row read with
    /// `Gemini`'s kind has to count it in the completion
    #[test]
    fn thinking_that_only_the_total_states_is_logged_as_completion_for_gemini() {
        use rolter_proxy::ThinkingCount;
        let gemini = ThinkingCount::of(rolter_core::ProviderKind::Gemini);
        // (usage, prompt, completion as stated, total)
        for (usage, prompt, stated, total) in [
            // forum, 2026-01-15, gemini-2.5-flash
            (
                r#"{"completion_tokens":18,"prompt_tokens":15,"total_tokens":175}"#,
                15,
                18,
                175,
            ),
            // forum, 2025-08-21, gemini-2.5-pro
            (
                r#"{"completion_tokens":102,"prompt_tokens":758,"total_tokens":1725}"#,
                758,
                102,
                1725,
            ),
        ] {
            let body = format!(r#"{{"usage":{usage}}}"#);
            let read = parse_usage_with(false, body.as_bytes(), gemini);
            assert_eq!(read.prompt, prompt, "{usage}");
            assert_eq!(read.completion, total - prompt, "{usage}");
            assert_eq!(read.total, total, "{usage}");
            // the same body from a provider read as stated keeps the completion
            // the provider gave, which is the short one
            let stated_read = parse_usage(false, body.as_bytes());
            assert_eq!(stated_read.completion, stated, "{usage}");
        }
    }

    /// the endpoint sends usage on every chunk of a stream, the thinking already
    /// spent in each total (forum, 2026-02-27, gemini-3-flash-preview)
    #[test]
    fn a_gemini_stream_with_usage_on_every_chunk_is_logged_with_its_thinking() {
        use rolter_proxy::ThinkingCount;
        let sse = concat!(
            r#"data: {"choices":[{"index":0,"delta":{"content":"1"}}],"usage":{"completion_tokens":1,"prompt_tokens":9,"total_tokens":103}}"#,
            "\n\n",
            r#"data: {"choices":[{"index":0,"delta":{"content":", "}}],"usage":{"completion_tokens":3,"prompt_tokens":9,"total_tokens":105}}"#,
            "\n\n",
            r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"length"}],"usage":{"completion_tokens":3,"prompt_tokens":9,"total_tokens":105}}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let read = parse_usage_with(
            true,
            sse.as_bytes(),
            ThinkingCount::of(rolter_core::ProviderKind::Gemini),
        );
        assert_eq!(read.prompt, 9);
        // 3 answer tokens and the 93 the total has beyond them and the prompt
        assert_eq!(read.completion, 96);
        assert_eq!(read.total, 105);
        assert_eq!(parse_usage(true, sse.as_bytes()).completion, 3);
    }

    /// whole bodies are left alone: a model that did not think, a total below
    /// its parts, and a body the endpoint is later fixed to state in full
    #[test]
    fn a_gemini_body_with_nothing_beyond_its_parts_is_logged_as_stated() {
        use rolter_proxy::ThinkingCount;
        let gemini = ThinkingCount::of(rolter_core::ProviderKind::Gemini);
        for usage in [
            r#"{"prompt_tokens":9,"completion_tokens":4,"total_tokens":13}"#,
            r#"{"prompt_tokens":15,"completion_tokens":160,"total_tokens":175,"completion_tokens_details":{"reasoning_tokens":142}}"#,
            r#"{"prompt_tokens":2000,"completion_tokens":3,"total_tokens":503}"#,
            // an embeddings answer
            r#"{"prompt_tokens":9,"total_tokens":9}"#,
        ] {
            let body = format!(r#"{{"usage":{usage}}}"#);
            assert_eq!(
                parse_usage_with(false, body.as_bytes(), gemini),
                parse_usage(false, body.as_bytes()),
                "{usage}"
            );
        }
    }

    /// vertex ai's endpoint names the thinking beside a completion that leaves
    /// it out, and the total adds the three up, which every provider is read
    /// for (#2888): two responses people posted from it
    #[test]
    fn vertex_ais_named_thinking_is_logged_as_completion() {
        for (usage, prompt, completion, total) in [
            // forum, 2026-01-15
            (
                r#"{"completion_tokens":21,"completion_tokens_details":{"reasoning_tokens":78},"extra_properties":{"google":{"traffic_type":"ON_DEMAND"}},"prompt_tokens":14,"total_tokens":113}"#,
                14,
                99,
                113,
            ),
            // taipanbox/tokenfuse#367, 2026-10-07, gemini-2.5-flash
            (
                r#"{"completion_tokens":59,"completion_tokens_details":{"reasoning_tokens":560},"prompt_tokens":14,"total_tokens":633}"#,
                14,
                619,
                633,
            ),
        ] {
            let body = format!(r#"{{"usage":{usage}}}"#);
            for thinking in [
                rolter_proxy::ThinkingCount::of(rolter_core::ProviderKind::Vertex),
                rolter_proxy::ThinkingCount::Stated,
            ] {
                let read = parse_usage_with(false, body.as_bytes(), thinking);
                assert_eq!(read.prompt, prompt, "{usage}");
                assert_eq!(read.completion, completion, "{usage}");
                assert_eq!(read.total, total, "{usage}");
            }
        }
    }

    /// priced: 10 fresh input and 30 written ones at one, 80 cached at the
    /// cached rate, 5 output at one
    #[test]
    fn an_anthropic_bodys_cache_is_priced_at_the_cached_rate() {
        let price: rolter_core::ModelPriceConfig = serde_json::from_value(serde_json::json!({
            "model": "m", "input_per_mtok": 1_000_000, "output_per_mtok": 1_000_000,
            "cached_input_per_mtok": 100_000
        }))
        .unwrap();
        let body = br#"{"usage":{"input_tokens":10,"cache_creation_input_tokens":30,
            "cache_read_input_tokens":80,"output_tokens":5}}"#;
        let usage = parse_usage(false, body);
        let cost = price.cost(
            usage.prompt,
            usage.completion,
            usage.cache_read,
            usage.cache_write,
            usage.cache_write_1h,
        );
        assert_eq!(cost, rust_decimal::Decimal::from(53));
    }

    // ── the 5 minute / 1 hour split of cache writes (#2891) ─────────────────
    // anthropic reports a write to each cache in `usage.cache_creation`, beside
    // the combined `cache_creation_input_tokens`, and bills them at different
    // rates. `cache_write` stays the total; `cache_write_1h` is a share of it

    /// Anthropic's own example from the prompt-caching guide: 148 tokens
    /// written to the 5 minute cache and 100 to the 1 hour one
    const SPLIT_BODY: &[u8] = br#"{"id":"msg_1","usage":{"input_tokens":2048,
        "cache_read_input_tokens":1800,"cache_creation_input_tokens":248,
        "cache_creation":{"ephemeral_5m_input_tokens":148,"ephemeral_1h_input_tokens":100},
        "output_tokens":503}}"#;

    #[test]
    fn an_anthropic_split_is_read_beside_the_combined_write() {
        assert_eq!(
            parse_usage(false, SPLIT_BODY),
            Usage {
                // 2048 uncached + 1800 read + 248 written
                prompt: 4096,
                completion: 503,
                total: 4599,
                cache_read: 1800,
                // the total, not the 5 minute share and not the sum of both
                cache_write: 248,
                cache_write_1h: 100,
                reported: true,
            }
        );
    }

    /// the split is on `message_start`, and `message_delta` repeats the
    /// totals without it: the split must survive the closing event
    #[test]
    fn an_anthropic_stream_keeps_the_split_from_message_start() {
        let start = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2048,\"cache_creation_input_tokens\":248,\"cache_read_input_tokens\":1800,\"cache_creation\":{\"ephemeral_5m_input_tokens\":148,\"ephemeral_1h_input_tokens\":100},\"output_tokens\":1}}}\n\n";
        let output_only = "event: message_delta\n\
data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":503}}\n\n";
        let cumulative = "event: message_delta\n\
data: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":2048,\"cache_creation_input_tokens\":248,\"cache_read_input_tokens\":1800,\"output_tokens\":503}}\n\n";
        for (name, delta) in [("output only", output_only), ("cumulative", cumulative)] {
            let usage = parse_usage(true, format!("{start}{delta}").as_bytes());
            assert_eq!(usage.prompt, 4096, "{name}");
            assert_eq!(usage.cache_write, 248, "{name}");
            assert_eq!(usage.cache_write_1h, 100, "{name}");
        }
    }

    /// a body that states the breakdown and no total still has its writes
    /// counted, in the prompt as well
    #[test]
    fn a_breakdown_without_a_total_is_counted() {
        let body = br#"{"usage":{"input_tokens":10,"output_tokens":5,
            "cache_creation":{"ephemeral_5m_input_tokens":3,"ephemeral_1h_input_tokens":4}}}"#;
        let usage = parse_usage(false, body);
        assert_eq!(usage.cache_write, 7);
        assert_eq!(usage.cache_write_1h, 4);
        assert_eq!(usage.prompt, 17);
    }

    /// no breakdown, `null` or not an object: nothing is split, so every write
    /// is priced at the plain write rate as it was before the split existed
    #[test]
    fn a_write_the_provider_did_not_split_has_no_one_hour_share() {
        for body in [
            &br#"{"usage":{"input_tokens":10,"cache_creation_input_tokens":30,"output_tokens":5}}"#[..],
            br#"{"usage":{"input_tokens":10,"cache_creation_input_tokens":30,"cache_creation":null,"output_tokens":5}}"#,
            br#"{"usage":{"input_tokens":10,"cache_creation_input_tokens":30,"cache_creation":{},"output_tokens":5}}"#,
            br#"{"usage":{"prompt_tokens":40,"completion_tokens":5,"prompt_tokens_details":{"cache_write_tokens":30}}}"#,
        ] {
            let usage = parse_usage(false, body);
            assert_eq!(usage.cache_write, 30, "{}", String::from_utf8_lossy(body));
            assert_eq!(usage.cache_write_1h, 0, "{}", String::from_utf8_lossy(body));
        }
    }

    /// the translators name the share `cache_write_1h_tokens` in the details
    /// block of both OpenAI shapes, so a request served across dialects (a
    /// Messages upstream behind a chat client) keeps its split
    #[test]
    fn a_translated_body_carries_its_split_in_the_details_block() {
        let chat = br#"{"usage":{"prompt_tokens":4096,"completion_tokens":503,"total_tokens":4599,
            "prompt_tokens_details":{"cached_tokens":1800,"cache_write_tokens":248,
            "cache_write_1h_tokens":100}}}"#;
        let responses = br#"{"usage":{"input_tokens":4096,"output_tokens":503,"total_tokens":4599,
            "input_tokens_details":{"cached_tokens":1800,"cache_write_tokens":248,
            "cache_write_1h_tokens":100}}}"#;
        let expected = Usage {
            prompt: 4096,
            completion: 503,
            total: 4599,
            cache_read: 1800,
            cache_write: 248,
            cache_write_1h: 100,
            reported: true,
        };
        assert_eq!(parse_usage(false, chat), expected);
        assert_eq!(parse_usage(false, responses), expected);
    }

    /// priced at the rates in Anthropic's multiples of a 3.00 input rate:
    /// 2048 fresh at 3, 1800 read at 0.30, 148 written at 3.75, 100 written
    /// to the 1 hour cache at 6.00, 503 out at 15
    #[test]
    fn each_cache_is_priced_at_its_own_rate() {
        let price: rolter_core::ModelPriceConfig = serde_json::from_value(serde_json::json!({
            "model": "claude", "input_per_mtok": 3.0, "output_per_mtok": 15.0,
            "cached_input_per_mtok": 0.3, "cache_write_per_mtok": 3.75,
            "cache_write_1h_per_mtok": 6.0
        }))
        .unwrap();
        let usage = parse_usage(false, SPLIT_BODY);
        let cost = price.cost(
            usage.prompt,
            usage.completion,
            usage.cache_read,
            usage.cache_write,
            usage.cache_write_1h,
        );
        // 6144 + 540 + 555 + 600 + 7545 = 15384 / 1e6
        assert_eq!(cost, rust_decimal::Decimal::new(15384, 6));
        // a row with no 1 hour rate prices the same 100 tokens at 3.75
        let mut no_long = price.clone();
        no_long.cache_write_1h_per_mtok = None;
        let cost = no_long.cost(
            usage.prompt,
            usage.completion,
            usage.cache_read,
            usage.cache_write,
            usage.cache_write_1h,
        );
        // 6144 + 540 + 248 * 3.75 (930) + 7545 = 15159 / 1e6
        assert_eq!(cost, rust_decimal::Decimal::new(15159, 6));
    }

    /// `cache_write_tokens` stays the total and the new column only appears
    /// on a row that has a share to record, so every other row is what it was
    /// before the column existed
    #[test]
    fn the_one_hour_column_is_serialized_only_when_there_is_a_share() {
        let plain = serde_json::to_value(RequestLog::default()).unwrap();
        assert!(plain.get("cache_write_1h_tokens").is_none(), "{plain}");
        assert_eq!(plain["cache_write_tokens"], 0);

        let split = serde_json::to_value(RequestLog {
            cache_write_tokens: 248,
            cache_write_1h_tokens: 100,
            ..RequestLog::default()
        })
        .unwrap();
        assert_eq!(split["cache_write_tokens"], 248);
        assert_eq!(split["cache_write_1h_tokens"], 100);
    }

    /// An answer cut short by `max_output_tokens` still spent its tokens, and
    /// the provider reports them on `response.incomplete`.
    #[test]
    fn parses_responses_api_sse_incomplete_usage() {
        let sse = b"event: response.incomplete\n\
data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"usage\":{\"input_tokens\":9,\"output_tokens\":16,\"total_tokens\":25}}}\n\n";
        assert_eq!(
            parse_usage(true, sse),
            Usage {
                prompt: 9,
                completion: 16,
                total: 25,
                reported: true,
                ..Usage::default()
            }
        );
    }

    /// `response.failed` carries whatever usage the provider settled on. When
    /// there is none the row says so (`usage_unknown`) instead of claiming a
    /// known zero, exactly as for any other body without a usage object.
    #[test]
    fn a_failed_responses_api_stream_is_reported_only_if_it_carries_usage() {
        let with_usage = b"data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"usage\":{\"input_tokens\":4,\"output_tokens\":0,\"total_tokens\":4}}}\n\n";
        assert_eq!(
            parse_usage(true, with_usage),
            Usage {
                prompt: 4,
                total: 4,
                reported: true,
                ..Usage::default()
            }
        );
        let without = b"data: {\"type\":\"response.created\",\"response\":{\"usage\":null}}\n\n\
data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"usage\":null}}\n\n";
        assert_eq!(parse_usage(true, without), Usage::default());
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

    /// #2807: a caller handed the upstream's own status is judged by it. The
    /// words in `error` are then the upstream's, and a 429 whose message says
    /// "timed out" is a rate limit, not a timeout.
    #[test]
    fn the_upstreams_own_words_do_not_decide_its_health_verdict() {
        use crate::health_events::HealthOutcome;

        let upstream_said = RequestLog {
            status: 429,
            upstream_status: 429,
            error: "upstream returned 429: timed out waiting for a slot".to_string(),
            provider: "p".to_string(),
            target: "t".to_string(),
            ..Default::default()
        };
        let event = passive_health_event(&upstream_said);
        assert_eq!(event.outcome, HealthOutcome::Error);
        assert_eq!(event.error_kind.as_deref(), Some("rate_limited"));

        // a status the gateway made up from an upstream's 200 still reads its
        // own error text: the body could not be read, and it timed out
        let gateway_said = RequestLog {
            status: 502,
            upstream_status: 200,
            error: "upstream response body could not be read: operation timed out".to_string(),
            ..upstream_said
        };
        let event = passive_health_event(&gateway_said);
        assert_eq!(event.outcome, HealthOutcome::Timeout);
    }

    /// #2807: a refusal is written to ClickHouse and nowhere else. The latency
    /// histograms are keyed by the model the caller named, and a refusal is
    /// written before any route vouches for it.
    #[test]
    fn a_refusal_row_touches_no_metric_series() {
        let metrics = Arc::new(Metrics::default());
        let sink = LogSink::disabled(metrics.clone());
        sink.log_refusal(RequestLog {
            model: "a-model-nobody-configured".to_string(),
            status: 404,
            ..Default::default()
        });
        let rendered = metrics.render();
        assert!(
            !rendered.contains("a-model-nobody-configured"),
            "a refusal minted a series: {rendered}"
        );
    }

    /// #1646: a superseded attempt lands in `provider_health_events` as a
    /// passive observation of the target that produced it, classified the same
    /// way the request-level event would classify it.
    #[test]
    fn a_failed_attempt_becomes_a_passive_health_event_for_its_own_target() {
        use crate::health_events::{HealthOutcome, HealthSource};

        let e = failed_attempt_health_event(&FailedAttempt {
            provider: "vllm-spot-02",
            target: "llama-3.1-8b",
            status: 503,
            latency_ms: 41,
            error: "",
        });
        // the sick target, not the peer that rescued the request
        assert_eq!(e.provider, "vllm-spot-02");
        assert_eq!(e.target_id, "llama-3.1-8b");
        assert_eq!(e.source, HealthSource::Passive);
        assert_eq!(e.outcome, HealthOutcome::Error);
        assert_eq!(e.status_code, Some(503));
        assert_eq!(e.error_kind.as_deref(), Some("upstream_error"));
        assert_eq!(e.latency_ms, 41);

        // a connection-level attempt never got a status
        let dead = failed_attempt_health_event(&FailedAttempt {
            provider: "vllm-spot-02",
            target: "llama-3.1-8b",
            status: 0,
            latency_ms: 3,
            error: "connection refused",
        });
        assert_eq!(dead.status_code, None);
        assert_eq!(dead.error_kind.as_deref(), Some("connect_error"));

        // and a timeout is a timeout, not a generic error
        let slow = failed_attempt_health_event(&FailedAttempt {
            provider: "vllm-spot-02",
            target: "llama-3.1-8b",
            status: 0,
            latency_ms: 30_000,
            error: "upstream request timed out after 30s",
        });
        assert_eq!(slow.outcome, HealthOutcome::Timeout);
        assert_eq!(slow.error_kind.as_deref(), Some("timeout"));
    }

    /// An attempt with no provider or target is the builtin `fake-llm` or a
    /// route that never reached an upstream: nothing to attribute it to.
    #[test]
    fn an_attempt_without_a_target_is_not_recorded() {
        let metrics = Arc::new(Metrics::default());
        let sink = LogSink::disabled(metrics.clone());
        sink.record_failed_attempt(&FailedAttempt {
            provider: "",
            target: "",
            status: 503,
            latency_ms: 1,
            error: "",
        });
        assert_eq!(metrics.upstream_errors_total.load(Relaxed), 0);
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

    #[test]
    fn a_kept_row_weighs_the_inverse_of_its_sample_rate() {
        assert_eq!(sample_weight(0.5), 2.0);
        assert_eq!(sample_weight(0.25), 4.0);
        // unsampled and out-of-range rates count each row once
        assert_eq!(sample_weight(1.0), 1.0);
        assert_eq!(sample_weight(2.0), 1.0);
        assert_eq!(sample_weight(0.0), 1.0);
    }

    #[tokio::test]
    async fn a_sampled_row_is_written_with_its_weight() {
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
        let sink = LogSink::spawn(
            format!("http://{addr}"),
            10,
            Duration::from_millis(50),
            100,
            Arc::new(Metrics::default()),
        );
        // a request id the 50 % hash keeps
        let kept = (0..200)
            .map(|i| format!("req-{i}"))
            .find(|id| should_sample_request(id, 0.5))
            .expect("some id is kept at 50 %");
        sink.log(RequestLog {
            request_id: kept,
            sample_rate: 0.5,
            ..Default::default()
        });
        let req = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        assert!(req.contains("\"sample_weight\":2.0"), "{req}");
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
        assert!(req.contains("input_format_skip_unknown_fields=1"));

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
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
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

    /// The shutdown close lands on the writer's first turn after the stop, so a
    /// row offered while it is still flushing the backlog is refused and counted
    /// dropped, not written (#2618). The stand-in answers slowly with one row per
    /// batch, so the backlog outlives the stop. Probabilistic by nature: without
    /// `biased;` the writer keeps taking queued rows from the open queue with
    /// even odds on each turn, so the close assertion fails most runs.
    #[tokio::test]
    async fn a_send_racing_the_shutdown_close_is_counted_as_dropped() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    if sock.read(&mut buf).await.unwrap_or(0) == 0 {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    let _ = sock
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                });
            }
        });

        // each round is a coin flip without `biased;`, so repeat it
        for _ in 0..6 {
            let metrics = Arc::new(Metrics::default());
            // one row per batch, so every queued row is its own slow flush
            let sink = LogSink::spawn(
                format!("http://{addr}"),
                1,
                Duration::from_secs(3600),
                16,
                metrics.clone(),
            );
            for _ in 0..4 {
                sink.log(RequestLog::default());
            }
            // let the writer take the first row, so the stop lands mid-flush
            let tx = sink.tx.clone().expect("a spawned sink has a channel");
            for _ in 0..1_000 {
                if tx.capacity() == 13 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            assert_eq!(tx.capacity(), 13, "the writer never took a row");

            let draining = tokio::spawn({
                let sink = sink.clone();
                async move { sink.shutdown().await }
            });
            for _ in 0..2_000 {
                if tx.is_closed() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            assert!(tx.is_closed(), "the drain never closed the queue");
            // one row was in flight when the stop fired; the close must come on the
            // very next turn, not once the backlog ran dry
            assert_eq!(
                metrics.logs_written_total.load(Relaxed),
                1,
                "the queue stayed open while the backlog was flushed"
            );

            sink.log(RequestLog::default());
            assert_eq!(metrics.logs_dropped_total.load(Relaxed), 1);

            tokio::time::timeout(Duration::from_secs(10), draining)
                .await
                .expect("the drain finished")
                .expect("the drain task did not panic");
            assert_eq!(
                metrics.logs_written_total.load(Relaxed),
                4,
                "only the rows queued before the close are written"
            );
        }
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

    /// A body with no usage object parses to zeros, and says so: the zeros are
    /// unknown, not a free request (#1478).
    #[test]
    fn a_body_without_usage_is_not_reported() {
        let silent = parse_usage(false, br#"{"choices":[]}"#);
        assert_eq!(silent, Usage::default());
        assert!(!silent.reported);
        let null = parse_usage(false, br#"{"usage":null}"#);
        assert!(!null.reported);
        let zero = parse_usage(
            false,
            br#"{"usage":{"prompt_tokens":0,"completion_tokens":0}}"#,
        );
        assert!(zero.reported, "an explicit zero is a known zero");
        let streamed = parse_usage(true, b"data: {\"choices\":[]}\n\ndata: [DONE]\n\n");
        assert!(!streamed.reported);
    }

    /// Accept one ClickHouse insert on a raw socket and return the request text.
    async fn capture_one_insert() -> (std::net::SocketAddr, tokio::task::JoinHandle<String>) {
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
        (addr, server)
    }

    /// A withheld response bills what the upstream reported, not what the
    /// delivered bytes say, is marked `withheld`, and never captures the
    /// rejected body (#1478).
    #[tokio::test]
    async fn a_withheld_response_keeps_its_billed_usage_and_drops_its_body() {
        let (addr, server) = capture_one_insert().await;
        let metrics = Arc::new(Metrics::default());
        let sink = LogSink::spawn(
            format!("http://{addr}"),
            1,
            Duration::from_millis(20),
            100,
            metrics.clone(),
        );
        // a dollar a token, so the cost is the token count
        let price: rolter_core::ModelPriceConfig = serde_json::from_value(serde_json::json!({
            "model": "gpt-4o",
            "input_per_mtok": 1_000_000,
            "output_per_mtok": 1_000_000,
        }))
        .unwrap();
        let billed = parse_usage(
            false,
            br#"{"usage":{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}}"#,
        );
        UsageLoggingStream::new(
            Box::pin(futures_util::stream::empty()),
            false,
            Instant::now(),
            sink,
            Some(price),
            RequestLog {
                request_id: "req-withheld".to_string(),
                model: "gpt-4o".to_string(),
                status: 200,
                capture_payloads: true,
                payload_max_bytes: 4096,
                ..Default::default()
            },
            None,
            None,
            None,
        )
        .with_billed_usage(billed)
        .withhold(403, "guardrail_blocked: email".to_string());

        let req = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        assert!(req.contains("\"request_id\":\"req-withheld\""), "{req}");
        assert!(req.contains("\"status\":403"), "{req}");
        assert!(req.contains("\"withheld\":1"), "{req}");
        assert!(req.contains("\"usage_unknown\":0"), "{req}");
        assert!(req.contains("\"total_tokens\":7"), "{req}");
        assert!(req.contains("\"cost_usd\":7.0"), "{req}");
        assert!(req.contains("guardrail_blocked: email"), "{req}");
        assert_eq!(metrics.withheld_responses_total.load(Relaxed), 1);
        // a refusal is not a hang-up
        assert_eq!(metrics.client_disconnects_total.load(Relaxed), 0);
    }

    /// A billed-usage override wins over whatever the delivered body says, so
    /// a policy that rewrites the usage object cannot change the bill (#1478).
    #[tokio::test]
    async fn billed_usage_overrides_the_delivered_body() {
        use futures_util::StreamExt;
        let (addr, server) = capture_one_insert().await;
        let metrics = Arc::new(Metrics::default());
        let sink = LogSink::spawn(
            format!("http://{addr}"),
            1,
            Duration::from_millis(20),
            100,
            metrics,
        );
        let delivered = Bytes::from_static(br#"{"usage":{"total_tokens":0}}"#);
        let mut wrapped = UsageLoggingStream::new(
            Box::pin(futures_util::stream::iter(vec![
                Ok::<Bytes, reqwest::Error>(delivered),
            ])),
            false,
            Instant::now(),
            sink,
            None,
            RequestLog {
                request_id: "req-rewritten".to_string(),
                status: 200,
                ..Default::default()
            },
            None,
            None,
            None,
        )
        .with_billed_usage(parse_usage(
            false,
            br#"{"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
        ));
        while wrapped.next().await.is_some() {}
        drop(wrapped);
        let req = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        assert!(req.contains("\"total_tokens\":2"), "{req}");
        assert!(req.contains("\"withheld\":0"), "{req}");
        // no price row: unpriced, which is a different fact from unknown usage
        assert!(req.contains("\"unpriced\":1"), "{req}");
        assert!(req.contains("\"usage_unknown\":0"), "{req}");
    }

    /// A successful answer with no usage object is logged as unknown usage.
    #[tokio::test]
    async fn a_successful_answer_without_usage_is_logged_as_unknown() {
        use futures_util::StreamExt;
        let (addr, server) = capture_one_insert().await;
        let metrics = Arc::new(Metrics::default());
        let sink = LogSink::spawn(
            format!("http://{addr}"),
            1,
            Duration::from_millis(20),
            100,
            metrics,
        );
        let mut wrapped = UsageLoggingStream::new(
            Box::pin(futures_util::stream::iter(vec![
                Ok::<Bytes, reqwest::Error>(Bytes::from_static(br#"{"choices":[]}"#)),
            ])),
            false,
            Instant::now(),
            sink,
            None,
            RequestLog {
                request_id: "req-silent".to_string(),
                status: 200,
                ..Default::default()
            },
            None,
            None,
            None,
        );
        while wrapped.next().await.is_some() {}
        drop(wrapped);
        let req = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap();
        assert!(req.contains("\"usage_unknown\":1"), "{req}");
        assert!(req.contains("\"total_tokens\":0"), "{req}");
    }

    /// Drain `body` through the accounting stream for a row that carries
    /// `upstream_status`, and return the insert ClickHouse received (#2807).
    async fn insert_for_upstream_answer(
        id: &str,
        status: u16,
        upstream_status: u16,
        body: &'static [u8],
    ) -> String {
        use futures_util::StreamExt;
        let (addr, server) = capture_one_insert().await;
        let sink = LogSink::spawn(
            format!("http://{addr}"),
            1,
            Duration::from_millis(20),
            100,
            Arc::new(Metrics::default()),
        );
        let mut wrapped = UsageLoggingStream::new(
            Box::pin(futures_util::stream::iter(vec![
                Ok::<Bytes, reqwest::Error>(Bytes::from_static(body)),
            ])),
            false,
            Instant::now(),
            sink,
            None,
            RequestLog {
                request_id: id.to_string(),
                status,
                upstream_status,
                attempts: 1,
                ..Default::default()
            },
            None,
            None,
            None,
        );
        while wrapped.next().await.is_some() {}
        drop(wrapped);
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server timed out")
            .unwrap()
    }

    /// An upstream error the caller received as it was says why in the row, from
    /// the body the stream already buffered (#2807).
    #[tokio::test]
    async fn an_upstream_error_handed_back_as_it_was_records_its_reason() {
        let req = insert_for_upstream_answer(
            "req-429",
            429,
            429,
            br#"{"error":{"message":"slow down"}}"#,
        )
        .await;
        assert!(
            req.contains("\"error\":\"upstream returned 429: slow down\""),
            "{req}"
        );
        assert!(req.contains("\"upstream_status\":429"), "{req}");
        assert!(req.contains("\"attempts\":1"), "{req}");
    }

    /// A success has nothing to explain, whatever its body says.
    #[tokio::test]
    async fn a_successful_answer_records_no_error() {
        let req = insert_for_upstream_answer(
            "req-200",
            200,
            200,
            br#"{"error":{"message":"not an error"}}"#,
        )
        .await;
        assert!(req.contains("\"error\":\"\""), "{req}");
    }
}
