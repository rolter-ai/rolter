//! Metering for `/v1/realtime` sessions (#1396).
//!
//! An HTTP request is metered once, when its response ends. A realtime session
//! has no such moment. It can stay open for an hour, run hundreds of model
//! responses, and end because the network dropped rather than because anyone
//! closed it. Metering on close would leave a session that never closes
//! unaccounted, and metering by wall-clock time would bill something no
//! provider charges for.
//!
//! So a session is metered in the unit the upstream bills and reports: the
//! **response turn**. Every `response.done` server event carries the `usage` of
//! the response it ends, and that is what gets priced.
//!
//! - The relay loop hands each upstream text frame to a [`TurnTracker`]. Almost
//!   every frame is an audio or text delta; the tracker reads its `type` from
//!   the first bytes and never parses it. `response.created` opens a turn and
//!   `response.done` closes it with its usage.
//! - A closed [`Turn`] goes to the session's meter task over a bounded channel,
//!   so the relay never waits on Redis or ClickHouse.
//! - Every `[realtime] usage_flush_secs`, and once more when the session ends,
//!   the meter flushes: one `request_logs` row per turn, the window's cost
//!   added to every applicable budget and its tokens to every applicable `tpm`
//!   window. It then re-reads the budgets, and a spent budget closes the
//!   session, whoever spent it. With `usage_flush_secs = 0` every turn flushes
//!   as it finishes, and the budgets are still re-read every second while the
//!   session is quiet.
//! - Every tick also looks the session's virtual key up again, so a key
//!   disabled, expired or deleted, or a narrowed `models` list, closes the
//!   session within one interval (#1881). A key moved to another team or
//!   project re-scopes the session for its later turns; a key moved to another
//!   organization, or into a scope with different content plugins, closes it
//!   (#2384).
//! - A gateway shutting down closes every session and waits for its meter's
//!   last flush, since axum's own drain does not see an upgraded socket.
//! - A response still in flight when the session ends never reports usage. Its
//!   row carries `usage_unknown = 1`, so the spend shows up as unknown rather
//!   than disappearing.
//!
//! `docs/dev-docs/architecture/realtime-metering.md` records why each of these
//! was chosen over the alternatives.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rolter_core::ModelPriceConfig;
use rolter_core::PluginStage;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

use crate::budgets::ScopeIds;
use crate::handlers::AccessRevoked;
use crate::logging::{RequestLog, Usage, CLIENT_DISCONNECT_ERROR, CLIENT_DISCONNECT_STATUS};
use crate::metrics::Metrics;
use crate::state::{AppState, KeyMeta, Snapshot};

/// Finished turns that may wait for the meter before the relay drops them.
///
/// The meter drains continuously and each flush is bounded by the Redis
/// timeouts, so a session would need to finish a thousand responses inside one
/// stalled flush to reach this.
const QUEUE_CAPACITY: usize = 1024;

/// Responses one session may have open at once before the oldest stops being
/// timed. Its turn is still metered when it ends; it only loses its latency.
const MAX_OPEN: usize = 32;

/// How far into a `type` value the tracker reads before giving up on it.
const TYPE_MAX_LEN: usize = 128;

/// How often a session that flushes per turn re-reads its budgets while it is
/// quiet. Such a session has no flush timer, and without this a budget spent
/// by other traffic would only be noticed after its own next billed turn.
const IDLE_BUDGET_CHECK: Duration = Duration::from_secs(1);

/// One finished response and what the upstream reported it used.
#[derive(Debug)]
pub(crate) struct Turn {
    started: Instant,
    latency_ms: u32,
    ttft_ms: u32,
    usage: Usage,
    /// the upstream ended the response with `status: "failed"`
    failed: bool,
}

/// A response the upstream has started and not yet finished.
#[derive(Debug)]
pub(crate) struct OpenTurn {
    id: String,
    started: Instant,
    first_delta: Option<Instant>,
}

impl OpenTurn {
    fn timings(&self, now: Instant) -> (u32, u32) {
        let latency = millis(now.saturating_duration_since(self.started));
        let ttft = self.first_delta.map_or(latency, |at| {
            millis(at.saturating_duration_since(self.started))
        });
        (latency, ttft)
    }
}

fn millis(elapsed: Duration) -> u32 {
    u32::try_from(elapsed.as_millis()).unwrap_or(u32::MAX)
}

/// Follows the response lifecycle in the upstream's server events.
///
/// Owned by the relay loop, so it needs no lock. It sees every upstream text
/// frame, and nearly all of them are deltas carrying base64 audio, so the
/// common path reads a few bytes and returns.
#[derive(Debug, Default)]
pub(crate) struct TurnTracker {
    open: Vec<OpenTurn>,
    /// set while some open response has not produced its first delta, so the
    /// search for one stops as soon as it is found
    awaiting_delta: bool,
}

impl TurnTracker {
    /// Observe one upstream text frame. Returns the turn a `response.done`
    /// finished, if this frame was one.
    pub(crate) fn observe(&mut self, frame: &str) -> Option<Turn> {
        self.observe_at(frame, Instant::now())
    }

    fn observe_at(&mut self, frame: &str, now: Instant) -> Option<Turn> {
        match leading_type(frame) {
            // only a response's own output is its first token. an input
            // transcription delta describes what the user said, and can land
            // well before the model answers
            Some(kind) if kind.ends_with(".delta") => {
                if kind.starts_with("response.") {
                    self.first_delta(frame, now);
                }
                None
            }
            Some("response.created" | "response.done") => self.lifecycle(frame, now),
            Some(_) => None,
            // `type` was not among the leading keys. a frame shaped that way is
            // still metered correctly, it just costs a scan of the whole frame
            None => {
                if self.awaiting_delta
                    && frame.contains(".delta\"")
                    && frame.contains("\"response.")
                {
                    self.first_delta(frame, now);
                }
                if frame.contains("\"response.done\"") || frame.contains("\"response.created\"") {
                    self.lifecycle(frame, now)
                } else {
                    None
                }
            }
        }
    }

    fn lifecycle(&mut self, frame: &str, now: Instant) -> Option<Turn> {
        let event: Value = serde_json::from_str(frame).ok()?;
        let response = event.get("response")?;
        let id = response.get("id").and_then(Value::as_str).unwrap_or("");
        match event.get("type").and_then(Value::as_str)? {
            "response.created" => {
                if self.open.len() >= MAX_OPEN {
                    self.open.remove(0);
                }
                self.open.push(OpenTurn {
                    id: id.to_string(),
                    started: now,
                    first_delta: None,
                });
                self.awaiting_delta = true;
                None
            }
            "response.done" => {
                let open = self
                    .open
                    .iter()
                    .position(|turn| turn.id == id)
                    .map(|index| self.open.remove(index));
                self.awaiting_delta = self.open.iter().any(|turn| turn.first_delta.is_none());
                // a response whose start was never seen is still billed; it
                // only has no latency to report
                let (started, (latency_ms, ttft_ms)) = match &open {
                    Some(turn) => (turn.started, turn.timings(now)),
                    None => (now, (0, 0)),
                };
                Some(Turn {
                    started,
                    latency_ms,
                    ttft_ms,
                    usage: realtime_usage(response),
                    failed: response.get("status").and_then(Value::as_str) == Some("failed"),
                })
            }
            _ => None,
        }
    }

    /// Stamp time to first token on the response this delta belongs to, or on
    /// every response still waiting for one when the delta names none.
    fn first_delta(&mut self, frame: &str, now: Instant) {
        if !self.awaiting_delta {
            return;
        }
        let owner = response_id(frame);
        for turn in &mut self.open {
            if owner.is_none_or(|owner| owner == turn.id) {
                turn.first_delta.get_or_insert(now);
            }
        }
        self.awaiting_delta = self.open.iter().any(|turn| turn.first_delta.is_none());
    }

    /// The responses still open, for the session's last flush.
    pub(crate) fn into_open(self) -> Vec<OpenTurn> {
        self.open
    }
}

/// The `type` of a server event when it is the object's first key, or its
/// second behind `event_id`.
///
/// Reading only the leading keys is what makes this exact rather than a guess:
/// a nested `type`, such as an item's, can never sit there. The Realtime API
/// writes `type` in one of those two places, so this answers from a few dozen
/// bytes of a frame that may carry tens of kilobytes of audio.
pub(crate) fn leading_type(frame: &str) -> Option<&str> {
    let rest = frame.trim_start().strip_prefix('{')?.trim_start();
    let rest = match rest.strip_prefix("\"event_id\"") {
        Some(after) => skip_string_member(after)?,
        None => rest,
    };
    let rest = rest.strip_prefix("\"type\"")?;
    leading_string(rest).map(|(value, _)| value)
}

/// After a key, the string value that follows its colon and the input past
/// the value's closing quote. `None` for anything else, including a value
/// with an escape, which no event type or id carries.
fn leading_string(after_key: &str) -> Option<(&str, &str)> {
    let rest = after_key.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest
        .bytes()
        .take(TYPE_MAX_LEN)
        .position(|byte| byte == b'"' || byte == b'\\')?;
    if rest.as_bytes().get(end) != Some(&b'"') {
        return None;
    }
    Some((rest.get(..end)?, rest.get(end + 1..)?))
}

/// The `response_id` a delta event names.
///
/// Delta events are flat and write it ahead of the delta itself, so the search
/// stops within the first few dozen bytes. A quoted `"response_id"` inside the
/// delta's text is escaped in the frame, so it cannot match here.
fn response_id(frame: &str) -> Option<&str> {
    let at = frame.find("\"response_id\"")?;
    let after_key = frame.get(at + "\"response_id\"".len()..)?;
    leading_string(after_key).map(|(value, _)| value)
}

/// Skip a leading member's string value and its trailing comma, returning the
/// input at the next key.
fn skip_string_member(after_key: &str) -> Option<&str> {
    let (_, rest) = leading_string(after_key)?;
    Some(rest.trim_start().strip_prefix(',')?.trim_start())
}

/// Token usage from a Realtime `response` object.
///
/// Realtime reports `input_tokens`/`output_tokens` with cached input under
/// `input_token_details.cached_tokens`, which is neither the Chat Completions
/// nor the Messages spelling, so [`crate::logging::parse_usage`] does not read
/// it. Audio and text tokens are summed into those totals by the upstream and
/// are priced alike here: a price row has one input and one output rate.
fn realtime_usage(response: &Value) -> Usage {
    let Some(usage) = response.get("usage").filter(|usage| usage.is_object()) else {
        return Usage::default();
    };
    let count = |value: Option<&Value>| {
        value
            .and_then(Value::as_u64)
            .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    let prompt = count(usage.get("input_tokens"));
    let completion = count(usage.get("output_tokens"));
    let total = match count(usage.get("total_tokens")) {
        0 => prompt.saturating_add(completion),
        total => total,
    };
    Usage {
        prompt,
        completion,
        total,
        cache_read: count(usage.pointer("/input_token_details/cached_tokens")),
        cache_write: 0,
        reported: true,
    }
}

/// Why a session ended, as recorded on the row of a response it cut short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionEnd {
    status: u16,
    error: String,
    /// the upstream leg failed, which counts against the target's health
    upstream_fault: bool,
}

impl SessionEnd {
    /// the client closed the socket or went away
    pub(crate) fn client_left() -> Self {
        Self {
            status: CLIENT_DISCONNECT_STATUS,
            error: CLIENT_DISCONNECT_ERROR.to_string(),
            upstream_fault: false,
        }
    }

    /// the upstream closed the session itself
    pub(crate) fn upstream_closed() -> Self {
        Self {
            status: 502,
            error: "upstream closed the realtime session".to_string(),
            upstream_fault: false,
        }
    }

    /// the upstream connection broke
    pub(crate) fn upstream_failed() -> Self {
        Self {
            status: 502,
            error: "upstream realtime connection failed".to_string(),
            upstream_fault: true,
        }
    }

    /// a `[realtime]` limit ended it; `limit` names the key
    pub(crate) fn limit_reached(limit: &str) -> Self {
        Self {
            status: 408,
            error: format!("realtime session closed by {limit}"),
            upstream_fault: false,
        }
    }

    /// a budget in the session's scope chain is spent
    pub(crate) fn budget_spent(message: String) -> Self {
        Self {
            status: 402,
            error: message,
            upstream_fault: false,
        }
    }

    /// the gateway is shutting down
    pub(crate) fn shutting_down() -> Self {
        Self {
            status: 503,
            error: "gateway shutting down".to_string(),
            upstream_fault: false,
        }
    }

    /// the key that opened the session, or its access, was revoked (#1881)
    pub(crate) fn access_revoked(revoked: &AccessRevoked) -> Self {
        Self {
            status: revoked.status.as_u16(),
            error: revoked.message.clone(),
            upstream_fault: false,
        }
    }

    pub(crate) fn upstream_fault(&self) -> bool {
        self.upstream_fault
    }
}

enum MeterEvent {
    Turn(Turn),
    End {
        end: SessionEnd,
        open: Vec<OpenTurn>,
    },
}

/// The relay's side of its session's meter.
pub(crate) struct MeterHandle {
    tx: mpsc::Sender<MeterEvent>,
    metrics: Arc<Metrics>,
}

impl MeterHandle {
    /// Hand a finished turn to the meter without waiting.
    ///
    /// A full queue drops the turn and counts it on
    /// `rolter_usage_records_dropped_total`, the policy the HTTP path's usage
    /// sink already has (#1051): the relay carries live audio and must never
    /// stall behind the counter store.
    pub(crate) fn turn(&self, turn: Turn) {
        if self.tx.try_send(MeterEvent::Turn(turn)).is_err() {
            self.metrics
                .usage_records_dropped_total
                .fetch_add(1, Relaxed);
        }
    }

    /// Tell the meter the session is over, so it flushes a last time and
    /// stops. Waits at most for the flush in progress to finish.
    pub(crate) async fn end(self, end: SessionEnd, open: Vec<OpenTurn>) {
        // a meter that already stopped has nothing left to flush
        let _ = self.tx.send(MeterEvent::End { end, open }).await;
    }
}

/// Why the meter asks the relay to close a session.
#[derive(Debug)]
pub(crate) enum Closure {
    /// a budget in the session's scope chain is spent; carries the refusal
    BudgetSpent(String),
    /// the session's key, or its access to the model, is gone (#1881)
    AccessRevoked(AccessRevoked),
}

/// Everything a session's meter needs to attribute and price its turns.
pub(crate) struct SessionMeter {
    pub(crate) state: AppState,
    pub(crate) scope: ScopeIds,
    /// the model the client asked for, as every other log row records it
    pub(crate) model: String,
    /// the route's model, which prices are keyed by
    pub(crate) price_model: String,
    pub(crate) provider: String,
    pub(crate) target: String,
    pub(crate) variant: String,
    /// the upgrade request's id; each turn's row appends its ordinal
    pub(crate) request_id: String,
    pub(crate) trace_id: String,
    /// `None` flushes after every turn instead of on a timer, and re-reads
    /// the budgets every [`IDLE_BUDGET_CHECK`] in between
    pub(crate) flush_every: Option<Duration>,
    /// digest of the key that opened the session, looked up again on every
    /// tick; `None` when the session was opened without a key
    pub(crate) key_digest: Option<String>,
}

/// The relay's view of a running meter.
pub(crate) struct MeterChannels {
    pub(crate) handle: MeterHandle,
    /// resolves with the reason to close when a tick finds the session's key
    /// revoked or moved out of reach, or a budget in its scope chain spent
    pub(crate) closure: oneshot::Receiver<Closure>,
    /// the scope the session bills to, changed when a tick re-scopes it
    pub(crate) scope: watch::Receiver<ScopeIds>,
}

impl SessionMeter {
    /// Start the meter.
    pub(crate) fn spawn(self) -> MeterChannels {
        let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
        let (exhausted_tx, exhausted_rx) = oneshot::channel();
        let (scope_tx, scope_rx) = watch::channel(self.scope.clone());
        let handle = MeterHandle {
            tx,
            metrics: self.state.metrics.clone(),
        };
        let sessions = self.state.realtime_sessions.clone();
        let runner = Runner {
            meter: self,
            pending: Vec::new(),
            seq: 0,
            exhausted: Some(exhausted_tx),
            rescoped: scope_tx,
        };
        // tracked, so a gateway shutting down waits for this meter's last
        // flush before the runtime is dropped under it
        sessions.spawn_meter(runner.run(rx));
        MeterChannels {
            handle,
            closure: exhausted_rx,
            scope: scope_rx,
        }
    }
}

/// What a tick does about the scope the session's key is in now.
#[derive(Debug, PartialEq, Eq)]
enum Rescope {
    /// the key is where the session was opened, or last re-scoped
    Unchanged,
    /// the key moved within its organization; later turns bill to this scope
    Moved(ScopeIds),
    /// the key moved somewhere the session cannot follow
    Close(AccessRevoked),
}

/// Whether `scope` is still the scope `key` resolves to. Compared field by
/// field rather than through [`crate::handlers::request_scope`], so the tick
/// that finds nothing changed, which is nearly every tick, allocates nothing.
fn scope_matches(scope: &ScopeIds, key: &KeyMeta) -> bool {
    scope.org == key.org_id
        && scope.team == key.team_id
        && scope.project == key.project_id
        && scope.key == key.id
        && scope.business_unit == key.business_unit_id
        && scope.customer == key.customer_id
}

/// Decide whether a session whose key now sits in `key`'s scope can follow it
/// there (#2384).
///
/// Budgets, rate limits and the request log are keyed by scope ids that the
/// meter reads on every flush, so a key moved to another team or project is
/// followed: its later turns bill and count against the new chain. Two moves
/// are not followed, because the session's content policy was built for the
/// scope it was opened in and cannot be rebuilt mid-session:
///
/// - another organization is another tenant, with its own guardrail tenancy,
///   plugins and provider credentials;
/// - a project whose pre-upstream plugins differ from the old one's would
///   leave the session running a plugin set the key no longer selects.
fn rescope(snap: &Snapshot, current: &ScopeIds, key: &KeyMeta) -> Rescope {
    if scope_matches(current, key) {
        return Rescope::Unchanged;
    }
    let moved = crate::handlers::request_scope(Some(key));
    let closed = |message: &str| {
        Rescope::Close(AccessRevoked {
            status: axum::http::StatusCode::FORBIDDEN,
            code: "key_scope_changed",
            message: message.to_string(),
        })
    };
    if moved.org != current.org {
        return closed(
            "the virtual key moved to another organization; open a new realtime session",
        );
    }
    let plugins = |scope: &ScopeIds| {
        let project = (!scope.project.is_empty()).then_some(scope.project.as_str());
        snap.plugins
            .for_stage(PluginStage::PreUpstream, &scope.org, project)
    };
    let (before, after) = (plugins(current), plugins(&moved));
    let same_plugins = before.len() == after.len()
        && before
            .iter()
            .zip(&after)
            .all(|(old, new)| std::ptr::eq(*old, *new));
    if !same_plugins {
        return closed(
            "the virtual key moved to a project with different plugins; open a new realtime \
             session",
        );
    }
    Rescope::Moved(moved)
}

struct Runner {
    meter: SessionMeter,
    pending: Vec<Turn>,
    /// turns logged so far, so every row of the session has its own id
    seq: u64,
    /// taken once a spent budget has been reported
    exhausted: Option<oneshot::Sender<Closure>>,
    /// tells the relay the session was re-scoped, so its content policy
    /// attributes later events to the same tenant the meter bills
    rescoped: watch::Sender<ScopeIds>,
}

impl Runner {
    async fn run(mut self, mut rx: mpsc::Receiver<MeterEvent>) {
        // with no flush timer the tick only re-reads the budgets: every turn
        // was flushed as it arrived, so there is nothing pending to write
        let every = self.meter.flush_every.unwrap_or(IDLE_BUDGET_CHECK);
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let tick = ticker.tick();
            tokio::select! {
                event = rx.recv() => match event {
                    Some(MeterEvent::Turn(turn)) => {
                        self.pending.push(turn);
                        if self.meter.flush_every.is_none() {
                            self.flush_and_check().await;
                        }
                    }
                    Some(MeterEvent::End { end, open }) => {
                        self.finish(&end, open).await;
                        return;
                    }
                    // the relay went away without saying why; what it already
                    // handed over is still owed
                    None => {
                        self.finish(&SessionEnd::client_left(), Vec::new()).await;
                        return;
                    }
                },
                _ = tick => self.flush_and_check().await,
            }
        }
    }

    /// Flush, then report a revoked key or a spent budget to the relay.
    ///
    /// The key is looked up in the snapshot loaded for this tick, so a key
    /// disabled, expired or deleted, or a `models` list or route access
    /// narrowed, reaches a live session within one flush interval (#1881).
    /// A key moved to another team or project re-scopes the session before
    /// its budgets are read, so a spent budget in the new chain closes it on
    /// the same tick (#2384). Turns already pending are flushed to the scope
    /// they were served under first.
    ///
    /// The check runs on every tick, not only after this session spent
    /// something: a budget is shared by the whole scope chain, so another
    /// session or plain HTTP traffic may be what used it up. It costs nothing
    /// when no budget applies to the session or Redis is not configured.
    async fn flush_and_check(&mut self) {
        let snap = self.meter.state.snapshot.load_full();
        self.flush(&snap).await;
        if self.exhausted.is_none() {
            return;
        }
        if let Some(digest) = &self.meter.key_digest {
            let verdict = crate::handlers::recheck_session_access(
                &snap,
                digest,
                &self.meter.model,
                &self.meter.provider,
            )
            .map(|key| rescope(&snap, &self.meter.scope, key));
            match verdict {
                Ok(Rescope::Unchanged) => {}
                Ok(Rescope::Moved(scope)) => {
                    self.meter.scope = scope.clone();
                    // the relay is gone only when the session is ending
                    let _ = self.rescoped.send(scope);
                }
                Err(revoked) | Ok(Rescope::Close(revoked)) => {
                    if let Some(tx) = self.exhausted.take() {
                        let _ = tx.send(Closure::AccessRevoked(revoked));
                    }
                    return;
                }
            }
        }
        let state = &self.meter.state;
        if let Some(spent) = state
            .budgets
            .exceeded(&snap.budgets, &self.meter.scope)
            .await
        {
            state.metrics.budget_blocks_total.fetch_add(1, Relaxed);
            if let Some(tx) = self.exhausted.take() {
                let _ = tx.send(Closure::BudgetSpent(
                    crate::handlers::budget_exceeded_message(&spent),
                ));
            }
        }
    }

    /// Log every pending turn and charge them to the session's budgets and
    /// `tpm` windows.
    ///
    /// The snapshot is read per flush, so a price, budget or limit edited
    /// mid-session applies from the next flush on rather than from the next
    /// session.
    async fn flush(&mut self, snap: &Snapshot) {
        if self.pending.is_empty() {
            return;
        }
        let price = snap.prices.get(&self.meter.price_model);
        let mut cost = Decimal::ZERO;
        let mut tokens = 0u64;
        for turn in std::mem::take(&mut self.pending) {
            self.seq += 1;
            let (row, turn_cost) = self.turn_row(snap, price, &turn, self.seq);
            cost += turn_cost;
            tokens += u64::from(row.total_tokens);
            self.meter.state.log.log(row);
        }
        let (state, scope) = (&self.meter.state, &self.meter.scope);
        // one write per flush rather than per turn: this is the point of the
        // window, and the sum is exact until the single narrowing below
        if let Some(cost) = cost.to_f64().filter(|cost| *cost > 0.0) {
            state.budgets.record(&snap.budgets, scope, cost).await;
        }
        state
            .rate_limiter
            .record_tokens(&snap.rate_limits, scope, tokens)
            .await;
    }

    /// The last flush, plus a row for each response the end cut short.
    async fn finish(&mut self, end: &SessionEnd, open: Vec<OpenTurn>) {
        let snap = self.meter.state.snapshot.load_full();
        self.flush(&snap).await;
        let price = snap.prices.get(&self.meter.price_model);
        let now = Instant::now();
        let state = &self.meter.state;
        for turn in open {
            self.seq += 1;
            let row = self.cut_short_row(&snap, price, &turn, end, now, self.seq);
            if end.status == CLIENT_DISCONNECT_STATUS {
                state.metrics.client_disconnects_total.fetch_add(1, Relaxed);
            }
            // the relay already attributed a broken upstream leg to its target
            // once, and a client leaving, a budget or a session limit is not the
            // target's failure, so these rows must not count against its health
            state.log.log_recorded_attempt(row);
        }
    }

    /// The fields every row of this session shares.
    fn base_row(
        &self,
        snap: &Snapshot,
        price: Option<&ModelPriceConfig>,
        started: Instant,
        seq: u64,
    ) -> RequestLog {
        let meter = &self.meter;
        RequestLog {
            ts: crate::logging::started_at(started),
            request_id: format!("{}:{seq}", meter.request_id),
            trace_id: meter.trace_id.clone(),
            org_id: meter.scope.org.clone(),
            team_id: meter.scope.team.clone(),
            project_id: meter.scope.project.clone(),
            virtual_key_id: meter.scope.key.clone(),
            business_unit_id: meter.scope.business_unit.clone(),
            customer_id: meter.scope.customer.clone(),
            model: meter.model.clone(),
            provider: meter.provider.clone(),
            target: meter.target.clone(),
            variant: meter.variant.clone(),
            // a turn is streamed to the client as it is generated
            stream: 1,
            unpriced: u8::from(price.is_none()),
            sample_rate: snap.logging.sample_rate,
            ..Default::default()
        }
    }

    fn turn_row(
        &self,
        snap: &Snapshot,
        price: Option<&ModelPriceConfig>,
        turn: &Turn,
        seq: u64,
    ) -> (RequestLog, Decimal) {
        let usage = turn.usage;
        let cost = price
            .map(|price| price.cost(usage.prompt, usage.completion, usage.cache_read))
            .unwrap_or(Decimal::ZERO);
        let mut row = self.base_row(snap, price, turn.started, seq);
        row.status = if turn.failed { 502 } else { 200 };
        if turn.failed {
            row.error = "realtime response failed".to_string();
        }
        row.prompt_tokens = usage.prompt;
        row.completion_tokens = usage.completion;
        row.total_tokens = usage.total;
        row.cache_read_tokens = usage.cache_read;
        row.cost_usd = cost.to_f64().unwrap_or(0.0);
        // a failed response reports no usage because nothing was billed, the
        // same reading the HTTP path gives an upstream error (#1478)
        row.usage_unknown = u8::from(!turn.failed && !usage.reported);
        row.latency_ms = turn.latency_ms;
        row.ttft_ms = turn.ttft_ms;
        (row, cost)
    }

    fn cut_short_row(
        &self,
        snap: &Snapshot,
        price: Option<&ModelPriceConfig>,
        turn: &OpenTurn,
        end: &SessionEnd,
        now: Instant,
        seq: u64,
    ) -> RequestLog {
        let mut row = self.base_row(snap, price, turn.started, seq);
        let (latency_ms, ttft_ms) = turn.timings(now);
        row.status = end.status;
        row.error = end.error.clone();
        // the upstream generated part of this response and will bill it, but
        // it never said how much: unknown, not free (#1083, #1478)
        row.usage_unknown = 1;
        row.latency_ms = latency_ms;
        row.ttft_ms = ttft_ms;
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created(id: &str) -> String {
        format!(
            r#"{{"type":"response.created","event_id":"e1","response":{{"id":"{id}","status":"in_progress"}}}}"#
        )
    }

    fn done(id: &str, usage: &str) -> String {
        format!(
            r#"{{"type":"response.done","event_id":"e2","response":{{"id":"{id}","status":"completed","output":[],"usage":{usage}}}}}"#
        )
    }

    const USAGE: &str = r#"{"total_tokens":330,"input_tokens":120,"output_tokens":210,"input_token_details":{"cached_tokens":64,"text_tokens":20,"audio_tokens":100},"output_token_details":{"text_tokens":10,"audio_tokens":200}}"#;

    #[test]
    fn the_leading_type_is_read_without_parsing_the_frame() {
        assert_eq!(
            leading_type(r#"{"type":"response.output_audio.delta","delta":"AAAA"}"#),
            Some("response.output_audio.delta")
        );
        assert_eq!(
            leading_type(" { \"type\" : \"response.done\" , \"response\": {} }"),
            Some("response.done")
        );
        // the spelling the Realtime reference uses, with `event_id` first
        assert_eq!(
            leading_type(r#"{"event_id": "event_1234", "type": "response.done", "response": {}}"#),
            Some("response.done")
        );
        // not a leading key: a nested `type` must never be mistaken for it
        assert_eq!(
            leading_type(r#"{"event_id":"e","item":{"type":"message"},"type":"x"}"#),
            None
        );
        assert_eq!(
            leading_type(r#"{"item":{"type":"message"},"type":"x"}"#),
            None
        );
        assert_eq!(leading_type(r#"{"event_id":"e\"","type":"x"}"#), None);
        assert_eq!(leading_type("not json"), None);
        assert_eq!(leading_type(r#"{"type":"unterminated"#), None);
        assert_eq!(leading_type(r#"{"event_id":"e""#), None);
    }

    #[test]
    fn an_event_id_first_frame_is_metered_on_the_fast_path() {
        let mut tracker = TurnTracker::default();
        let start = Instant::now();
        let created = r#"{"event_id":"e1","type":"response.created","response":{"id":"r"}}"#;
        assert!(tracker.observe_at(created, start).is_none());
        let delta = r#"{"event_id":"e2","type":"response.output_text.delta","delta":"hi"}"#;
        tracker.observe_at(delta, start + Duration::from_millis(30));
        let done = format!(
            r#"{{"event_id":"e3","type":"response.done","response":{{"id":"r","usage":{USAGE}}}}}"#
        );
        let turn = tracker
            .observe_at(&done, start + Duration::from_millis(500))
            .unwrap();
        assert_eq!(turn.usage.total, 330);
        assert_eq!((turn.latency_ms, turn.ttft_ms), (500, 30));
    }

    #[test]
    fn a_completed_turn_reports_the_upstream_usage() {
        let mut tracker = TurnTracker::default();
        let start = Instant::now();
        assert!(tracker.observe_at(&created("resp_1"), start).is_none());
        let delta =
            r#"{"type":"response.output_audio.delta","response_id":"resp_1","delta":"UklGRg=="}"#;
        assert!(tracker
            .observe_at(delta, start + Duration::from_millis(40))
            .is_none());
        let turn = tracker
            .observe_at(&done("resp_1", USAGE), start + Duration::from_millis(900))
            .expect("response.done finishes the turn");
        assert_eq!(
            turn.usage,
            Usage {
                prompt: 120,
                completion: 210,
                total: 330,
                cache_read: 64,
                cache_write: 0,
                reported: true,
            }
        );
        assert_eq!((turn.latency_ms, turn.ttft_ms), (900, 40));
        assert!(!turn.failed);
        assert!(tracker.into_open().is_empty(), "the turn is closed");
    }

    #[test]
    fn frames_that_are_not_lifecycle_events_are_ignored() {
        let mut tracker = TurnTracker::default();
        for frame in [
            r#"{"type":"session.updated","session":{"type":"realtime"}}"#,
            r#"{"type":"conversation.item.created","item":{"type":"message"}}"#,
            r#"{"type":"response.output_audio.delta","delta":"AAAA"}"#,
            "not json at all",
        ] {
            assert!(tracker.observe(frame).is_none(), "{frame}");
        }
        assert!(tracker.into_open().is_empty());
    }

    #[test]
    fn a_response_without_usage_is_billed_as_unknown() {
        let mut tracker = TurnTracker::default();
        tracker.observe(&created("resp_1"));
        let turn = tracker.observe(&done("resp_1", "null")).unwrap();
        assert!(!turn.usage.reported);
        assert_eq!(turn.usage.total, 0);
    }

    #[test]
    fn a_failed_response_is_marked() {
        let mut tracker = TurnTracker::default();
        let frame =
            r#"{"type":"response.done","response":{"id":"r","status":"failed","usage":null}}"#;
        assert!(tracker.observe(frame).unwrap().failed);
    }

    #[test]
    fn a_done_without_its_created_is_still_metered() {
        let mut tracker = TurnTracker::default();
        let turn = tracker.observe(&done("resp_unseen", USAGE)).unwrap();
        assert_eq!(turn.usage.total, 330);
        assert_eq!((turn.latency_ms, turn.ttft_ms), (0, 0));
    }

    #[test]
    fn total_falls_back_to_input_plus_output() {
        let usage = realtime_usage(&serde_json::json!({
            "usage": {"input_tokens": 7, "output_tokens": 5}
        }));
        assert_eq!(usage.total, 12);
        assert!(usage.reported);
    }

    #[test]
    fn a_frame_with_type_later_still_falls_back_to_a_full_read() {
        let mut tracker = TurnTracker::default();
        let created = r#"{"event_id":"e","response":{"id":"r"},"type":"response.created"}"#;
        assert!(tracker.observe(created).is_none());
        let done = format!(
            r#"{{"event_id":"e","response":{{"id":"r","usage":{USAGE}}},"type":"response.done"}}"#
        );
        assert_eq!(tracker.observe(&done).unwrap().usage.total, 330);
    }

    #[test]
    fn open_responses_are_handed_back_at_the_end() {
        let mut tracker = TurnTracker::default();
        tracker.observe(&created("resp_1"));
        tracker.observe(&created("resp_2"));
        tracker.observe(&done("resp_1", USAGE));
        let open = tracker.into_open();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, "resp_2");
    }

    /// With server VAD the user's transcript streams while the model is still
    /// thinking. It is input, so it must not count as the answer's first token.
    #[test]
    fn an_input_transcription_delta_is_not_a_first_token() {
        let mut tracker = TurnTracker::default();
        let start = Instant::now();
        tracker.observe_at(&created("resp_1"), start);
        let transcript = r#"{"type":"conversation.item.input_audio_transcription.delta","event_id":"e","item_id":"item_1","delta":"good"}"#;
        tracker.observe_at(transcript, start + Duration::from_millis(20));
        // the same event with `type` behind other keys takes the slow path
        let late_type = r#"{"event_id":"e","item_id":"item_1","type":"conversation.item.input_audio_transcription.delta","delta":" day"}"#;
        tracker.observe_at(late_type, start + Duration::from_millis(30));
        let audio = r#"{"type":"response.output_audio.delta","event_id":"e","response_id":"resp_1","delta":"AAAA"}"#;
        tracker.observe_at(audio, start + Duration::from_millis(400));
        let turn = tracker
            .observe_at(&done("resp_1", USAGE), start + Duration::from_millis(900))
            .unwrap();
        assert_eq!((turn.latency_ms, turn.ttft_ms), (900, 400));
    }

    #[test]
    fn a_delta_stamps_only_the_response_it_names() {
        let mut tracker = TurnTracker::default();
        let start = Instant::now();
        tracker.observe_at(&created("resp_1"), start);
        tracker.observe_at(&created("resp_2"), start);
        let second = r#"{"type":"response.output_text.delta","response_id":"resp_2","delta":"a"}"#;
        tracker.observe_at(second, start + Duration::from_millis(50));
        let first = r#"{"type":"response.output_text.delta","response_id":"resp_1","delta":"b"}"#;
        tracker.observe_at(first, start + Duration::from_millis(300));
        let one = tracker
            .observe_at(&done("resp_1", USAGE), start + Duration::from_millis(500))
            .unwrap();
        let two = tracker
            .observe_at(&done("resp_2", USAGE), start + Duration::from_millis(600))
            .unwrap();
        assert_eq!(one.ttft_ms, 300);
        assert_eq!(two.ttft_ms, 50);
    }

    #[test]
    fn the_response_id_is_read_from_its_own_key() {
        assert_eq!(
            response_id(r#"{"type":"x","response_id":"resp_1","delta":"a"}"#),
            Some("resp_1")
        );
        // the key quoted inside the delta's text is escaped in the frame
        assert_eq!(
            response_id(r#"{"type":"x","delta":"say \"response_id\": \"no\""}"#),
            None
        );
        assert_eq!(response_id(r#"{"type":"x"}"#), None);
    }

    fn plugin(slug: &str, org: &str, project: Option<&str>) -> rolter_core::PluginInstanceConfig {
        serde_json::from_value(serde_json::json!({
            "slug": slug,
            "org_id": org,
            "project_id": project,
            "stage": "pre_upstream",
            "position": 0,
            "failure_mode": "fail_open",
            "endpoint": "https://plugin.example.com/hook",
        }))
        .unwrap()
    }

    fn snapshot(plugins: Vec<rolter_core::PluginInstanceConfig>) -> Snapshot {
        let mut config = rolter_core::GatewayConfig::default();
        config.plugins.instances = plugins;
        Snapshot::build(&config, &crate::load::LoadTracker::new())
    }

    fn key(org: &str, team: &str, project: &str) -> KeyMeta {
        KeyMeta {
            id: "key-1".into(),
            org_id: org.into(),
            team_id: team.into(),
            project_id: project.into(),
            ..Default::default()
        }
    }

    fn opened_in(org: &str, team: &str, project: &str) -> ScopeIds {
        crate::handlers::request_scope(Some(&key(org, team, project)))
    }

    #[test]
    fn a_key_left_where_it_was_keeps_its_scope() {
        let snap = snapshot(Vec::new());
        let scope = opened_in("org", "team", "proj-a");
        assert_eq!(
            rescope(&snap, &scope, &key("org", "team", "proj-a")),
            Rescope::Unchanged
        );
    }

    #[test]
    fn a_key_moved_within_its_org_is_followed() {
        let snap = snapshot(vec![plugin("org-wide", "org", None)]);
        let scope = opened_in("org", "team-a", "proj-a");
        let Rescope::Moved(moved) = rescope(&snap, &scope, &key("org", "team-b", "proj-b")) else {
            panic!("an in-org move with the same plugins re-scopes");
        };
        assert_eq!(moved, opened_in("org", "team-b", "proj-b"));
    }

    #[test]
    fn a_key_moved_to_another_org_closes_the_session() {
        let snap = snapshot(Vec::new());
        let scope = opened_in("org-a", "team", "proj");
        let Rescope::Close(revoked) = rescope(&snap, &scope, &key("org-b", "team", "proj")) else {
            panic!("a cross-org move closes");
        };
        assert_eq!(revoked.code, "key_scope_changed");
        assert_eq!(revoked.status, axum::http::StatusCode::FORBIDDEN);
    }

    #[test]
    fn a_key_moved_into_a_project_with_other_plugins_closes_the_session() {
        let snap = snapshot(vec![plugin("audit", "org", Some("proj-b"))]);
        let scope = opened_in("org", "team", "proj-a");
        assert!(matches!(
            rescope(&snap, &scope, &key("org", "team", "proj-b")),
            Rescope::Close(_)
        ));
        // and out of one, which would leave the session running a plugin the
        // key no longer selects
        let scope = opened_in("org", "team", "proj-b");
        assert!(matches!(
            rescope(&snap, &scope, &key("org", "team", "proj-a")),
            Rescope::Close(_)
        ));
    }

    #[test]
    fn open_responses_are_bounded() {
        let mut tracker = TurnTracker::default();
        for i in 0..(MAX_OPEN + 5) {
            tracker.observe(&created(&format!("resp_{i}")));
        }
        let open = tracker.into_open();
        assert_eq!(open.len(), MAX_OPEN);
        assert_eq!(open[0].id, "resp_5", "the oldest are dropped first");
    }
}
