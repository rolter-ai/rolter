//! CSV export of the cost rollups and the request log for chargeback (#1838).
//!
//! FinOps work ends in a spreadsheet: a monthly chargeback per business unit, a
//! per-customer usage statement, a reconciliation against a provider's bill.
//! These routes are the same reads the Cost Attribution and LLM Logs screens
//! make, as files.
//!
//! * They take the same [`AnalyticsAccess`] extractor and bind the same row
//!   predicate as the JSON reads (#1820), so a caller exports exactly the rows
//!   they could page through and none other. There is no second authorization
//!   path to drift out of step with the first.
//! * The request log is **metadata only**. No captured body, no error text
//!   (an upstream error can quote the prompt it rejected), no guardrail or
//!   plugin event detail. The columns are the allow-list in [`LOG_COLUMNS`];
//!   a new `request_logs` column is not exported until it is added there.
//! * The log is streamed. The handler walks the log with the keyset cursor the
//!   JSON list uses, one bounded page per query, and writes each page as it
//!   arrives, so a month of logs costs one page of memory and no request is
//!   held to the ClickHouse client's per-call time bound. The first page is
//!   read before the response starts, so a ClickHouse failure is a JSON error
//!   and not a truncated file; one part way through ends the body with an
//!   error so the download fails rather than passing for complete.
//! * An export is bounded (#3360). The log can span the whole retention period
//!   and a stream holds a connection and a run of ClickHouse queries for as
//!   long as the walk takes, so [`ExportLimits`] caps the rows and the wall-clock
//!   time of one export, and [`ExportGate`] the number running at once, per
//!   process and per caller. An export past a row or time cap ends its body
//!   with the error a failed page produces, so the file cannot pass for
//!   complete; one past the concurrency bound is answered `429` with
//!   `Retry-After` before it starts.
//! * A spend figure is only final when every request in it was priced. Every
//!   export carries `unpriced_requests` and a `cost_is_partial` flag (the log
//!   carries `unpriced` per row and leaves the cost blank, as the screen does),
//!   so a total with a gap is never read as the whole.
//! * Every cell goes through [`csv_cell`], which defuses a value a spreadsheet
//!   would run as a formula (a leading `=`, `+`, `-`, `@`, tab or carriage return).
//! * A browser without the File System Access API downloads an export through
//!   a single-use ticket (#3504, `export_tickets`). The download route redeems
//!   the ticket, re-resolves the principal that minted it and calls these same
//!   handlers through [`run_redeemed`] with that principal's [`Caller`] and
//!   [`AnalyticsAccess`], so a ticket is checked exactly as a bearer request is.
//!   An [`ExportProgress`] rides along and is what the ticket's status reports.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode, Version};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Extension;
use axum::Router;
use futures_util::stream::{self, StreamExt};
use serde_json::Value;

use crate::analytics::{
    attribution_column, bucket_expr, client_or_503, invocations_keyset_predicate,
    is_outcome_filter, next_invocations_cursor, outcome_filter_values, query_failed,
    row_filter_params, split_invocations_cursor, status_predicate, window_params, with_access,
    AttributionQuery, ClickHouseClient, DashboardQuery, InvocationsQuery, WindowQuery,
    COMPLETION_TOKENS, COST, ERRORS, PROMPT_TOKENS, REQUESTS, ROW_FILTERS, TOKENS, UNPRICED,
    WHERE_WINDOW,
};
use crate::analytics_access::{AnalyticsAccess, ROW_VISIBLE};
use crate::conn_close::{CloseSwitch, ConnectionHandle};
use crate::request_log_columns::Columns;
use crate::time_bounds::Query;
use crate::ControlState;

/// What a refused export tells the client to wait, in seconds. A running export
/// is bounded by [`ExportLimits::max_duration`], which is minutes, so a short
/// pause is a poll interval and not a promise that a slot has freed.
const RETRY_AFTER_SECS: u64 = 15;

/// The bounds on one request log export (#3360). A value of `0` for any of them
/// turns that bound off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExportLimits {
    /// most rows one export carries
    pub(crate) max_rows: Option<u64>,
    /// longest wall-clock time one export runs
    pub(crate) max_duration: Option<Duration>,
    /// exports running at once in this process
    pub(crate) max_concurrent: Option<u32>,
    /// exports running at once for one caller
    pub(crate) max_per_caller: Option<u32>,
}

impl ExportLimits {
    /// The limits from the control plane's settings, `0` meaning unbounded.
    pub(crate) fn new(rows: u64, secs: u64, concurrent: u32, per_caller: u32) -> Self {
        Self {
            max_rows: (rows > 0).then_some(rows),
            max_duration: (secs > 0).then(|| Duration::from_secs(secs)),
            max_concurrent: (concurrent > 0).then_some(concurrent),
            max_per_caller: (per_caller > 0).then_some(per_caller),
        }
    }
}

impl Default for ExportLimits {
    /// A million rows, ten minutes, four at once and one per caller: a month of
    /// a busy deployment's log fits, and one caller cannot take the process's
    /// slots.
    fn default() -> Self {
        Self::new(1_000_000, 600, 4, 1)
    }
}

/// Which concurrency bound refused an export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Busy {
    Process,
    Caller,
}

#[derive(Default)]
struct Running {
    total: u32,
    by_caller: HashMap<String, u32>,
}

/// How many request log exports are running, against [`ExportLimits`] (#3360).
/// Process-local: it guards this process's connections and its ClickHouse
/// queries.
#[derive(Default)]
pub(crate) struct ExportGate {
    limits: ExportLimits,
    running: parking_lot::Mutex<Running>,
}

impl ExportGate {
    pub(crate) fn new(limits: ExportLimits) -> Self {
        Self {
            limits,
            running: parking_lot::Mutex::default(),
        }
    }

    /// Whether `running` has room for one more export by `caller`.
    fn room(&self, running: &Running, caller: &str) -> Result<u32, Busy> {
        if self
            .limits
            .max_concurrent
            .is_some_and(|max| running.total >= max)
        {
            return Err(Busy::Process);
        }
        let held = running.by_caller.get(caller).copied().unwrap_or(0);
        if self.limits.max_per_caller.is_some_and(|max| held >= max) {
            return Err(Busy::Caller);
        }
        Ok(held)
    }

    /// The longest an export may run, `None` when unbounded.
    pub(crate) fn max_duration(&self) -> Option<Duration> {
        self.limits.max_duration
    }

    /// Whether an export by `caller` would be let in now, without taking a
    /// place: a download ticket is refused at mint when the gate is already
    /// full (#3504). The export itself still takes its place when it starts,
    /// so this is a courtesy and not a reservation.
    pub(crate) fn would_admit(&self, caller: &str) -> Result<(), Busy> {
        self.room(&self.running.lock(), caller).map(|_| ())
    }

    /// A place for one more export by `caller`, held until the permit drops.
    fn try_acquire(self: &Arc<Self>, caller: &str) -> Result<ExportPermit, Busy> {
        let mut running = self.running.lock();
        let held = self.room(&running, caller)?;
        running.total += 1;
        running.by_caller.insert(caller.to_string(), held + 1);
        Ok(ExportPermit {
            gate: Arc::clone(self),
            caller: caller.to_string(),
        })
    }
}

/// A running export's place in the [`ExportGate`], given back when the body is
/// dropped: it finished, failed, or the client went away.
struct ExportPermit {
    gate: Arc<ExportGate>,
    caller: String,
}

impl Drop for ExportPermit {
    fn drop(&mut self) {
        let mut running = self.gate.running.lock();
        running.total = running.total.saturating_sub(1);
        if let Some(held) = running.by_caller.get_mut(&self.caller) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                running.by_caller.remove(&self.caller);
            }
        }
    }
}

/// A running export's permit behind a timer (#3433). The row, time and cancel
/// checks of [`LogWalk`] only run when the body is polled, and a client that
/// stops reading without closing its connection parks the body on a full
/// socket buffer, where it is never polled again and would hold its slot until
/// TCP gave up. So the permit sits in a slot a deadline task empties at
/// [`ExportLimits::max_duration`] whether or not the body is ever polled; the
/// slot is also emptied by the usual drop, and the timer is aborted when the
/// body is dropped first.
///
/// The slot alone frees the place but leaves the stalled response open: hyper
/// is parked writing to a full socket, so the connection, its task and the
/// buffered chunk would stay until TCP gave up (#3477). The timer therefore
/// also trips the connection's [`CloseSwitch`], which fails the parked write
/// without the body being polled.
struct TimedPermit {
    slot: Arc<parking_lot::Mutex<Option<ExportPermit>>>,
    /// the last page has been produced, so the connection is no longer this
    /// export's to cut
    finished: Arc<std::sync::atomic::AtomicBool>,
    timer: Option<tokio::task::JoinHandle<()>>,
}

impl TimedPermit {
    /// Hold `permit` until the guard drops or `limit` passes, and close
    /// `connection` then, recording the time limit as the cut on `progress`
    /// so a download ticket reads `time_limit` rather than a closed client.
    /// No limit leaves the permit to the drop alone.
    fn new(
        permit: ExportPermit,
        limit: Option<Duration>,
        connection: Option<Arc<CloseSwitch>>,
        progress: Option<Arc<ExportProgress>>,
    ) -> Self {
        let slot = Arc::new(parking_lot::Mutex::new(Some(permit)));
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let timer = limit.map(|limit| {
            let slot = Arc::clone(&slot);
            let finished = Arc::clone(&finished);
            tokio::spawn(async move {
                tokio::time::sleep(limit).await;
                // decided under the slot's lock, with the export's end marked
                // before the body is dropped: a keep-alive connection that has
                // moved on to another request is never cut by the timer of an
                // export that already delivered its last page
                let mut held = slot.lock();
                if held.take().is_some() {
                    if let Some(progress) = &progress {
                        progress.set_cut(Cut::TimeLimit);
                    }
                    let close = !finished.load(std::sync::atomic::Ordering::Acquire);
                    tracing::warn!(
                        closing = close && connection.is_some(),
                        "request log export passed its time limit; its place was given back"
                    );
                    if let (true, Some(connection)) = (close, connection) {
                        connection.trip();
                    }
                }
            })
        });
        Self {
            slot,
            finished,
            timer,
        }
    }

    /// The export has produced its last page: disarm the timer's close, so a
    /// connection that outlives the export is left be. The place is still held
    /// until the body drops or the timer gives it back.
    fn finish(&self) {
        let _held = self.slot.lock();
        self.finished
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// The permit is still held, so the export may keep running.
    fn is_held(&self) -> bool {
        self.slot.lock().is_some()
    }
}

impl Drop for TimedPermit {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
        // an aborted task drops its clone of the slot only when the runtime
        // next polls it, so the permit is given back here and not left to that
        self.slot.lock().take();
    }
}

/// The HTTP version the request arrived on.
pub(crate) struct RequestVersion(pub(crate) Version);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for RequestVersion {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(parts.version))
    }
}

/// The switch the timer may trip, if this request owns its connection (#3477).
///
/// Only an HTTP/1.x connection is one client's alone. An HTTP/2 one carries
/// other requests, often other users' behind a proxy that speaks h2c to the
/// control plane, and tripping it would reset them all; there the timer only
/// gives the place back, and h2's per-stream flow control bounds what is
/// buffered for the stalled stream.
fn closable(
    version: Version,
    connection: Option<Extension<ConnectionHandle>>,
) -> Option<Arc<CloseSwitch>> {
    connection
        .filter(|_| version <= Version::HTTP_11)
        .map(|Extension(ConnectionHandle(switch))| switch)
}

/// `429` for an export the gate refused, with `Retry-After`.
pub(crate) fn too_many_exports(busy: Busy) -> Response {
    let message = match busy {
        Busy::Process => "too many request log exports are running; try again shortly",
        Busy::Caller => "you already have a request log export running; wait for it to finish",
    };
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        axum::Json(serde_json::json!({"error": {"message": message}})),
    )
        .into_response();
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&RETRY_AFTER_SECS.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("15")),
    );
    response
}

/// Why an export ended before its last row, for a download ticket's status
/// (#3504). The body ends in an error either way; this is the part only the
/// handler knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cut {
    /// the log holds more rows than [`ExportLimits::max_rows`]
    RowLimit,
    /// the export ran past [`ExportLimits::max_duration`]
    TimeLimit,
}

/// What an export has done so far, written by the handler and read by the
/// download route that serves a ticket (#3504). A bearer request carries none.
#[derive(Debug, Default)]
pub(crate) struct ExportProgress {
    rows: std::sync::atomic::AtomicU64,
    cut: parking_lot::Mutex<Option<Cut>>,
}

impl ExportProgress {
    /// Data rows produced so far.
    pub(crate) fn rows(&self) -> u64 {
        self.rows.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn set_rows(&self, rows: u64) {
        self.rows.store(rows, std::sync::atomic::Ordering::Relaxed);
    }

    /// Why the export stopped short, if a bound stopped it.
    pub(crate) fn cut(&self) -> Option<Cut> {
        *self.cut.lock()
    }

    fn set_cut(&self, cut: Cut) {
        self.cut.lock().get_or_insert(cut);
    }
}

/// The progress a handler reports to, when a ticket's download asked for one.
type Progress = Option<Extension<Arc<ExportProgress>>>;

/// Rows read per query while streaming the request log. A page is the unit of
/// memory, so this bounds what one export holds however long the window is.
const PAGE_SIZE: u32 = 1000;

/// The `request_logs` columns the log export carries, in order. An allow-list
/// on purpose: the table also holds the places a body or a quote of one could
/// sit, and nothing reaches a file by being added to the table.
const LOG_COLUMNS: &[&str] = &[
    "ts",
    "request_id",
    "trace_id",
    "org_id",
    "team_id",
    "project_id",
    "virtual_key_id",
    "business_unit_id",
    "customer_id",
    "model",
    "provider",
    "target",
    "variant",
    "status",
    "stream",
    "cache_hit",
    "cache_read_tokens",
    "cache_write_tokens",
    "prompt_tokens",
    "completion_tokens",
    "total_tokens",
    "cost",
    "currency",
    "unpriced",
    "latency_ms",
    "ttft_ms",
    "seq",
    "sample_weight",
];

/// The columns of [`LOG_COLUMNS`] that are numbers, which are written as they
/// are: a number cannot be a formula, and a leading `'` would corrupt one.
const LOG_NUMERIC: &[&str] = &[
    "status",
    "stream",
    "cache_hit",
    "cache_read_tokens",
    "cache_write_tokens",
    "prompt_tokens",
    "completion_tokens",
    "total_tokens",
    "cost",
    "unpriced",
    "latency_ms",
    "ttft_ms",
    "seq",
    "sample_weight",
];

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/analytics/by-attribution/export",
            get(export_attribution),
        )
        .route(
            "/api/v1/analytics/timeseries/export",
            get(export_timeseries),
        )
        .route(
            "/api/v1/analytics/invocations/export",
            get(export_invocations),
        )
        .merge(crate::export_tickets::router())
}

/// The exports a download ticket may name (#3504), by the path of the bearer
/// route each one is. The audit log's two (#3545) exist only where the audit
/// log does, a control plane with a database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportKind {
    Attribution,
    Timeseries,
    Invocations,
    /// `GET /api/v1/audit-log/export`, the deployment-wide audit log
    #[cfg(feature = "postgres")]
    DeploymentAudit,
    /// `GET /api/v1/orgs/{org_id}/audit-log/export`, one org's audit log
    #[cfg(feature = "postgres")]
    OrgAudit(uuid::Uuid),
}

/// The audit log export of one org lives under the org's own path.
#[cfg(feature = "postgres")]
const ORG_AUDIT_PREFIX: &str = "/api/v1/orgs/";
#[cfg(feature = "postgres")]
const ORG_AUDIT_SUFFIX: &str = "/audit-log/export";
#[cfg(feature = "postgres")]
const DEPLOYMENT_AUDIT_PATH: &str = "/api/v1/audit-log/export";
/// The stored name of an org's audit export starts with this, the org id after.
#[cfg(feature = "postgres")]
const ORG_AUDIT_NAME: &str = "org_audit_log";

impl ExportKind {
    /// The analytics exports, whose bearer routes have fixed paths.
    pub(crate) const ANALYTICS: [ExportKind; 3] = [
        ExportKind::Attribution,
        ExportKind::Timeseries,
        ExportKind::Invocations,
    ];

    /// The bearer route of this export.
    pub(crate) fn path(self) -> std::borrow::Cow<'static, str> {
        match self {
            ExportKind::Attribution => "/api/v1/analytics/by-attribution/export".into(),
            ExportKind::Timeseries => "/api/v1/analytics/timeseries/export".into(),
            ExportKind::Invocations => "/api/v1/analytics/invocations/export".into(),
            #[cfg(feature = "postgres")]
            ExportKind::DeploymentAudit => DEPLOYMENT_AUDIT_PATH.into(),
            #[cfg(feature = "postgres")]
            ExportKind::OrgAudit(org) => {
                format!("{ORG_AUDIT_PREFIX}{org}{ORG_AUDIT_SUFFIX}").into()
            }
        }
    }

    /// The export whose bearer route is exactly `path`; any other path is none.
    /// An audit export is one only where `audit` is set, on a control plane
    /// with a database: without one its route is not mounted.
    pub(crate) fn from_path(path: &str, audit: bool) -> Option<Self> {
        if let Some(kind) = Self::ANALYTICS.into_iter().find(|kind| kind.path() == path) {
            return Some(kind);
        }
        Self::audit_from_path(path).filter(|_| audit)
    }

    #[cfg(feature = "postgres")]
    fn audit_from_path(path: &str) -> Option<Self> {
        if path == DEPLOYMENT_AUDIT_PATH {
            return Some(ExportKind::DeploymentAudit);
        }
        let org = path
            .strip_prefix(ORG_AUDIT_PREFIX)?
            .strip_suffix(ORG_AUDIT_SUFFIX)?;
        // only the canonical hyphenated spelling, so a path names one org one way
        let id = org.parse::<uuid::Uuid>().ok()?;
        (id.hyphenated().to_string() == org).then_some(ExportKind::OrgAudit(id))
    }

    #[cfg(not(feature = "postgres"))]
    fn audit_from_path(_: &str) -> Option<Self> {
        None
    }

    /// The name reported in a ticket's status.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ExportKind::Attribution => "attribution",
            ExportKind::Timeseries => "timeseries",
            ExportKind::Invocations => "invocations",
            #[cfg(feature = "postgres")]
            ExportKind::DeploymentAudit => "audit_log",
            #[cfg(feature = "postgres")]
            ExportKind::OrgAudit(_) => ORG_AUDIT_NAME,
        }
    }

    /// The name stored with a ticket: the status name, and for an org's audit
    /// export the org it reads, so the redemption reads the same org.
    #[cfg(feature = "postgres")]
    pub(crate) fn stored(self) -> String {
        match self {
            ExportKind::OrgAudit(org) => format!("{ORG_AUDIT_NAME}:{org}"),
            kind => kind.as_str().to_string(),
        }
    }

    /// The export a stored name names: only the Postgres store reads it back.
    #[cfg(feature = "postgres")]
    pub(crate) fn parse(name: &str) -> Option<Self> {
        if let Some(org) = name
            .strip_prefix(ORG_AUDIT_NAME)
            .and_then(|rest| rest.strip_prefix(':'))
        {
            return org.parse().ok().map(ExportKind::OrgAudit);
        }
        Self::ANALYTICS
            .into_iter()
            .chain([ExportKind::DeploymentAudit])
            .find(|kind| kind.as_str() == name)
    }

    /// Whether the [`ExportGate`] bounds this export: only the request log is
    /// streamed from ClickHouse, the two rollups are one query each, and the
    /// audit log is paged from Postgres a page at a time.
    pub(crate) fn is_gated(self) -> bool {
        self == ExportKind::Invocations
    }

    /// Whether this is an audit log export, which the audit log's own
    /// capability governs rather than the request log's.
    pub(crate) fn is_audit(self) -> bool {
        match self {
            ExportKind::Attribution | ExportKind::Timeseries | ExportKind::Invocations => false,
            #[cfg(feature = "postgres")]
            ExportKind::DeploymentAudit | ExportKind::OrgAudit(_) => true,
        }
    }
}

/// The query string `query` read the way the bearer route of `kind` reads its
/// own, window checks included, as the request parts that carry it.
#[allow(clippy::result_large_err)]
fn stored_parts(kind: ExportKind, query: &str) -> Result<axum::http::request::Parts, Response> {
    let uri = if query.is_empty() {
        kind.path().into_owned()
    } else {
        format!("{}?{query}", kind.path())
    };
    axum::http::Request::get(uri)
        .body(())
        .map(|request| request.into_parts().0)
        .map_err(|_| bad_request("the export query is not a valid query string"))
}

/// Parse `query` as the export `kind` would, so a ticket for a query its export
/// would refuse is refused when it is minted (#3504).
#[allow(clippy::result_large_err)]
pub(crate) async fn check_query(
    state: &ControlState,
    kind: ExportKind,
    query: &str,
) -> Result<(), Response> {
    use axum::extract::FromRequestParts;
    let mut parts = stored_parts(kind, query)?;
    let parsed = match kind {
        ExportKind::Attribution => Query::<AttributionQuery>::from_request_parts(&mut parts, state)
            .await
            .map(drop),
        ExportKind::Timeseries => Query::<DashboardQuery>::from_request_parts(&mut parts, state)
            .await
            .map(drop),
        ExportKind::Invocations => Query::<InvocationsQuery>::from_request_parts(&mut parts, state)
            .await
            .map(drop),
        #[cfg(feature = "postgres")]
        ExportKind::DeploymentAudit | ExportKind::OrgAudit(_) => {
            return crate::audit_export::check_query(&mut parts, state).await;
        }
    };
    parsed.map_err(IntoResponse::into_response)
}

/// Run the export `kind` for a redeemed download ticket: the bearer route's
/// own handler, given the caller the ticket's principal re-resolved to instead
/// of one read from a bearer header. The handler runs every check it runs for
/// a bearer request (the query's, the row scope's and the gate's, and for the
/// audit log its own capability check, #3545), and reports to `progress` as it
/// goes. `access` is the principal's reach over the request log, which only
/// the analytics exports read; the caller works it out for those alone.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_redeemed(
    state: ControlState,
    kind: ExportKind,
    query: &str,
    principal: Caller,
    access: Option<AnalyticsAccess>,
    version: RequestVersion,
    connection: Option<Extension<ConnectionHandle>>,
    progress: Arc<ExportProgress>,
) -> Response {
    use axum::extract::FromRequestParts;
    let mut parts = match stored_parts(kind, query) {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    #[cfg(feature = "postgres")]
    if kind.is_audit() {
        return crate::audit_export::run_redeemed(state, kind, parts, principal, progress).await;
    }
    let Some(access) = access else {
        tracing::error!(
            export = kind.as_str(),
            "an analytics export redeemed without its reach"
        );
        return internal_error();
    };
    let progress = Some(Extension(progress));
    match kind {
        ExportKind::Attribution => {
            match Query::<AttributionQuery>::from_request_parts(&mut parts, &state).await {
                Ok(q) => export_attribution(access, principal, State(state), progress, q).await,
                Err(refused) => refused.into_response(),
            }
        }
        ExportKind::Timeseries => {
            match Query::<DashboardQuery>::from_request_parts(&mut parts, &state).await {
                Ok(q) => export_timeseries(access, State(state), progress, q).await,
                Err(refused) => refused.into_response(),
            }
        }
        ExportKind::Invocations => {
            match Query::<InvocationsQuery>::from_request_parts(&mut parts, &state).await {
                Ok(q) => {
                    export_invocations(
                        access,
                        principal,
                        State(state),
                        version,
                        connection,
                        progress,
                        q,
                    )
                    .await
                }
                Err(refused) => refused.into_response(),
            }
        }
        // handed to the audit log's own handler above
        #[cfg(feature = "postgres")]
        ExportKind::DeploymentAudit | ExportKind::OrgAudit(_) => internal_error(),
    }
}

/// One CSV field. A spreadsheet runs a cell that starts with `=`, `+`, `-`, `@`,
/// a tab or a carriage return as a formula, and a name or an id is typed by
/// someone other than the reader, so such a cell gets a leading `'`. Kept
/// identical to `csvCell` in `ui/src/lib/csv.ts`; the audit export writes its
/// cells the same way.
pub(crate) fn csv_cell(value: &str) -> String {
    let mut text = value.to_string();
    if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        text.insert(0, '\'');
    }
    if text.contains(['"', ',', '\r', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

/// The header row: names, never numbers.
fn line(cells: &[String]) -> String {
    let cells: Vec<String> = cells.iter().map(|c| csv_cell(c)).collect();
    format!("{}\r\n", cells.join(","))
}

/// A JSON value as the text of a cell.
fn cell_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// A row of `(text, is_number)` cells. A number is written as it is: it cannot
/// be a formula, and [`csv_cell`] would put a quote before a negative one.
fn line_of(row: &[(String, bool)]) -> String {
    let cells: Vec<String> = row
        .iter()
        .map(|(text, numeric)| {
            if *numeric && text.parse::<f64>().is_ok() {
                text.clone()
            } else {
                csv_cell(text)
            }
        })
        .collect();
    format!("{}\r\n", cells.join(","))
}

fn as_f64(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// The part of a file name a window contributes: its date, or what stands in
/// for an unset bound. Anything that is not a date character is dropped, so a
/// bound can never put a quote or a path separator into a header.
fn window_label(bound: Option<&str>, unset: &str) -> String {
    let bound = bound.map(str::trim).filter(|b| !b.is_empty());
    let Some(bound) = bound else {
        return unset.to_string();
    };
    let date: String = bound
        .chars()
        .take(10)
        .filter(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    if date.is_empty() {
        unset.to_string()
    } else {
        date
    }
}

/// `<what>_<from>_to_<to>_<CURRENCY>.csv`: the file says which window and which
/// currency its figures are in, so it can be told apart from last month's.
fn file_name(what: &str, since: Option<&str>, until: Option<&str>, currency: &str) -> String {
    let currency: String = currency
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_uppercase();
    format!(
        "{what}_{}_to_{}_{currency}.csv",
        window_label(since, "7-days-ago"),
        window_label(until, "now"),
    )
}

fn csv_response(body: Body, name: &str) -> Response {
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    // the name is built from digits, letters, `-` and `_` only
    if let Ok(disposition) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(serde_json::json!({"error": {"message": "an internal server error occurred"}})),
    )
        .into_response()
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({"error": {"message": message}})),
    )
        .into_response()
}

/// Who is asking, for the one read that is governed beyond the request log: a
/// control plane with no database has no accounts, so no caller to check.
#[cfg(feature = "postgres")]
pub(crate) type Caller = crate::rbac::Principal;
#[cfg(not(feature = "postgres"))]
pub(crate) type Caller = ();

/// Who a concurrency bound counts an export against: the account, or the API
/// token, or the one shared credential. Open mode and the admin token have no
/// person behind them, so they share a key.
#[cfg(feature = "postgres")]
pub(crate) fn caller_key(caller: &Caller) -> String {
    use crate::rbac::Principal;
    match caller {
        Principal::Superadmin { account: Some(id) } => format!("user:{id}"),
        Principal::Superadmin { account: None } => "admin".to_string(),
        Principal::User(user) => format!("user:{}", user.id),
        Principal::Token(token) => format!("token:{}", token.token.id),
    }
}

#[cfg(not(feature = "postgres"))]
pub(crate) fn caller_key(_: &Caller) -> String {
    "admin".to_string()
}

/// The names of the business units or customers a rollup refers to, by id,
/// for the ones the caller may read. A unit's or customer's name is governed
/// data of its org, so it needs `business_unit:read` / `customer:read` there:
/// a caller without it gets the id alone, as the row's other columns are theirs
/// to see but the name is not. An id with no row (deleted since) stays unnamed,
/// and a control plane with no database has no names to give.
#[cfg(feature = "postgres")]
async fn names_for(
    state: &ControlState,
    principal: &Caller,
    dimension: &str,
    ids: &[String],
) -> HashMap<String, String> {
    use crate::rbac::{ScopeChain, ScopeFilter};
    use crate::rbac_matrix::cap;

    let ids: Vec<uuid::Uuid> = ids.iter().filter_map(|id| id.parse().ok()).collect();
    if ids.is_empty() {
        return HashMap::new();
    }
    let (sql, requirement) = match dimension {
        "customer" => (
            "select id, name, org_id from customers where id = any($1)",
            cap!("customer", Read),
        ),
        _ => (
            "select id, name, org_id from business_units where id = any($1)",
            cap!("business_unit", Read),
        ),
    };
    let Ok(filter) = ScopeFilter::load(state, principal, requirement).await else {
        return HashMap::new();
    };
    sqlx::query_as::<_, (uuid::Uuid, String, uuid::Uuid)>(sql)
        .bind(ids)
        .fetch_all(crate::crud::pool(state))
        .await
        .map(|rows| {
            rows.into_iter()
                .filter(|(_, _, org)| filter.allows(ScopeChain::org(*org)))
                .map(|(id, name, _)| (id.to_string(), name))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(not(feature = "postgres"))]
async fn names_for(_: &ControlState, _: &Caller, _: &str, _: &[String]) -> HashMap<String, String> {
    HashMap::new()
}

/// `GET /api/v1/analytics/by-attribution/export`: the Cost Attribution rollup,
/// one row per business unit or customer.
async fn export_attribution(
    // the access first: without a database it is what refuses a wrong bearer,
    // and `Principal` would reach for the pool that is not there
    access: AnalyticsAccess,
    principal: Caller,
    State(state): State<ControlState>,
    progress: Progress,
    Query(q): Query<AttributionQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let dimension = q.dimension.as_deref().unwrap_or("business_unit");
    let Some(column) = attribution_column(dimension) else {
        return bad_request("dimension must be one of business_unit|customer");
    };
    let attributed = if q.include_unattributed {
        "1"
    } else {
        "notEmpty(id)"
    };
    let sql = format!(
        "select {column} as id, \
                {REQUESTS} as requests, \
                {TOKENS} as tokens, \
                {PROMPT_TOKENS} as prompt_tokens, \
                {COMPLETION_TOKENS} as completion_tokens, \
                {COST} as cost_usd, \
                {UNPRICED} as unpriced_requests, \
                {ERRORS} as errors \
         from request_logs where {WHERE_WINDOW} and {ROW_VISIBLE} \
         group by id having {attributed} order by cost_usd desc format JSON"
    );
    let window = window_params(&WindowQuery {
        since: q.since.clone(),
        until: q.until.clone(),
        bucket: None,
    });
    let rows = match ch.query(&sql, &with_access(window, &access)).await {
        Ok(rows) => rows,
        Err(error) => return query_failed("analytics query failed", &error),
    };
    let ids: Vec<String> = rows
        .iter()
        .filter_map(|r| Some(r.get("id")?.as_str()?.to_string()))
        .collect();
    let names = names_for(&state, &principal, dimension, &ids).await;
    let currency = state.currency.base_code();
    let mut out = line(
        &[
            "dimension",
            "id",
            "name",
            "requests",
            "tokens",
            "prompt_tokens",
            "completion_tokens",
            "errors",
            "cost",
            "currency",
            "unpriced_requests",
            "cost_is_partial",
        ]
        .map(String::from),
    );
    for row in &rows {
        let id = cell_text(row.get("id"));
        let name = if id.is_empty() {
            "(unattributed)".to_string()
        } else {
            names.get(&id).cloned().unwrap_or_default()
        };
        let unpriced = as_f64(row.get("unpriced_requests"));
        out.push_str(&line_of(&[
            (dimension.to_string(), false),
            (id, false),
            (name, false),
            (cell_text(row.get("requests")), true),
            (cell_text(row.get("tokens")), true),
            (cell_text(row.get("prompt_tokens")), true),
            (cell_text(row.get("completion_tokens")), true),
            (cell_text(row.get("errors")), true),
            (cell_text(row.get("cost_usd")), true),
            (currency.clone(), false),
            (cell_text(row.get("unpriced_requests")), true),
            ((unpriced > 0.0).to_string(), false),
        ]));
    }
    if let Some(Extension(progress)) = &progress {
        progress.set_rows(rows.len() as u64);
    }
    let name = file_name(
        &format!("cost-attribution-{dimension}"),
        q.since.as_deref(),
        q.until.as_deref(),
        &currency,
    );
    csv_response(Body::from(out), &name)
}

/// `GET /api/v1/analytics/timeseries/export`: requests, tokens and cost per
/// bucket, under the filters the Dashboard applies.
async fn export_timeseries(
    access: AnalyticsAccess,
    State(state): State<ControlState>,
    progress: Progress,
    Query(q): Query<DashboardQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let bucket = q.window.bucket.as_deref().unwrap_or("day");
    let Some(bucket_expr) = bucket_expr(bucket) else {
        return bad_request("bucket must be one of hour|day|week|month");
    };
    let sql = format!(
        "select {bucket_expr} as bucket, \
                {REQUESTS} as requests, \
                {TOKENS} as tokens, \
                {COST} as cost_usd, \
                {UNPRICED} as unpriced_requests \
         from request_logs where {WHERE_WINDOW} and {ROW_VISIBLE} and {ROW_FILTERS} \
         group by bucket order by bucket format JSON"
    );
    let rows = match ch.query(&sql, &with_access(q.params(), &access)).await {
        Ok(rows) => rows,
        Err(error) => return query_failed("analytics query failed", &error),
    };
    let currency = state.currency.base_code();
    let mut out = line(
        &[
            "bucket",
            "requests",
            "tokens",
            "cost",
            "currency",
            "unpriced_requests",
            "cost_is_partial",
        ]
        .map(String::from),
    );
    for row in &rows {
        let unpriced = as_f64(row.get("unpriced_requests"));
        out.push_str(&line_of(&[
            (cell_text(row.get("bucket")), false),
            (cell_text(row.get("requests")), true),
            (cell_text(row.get("tokens")), true),
            (cell_text(row.get("cost_usd")), true),
            (currency.clone(), false),
            (cell_text(row.get("unpriced_requests")), true),
            ((unpriced > 0.0).to_string(), false),
        ]));
    }
    if let Some(Extension(progress)) = &progress {
        progress.set_rows(rows.len() as u64);
    }
    let name = file_name(
        &format!("spend-by-{bucket}"),
        q.window.since.as_deref(),
        q.window.until.as_deref(),
        &currency,
    );
    csv_response(Body::from(out), &name)
}

/// The request-log query: [`LOG_COLUMNS`] only, under the visibility predicate
/// and the row filters of the JSON list, resumed after a keyset cursor. No join
/// to the payload table, so a body cannot reach the file through a column.
fn log_sql(status_expr: &str, columns: &Columns) -> String {
    let cursor = invocations_keyset_predicate(&columns.expr("seq"));
    let seq = columns.select("seq");
    let seq_order = columns.expr("seq");
    // a column an older table lacks matches nothing, which is true: no event
    // was ever recorded there
    let guardrail_filter = columns.expr("guardrail_events");
    let plugin_filter = columns.expr("plugin_events");
    // without the column every row reads `unknown`, which the filter matches
    let outcome_filter = columns.expr("outcome");
    format!(
        "select ts, request_id, trace_id, org_id, team_id, project_id, virtual_key_id, \
                business_unit_id, customer_id, model, provider, target, variant, status, \
                stream, cache_hit, cache_read_tokens, cache_write_tokens, prompt_tokens, \
                completion_tokens, total_tokens, cost_usd, unpriced, latency_ms, ttft_ms, \
                {seq}, sample_weight \
         from request_logs \
         where {WHERE_WINDOW} \
           and {ROW_VISIBLE} \
           and {ROW_FILTERS} \
           and ({{unpriced:UInt8}} = 0 or unpriced = 1) \
           and ({{withheld:UInt8}} = 0 or withheld = 1) \
           and ({{usage_unknown:UInt8}} = 0 or usage_unknown = 1) \
           and ({{guardrail:String}} = '' or has({guardrail_filter}, {{guardrail:String}})) \
           and ({{plugin:String}} = '' or has({plugin_filter}, {{plugin:String}})) \
           and ({{outcome:String}} = '' or {outcome_filter} = {{outcome:String}}) \
           and {status_expr} \
           and {cursor} \
         order by ts desc, {seq_order} desc, request_id desc \
         limit {{limit:UInt32}} format JSON"
    )
}

/// One log row as a CSV line. The cost is left blank on an unpriced row, which
/// the screen shows as "unpriced" rather than as a free request.
fn log_line(row: &Value, currency: &str) -> String {
    let unpriced = as_f64(row.get("unpriced")) >= 1.0;
    let cells: Vec<(String, bool)> = LOG_COLUMNS
        .iter()
        .map(|column| {
            let numeric = LOG_NUMERIC.contains(column);
            let text = match *column {
                "cost" if unpriced => String::new(),
                "cost" => cell_text(row.get("cost_usd")),
                "currency" => currency.to_string(),
                other => cell_text(row.get(other)),
            };
            (text, numeric)
        })
        .collect();
    line_of(&cells)
}

/// The walk over the log: the state one export carries from page to page.
struct LogWalk {
    ch: ClickHouseClient,
    sql: String,
    params: Vec<(String, String)>,
    currency: String,
    header_sent: bool,
    /// the keyset position: timestamp, start sequence and request id
    cursor: (String, String, String),
    done: bool,
    /// the row and time bounds of this export (#3360)
    max_rows: Option<u64>,
    deadline: Option<Instant>,
    /// rows written so far
    written: u64,
    /// the log has more rows than `max_rows`: the next chunk is the error
    over_rows: bool,
    /// the exports gate's place for this walk, given back when the body drops
    permit: Option<TimedPermit>,
    /// where a download ticket reads how far the walk got and why it stopped
    progress: Option<Arc<ExportProgress>>,
}

impl LogWalk {
    /// Record why the walk is about to stop short, for a ticket's status.
    fn cut(&self, cut: Cut) {
        if let Some(progress) = &self.progress {
            progress.set_cut(cut);
        }
    }

    /// The next chunk of the file, or `None` once the log is exhausted.
    async fn next_chunk(&mut self) -> anyhow::Result<Option<Bytes>> {
        if self.done {
            return Ok(None);
        }
        // the timer gave the place back while the body sat unread
        if self.permit.as_ref().is_some_and(|permit| !permit.is_held()) {
            self.cut(Cut::TimeLimit);
            anyhow::bail!("the export ran past its time limit");
        }
        if self.over_rows {
            self.cut(Cut::RowLimit);
            anyhow::bail!(
                "the log holds more rows than the export limit of {}",
                self.max_rows.unwrap_or_default()
            );
        }
        // one row past the cap tells a log of exactly `max_rows` from a longer
        // one without reading a page that would be thrown away
        let remaining = self.max_rows.map(|max| max.saturating_sub(self.written));
        let page = remaining.map_or(u64::from(PAGE_SIZE), |left| {
            left.saturating_add(1).min(u64::from(PAGE_SIZE))
        }) as u32;
        let mut params = self.params.clone();
        params.push(("param_cursor_ts".to_string(), self.cursor.0.clone()));
        params.push(("param_cursor_seq".to_string(), self.cursor.1.clone()));
        params.push(("param_cursor_id".to_string(), self.cursor.2.clone()));
        params.push(("param_limit".to_string(), page.to_string()));
        let mut rows = match self.deadline {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    self.cut(Cut::TimeLimit);
                    anyhow::bail!("the export ran past its time limit");
                }
                tokio::time::timeout(left, self.ch.query(&self.sql, &params))
                    .await
                    .map_err(|_| {
                        self.cut(Cut::TimeLimit);
                        anyhow::anyhow!("the export ran past its time limit")
                    })??
            }
            None => self.ch.query(&self.sql, &params).await?,
        };
        let over_rows = remaining.is_some_and(|left| rows.len() as u64 > left);
        if let Some(left) = remaining.filter(|_| over_rows) {
            rows.truncate(left as usize);
        }
        self.written += rows.len() as u64;
        if let Some(progress) = &self.progress {
            progress.set_rows(self.written);
        }
        let mut out = String::new();
        if !self.header_sent {
            self.header_sent = true;
            out.push_str(&line(
                &LOG_COLUMNS
                    .iter()
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>(),
            ));
        }
        for row in &rows {
            out.push_str(&log_line(row, &self.currency));
        }
        match next_invocations_cursor(&rows) {
            // past the cap: what fits is written, and the next chunk is the error
            _ if over_rows => self.over_rows = true,
            // a full page may be followed by older rows; a short one is the end.
            // the cursor is a position in the sort key, so rows the gateway
            // writes while the file is built can neither repeat nor skip one
            Some(next) if rows.len() == page as usize => {
                self.cursor = next
                    .split_once('|')
                    .map(|(ts, tail)| {
                        let (seq, id) = split_invocations_cursor(tail);
                        (ts.to_string(), seq, id)
                    })
                    .unwrap_or_default();
            }
            _ => {
                self.done = true;
                if let Some(permit) = &self.permit {
                    permit.finish();
                }
            }
        }
        Ok(Some(Bytes::from(out)))
    }
}

/// `GET /api/v1/analytics/invocations/export`: the LLM Logs list as a file,
/// metadata only, under the filters the screen applies (window, model, key,
/// project, business unit, customer, status, unpriced, withheld, unknown
/// usage, guardrail, plugin and outcome), the same set the JSON list takes, so
/// a filtered view never exports unfiltered (#3386).
async fn export_invocations(
    // the access first, as for the rollup above
    access: AnalyticsAccess,
    principal: Caller,
    State(state): State<ControlState>,
    RequestVersion(version): RequestVersion,
    connection: Option<Extension<ConnectionHandle>>,
    progress: Progress,
    Query(q): Query<InvocationsQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let status = q.status.as_deref().unwrap_or("all");
    let Some(status_expr) = status_predicate(status) else {
        return bad_request("status must be one of all|error|success");
    };
    let outcome = q.outcome.clone().unwrap_or_default();
    if !is_outcome_filter(&outcome) {
        return bad_request(&format!(
            "outcome must be one of {}",
            outcome_filter_values().join("|")
        ));
    }
    let mut params = window_params(&WindowQuery {
        since: q.since.clone(),
        until: q.until.clone(),
        bucket: None,
    });
    params.push((
        "param_unpriced".to_string(),
        u8::from(q.unpriced.unwrap_or(false)).to_string(),
    ));
    for (name, on) in [("withheld", q.withheld), ("usage_unknown", q.usage_unknown)] {
        params.push((
            format!("param_{name}"),
            u8::from(on.unwrap_or(false)).to_string(),
        ));
    }
    params.push(("param_outcome".to_string(), outcome));
    params.push((
        "param_guardrail".to_string(),
        q.guardrail.clone().unwrap_or_default(),
    ));
    params.push((
        "param_plugin".to_string(),
        q.plugin.clone().unwrap_or_default(),
    ));
    params.extend(row_filter_params(
        q.model.as_deref(),
        q.key.as_deref(),
        q.business_unit.as_deref(),
        q.customer.as_deref(),
        q.project.as_deref(),
    ));
    let currency = state.currency.base_code();
    let name = file_name(
        "request-log",
        q.since.as_deref(),
        q.until.as_deref(),
        &currency,
    );
    // refuse before any ClickHouse work, and hold the place for as long as the
    // body lives (#3360)
    let permit = match state.export_gate.try_acquire(&caller_key(&principal)) {
        Ok(permit) => permit,
        Err(busy) => return too_many_exports(busy),
    };
    let limits = state.export_gate.limits;
    // only an HTTP/1.x connection is this client's alone: an HTTP/2 one
    // carries other requests, which a trip would reset, so there the timer
    // gives the place back and h2's per-stream flow control bounds the buffer
    let connection = closable(version, connection);
    let progress = progress.map(|Extension(progress)| progress);
    let permit = TimedPermit::new(permit, limits.max_duration, connection, progress.clone());
    // which optional columns the table has, so the file still builds before an
    // operator applies the sequence column (see `request_log_columns`)
    let columns = match ch.request_log_columns().await {
        Ok(columns) => columns,
        Err(error) => return query_failed("analytics query failed", &error),
    };
    let mut walk = LogWalk {
        ch: ch.clone(),
        sql: log_sql(status_expr, &columns),
        params: with_access(params, &access),
        currency,
        header_sent: false,
        cursor: (String::new(), "0".to_string(), String::new()),
        done: false,
        max_rows: limits.max_rows,
        deadline: limits.max_duration.map(|limit| Instant::now() + limit),
        written: 0,
        over_rows: false,
        permit: Some(permit),
        progress,
    };
    // the first page is read before the response starts, so a ClickHouse
    // failure is an error the client can read and not a download that ends
    // after the header
    let first = match walk.next_chunk().await {
        Ok(first) => first,
        Err(error) => return query_failed("analytics query failed", &error),
    };
    let rest = stream::unfold(walk, |mut walk| async move {
        match walk.next_chunk().await {
            Ok(Some(chunk)) => Some((Ok::<_, io::Error>(chunk), walk)),
            Ok(None) => None,
            Err(error) => {
                tracing::error!(
                    error = %rolter_core::redact::redact_urls_in_text(&format!("{error:#}")),
                    "request log export failed part way"
                );
                walk.done = true;
                // an error ends the body, so the download fails instead of
                // looking like a complete file
                Some((Err(io::Error::other("request log export failed")), walk))
            }
        }
    });
    let body = stream::iter(first.map(Ok::<_, io::Error>)).chain(rest);
    csv_response(Body::from_stream(body), &name)
}

/// The fake ClickHouse and the body reader the download ticket tests share.
#[cfg(test)]
pub(crate) use tests::{drain, fake_log};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_analytics_export_is_named_by_its_exact_path() {
        for kind in ExportKind::ANALYTICS {
            assert_eq!(ExportKind::from_path(&kind.path(), false), Some(kind));
            assert_eq!(ExportKind::from_path(&kind.path(), true), Some(kind));
            assert!(!kind.is_audit());
        }
        for path in [
            "/api/v1/analytics/invocations/export/",
            "/API/V1/ANALYTICS/INVOCATIONS/EXPORT",
            "/api/v1/analytics/invocations",
            "",
        ] {
            assert_eq!(ExportKind::from_path(path, true), None, "{path}");
        }
    }

    /// The audit exports (#3545): the deployment's path, and one org's with
    /// its id in the canonical hyphenated spelling only, so a path names one
    /// org one way; neither where the audit log is not there.
    #[cfg(feature = "postgres")]
    #[test]
    fn an_audit_export_is_named_by_one_spelling_of_its_path() {
        let org = uuid::Uuid::new_v4();
        let path = format!("/api/v1/orgs/{org}/audit-log/export");
        assert_eq!(
            ExportKind::from_path(&path, true),
            Some(ExportKind::OrgAudit(org))
        );
        assert_eq!(ExportKind::OrgAudit(org).path(), path);
        assert_eq!(
            ExportKind::from_path("/api/v1/audit-log/export", true),
            Some(ExportKind::DeploymentAudit)
        );
        assert!(ExportKind::DeploymentAudit.is_audit() && ExportKind::OrgAudit(org).is_audit());
        // without a database neither is an export path
        assert_eq!(ExportKind::from_path(&path, false), None);
        assert_eq!(
            ExportKind::from_path("/api/v1/audit-log/export", false),
            None
        );
        let hyphenated = org.hyphenated().to_string();
        for id in [
            hyphenated.to_uppercase(),
            org.simple().to_string(),
            format!("{{{hyphenated}}}"),
            org.urn().to_string(),
            format!("{hyphenated}0"),
            hyphenated[1..].to_string(),
            "not-a-uuid".to_string(),
            String::new(),
        ] {
            let path = format!("/api/v1/orgs/{id}/audit-log/export");
            assert_eq!(ExportKind::from_path(&path, true), None, "{path}");
        }
        for path in [
            format!("/api/v1/orgs/{org}/audit-log/export/"),
            format!("/api/v1/orgs/{org}/audit-log"),
            format!("/api/v1/orgs/{org}/x/audit-log/export"),
        ] {
            assert_eq!(ExportKind::from_path(&path, true), None, "{path}");
        }
    }

    /// What a ticket stores reads back as the same export, and nothing else
    /// reads as one.
    #[cfg(feature = "postgres")]
    #[test]
    fn a_stored_export_name_reads_back_as_itself() {
        let org = uuid::Uuid::new_v4();
        let kinds = ExportKind::ANALYTICS
            .into_iter()
            .chain([ExportKind::DeploymentAudit, ExportKind::OrgAudit(org)]);
        for kind in kinds {
            assert_eq!(ExportKind::parse(&kind.stored()), Some(kind), "{kind:?}");
        }
        assert_eq!(
            ExportKind::OrgAudit(org).stored(),
            format!("org_audit_log:{org}")
        );
        assert_eq!(ExportKind::OrgAudit(org).as_str(), "org_audit_log");
        assert_eq!(ExportKind::DeploymentAudit.stored(), "audit_log");
        for name in [
            "org_audit_log",
            "org_audit_log:",
            "org_audit_log:not-a-uuid",
            "org_audit_logx",
            "Invocations",
            "",
        ] {
            assert_eq!(ExportKind::parse(name), None, "{name}");
        }
    }

    #[test]
    fn a_file_names_its_window_and_currency() {
        assert_eq!(
            file_name(
                "cost-attribution-business_unit",
                Some("2026-10-01T00:00:00Z"),
                Some("2026-11-01"),
                "eur"
            ),
            "cost-attribution-business_unit_2026-10-01_to_2026-11-01_EUR.csv"
        );
        assert_eq!(
            file_name("request-log", None, Some(""), "USD"),
            "request-log_7-days-ago_to_now_USD.csv"
        );
        // nothing a bound holds reaches the header
        let name = file_name("x", Some("\"; rm -rf /"), None, "U\"S\r\nD");
        assert!(!name.contains(['"', ';', '/', '\r', '\n', ' ']), "{name}");
    }

    #[test]
    fn a_cell_a_spreadsheet_would_run_is_defused() {
        for hostile in ["=1+1", "+cmd", "-2+3", "@SUM(A1)", "\tx", "\rx"] {
            let row =
                json!({"model": hostile, "request_id": hostile, "unpriced": 0, "cost_usd": 0.5});
            let text = log_line(&row, "USD");
            assert!(
                !text.contains(&format!(",{hostile}")) && !text.starts_with(hostile),
                "{hostile:?} reached the file raw: {text}"
            );
            assert!(
                text.contains(&format!("'{hostile}")) || text.contains(&format!("\"'{hostile}"))
            );
        }
        // a formula in the currency or in a name too
        assert!(line_of(&[("=HYPERLINK(\"x\")".into(), false)]).starts_with("\"'="));
        // and a number is left alone rather than quoted
        assert_eq!(line_of(&[("-0.5".into(), true)]), "-0.5\r\n");
    }

    #[test]
    fn an_unpriced_row_is_flagged_with_its_cost_left_blank() {
        let priced = log_line(&json!({"unpriced": 0, "cost_usd": 0.25}), "USD");
        let unpriced = log_line(&json!({"unpriced": 1, "cost_usd": 0.0}), "USD");
        let cost = LOG_COLUMNS.iter().position(|c| *c == "cost").unwrap();
        let flag = LOG_COLUMNS.iter().position(|c| *c == "unpriced").unwrap();
        let cells = |l: &str| {
            l.trim_end()
                .split(',')
                .map(String::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(cells(&priced)[cost], "0.25");
        assert_eq!(cells(&priced)[flag], "0");
        assert_eq!(cells(&unpriced)[cost], "");
        assert_eq!(cells(&unpriced)[flag], "1");
    }

    #[test]
    fn the_log_export_carries_no_body_and_no_error_text() {
        for forbidden in [
            "request_payload",
            "response_payload",
            "payload",
            "error",
            "guardrail_events",
            "plugin_events",
        ] {
            assert!(!LOG_COLUMNS.contains(&forbidden), "{forbidden}");
        }
        let sql = log_sql("1", &Columns::default());
        // the export never joins the payload table or selects a body column
        assert!(
            !sql.contains("request_payloads") && !sql.contains("payload"),
            "{sql}"
        );
        // and it binds the same visibility predicate as the JSON list
        assert!(sql.contains(ROW_VISIBLE));
        assert!(sql.contains("order by ts desc, seq desc, request_id desc"));
    }

    #[test]
    fn a_zero_bound_is_no_bound_and_the_defaults_are_finite() {
        let off = ExportLimits::new(0, 0, 0, 0);
        assert_eq!(off.max_rows, None);
        assert_eq!(off.max_duration, None);
        assert_eq!(off.max_concurrent, None);
        assert_eq!(off.max_per_caller, None);
        let defaults = ExportLimits::default();
        assert_eq!(defaults.max_rows, Some(1_000_000));
        assert_eq!(defaults.max_duration, Some(Duration::from_secs(600)));
        assert_eq!(defaults.max_concurrent, Some(4));
        assert_eq!(defaults.max_per_caller, Some(1));
    }

    #[test]
    fn the_gate_bounds_exports_per_process_and_per_caller_and_gives_places_back() {
        let gate = Arc::new(ExportGate::new(ExportLimits::new(0, 0, 3, 2)));
        let a1 = gate.try_acquire("a").unwrap();
        let _a2 = gate.try_acquire("a").unwrap();
        // a third for `a` is over its own bound, though the process has room
        assert_eq!(gate.try_acquire("a").err(), Some(Busy::Caller));
        let _b1 = gate.try_acquire("b").unwrap();
        // the process is now full, whoever asks
        assert_eq!(gate.try_acquire("c").err(), Some(Busy::Process));
        assert_eq!(gate.try_acquire("a").err(), Some(Busy::Process));
        drop(a1);
        let _c1 = gate.try_acquire("c").unwrap();
        let running = gate.running.lock();
        assert_eq!(running.total, 3);
        assert_eq!(running.by_caller.get("a"), Some(&1));
    }

    #[tokio::test]
    async fn a_client_that_stops_reading_loses_its_place_at_the_deadline() {
        let mut limits = ExportLimits::new(0, 0, 1, 1);
        limits.max_duration = Some(Duration::from_millis(60));
        let gate = Arc::new(ExportGate::new(limits));
        let permit = TimedPermit::new(
            gate.try_acquire("a").unwrap(),
            limits.max_duration,
            None,
            None,
        );
        // the body of a stalled download: held, and never polled again
        let stalled = stream::unfold(permit, |permit| async move {
            Some((Ok::<_, io::Error>(Bytes::new()), permit))
        });
        let _body = Body::from_stream(stalled);
        assert_eq!(gate.try_acquire("a").err(), Some(Busy::Process));
        let started = Instant::now();
        loop {
            if let Ok(again) = gate.try_acquire("a") {
                drop(again);
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the place was never given back"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn a_finished_export_gives_its_place_back_at_once_and_stops_the_timer() {
        let gate = Arc::new(ExportGate::new(ExportLimits::new(0, 600, 1, 1)));
        let held = TimedPermit::new(
            gate.try_acquire("a").unwrap(),
            Some(Duration::from_secs(600)),
            None,
            None,
        );
        assert!(held.is_held());
        assert_eq!(gate.try_acquire("b").err(), Some(Busy::Process));
        let timer = held
            .timer
            .as_ref()
            .expect("a limit starts a timer")
            .abort_handle();
        drop(held);
        assert!(gate.try_acquire("b").is_ok());
        // and the ten minute timer does not outlive the export it was for
        for _ in 0..50 {
            if timer.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(timer.is_finished(), "the timer was left running");
    }

    #[test]
    fn an_unbounded_gate_refuses_nothing() {
        let gate = Arc::new(ExportGate::new(ExportLimits::new(0, 0, 0, 0)));
        let permits: Vec<_> = (0..64).map(|_| gate.try_acquire("a").unwrap()).collect();
        assert_eq!(permits.len(), 64);
    }

    /// A ClickHouse that holds a request log of `total` rows, `delay` slower on
    /// every page, answering the keyset walk the export makes.
    pub(crate) async fn fake_log(total: u64, delay: Duration) -> String {
        let app = axum::Router::new().fallback(
            move |axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
                  body: String| async move {
                if body.contains("system.columns") {
                    return axum::Json(serde_json::json!({"data": []}));
                }
                tokio::time::sleep(delay).await;
                let number = |key: &str| params.get(key).and_then(|v| v.parse::<u64>().ok());
                let limit = number("param_limit").unwrap_or(0);
                // sequences count down from `total`, newest first
                let first = match number("param_cursor_seq") {
                    Some(seq) if seq > 0 => seq - 1,
                    _ => total,
                };
                let rows: Vec<Value> = (0..limit)
                    .map_while(|i| first.checked_sub(i).filter(|seq| *seq >= 1))
                    .map(|seq| {
                        serde_json::json!({
                            "ts": "2026-10-01T00:00:00.000Z", "seq": seq.to_string(),
                            "request_id": format!("req-{seq}"), "status": 200,
                        })
                    })
                    .collect();
                axum::Json(serde_json::json!({ "data": rows }))
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let url = format!("http://{}", listener.local_addr().expect("an address"));
        tokio::spawn(async move { axum::serve(listener, app).await });
        url
    }

    fn export_app(clickhouse_url: &str, limits: ExportLimits) -> axum::Router {
        let mut state = crate::tests::state_with_token(None);
        state.clickhouse = Some(ClickHouseClient::new(clickhouse_url));
        state.export_gate = Arc::new(ExportGate::new(limits));
        router().with_state(state)
    }

    async fn export(app: &axum::Router) -> Response {
        use tower::ServiceExt;
        app.clone()
            .oneshot(
                axum::http::Request::get("/api/v1/analytics/invocations/export")
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers")
    }

    /// The data rows a body carries and whether it ended cleanly.
    pub(crate) async fn drain(response: Response) -> (usize, bool) {
        let mut stream = response.into_body().into_data_stream();
        let mut text = String::new();
        let mut complete = true;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => text.push_str(&String::from_utf8_lossy(&bytes)),
                Err(_) => {
                    complete = false;
                    break;
                }
            }
        }
        (text.lines().count().saturating_sub(1), complete)
    }

    #[tokio::test]
    async fn an_export_past_the_row_cap_ends_in_an_error_after_the_cap() {
        let url = fake_log(2500, Duration::ZERO).await;
        let app = export_app(&url, ExportLimits::new(1500, 0, 0, 0));
        let response = export(&app).await;
        assert_eq!(response.status(), StatusCode::OK);
        // the rows that fit are written, then the body fails: the download
        // cannot pass for the whole log
        assert_eq!(drain(response).await, (1500, false));
    }

    #[tokio::test]
    async fn a_log_of_exactly_the_cap_is_a_complete_file() {
        // on a page boundary, where "a full page" cannot tell the end
        let url = fake_log(2000, Duration::ZERO).await;
        let app = export_app(&url, ExportLimits::new(2000, 0, 0, 0));
        assert_eq!(drain(export(&app).await).await, (2000, true));
        // and one row over is not
        let app = export_app(&url, ExportLimits::new(1999, 0, 0, 0));
        assert_eq!(drain(export(&app).await).await, (1999, false));
        // and a log under the cap is whole
        let app = export_app(&url, ExportLimits::new(5000, 0, 0, 0));
        assert_eq!(drain(export(&app).await).await, (2000, true));
    }

    #[tokio::test]
    async fn an_export_past_the_time_limit_ends_in_an_error() {
        // a log far longer than the pages that fit in the time allowed
        let url = fake_log(1_000_000_000, Duration::from_millis(150)).await;
        let limits = ExportLimits {
            max_duration: Some(Duration::from_millis(400)),
            ..ExportLimits::new(0, 0, 0, 0)
        };
        let app = export_app(&url, limits);
        let started = Instant::now();
        let (rows, complete) = drain(export(&app).await).await;
        assert!(!complete, "a timed out export passed for a complete file");
        assert!(
            rows >= PAGE_SIZE as usize && rows < 10 * PAGE_SIZE as usize,
            "{rows}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// The handler's wiring of the timer (#3433): a body that is never polled,
    /// as when a client stops reading, still gives its caller's place back.
    #[tokio::test]
    async fn an_export_whose_body_is_never_read_gives_its_place_back_at_the_deadline() {
        let url = fake_log(5000, Duration::ZERO).await;
        let limits = ExportLimits {
            max_duration: Some(Duration::from_millis(300)),
            ..ExportLimits::new(0, 0, 0, 1)
        };
        let app = export_app(&url, limits);
        // the headers arrive; the body is held and never polled
        let stalled = export(&app).await;
        assert_eq!(stalled.status(), StatusCode::OK);
        assert_eq!(
            export(&app).await.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "the caller's place is held while the export runs"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            export(&app).await.status(),
            StatusCode::OK,
            "the place came back with nothing polling the body"
        );
        // and the stalled body, if it is read after all, ends in the error
        // rather than passing for a complete file
        let (_, complete) = drain(stalled).await;
        assert!(!complete);
    }

    /// #3477: the permit coming back is not enough, the stalled client's
    /// connection has to go too. A real server over the closeable listener, and
    /// a client that sends the request and then never reads.
    #[tokio::test]
    async fn a_client_that_stops_reading_has_its_connection_closed_at_the_deadline() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // far more rows than fit in the socket buffers
        let url = fake_log(3_000_000, Duration::ZERO).await;
        let limits = ExportLimits {
            max_duration: Some(Duration::from_millis(600)),
            ..ExportLimits::new(0, 0, 0, 0)
        };
        let app = crate::conn_close::MakeCloseable(export_app(&url, limits));
        // small socket buffers on both ends, so the response stalls after a few
        // pages and not after however much the kernel will absorb
        let server = tokio::net::TcpSocket::new_v4().unwrap();
        server.set_send_buffer_size(4096).unwrap();
        server.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = crate::conn_close::CloseableListener(server.listen(16).unwrap());
        let addr = listener.0.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });

        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(4096).unwrap();
        let mut client = socket.connect(addr).await.unwrap();
        client
            .write_all(b"GET /api/v1/analytics/invocations/export HTTP/1.1\r\nhost: x\r\n\r\n")
            .await
            .unwrap();
        // what the client still holds is only what its small buffers took
        // before the stall, and then the end. Reading observes the close itself:
        // an open connection would keep delivering the log
        let started = Instant::now();
        let mut total = 0usize;
        let mut sink = vec![0u8; 64 * 1024];
        loop {
            match client.read(&mut sink).await {
                Ok(0) | Err(_) => break,
                Ok(n) => total += n,
            }
            assert!(
                total < 2_000_000 && started.elapsed() < Duration::from_secs(5),
                "the connection was still open after the deadline ({total} bytes)"
            );
        }
    }

    #[test]
    fn only_an_http1_connection_is_cut_by_the_timer() {
        use crate::conn_close::CloseSwitch;
        let handle = || {
            Some(Extension(ConnectionHandle(
                Arc::new(CloseSwitch::default()),
            )))
        };
        assert!(closable(Version::HTTP_10, handle()).is_some());
        assert!(closable(Version::HTTP_11, handle()).is_some());
        // an h2 connection carries other users' requests behind a proxy
        assert!(closable(Version::HTTP_2, handle()).is_none());
        assert!(closable(Version::HTTP_3, handle()).is_none());
        // and a request with no handle (a test, an unwrapped listener) has none
        assert!(closable(Version::HTTP_11, None).is_none());
    }

    /// The same request over h2c, which the control plane also serves: the
    /// export is stalled past its deadline and the connection it shares is
    /// left open, so only its permit is gone.
    #[tokio::test]
    async fn an_h2_export_past_the_deadline_leaves_the_shared_connection_open() {
        let url = fake_log(3_000_000, Duration::ZERO).await;
        let limits = ExportLimits {
            max_duration: Some(Duration::from_millis(400)),
            ..ExportLimits::new(0, 0, 0, 1)
        };
        let app = crate::conn_close::MakeCloseable(export_app(&url, limits));
        let listener = crate::conn_close::CloseableListener(
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
        );
        let addr = listener.0.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .pool_max_idle_per_host(1)
            .build()
            .unwrap();
        let export_url = format!("http://{addr}/api/v1/analytics/invocations/export");
        let stalled = client.get(&export_url).send().await.unwrap();
        assert_eq!(stalled.version(), reqwest::Version::HTTP_2);
        assert_eq!(stalled.status(), 200);
        tokio::time::sleep(Duration::from_millis(800)).await;
        // the permit came back (the caller's bound is 1), and the same
        // connection answers a new stream
        let again = client.get(&export_url).send().await.unwrap();
        assert_eq!(again.status(), 200);
        assert_eq!(again.version(), reqwest::Version::HTTP_2);
        // the first stream, read late, ends in the time limit's error: its own
        // stream, not the connection
        let mut body = stalled;
        let mut ended_in_error = false;
        while let Ok(Some(_)) = body.chunk().await.map_err(|_| ended_in_error = true) {}
        assert!(ended_in_error);
    }

    #[tokio::test]
    async fn a_second_export_over_the_process_bound_is_refused_with_retry_after() {
        let url = fake_log(10, Duration::ZERO).await;
        let app = export_app(&url, ExportLimits::new(0, 0, 1, 0));
        let first = export(&app).await;
        assert_eq!(first.status(), StatusCode::OK);
        let refused = export(&app).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            refused
                .headers()
                .get(header::RETRY_AFTER)
                .map(HeaderValue::as_bytes),
            Some(&b"15"[..])
        );
        // the place comes back when the first body ends, or is dropped
        assert_eq!(drain(first).await, (10, true));
        let again = export(&app).await;
        assert_eq!(again.status(), StatusCode::OK);
        drop(again);
        assert_eq!(export(&app).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn one_caller_cannot_hold_more_than_its_own_bound() {
        let url = fake_log(10, Duration::ZERO).await;
        // room in the process, none for the same caller
        let app = export_app(&url, ExportLimits::new(0, 0, 4, 1));
        let _held = export(&app).await;
        let refused = export(&app).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(refused.headers().contains_key(header::RETRY_AFTER));
        let body = axum::body::to_bytes(refused.into_body(), 1 << 16)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("already have"));
    }

    /// A store-less control plane refuses a wrong bearer on every export with
    /// `401`. In a postgres build the `Principal` extractor reaches for the
    /// pool, which is not there, so the access check has to run first (#3504).
    #[tokio::test]
    async fn a_wrong_bearer_is_refused_without_a_database() {
        use tower::ServiceExt;
        let token = format!("export-admin-{}", uuid::Uuid::new_v4().simple());
        let mut state = crate::tests::state_with_token(Some(&token));
        state.clickhouse = Some(ClickHouseClient::new(&fake_log(10, Duration::ZERO).await));
        let app = router().with_state(state);
        for path in [
            "/api/v1/analytics/by-attribution/export",
            "/api/v1/analytics/timeseries/export",
            "/api/v1/analytics/invocations/export",
        ] {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::get(path)
                        .header(header::AUTHORIZATION, "Bearer rolter_sess_not-a-session")
                        .body(Body::empty())
                        .expect("a valid request"),
                )
                .await
                .expect("the router answers");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
    }

    /// The query parameters a statement binds, `{name:Type}`.
    fn bound(sql: &str) -> std::collections::BTreeSet<String> {
        sql.split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(inside, _)| inside))
            .filter(|inside| inside.contains(':'))
            .map(str::to_string)
            .collect()
    }

    /// #3386: an export under a filtered LLM Logs view came back unfiltered for
    /// `withheld`, `usage_unknown` and `outcome`. The export binds every filter
    /// the JSON list binds, apart from the two that look one request up and
    /// the payload-visibility bindings of a file that carries no body, so a
    /// filter added to one cannot be missing from the other.
    #[test]
    fn the_log_export_takes_every_filter_the_list_takes() {
        let list = bound(&crate::analytics::invocations_sql("1", &Columns::default()));
        let export = bound(&log_sql("1", &Columns::default()));
        let missing: Vec<_> = list
            .difference(&export)
            .filter(|p| {
                ![
                    "request_id:",
                    "trace_id:",
                    "payload_rank:",
                    "viewer_payload_projects:",
                ]
                .iter()
                .any(|own| p.starts_with(own))
            })
            .collect();
        assert!(missing.is_empty(), "the export does not bind {missing:?}");
        for filter in ["withheld:UInt8", "usage_unknown:UInt8", "outcome:String"] {
            assert!(export.contains(filter), "{filter}");
        }
        // a table without 030 reads every row as unknown, which the filter matches
        let old = log_sql("1", &Columns::lacking(&["outcome"]));
        assert!(old.contains("'unknown' = {outcome:String}"), "{old}");
    }
}
