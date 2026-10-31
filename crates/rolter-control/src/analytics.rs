//! Usage/cost aggregation over the ClickHouse `request_logs` table for the
//! dashboard. All endpoints are read-only and return ClickHouse's `FORMAT JSON`
//! `data` array straight through; the keyset-paged invocation list adds a
//! `next_cursor` beside it.
//!
//! Injection safety: time bounds are passed as ClickHouse query **parameters**
//! (`{since:DateTime64}` / `param_since=…`), never interpolated into SQL. The
//! only value spliced into SQL text is the time bucket, which is validated
//! against a fixed whitelist first.
//!
//! Every handler here reads its query string through [`crate::time_bounds`]'s
//! `Query`, not axum's, so a `since`/`until` ClickHouse would misread as the
//! epoch is a `400` before any SQL is built (#1192).

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::analytics_access::{AnalyticsAccess, PAYLOAD_VISIBLE, ROW_VISIBLE};
use crate::request_log_columns::{ColumnCache, Columns, PROBE_SQL};
use crate::time_bounds::{is_time_bound, InvalidParam, Query, TimeBounds};

/// Minimal ClickHouse HTTP read client.
#[derive(Clone)]
pub struct ClickHouseClient {
    base: String,
    client: reqwest::Client,
    /// what the last `system.columns` probe found, shared by every clone so the
    /// whole control plane asks once per window (#2903)
    columns: Arc<ColumnCache>,
    /// the same for `mcp_tool_call_logs`' optional columns (#3375)
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    pub(crate) mcp_columns: Arc<crate::request_log_columns::McpColumnCache>,
}

/// How long a connection to ClickHouse may take to open. A healthy server on
/// the same network answers in milliseconds, so three seconds already means it
/// is down or unreachable; failing fast keeps a dashboard request from waiting
/// on a host that will not answer. Matches the gateway's client (#2373).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Whole-request bound for every ClickHouse call the control plane makes: the
/// analytics and health reads, the alert signal reads, and the MCP and UX
/// ingest inserts.
///
/// Fifteen seconds is above what an interactive dashboard read over the
/// indexed `request_logs` window should ever need, yet short enough that the
/// operator gets an error rather than a spinner, and it stays under the 30s an
/// alert pass allows one signal query ([`crate::alerting`]). The gateway uses
/// ten seconds for its inserts; the control plane's inserts are small
/// single-batch posts of the same shape, so one bound serves them too rather
/// than a second client. The retention DDL runs under [`DDL_TIMEOUT`] since
/// `alter table ... modify ttl` is not an interactive read.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// How long an idle pooled connection to ClickHouse may be reused.
///
/// ClickHouse closes an idle keep-alive connection after its own
/// `keep_alive_timeout` (10 seconds by default, 3 on older releases), and
/// reqwest's default of 90 seconds kept handing that connection out regardless.
/// A request written onto a socket the server is closing at the same instant
/// never gets a response: hyper reports an incomplete message, and the UX and
/// MCP ingest handlers turn that into a 500 for a batch ClickHouse would have
/// accepted (#1940). Retiring connections well inside the server's window means
/// a reused connection is always one the server still holds open; a request
/// after a longer pause opens a fresh one, which on the same network costs a
/// millisecond.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(2);

// below the shortest keep_alive_timeout ClickHouse has shipped as a default
const _: () = assert!(POOL_IDLE_TIMEOUT.as_secs() < 3);

/// Bound for the log-retention `alter table` statements, which may rewrite
/// table metadata on a large table and are run by an admin, not a dashboard
/// poll.
const DDL_TIMEOUT: Duration = Duration::from_secs(60);

/// Build the one `reqwest` client every ClickHouse call goes through, so a
/// stalled server can never hold a request, an ingest handler or an alert pass
/// open (#1951).
fn build_http(connect: Duration, request: Duration, pool_idle: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect)
        .timeout(request)
        .pool_idle_timeout(pool_idle)
        .build()
        // building only fails when the TLS backend cannot initialise, which
        // `Client::new()` would hit as well; fall back to it rather than panic
        .unwrap_or_else(|_| reqwest::Client::new())
}

impl ClickHouseClient {
    pub fn new(url: &str) -> Self {
        Self::with_timeouts(url, CONNECT_TIMEOUT, REQUEST_TIMEOUT)
    }

    /// [`ClickHouseClient::new`] with explicit bounds, so a test can prove a
    /// stalled server is cut off without waiting out the production values.
    pub(crate) fn with_timeouts(url: &str, connect: Duration, request: Duration) -> Self {
        Self::with_pool_idle(url, connect, request, POOL_IDLE_TIMEOUT)
    }

    /// [`ClickHouseClient::with_timeouts`] with an explicit idle-connection
    /// bound, so a test can cross it without waiting out the production value.
    fn with_pool_idle(
        url: &str,
        connect: Duration,
        request: Duration,
        pool_idle: Duration,
    ) -> Self {
        Self {
            base: url.trim_end_matches('/').to_string(),
            client: build_http(connect, request, pool_idle),
            columns: Arc::new(ColumnCache::new()),
            mcp_columns: Arc::new(crate::request_log_columns::McpColumnCache::default()),
        }
    }

    /// The endpoint this client reads from. `/internal/snapshot` publishes it
    /// so the fleet writes where the dashboard reads (#929).
    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    /// Run `sql` (which must end with `FORMAT JSON`) with the given ClickHouse
    /// `param_*` bindings and return the parsed `data` array.
    pub(crate) async fn query(
        &self,
        sql: &str,
        params: &[(String, String)],
    ) -> anyhow::Result<Vec<Value>> {
        let mut req = self
            .client
            .post(format!("{}/", self.base))
            .query(&READ_SETTINGS);
        for (k, v) in params {
            req = req.query(&[(k.as_str(), v.as_str())]);
        }
        let resp = req.body(sql.to_string()).send().await.map_err(strip_url)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("clickhouse query failed ({status}): {body}");
        }
        let value: Value = resp.json().await.map_err(strip_url)?;
        Ok(value
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Which of the optional `request_logs` columns this server has, so a
    /// select can name the ones that exist and read a default for the rest
    /// instead of failing the whole statement (#2903).
    ///
    /// The answer is cached on the client (see [`crate::request_log_columns`])
    /// and shared by its clones. A server that cannot be reached is an error,
    /// since the select that follows would fail on the same connection; one that
    /// answers but cannot say, because it refuses `system.columns` or lists no
    /// column for the table, reads as "every column is there", which is what a
    /// select assumed before the probe existed.
    pub(crate) async fn request_log_columns(&self) -> anyhow::Result<Columns> {
        let now = Instant::now();
        if let Some(columns) = self.columns.fresh(now) {
            return Ok(columns);
        }
        let found = match self.query(PROBE_SQL, &[]).await {
            Ok(rows) => Columns::from_rows(&rows),
            Err(err) if is_transport(&err) => return Err(err),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "could not read the columns of request_logs; selecting every optional column"
                );
                None
            }
        };
        let columns = self.columns.store(now, found);
        if !columns.complete() {
            tracing::info!(
                missing = ?columns.missing(),
                "request_logs lacks columns the control plane can read; apply the clickhouse/ migrations to keep them"
            );
        }
        Ok(columns)
    }

    /// Forget what the probe found, so the next list probes again. A select
    /// that failed may have failed because the table changed under the answer.
    pub(crate) fn forget_request_log_columns(&self) {
        self.columns.forget();
    }

    /// Apply the admin-configured retention clocks to the log tables. Both
    /// bounds are integers validated by the caller and formatted into the DDL —
    /// ClickHouse does not accept query parameters in an `alter table` — so the
    /// only values that ever reach this SQL are in-range numbers (#537).
    // only reachable via the postgres-gated logging-settings router
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    pub(crate) async fn apply_log_retention(
        &self,
        retention_days: u32,
        payload_retention_hours: u32,
    ) -> anyhow::Result<()> {
        for statement in retention_statements(retention_days, payload_retention_hours) {
            let response = self
                .client
                .post(format!("{}/", self.base))
                .body(statement.clone())
                .timeout(DDL_TIMEOUT)
                .send()
                .await
                .map_err(strip_url)?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                anyhow::bail!("clickhouse retention change failed ({status}): {body}");
            }
        }
        Ok(())
    }

    /// Run one already-built statement that returns no rows — an
    /// `alter table … delete`, whose predicate cannot be a query parameter.
    /// Every value in it is a validated id or an escaped string; see
    /// [`crate::log_erasure`]. A mutation is queued, not finished, when this
    /// returns.
    // only reachable via the postgres-gated log-erasure router
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    pub(crate) async fn execute_ddl(&self, statement: &str) -> anyhow::Result<()> {
        let response = self
            .client
            .post(format!("{}/", self.base))
            .body(statement.to_string())
            .timeout(DDL_TIMEOUT)
            .send()
            .await
            .map_err(strip_url)?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("clickhouse statement failed ({status}): {body}")
    }

    /// Persist one already-sanitized MCP tool-call event. The table and insert
    /// statement are fixed here rather than supplied by a caller, so event
    /// metadata can never alter ClickHouse SQL.
    // only reachable via the postgres-gated mcp_logs router
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    pub(crate) async fn insert_mcp_tool_call(&self, event: &Value) -> anyhow::Result<()> {
        let response = self
            .client
            .post(format!(
                // best_effort so the row's own RFC 3339 `ts` is read into the
                // DateTime64(3) column instead of being rejected (#1210).
                // unknown fields are skipped, as the gateway's writer does, so
                // a row naming a column a hand-applied migration has not added
                // yet (`notification`, 047) is still stored (#3466)
                "{}/?query=INSERT%20INTO%20mcp_tool_call_logs%20FORMAT%20JSONEachRow\
                 &date_time_input_format=best_effort&input_format_skip_unknown_fields=1",
                self.base
            ))
            .body(serde_json::to_vec(event)?)
            .send()
            .await
            .map_err(strip_url)?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("clickhouse MCP event insert failed ({status}): {body}")
    }

    /// Persist a batch of already-validated dashboard UX events. Like the MCP
    /// insert above, the table and statement are fixed here rather than passed
    /// in, so event data can never reach ClickHouse as SQL.
    ///
    /// `JSONEachRow` is one object per line, which is why a batch costs one
    /// round trip rather than one per event.
    // only reachable via the postgres-gated ui-events router
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    pub(crate) async fn insert_ui_events(&self, rows: &[Value]) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut body = Vec::with_capacity(rows.len() * 1024);
        for row in rows {
            serde_json::to_writer(&mut body, row)?;
            body.push(b'\n');
        }
        let response = self
            .client
            .post(format!(
                // best_effort so the per-event RFC 3339 `ts` the browser
                // stamps, offset and all, parses into DateTime64 (#1224)
                "{}/?query=INSERT%20INTO%20ui_events%20FORMAT%20JSONEachRow&date_time_input_format=best_effort",
                self.base
            ))
            .body(body)
            .send()
            .await
            .map_err(strip_url)?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("clickhouse UX event insert failed ({status}): {text}")
    }
}

/// Drop the request URL from a transport error. `CLICKHOUSE_URL` can carry
/// userinfo, and reqwest's message quotes the URL, which would put the password
/// in the warn log and in an alert rule's stored `last_error`. The error keeps
/// its type, so `is_timeout()` and `is_connect()` still classify it.
fn strip_url(err: reqwest::Error) -> anyhow::Error {
    err.without_url().into()
}

/// Map a bucket name to a ClickHouse start-of-interval expression over `ts`.
/// Whitelisted so the returned string is safe to splice into SQL. The zone is
/// always explicit: without it the boundary is computed in the ClickHouse
/// server's own zone, so a day starts at `21:00Z` on a Moscow server (#2984).
/// `toStartOfWeek` takes the zone as its third argument, after the mode (0 is
/// Sunday, as before).
pub(crate) fn bucket_expr(bucket: &str) -> Option<&'static str> {
    match bucket {
        "hour" => Some("toStartOfHour(ts, 'UTC')"),
        "day" => Some("toStartOfDay(ts, 'UTC')"),
        "week" => Some("toStartOfWeek(ts, 0, 'UTC')"),
        "month" => Some("toStartOfMonth(ts, 'UTC')"),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
pub struct WindowQuery {
    /// inclusive lower bound (RFC3339); defaults to 7 days ago
    pub(crate) since: Option<String>,
    /// exclusive upper bound (RFC3339); defaults to now
    pub(crate) until: Option<String>,
    /// time bucket for the timeseries endpoint: hour|day|week|month
    pub(crate) bucket: Option<String>,
}

impl TimeBounds for WindowQuery {
    fn time_bounds(&self) -> (Option<&str>, Option<&str>) {
        (self.since.as_deref(), self.until.as_deref())
    }

    fn bucket(&self) -> Option<&str> {
        self.bucket.as_deref()
    }
}

impl WindowQuery {
    /// Refuse `bucket`, the one this read cuts the window by (the caller's or
    /// the handler's default), when it would answer more than
    /// [`crate::time_bounds::MAX_BUCKETS`] points (#3506). [`Query`] already
    /// checked a bucket the caller sent; this covers the default.
    pub(crate) fn check_buckets(&self, bucket: &str) -> Result<(), InvalidParam> {
        crate::time_bounds::check_buckets(
            self.since.as_deref(),
            self.until.as_deref(),
            bucket,
            chrono::Utc::now(),
        )
    }
}

/// Build the `param_*` bindings for the time window, applying defaults.
pub(crate) fn window_params(q: &WindowQuery) -> Vec<(String, String)> {
    vec![
        (
            "param_since".to_string(),
            q.since.clone().unwrap_or_default(),
        ),
        (
            "param_until".to_string(),
            q.until.clone().unwrap_or_default(),
        ),
    ]
}

/// Query params for the dashboard's summary, timeseries and by-model reads:
/// the shared time window plus the row filters the LLM Logs screen already
/// sends to the invocation list (#2453).
///
/// The four filters have exactly [`InvocationsQuery`]'s shapes: `model` and
/// `key` are one exact value, `business_unit` and `customer` a comma-separated
/// set, and an empty or omitted one applies no filter. They narrow what the
/// caller's row visibility (#1820) already lets through and never widen it,
/// because [`ROW_FILTERS`] is `and`-ed beside `ROW_VISIBLE` rather than
/// in place of it.
#[derive(Debug, Deserialize)]
pub struct DashboardQuery {
    #[serde(flatten)]
    pub(crate) window: WindowQuery,
    /// exact model name to filter to; empty/omitted means all models
    pub(crate) model: Option<String>,
    /// exact virtual key id to filter to; empty/omitted means all keys
    pub(crate) key: Option<String>,
    /// comma-separated business unit ids to filter to; empty/omitted means
    /// every unit, and an id nothing was attributed to matches nothing
    pub(crate) business_unit: Option<String>,
    /// comma-separated customer ids to filter to; empty/omitted means every
    /// customer
    pub(crate) customer: Option<String>,
    /// exact project id to filter to; empty/omitted means every project the
    /// caller may read (#3090)
    pub(crate) project: Option<String>,
}

impl TimeBounds for DashboardQuery {
    fn time_bounds(&self) -> (Option<&str>, Option<&str>) {
        self.window.time_bounds()
    }

    fn bucket(&self) -> Option<&str> {
        self.window.bucket()
    }
}

impl DashboardQuery {
    /// The window bindings plus one `param_*` per filter [`ROW_FILTERS`]
    /// reads. Every filter is bound, an absent one as the empty string the
    /// predicate short-circuits on, so the SQL never changes shape with the
    /// filters a caller happened to send.
    pub(crate) fn params(&self) -> Vec<(String, String)> {
        let mut params = window_params(&self.window);
        params.extend(row_filter_params(
            self.model.as_deref(),
            self.key.as_deref(),
            self.business_unit.as_deref(),
            self.customer.as_deref(),
            self.project.as_deref(),
        ));
        params
    }
}

/// The `param_*` bindings [`ROW_FILTERS`] reads, for any read that carries the
/// five row filters. Every filter is bound, an absent one as the empty string
/// the predicate short-circuits on, so the SQL never changes shape with the
/// filters a caller happened to send.
pub(crate) fn row_filter_params(
    model: Option<&str>,
    key: Option<&str>,
    business_unit: Option<&str>,
    customer: Option<&str>,
    project: Option<&str>,
) -> [(String, String); 5] {
    [
        ("param_model", model),
        ("param_key", key),
        ("param_business_unit", business_unit),
        ("param_customer", customer),
        ("param_project", project),
    ]
    .map(|(name, value)| (name.to_string(), value.unwrap_or_default().to_string()))
}

/// The row filters shared by the dashboard rollups and the invocation list,
/// every value a bound ClickHouse parameter and never spliced text. One
/// predicate, so a filter means the same rows on the Dashboard as on LLM Logs
/// (#2453, #2726).
pub(crate) const ROW_FILTERS: &str = "({model:String} = '' or model = {model:String}) \
     and ({key:String} = '' or virtual_key_id = {key:String}) \
     and ({business_unit:String} = '' \
          or has(splitByChar(',', {business_unit:String}), business_unit_id)) \
     and ({customer:String} = '' \
          or has(splitByChar(',', {customer:String}), customer_id)) \
     and ({project:String} = '' or project_id = {project:String})";

/// `params` plus the caller's scope filter, which every analytics and health
/// query binds (#1820).
pub(crate) fn with_access(
    mut params: Vec<(String, String)>,
    access: &AnalyticsAccess,
) -> Vec<(String, String)> {
    params.extend(access.params());
    params
}

/// The shared `where` clause. Empty since/until fall back to a default range so
/// callers can omit either bound.
///
/// The parse is the `OrZero` variant on purpose: ClickHouse constant-folds both
/// branches of `if` over constant arguments, so the strict
/// `parseDateTime64BestEffort('')` throws "Cannot read DateTime" before the
/// empty-string branch is ever selected (#1177). With `OrZero` the failed parse
/// yields an unused epoch value and the `if` still picks the default bound.
///
/// Both parses pass scale `3` and the zone `'UTC'`, so a zone-less bound
/// (`2026-08-06`) means UTC midnight whatever zone the ClickHouse server runs
/// in; a bound with `Z` or an offset keeps its own instant (#2984).
pub(crate) const WHERE_WINDOW: &str = "ts >= if({since:String} = '', now64(3) - interval 7 day, parseDateTime64BestEffortOrZero({since:String}, 3, 'UTC')) \
     and ts < if({until:String} = '', now64(3), parseDateTime64BestEffortOrZero({until:String}, 3, 'UTC'))";

/// The key a captured body is joined to its log row on, evaluated against
/// whichever table the clause sits in; both carry the same columns it reads.
///
/// A keyed row (`log_id` set) is `log_id|org_id|project_id`: the gateway-minted
/// id, plus the tenancy the gateway copied onto the payload so a body can never
/// meet a row of another project even if an id were reused. An unkeyed row
/// predates `log_id` and keeps the `request_id|ts` pair. The two shapes cannot
/// equal each other (a UUID holds no `|`), so a keyed row never falls back.
const PAYLOAD_KEY: &str = "if(log_id != '', concat(log_id, '|', org_id, '|', project_id), \
     concat(request_id, '|', toString(ts)))";

pub fn router() -> Router<crate::ControlState> {
    Router::new()
        .route("/api/v1/analytics/summary", get(summary))
        .route("/api/v1/analytics/timeseries", get(timeseries))
        .route("/api/v1/analytics/by-model", get(by_model))
        .route("/api/v1/analytics/by-variant", get(by_variant))
        .route("/api/v1/analytics/by-attribution", get(by_attribution))
        .route("/api/v1/analytics/invocations", get(invocations))
        .route("/api/v1/analytics/guardrails", get(guardrails))
        .route("/api/v1/analytics/plugins", get(plugins))
        .merge(crate::analytics_export::router())
        .route(
            "/api/v1/analytics/param-adjustments",
            get(param_adjustments),
        )
}

/// Map a cost-attribution dimension name to its whitelisted column. Anything
/// else is rejected so it can never be spliced into SQL.
pub(crate) fn attribution_column(dimension: &str) -> Option<&'static str> {
    match dimension {
        "business_unit" => Some("business_unit_id"),
        "customer" => Some("customer_id"),
        _ => None,
    }
}

/// What the dashboard calls an error: a status the caller received that is a
/// client or server failure (`>= 400`), or `0`, the row of a transport failure
/// or an exhausted retry that never got a status (#1995, #3294). One spelling
/// for the LLM Logs `error` filter ([`status_predicate`]) and the error totals
/// ([`ERRORS`]), so the two screens cannot disagree about how many requests
/// failed. Parenthesised because it is spliced after `and`.
pub(crate) const ERROR_STATUS: &str = "(status = 0 or status >= 400)";

/// Map a status filter name to a whitelisted SQL predicate. Anything else is
/// rejected so it can never be spliced into SQL. `all` applies no filter.
///
/// `error` includes status `0`, the row of a transport failure or an exhausted
/// retry that never got an upstream status: the dashboard paints it as `ERR`,
/// and an "errors" filter that dropped it hid the failures an operator most
/// needs to see (#1995). The predicate is spliced after `and`, so the `or` is
/// parenthesised.
pub(crate) fn status_predicate(status: &str) -> Option<&'static str> {
    match status {
        "all" => Some("1"),
        "error" => Some(ERROR_STATUS),
        "success" => Some("status > 0 and status < 400"),
        _ => None,
    }
}

/// Clamp a page size into `[1, 200]`, defaulting to 50.
pub(crate) fn clamp_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(50).clamp(1, 200)
}

/// Decode an opaque `timestamp|id` keyset cursor into the two values its
/// predicate binds.
///
/// An absent or empty cursor decodes to a pair of empty strings, which
/// [`keyset_predicate`] short-circuits on: the first page asks for no bound at
/// all rather than for a bound that happens to sort before every row.
///
/// `id_label` only names the id half in the rejection message, so each list
/// tells the caller which column its cursor is over.
///
/// The timestamp half has to be one [`is_time_bound`] accepts. It reaches the
/// same `OrZero` parse as a window bound, so a cursor edited by hand used to
/// resume from the epoch and read as the end of the list (#1192).
#[cfg(any(test, feature = "postgres"))]
pub(crate) fn parse_keyset_cursor(
    cursor: Option<&str>,
    id_label: &str,
) -> Result<(String, String), String> {
    parse_keyset_cursor_within(cursor, id_label, MAX_CURSOR_ID_LEN)
}

/// The longest id half a keyset cursor may carry.
const MAX_CURSOR_ID_LEN: usize = 256;

/// [`parse_keyset_cursor`] with its own bound on the id half, for a list whose
/// id half also holds something besides the id.
pub(crate) fn parse_keyset_cursor_within(
    cursor: Option<&str>,
    id_label: &str,
    max_id_len: usize,
) -> Result<(String, String), String> {
    let Some(cursor) = cursor.filter(|value| !value.is_empty()) else {
        return Ok((String::new(), String::new()));
    };
    let malformed = || format!("cursor must be timestamp|{id_label}");
    let Some((timestamp, id)) = cursor.split_once('|') else {
        return Err(malformed());
    };
    if timestamp.is_empty() || id.is_empty() || timestamp.len() > 64 || id.len() > max_id_len {
        return Err(malformed());
    }
    if !is_time_bound(timestamp) {
        return Err(format!(
            "cursor must be timestamp|{id_label}, and its timestamp does not parse; \
             send the previous page's next_cursor back unchanged"
        ));
    }
    Ok((timestamp.to_string(), id.to_string()))
}

/// The `where` fragment that resumes a page ordered `ts desc, <id_column> desc`
/// after the row the cursor names, binding `{cursor_ts}` and `{cursor_id}`.
///
/// A keyset bound is what makes paging a table that is still being written
/// lossless: an offset counts rows from the top, so every row ingested between
/// two pages shifts the window and pushes a row the caller already saw onto the
/// next page — or hides one it has not. The bound is a position in the sort
/// key instead, which newer rows cannot move.
///
/// The parse is the `OrZero` variant on purpose: ClickHouse evaluates the parse
/// of the bound constant even though the `= ''` disjunct already short-circuits
/// the predicate, so a strict parse failed every first page with "Cannot read
/// DateTime" (#1177).
#[cfg(feature = "postgres")]
pub(crate) fn keyset_predicate(id_column: &str) -> String {
    format!(
        "({{cursor_ts:String}} = '' or ts < parseDateTime64BestEffortOrZero({{cursor_ts:String}}) \
              or (ts = parseDateTime64BestEffortOrZero({{cursor_ts:String}}) \
                  and {id_column} < {{cursor_id:String}}))"
    )
}

/// The cursor that resumes paging after the last row of `rows`, or `None` when
/// the page came back empty and there is nothing left to resume from.
#[cfg(any(test, feature = "postgres"))]
pub(crate) fn next_keyset_cursor(rows: &[Value], id_column: &str) -> Option<String> {
    let row = rows.last()?;
    Some(format!(
        "{}|{}",
        row.get("ts")?.as_str()?,
        row.get(id_column)?.as_str()?
    ))
}

/// The `where` fragment that resumes the invocation list after the row its
/// cursor names. The list orders by `(ts desc, seq desc, request_id desc)`:
/// `seq` is the gateway's start sequence (#1392), which orders requests that
/// began inside one millisecond by when they began, and `request_id` still
/// makes the key total for rows with the same `seq` (every row written before
/// the column, and those of two gateways that happened to agree). Binds
/// `{cursor_ts}`, `{cursor_seq}` and `{cursor_id}`; `seq` is the column, or its
/// default on a table that lacks it.
pub(crate) fn invocations_keyset_predicate(seq: &str) -> String {
    format!(
        "({{cursor_ts:String}} = '' or ts < parseDateTime64BestEffortOrZero({{cursor_ts:String}}) \
              or (ts = parseDateTime64BestEffortOrZero({{cursor_ts:String}}) \
                  and ({seq} < {{cursor_seq:UInt64}} \
                       or ({seq} = {{cursor_seq:UInt64}} and request_id < {{cursor_id:String}}))))"
    )
}

/// `<seq>|` at its longest: twenty digits and the bar.
const MAX_CURSOR_SEQ_LEN: usize = 21;

/// The `seq` a cursor from before the sequence existed resumes from: past every
/// row of its millisecond, so such a cursor can repeat rows of that one
/// millisecond but never skip one.
const LEGACY_CURSOR_SEQ: &str = "18446744073709551615";

/// A cursor's id half is `<seq>|<request_id>`. One without the number is a
/// `timestamp|request_id` cursor handed out before the sequence, still valid.
pub(crate) fn split_invocations_cursor(id: &str) -> (String, String) {
    match id.split_once('|') {
        Some((seq, request_id))
            if !seq.is_empty()
                && seq.len() <= 20
                && seq.bytes().all(|b| b.is_ascii_digit())
                && seq.parse::<u64>().is_ok() =>
        {
            (seq.to_string(), request_id.to_string())
        }
        _ => (LEGACY_CURSOR_SEQ.to_string(), id.to_string()),
    }
}

/// The cursor that resumes the invocation list after the last of `rows`:
/// `<ts>|<seq>|<request_id>`.
pub(crate) fn next_invocations_cursor(rows: &[Value]) -> Option<String> {
    let row = rows.last()?;
    // ClickHouse quotes a 64-bit integer in JSON output
    let seq = match row.get("seq")? {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    Some(format!(
        "{}|{}|{}",
        row.get("ts")?.as_str()?,
        seq,
        row.get("request_id")?.as_str()?
    ))
}

/// The settings every read carries. `date_time_output_format=iso` makes a
/// `DateTime64` leave as `2026-08-06T10:00:00.123Z`: the instant in UTC with
/// its zone. The default prints the wall clock of the server's own zone with
/// none (`2026-08-06 13:00:00.123`), which the dashboard's `new Date(...)`
/// reads as browser-local time (#2255). ClickHouse converts to UTC itself, so
/// the output is right whatever zone the server runs in.
const READ_SETTINGS: [(&str, &str); 2] = [
    ("default_format", "JSON"),
    ("date_time_output_format", "iso"),
];

#[allow(clippy::result_large_err)]
pub(crate) fn client_or_503(state: &crate::ControlState) -> Result<&ClickHouseClient, Response> {
    state.clickhouse.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": {"message": "analytics unavailable: no clickhouse_url configured"}})),
        )
            .into_response()
    })
}

pub(crate) fn run(rows: anyhow::Result<Vec<Value>>) -> Response {
    match rows {
        Ok(data) => Json(json!({ "data": data })).into_response(),
        Err(err) => query_failed("analytics query failed", &err),
    }
}

/// The response for a ClickHouse query that did not come back.
///
/// The database's own message carries the full SQL, the table layout and its
/// host, and it used to be echoed verbatim into the dashboard (#1221). The
/// operator gets a stable sentence plus a `code` the UI can key on; the detail
/// goes to the control-plane log, which is where a query failure is debugged.
///
/// A read that ran past the client's request timeout answers `504` with
/// `analytics_query_timeout` instead, so the dashboard can say the query was
/// slow (narrow the window) rather than that ClickHouse is down.
pub(crate) fn query_failed(what: &'static str, err: &anyhow::Error) -> Response {
    if is_timeout(err) {
        tracing::warn!(
            error = %rolter_core::redact::redact_urls_in_text(&format!("{err:#}")),
            "{what}: timed out"
        );
        return (
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({"error": {
                "message": "analytics query timed out; narrow the time window",
                "code": "analytics_query_timeout"
            }})),
        )
            .into_response();
    }
    tracing::warn!(
        error = %rolter_core::redact::redact_urls_in_text(&format!("{err:#}")),
        "{what}"
    );
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"error": {"message": what, "code": "analytics_query_failed"}})),
    )
        .into_response()
}

/// Whether the request never got an answer from ClickHouse: a refused or
/// timed-out connection, as opposed to a status line carrying its error.
pub(crate) fn is_transport(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<reqwest::Error>().is_some())
}

/// Whether any error in the chain is a `reqwest` timeout, however much context
/// a caller wrapped around it.
fn is_timeout(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_timeout)
    })
}

/// Totals over the window: request count, tokens, cost, error count, response-cache
/// hits, and the mean and 95th-percentile latency.
/// Totals for the window, plus how much of the window is **unpriced**.
///
/// `cost_usd` alone cannot say whether it is a complete figure: traffic against
/// a model with no price row contributes zero, which is indistinguishable from
/// traffic that genuinely cost nothing. `unpriced_requests` and
/// `unpriced_models` are what let a caller present a partial total as partial
/// rather than as final (#969).
async fn summary(
    access: AnalyticsAccess,
    State(state): State<crate::ControlState>,
    Query(q): Query<DashboardQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let columns = match ch.request_log_columns().await {
        Ok(columns) => columns,
        Err(error) => return query_failed("analytics query failed", &error),
    };
    let sql = |cache_measured: &str| {
        format!(
            "select {REQUESTS} as requests, \
                {TOKENS} as tokens, \
                {PROMPT_TOKENS} as prompt_tokens, \
                {COMPLETION_TOKENS} as completion_tokens, \
                {CACHE_READ_TOKENS} as cache_read_tokens, \
                {cache_measured}\
                {COST} as cost_usd, \
                {UNPRICED} as unpriced_requests, \
                uniqIf(model, unpriced = 1) as unpriced_models, \
                {WITHHELD} as withheld_requests, \
                {WITHHELD_COST} as withheld_cost, \
                {USAGE_UNKNOWN} as usage_unknown_requests, \
                {CACHE_HITS} as cache_hit_requests, \
                {ERRORS} as errors, \
                {AVG_LATENCY} as avg_latency_ms, \
                {P95_LATENCY} as p95_latency_ms \
         from request_logs where {WHERE_WINDOW} and {ROW_VISIBLE} and {ROW_FILTERS} \
         format JSON"
        )
    };
    let params = with_access(q.params(), &access);
    let cache_measured = cache_measured_terms(&columns);
    let mut result = ch.query(&sql(&cache_measured), &params).await;
    if result.is_err() {
        // the table may have changed under the probe's answer
        ch.forget_request_log_columns();
        // and a probe that could not read `system.columns` assumes every
        // column is there: the home summary then answers without the
        // prompt-cache measurement, as an older control plane does, rather
        // than failing over a column the table lacks (#3410)
        if !cache_measured.is_empty() {
            result = ch.query(&sql(""), &params).await;
        }
    }
    run(result)
}

/// The summary's prompt-cache measurement (#3410): the prompt tokens of the
/// requests whose upstream stated its cache reads, the denominator of the read
/// share, and how many requests with a prompt it could not measure. Empty when
/// `request_logs` lacks `cache_usage_reported`
/// (`clickhouse/045_cache_usage_reported.sql`): every row would read as
/// unmeasured, so the two fields are left out and the dashboard keeps the share
/// over every prompt token, as an older control plane does.
fn cache_measured_terms(columns: &Columns) -> String {
    if !columns.has("cache_usage_reported") {
        return String::new();
    }
    format!(
        "{CACHE_REPORTED_PROMPT_TOKENS} as cache_reported_prompt_tokens, \
         {CACHE_UNMEASURED} as cache_unmeasured_requests, "
    )
}

/// The 95th latency percentile, weighted by `sample_weight` (#3355): each kept row
/// stands for that many real requests, so a log whose sample rate changed
/// inside the window, or a route sampled at another rate than the rest, does not
/// over-represent its lightly sampled traffic.
///
/// `quantileExactWeighted` and not `quantileWeighted` or
/// `quantileTimingWeighted`: ClickHouse's weighted quantiles take an unsigned
/// integer weight, and `sample_weight` is a `Float64` (`1 / sample_rate`), so it
/// is scaled to thousandths and rounded, a constant factor that moves no
/// quantile and keeps three digits of a weight like `3.333`. The timing variant
/// is out because it clamps every value over 30 000 ms, and a slow completion
/// legitimately runs longer. The exact variant keeps one counter per distinct
/// latency, which a column of millisecond integers bounds. The result is
/// rounded like the other latency figures, and is `0` for no rows.
pub(crate) const P95_LATENCY: &str =
    "round(quantileExactWeighted(0.95)(latency_ms, toUInt64(round(sample_weight * 1000))), 1)";
/// The mean latency, weighted by `sample_weight` like the percentiles (#3420):
/// `sum(latency * weight) / sum(weight)`, so a kept row counts as the requests
/// it stands for and unequal weights (a sample rate that changed inside the
/// window, a route sampled apart from the rest) do not skew it. With every
/// weight equal it is the plain mean. `0` for no rows, where the quotient has
/// no denominator, the way the percentiles read.
pub(crate) const AVG_LATENCY: &str = "if(sum(sample_weight) > 0, \
     round(sum(latency_ms * sample_weight) / sum(sample_weight), 1), 0)";
/// The median counterpart of [`P95_LATENCY`].
pub(crate) const P50_LATENCY: &str =
    "round(quantileExactWeighted(0.5)(latency_ms, toUInt64(round(sample_weight * 1000))), 1)";

// Request-log sampling keeps 1 row in `1 / sample_rate`, and each stored row
// carries `sample_weight` (clickhouse/014) = how many real requests it stands
// for. Counts and sums are scaled by it so a sampled log does not under-report
// traffic or spend (#2239). The mean and the percentiles are weighted too
// (#3420, #3355), see `AVG_LATENCY` and `P95_LATENCY`. Rows written unsampled, or before the column existed,
// weigh 1.
pub(crate) const REQUESTS: &str = "round(sum(sample_weight))";
pub(crate) const TOKENS: &str = "round(sum(total_tokens * sample_weight))";
pub(crate) const PROMPT_TOKENS: &str = "round(sum(prompt_tokens * sample_weight))";
pub(crate) const COMPLETION_TOKENS: &str = "round(sum(completion_tokens * sample_weight))";
/// Prompt tokens the provider served from its own prompt cache (#3356), the
/// numerator of a read share over `prompt_tokens`. This is the provider's
/// cache, not rolter's response cache (`cache_hit`). The column is qualified
/// because the summary also selects this sum as `cache_read_tokens`, and
/// ClickHouse resolves a bare name to the select alias before the column.
pub(crate) const CACHE_READ_TOKENS: &str =
    "round(sum(request_logs.cache_read_tokens * sample_weight))";
/// The prompt tokens of the requests whose upstream stated its prompt-cache
/// reads, zero included (#3410): the read share's denominator, so a provider
/// that does not report them does not dilute it. Qualified for the same reason
/// as [`CACHE_READ_TOKENS`].
pub(crate) const CACHE_REPORTED_PROMPT_TOKENS: &str =
    "round(sumIf(request_logs.prompt_tokens * sample_weight, cache_usage_reported = 1))";
/// Requests with a prompt whose upstream did not state its prompt-cache reads,
/// so the read share leaves them out (#3410).
pub(crate) const CACHE_UNMEASURED: &str =
    "round(sumIf(sample_weight, cache_usage_reported = 0 and request_logs.prompt_tokens > 0))";
pub(crate) const COST: &str = "round(sum(cost_usd * sample_weight), 6)";
pub(crate) const UNPRICED: &str = "round(sumIf(sample_weight, unpriced = 1))";
/// Requests whose provider answered, and billed, but whose response a post-call
/// policy refused to deliver (#1478). Their cost is already inside `cost_usd`;
/// the figures below say how much of it bought an answer nobody saw (#1774).
pub(crate) const WITHHELD: &str = "round(sumIf(sample_weight, withheld = 1))";
/// `cost_usd` is qualified because the reads that use this also select `{COST}
/// as cost_usd`, and ClickHouse resolves a bare name to a select alias before a
/// column: unqualified, it would nest that sum inside this one and the query
/// would fail.
pub(crate) const WITHHELD_COST: &str =
    "round(sumIf(request_logs.cost_usd * sample_weight, withheld = 1), 6)";
/// Requests the upstream answered successfully but reported no usage for, so
/// their zero tokens and cost are unknown rather than free (#1478).
pub(crate) const USAGE_UNKNOWN: &str = "round(sumIf(sample_weight, usage_unknown = 1))";
/// Requests the gateway answered from a cache (#1978): the numerator of the
/// dashboard's cache-hit rate, over `requests`. `cache_hit` is not a select
/// alias in any read that uses this, so it needs no table qualifier.
pub(crate) const CACHE_HITS: &str = "round(sumIf(sample_weight, cache_hit = 1))";
/// Requests that ended in an error, by the same rule as the LLM Logs `error`
/// filter ([`ERROR_STATUS`]): status `0` counts, so a request that never got an
/// answer is in the error totals and rates (#3294).
pub(crate) const ERRORS: &str = "round(sumIf(sample_weight, (status = 0 or status >= 400)))";

/// Per-bucket time series of requests, tokens and cost.
async fn timeseries(
    access: AnalyticsAccess,
    State(state): State<crate::ControlState>,
    Query(q): Query<DashboardQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let bucket = q.window.bucket.as_deref().unwrap_or("day");
    let Some(bucket_expr) = bucket_expr(bucket) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "bucket must be one of hour|day|week|month"}})),
        )
            .into_response();
    };
    if let Err(refused) = q.window.check_buckets(bucket) {
        return refused.into_response();
    }
    let sql = format!(
        "select {bucket_expr} as bucket, \
                {REQUESTS} as requests, \
                {TOKENS} as tokens, \
                {COST} as cost_usd \
         from request_logs where {WHERE_WINDOW} and {ROW_VISIBLE} and {ROW_FILTERS} \
         group by bucket order by bucket format JSON"
    );
    run(ch.query(&sql, &with_access(q.params(), &access)).await)
}

/// Per-model aggregates: requests, tokens, cost, error rate, latency percentiles.
///
/// A row is one organization's model, `(org_id, model)` (#3486): since #1857
/// two organizations may each have a route named `gpt-4o`, and a caller who
/// reads both (a superadmin, a member of two) gets them as two rows rather
/// than one whose figures are summed across tenants.
async fn by_model(
    access: AnalyticsAccess,
    State(state): State<crate::ControlState>,
    Query(q): Query<DashboardQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    run(ch
        .query(&by_model_sql(), &with_access(q.params(), &access))
        .await)
}

/// The select behind [`by_model`].
pub(crate) fn by_model_sql() -> String {
    format!(
        "select org_id, model, \
                {REQUESTS} as requests, \
                {TOKENS} as tokens, \
                {COST} as cost_usd, \
                {UNPRICED} as unpriced_requests, \
                {WITHHELD} as withheld_requests, \
                {WITHHELD_COST} as withheld_cost, \
                {USAGE_UNKNOWN} as usage_unknown_requests, \
                {ERRORS} as errors, \
                {P50_LATENCY} as p50_latency_ms, \
                {P95_LATENCY} as p95_latency_ms \
         from request_logs where {WHERE_WINDOW} and {ROW_VISIBLE} and {ROW_FILTERS} \
         group by org_id, model order by cost_usd desc, org_id, model format JSON"
    )
}

/// Per-variant aggregates (#3342): what each weighted variant of a route
/// served, so an operator running an A/B split or a canary can compare the
/// observed share, errors, latency and cost with the configured split.
///
/// A variant name means something only inside its route, so rows are keyed by
/// `(org_id, model, variant)` and a caller reading one route passes `model`.
/// The organization is part of the key for [`by_model`]'s reason (#3548):
/// since #1857 two organizations may each have a route of one name, and a
/// caller who reads both gets each organization's variants as their own rows. Rows the
/// gateway served without a variant (the classic single pool, or traffic from
/// before the route had variants) come back under the empty string, so a
/// route's shares add up to its whole traffic. Everything else is
/// [`by_model`]'s: the same window, row filters and `AnalyticsAccess` scoping.
async fn by_variant(
    access: AnalyticsAccess,
    State(state): State<crate::ControlState>,
    Query(q): Query<DashboardQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    // `fallback_reason` (`023`) is applied by hand, so whether the table has it
    // decides what the first-attempt count reads
    let columns = match ch.request_log_columns().await {
        Ok(columns) => columns,
        Err(error) => return query_failed("analytics query failed", &error),
    };
    let sql = by_variant_sql(&columns);
    match ch.query(&sql, &with_access(q.params(), &access)).await {
        Ok(data) => Json(json!({"data": data})).into_response(),
        Err(error) => {
            // the table may have changed under the cached answer: look again next time
            ch.forget_request_log_columns();
            query_failed("analytics query failed", &error)
        }
    }
}

/// The requests a variant served on the first attempt (#3409): those the log
/// records with no `fallback_reason`, which is empty when the first dial
/// answered. Counted beside `requests`, which is every request the variant
/// answered, so the share a variant took in by-variant can be told apart from
/// the share the router gave it: a variant others fail over to reads higher
/// than its weight in `requests` and at its weight here. Weighted by
/// `sample_weight` like every count. The column is qualified with the table.
pub(crate) const FIRST_ATTEMPT_REQUESTS: &str =
    "round(sumIf(sample_weight, request_logs.fallback_reason = ''))";

/// The variant a row counts for in the assigned count (#3505): the one the
/// gateway first dialled (`first_variant`, `clickhouse/048`), or, for a row
/// that records none, the one that answered it. Which of those rows count at
/// all is [`assigned_rows`]'s rule.
const ASSIGNED_VARIANT: &str =
    "if(request_logs.first_variant != '', request_logs.first_variant, request_logs.variant)";

/// The rows the assigned count reads, for a table that has (or lacks) the
/// `fallback_reason` column.
///
/// A row that names its first-dialled variant counts for that variant. A row
/// served without a variant (the classic pool) has nothing to be reassigned
/// from and counts under the empty string. A variant row without
/// `first_variant` was written before `048` (or by an older gateway): it
/// counts for the variant that answered it only when it records no
/// `fallback_reason`, since a request that failed over could have been
/// assigned to any variant, and is otherwise left out rather than guessed. On
/// a table without `fallback_reason` no such row can be placed.
fn assigned_rows(columns: &Columns) -> &'static str {
    if columns.has("fallback_reason") {
        "(request_logs.first_variant != '' or request_logs.variant = '' \
          or request_logs.fallback_reason = '')"
    } else {
        "(request_logs.first_variant != '' or request_logs.variant = '')"
    }
}

/// The variant a row counts for in the sampled count (#3547): the one the
/// weighted split sampled as its primary (`sampled_variant`, `clickhouse/050`).
/// A row served without a variant (the classic pool) counts under the empty
/// string; a variant row that records no sampled variant was written before
/// `050` (or by an older gateway), and the sample it was given is not known,
/// so it is left out rather than guessed.
const SAMPLED_ROWS: &str = "(request_logs.sampled_variant != '' or request_logs.variant = '')";

/// The variant a row counts for in the sampled count (#3547). Parenthesised
/// so the literal is not a bare dotted name, which the dashboard's audit
/// vocabulary scan (`ui/scripts/audit-vocabulary-source.ts`) reads as an
/// audit action.
const SAMPLED_VARIANT: &str = "(request_logs.sampled_variant)";

/// The null a count reads as on a table that cannot make it.
const UNKNOWN_COUNT: &str = "CAST(NULL AS Nullable(Float64))";

/// The by-variant rollup's select, for a table that has (or lacks) the
/// `fallback_reason`, `first_variant` and `sampled_variant` columns.
///
/// A table without `fallback_reason` cannot say which requests failed over,
/// and reading every row as a first attempt would claim they all were, so
/// `first_attempt_requests` is `null` there and the dashboard states that the
/// split is unknown. A row logged before the column was applied reads as a
/// first attempt on a table that has it, the same default the invocation list
/// gives such a row.
///
/// `assigned_requests` (#3505) counts the requests each variant was dialled to
/// first, grouped by that variant rather than the one that answered, so a
/// variant that only ever failed over still has a row (with no answered
/// requests) and its assigned share. `sampled_requests` (#3547) counts the
/// requests the weighted split sampled each variant for, before the gateway
/// skipped a parked, unhealthy or open-breaker one, so a variant the gateway
/// routes around keeps its sampled share while its assigned share falls. Each
/// is a half over the same window and filters, grouped by its own variant,
/// beside the answered half grouped by the answering one; the halves are
/// summed per `(org_id, model, variant)`. Each half holds a key at most once,
/// so a sum (and a `max` for the percentiles) adds a figure to zero and is
/// exact. A count is `null` on a table without its column, and its half is
/// left out.
pub(crate) fn by_variant_sql(columns: &Columns) -> String {
    let has_first_attempt = columns.has("fallback_reason");
    let has_assigned = columns.has("first_variant");
    let has_sampled = columns.has("sampled_variant");
    let first_attempt = if has_first_attempt {
        FIRST_ATTEMPT_REQUESTS
    } else {
        UNKNOWN_COUNT
    };
    // without its column a count is not claimed; with it, every half but its
    // own contributes nothing to it
    let zero_unless = |known: bool| if known { "toFloat64(0)" } else { UNKNOWN_COUNT };
    let (assigned_zero, sampled_zero) = (zero_unless(has_assigned), zero_unless(has_sampled));
    // `org_id`, `model` and `variant` are selected under their own names, so
    // all three are qualified with the table: ClickHouse resolves a bare name
    // to the alias before the column. the sums alias other names, which carry
    // their own qualifiers where they need them
    let answered = format!(
        "select request_logs.org_id as org_id, request_logs.model as model, \
                request_logs.variant as variant, \
                {REQUESTS} as requests, \
                {first_attempt} as first_attempt_requests, \
                {assigned_zero} as assigned_requests, \
                {sampled_zero} as sampled_requests, \
                {TOKENS} as tokens, \
                {COST} as cost_usd, \
                {UNPRICED} as unpriced_requests, \
                {ERRORS} as errors, \
                {P50_LATENCY} as p50_latency_ms, \
                {P95_LATENCY} as p95_latency_ms \
         from request_logs where {WHERE_WINDOW} and {ROW_VISIBLE} and {ROW_FILTERS} \
         group by request_logs.org_id, request_logs.model, request_logs.variant"
    );
    // a union matches its halves by position and takes the first one's names,
    // so the other halves' columns carry names no column of `request_logs`
    // has (an alias such as `cost_usd` would shadow the column). typed as the
    // answered half's figures, or as their common supertype
    let first_attempt_zero = zero_unless(has_first_attempt);
    let half = |variant: &str, rows: &str, assigned: &str, sampled: &str| {
        format!(
            " union all \
             select request_logs.org_id as org_id, request_logs.model as model, \
                    {variant} as counted_variant, \
                    toFloat64(0) as zero_requests, {first_attempt_zero} as zero_first_attempts, \
                    {assigned} as counted_assigned, {sampled} as counted_sampled, \
                    toFloat64(0) as zero_tokens, \
                    toFloat64(0) as zero_cost, toFloat64(0) as zero_unpriced, \
                    toFloat64(0) as zero_errors, toFloat64(0) as zero_p50, \
                    toFloat64(0) as zero_p95 \
             from request_logs \
             where {WHERE_WINDOW} and {ROW_VISIBLE} and {ROW_FILTERS} and {rows} \
             group by request_logs.org_id, request_logs.model, {variant}"
        )
    };
    let mut halves = String::new();
    if has_assigned {
        halves.push_str(&half(
            ASSIGNED_VARIANT,
            assigned_rows(columns),
            REQUESTS,
            sampled_zero,
        ));
    }
    if has_sampled {
        halves.push_str(&half(
            SAMPLED_VARIANT,
            SAMPLED_ROWS,
            assigned_zero,
            REQUESTS,
        ));
    }
    if halves.is_empty() {
        return format!("{answered} order by requests desc, org_id, model, variant format JSON");
    }
    // the outer sums read the union through its alias, since a bare
    // `requests` inside `sum(...) as requests` would name the sum itself
    format!(
        "select by_role.org_id as org_id, by_role.model as model, \
                by_role.variant as variant, \
                sum(by_role.requests) as requests, \
                sum(by_role.first_attempt_requests) as first_attempt_requests, \
                sum(by_role.assigned_requests) as assigned_requests, \
                sum(by_role.sampled_requests) as sampled_requests, \
                sum(by_role.tokens) as tokens, \
                sum(by_role.cost_usd) as cost_usd, \
                sum(by_role.unpriced_requests) as unpriced_requests, \
                sum(by_role.errors) as errors, \
                max(by_role.p50_latency_ms) as p50_latency_ms, \
                max(by_role.p95_latency_ms) as p95_latency_ms \
         from ({answered}{halves}) as by_role \
         group by by_role.org_id, by_role.model, by_role.variant \
         order by requests desc, org_id, model, variant format JSON"
    )
}

/// Query params for the cost-attribution rollup: the shared time window plus
/// the governance dimension to group by.
#[derive(Debug, Deserialize)]
pub struct AttributionQuery {
    pub(crate) since: Option<String>,
    pub(crate) until: Option<String>,
    /// business_unit|customer (defaults to business_unit)
    pub(crate) dimension: Option<String>,
    /// include rows the key left unattributed (defaults to false)
    #[serde(default)]
    pub(crate) include_unattributed: bool,
}

impl TimeBounds for AttributionQuery {
    fn time_bounds(&self) -> (Option<&str>, Option<&str>) {
        (self.since.as_deref(), self.until.as_deref())
    }
}

/// Spend and usage grouped by a governance dimension. Rows the key left
/// unattributed are excluded unless `include_unattributed=true`, so a
/// business-unit chargeback report is not skewed by an empty bucket.
async fn by_attribution(
    access: AnalyticsAccess,
    State(state): State<crate::ControlState>,
    Query(q): Query<AttributionQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let dimension = q.dimension.as_deref().unwrap_or("business_unit");
    let Some(column) = attribution_column(dimension) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "dimension must be one of business_unit|customer"}})),
        )
            .into_response();
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
                {ERRORS} as errors \
         from request_logs where {WHERE_WINDOW} and {ROW_VISIBLE} \
         group by id having {attributed} order by cost_usd desc format JSON"
    );
    let window = window_params(&WindowQuery {
        since: q.since.clone(),
        until: q.until.clone(),
        bucket: None,
    });
    run(ch.query(&sql, &with_access(window, &access)).await)
}

/// What each guardrail did to requests in the window (#2158): one row per
/// `(rule, action)` with the number of requests it acted on.
///
/// `action` is `blocked`, `redacted` or `annotated` for a rule (named in
/// `rule`), and `provider_blocked`, `provider_transformed` or `semantic_truncated` for a guardrail
/// provider, with its name in `rule` (empty on rows written before #3038). A request counts once per entry
/// however many times the rule matched in it, scaled by the sampling weight
/// like every other count here. Rows older than
/// `clickhouse/018_guardrail_events.sql` carry no events, and a table without
/// the column answers `recorded: false` with no rows rather than failing, so
/// the dashboard can say the outcomes are not being kept instead of "no
/// matches yet".
async fn guardrails(
    access: AnalyticsAccess,
    state: State<crate::ControlState>,
    q: Query<WindowQuery>,
) -> Response {
    events_rollup(access, state, q, "guardrail_events", |_| guardrails_sql()).await
}

/// What each plugin did to requests in the window (#2243): one row per
/// organization and plugin slug, with its calls, verdicts and failures. A slug
/// is only unique within an organization, so `org_id` tells two apart; an
/// org-wide and a project plugin of one organization that share a slug are
/// counted together.
///
/// `calls` is every consultation, and `blocked`, `transformed`, `annotated` and
/// `errors` are the parts of it that ended that way; the rest were `allowed`.
/// An `errors` call gave no verdict, so a fail-closed plugin that refused
/// traffic because its endpoint was down counts there and not under `blocked`.
/// `last_error_at` and `last_error_kind` name the newest failure (`refused`,
/// `timeout`, `connect`, `status_<code>` or `body`), and are `null` and empty when
/// `errors` is `0`. Counts are scaled by the sampling weight like every other
/// count here, and `last_error_at` can only be a sampled row's. A table without
/// `clickhouse/019_plugin_events.sql` answers `recorded: false` with no rows,
/// so the dashboard can say the outcomes are not being kept instead of "no
/// calls yet".
async fn plugins(
    access: AnalyticsAccess,
    state: State<crate::ControlState>,
    q: Query<WindowQuery>,
) -> Response {
    events_rollup(access, state, q, "plugin_events", |_| plugins_sql()).await
}

/// Which request parameters dialect translation did not forward as sent, per
/// route and provider, in the window (#3323): one row per `route` (the public
/// model), `provider`, `disposition` (`dropped`, `clamped` or `translated`)
/// and `param`, with the `requests` that had it and the `route_requests` the
/// route sent that provider in all, so `share` is the part of the traffic it
/// touched. This is what lets an operator see "this route silently drops
/// `top_k` on 40% of its traffic" instead of each caller finding out alone.
/// Counts are scaled by the sampling weight like every other count here. A
/// table without `clickhouse/022_param_adjustments.sql` answers
/// `recorded: false` with no rows.
async fn param_adjustments(
    access: AnalyticsAccess,
    state: State<crate::ControlState>,
    q: Query<WindowQuery>,
) -> Response {
    events_rollup(access, state, q, "param_adjustments", param_adjustments_sql).await
}

/// The rollup behind [`param_adjustments`]. One scan: each `(route, provider)`
/// group sums its weight and, with `sumMap`, the weight of every
/// `<disposition>:<param>` entry its rows carry, which the outer query unfolds
/// into a row per entry. The gateway records a parameter once per request, so
/// an entry's weight is the requests it touched. The disposition never holds a
/// `:`, so the first one splits the entry.
///
/// A provider is `(provider_org_id, provider)`: two orgs may each have an
/// `openai` (#1857), and a caller who sees both gets a row for each (#3441).
/// A table without `clickhouse/037_request_log_provider_org.sql` reads every
/// row's org as empty, which is what a row written before it holds.
fn param_adjustments_sql(columns: &Columns) -> String {
    let provider_org_id = columns.expr("provider_org_id");
    format!(
        "select route, provider_org_id, provider, \
            substring(event, 1, position(event, ':') - 1) as disposition, \
            substring(event, position(event, ':') + 1) as param, \
            round(weight) as requests, \
            round(total) as route_requests, \
            round(weight / total, 4) as share \
         from (\
            select model as route, {provider_org_id} as provider_org_id, provider, \
                sum(sample_weight) as total, \
                sumMap(param_adjustments, arrayMap(entry -> sample_weight, param_adjustments)) as adjusted \
            from request_logs where {WHERE_WINDOW} and {ROW_VISIBLE} \
            group by route, provider_org_id, provider) \
         array join adjusted.1 as event, adjusted.2 as weight \
         where position(event, ':') > 0 \
         order by requests desc, route, provider, provider_org_id, disposition, param format JSON"
    )
}

/// Run an event-array rollup over `column`, answering `recorded: false` while
/// the table lacks the column. `sql` builds the query from the columns the
/// table has, so it can read another optional column through
/// [`Columns::expr`].
async fn events_rollup(
    access: AnalyticsAccess,
    State(state): State<crate::ControlState>,
    Query(q): Query<WindowQuery>,
    column: &str,
    sql: impl FnOnce(&Columns) -> String,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let columns = match ch.request_log_columns().await {
        Ok(columns) => columns,
        Err(error) => return query_failed("analytics query failed", &error),
    };
    if !columns.has(column) {
        return Json(json!({"data": [], "recorded": false})).into_response();
    }
    let sql = sql(&columns);
    match ch
        .query(&sql, &with_access(window_params(&q), &access))
        .await
    {
        Ok(data) => Json(json!({"data": data, "recorded": true})).into_response(),
        Err(error) => {
            ch.forget_request_log_columns();
            query_failed("analytics query failed", &error)
        }
    }
}

/// The rollup behind [`plugins`]. An entry is `<outcome>:<slug>`, or
/// `error:<slug>:<kind>`; a slug is `[a-z0-9-]`, so splitting on `:` is exact.
fn plugins_sql() -> String {
    format!(
        "select org_id, plugin, \
            round(sum(weight)) as calls, \
            round(sumIf(weight, outcome = 'blocked')) as blocked, \
            round(sumIf(weight, outcome = 'transformed')) as transformed, \
            round(sumIf(weight, outcome = 'annotated')) as annotated, \
            round(sumIf(weight, outcome = 'error')) as errors, \
            if(errors = 0, NULL, maxIf(ts, outcome = 'error')) as last_error_at, \
            argMaxIf(kind, ts, outcome = 'error') as last_error_kind \
         from (\
            select ts, org_id, sample_weight as weight, \
                splitByChar(':', event) as parts, \
                parts[1] as outcome, parts[2] as plugin, parts[3] as kind \
            from request_logs array join plugin_events as event \
            where {WHERE_WINDOW} and {ROW_VISIBLE}) \
         group by org_id, plugin order by calls desc, org_id, plugin format JSON"
    )
}

/// The rollup behind [`guardrails`]. An entry is `<action>:<rule>`, or just the
/// action for a provider recorded without a name; the action never holds a `:`, so the first one
/// splits it even when a rule name does.
fn guardrails_sql() -> String {
    format!(
        "select \
            if(position(event, ':') = 0, event, substring(event, 1, position(event, ':') - 1)) as action, \
            if(position(event, ':') = 0, '', substring(event, position(event, ':') + 1)) as rule, \
            round(sum(sample_weight)) as requests \
         from request_logs array join guardrail_events as event \
         where {WHERE_WINDOW} and {ROW_VISIBLE} \
         group by action, rule order by requests desc, action, rule format JSON"
    )
}

/// Query params for the per-invocation log list: the shared time window plus
/// optional model/key/status filters and pagination.
#[derive(Debug, Deserialize)]
pub struct InvocationsQuery {
    pub(crate) since: Option<String>,
    pub(crate) until: Option<String>,
    /// exact model name to filter to; empty/omitted means all models
    pub(crate) model: Option<String>,
    /// exact virtual key id to filter to; empty/omitted means all keys
    pub(crate) key: Option<String>,
    /// comma-separated business unit ids to filter to; empty/omitted means
    /// every unit. a set rather than one id because the dashboard's filter
    /// rail has always allowed several, and an id nothing was attributed to
    /// matches nothing — a filter narrows, it never widens (#1247)
    pub(crate) business_unit: Option<String>,
    /// comma-separated customer ids to filter to; empty/omitted means every
    /// customer
    pub(crate) customer: Option<String>,
    /// exact project id to filter to; empty/omitted means every project the
    /// caller may read (#3090)
    pub(crate) project: Option<String>,
    /// status class: all|error|success (defaults to all)
    pub(crate) status: Option<String>,
    /// exact request id — the one the gateway returned to the client in
    /// `x-request-id`, or in the header `request_id_header` names — to look
    /// one request up by (#1849)
    pub(crate) request_id: Option<String>,
    /// exact trace id, the W3C trace a request's spans were recorded under
    pub(crate) trace_id: Option<String>,
    /// `true` narrows the log to requests the gateway recorded as unpriced;
    /// `false` or omitted applies no filter
    pub(crate) unpriced: Option<bool>,
    /// `true` narrows the log to requests whose response a post-call policy
    /// withheld after the provider billed them (#3302); `false` or omitted
    /// applies no filter
    pub(crate) withheld: Option<bool>,
    /// `true` narrows the log to requests the upstream answered without
    /// reporting usage, so their zero tokens and cost are unknown (#3302);
    /// `false` or omitted applies no filter
    pub(crate) usage_unknown: Option<bool>,
    /// one guardrail event to filter to, as the `guardrails` rollup names it
    /// (`blocked:<rule>`, `redacted:<rule>`, `annotated:<rule>`,
    /// `provider_blocked`, `provider_transformed`, `semantic_truncated`); empty/omitted applies no
    /// filter (#2158)
    pub(crate) guardrail: Option<String>,
    /// one plugin event to filter to, as the `plugins` rollup names it
    /// (`blocked:<slug>`, `transformed:<slug>`, `annotated:<slug>` or
    /// `error:<slug>:<kind>`); empty/omitted applies no filter (#2243)
    pub(crate) plugin: Option<String>,
    /// one request outcome to filter to (#3307): a name from
    /// `rolter_core::request_outcome::RequestOutcome`, or `unknown` for rows
    /// written before the gateway recorded one; empty/omitted applies no
    /// filter
    pub(crate) outcome: Option<String>,
    /// page size, 1..=200 (defaults to 50)
    pub(crate) limit: Option<u32>,
    /// opaque `timestamp|request_id` cursor returned as the preceding page's
    /// `next_cursor`; omitted for the first page
    pub(crate) cursor: Option<String>,
}

impl TimeBounds for InvocationsQuery {
    fn time_bounds(&self) -> (Option<&str>, Option<&str>) {
        (self.since.as_deref(), self.until.as_deref())
    }
}

/// Build the per-invocation list query for a whitelisted `status_expr`.
///
/// All user-supplied values are passed as ClickHouse params; the only spliced
/// text is the status predicate, which `status_predicate` whitelists.
///
/// The two attribution dimensions filter here, in the database, rather than in
/// the dashboard over whatever the page happened to return. Filtering a page
/// makes `limit` mean something different from what the caller asked for: ask
/// for 50 rows from a unit that served 3 of the last 50 and you get 3 rows and
/// a "next page" button, with no way to tell a narrow window from a narrow
/// filter (#1247).
///
/// Both accept a comma-separated set, because the dashboard's filter rail has
/// always let an operator tick several units at once. `splitByChar` runs on
/// the bound parameter, so the ids are still values and never spliced text.
///
/// The two payload columns carry explicit aliases because ClickHouse names an
/// unaliased qualified column `payload.request_payload` in its JSON output,
/// while the dashboard reads `request_payload` / `response_payload` and would
/// otherwise always fall back to "payload logging is off" (#1177).
///
/// Rows are filtered to the caller's tenancy and the bodies are masked in the
/// database, never after the fact: a caller below the `request_payload` floor
/// at a row's scope gets both bodies blanked and `payload_withheld` set, so a
/// prompt they may not read never leaves ClickHouse (#1820). The flag is what
/// lets the dashboard say "hidden for your role" instead of "payload logging is
/// off", which is what an empty body used to mean.
///
/// A body is attached to the log row it was captured with, never to every row
/// that shares its `request_id`. The gateway takes that id from the caller's
/// request id header (`x-request-id` unless `request_id_header` names another),
/// so two requests from different tenants can carry the same one. Joining on the id alone let a caller log a bodiless request under
/// an id they had seen in another project and read that project's prompt
/// through their own row, which the mask above passes because it is evaluated
/// against the row, not against the body. Adding the row's `ts` narrowed that
/// to one millisecond but did not close it: a client that sends a constant id
/// still collides with itself, and a caller can reproduce a `ts` (#1937).
///
/// The join is therefore on `log_id`, a UUID the gateway mints per request and
/// writes to both tables, together with the row's `org_id` and `project_id`
/// as defence in depth (see [`PAYLOAD_KEY`]). A row with a `log_id` never falls
/// back to anything weaker. Only a row written before the key existed, where
/// it is empty, joins on `(request_id, ts)` as it always did, so old bodies
/// stay attached until they expire. The payload side is bounded by the same
/// window as the rows, since a body outside it has no row to join to, and
/// grouped on the key so a row never repeats.
///
/// `withheld` and `usage_unknown` ride along for the same reason (#1774): a
/// withheld row's 403 carries the cost the provider billed, and a zero token
/// count with `usage_unknown` set means "not reported", not "none".
///
/// `unpriced` rides along with `cost_usd` because the two are only meaningful
/// together: a zero cost means "free" when the flag is clear and "unknown" when
/// it is set. The gateway decides that per request, against the catalogue that
/// applied at the time, so a caller that instead re-derives it from today's
/// model prices re-judges old rows against new prices and drifts (#1226). The
/// `unpriced` filter reads that same recorded flag, in the database, for the
/// reason the attribution dimensions do: a page cut first and filtered after
/// would hold fewer rows than `limit` asked for.
///
/// Several columns are optional: `upstream_status`, `attempts` (`015`),
/// `lifecycle_operation` (`016`), `cache_write_1h_tokens` (`017`),
/// `guardrail_events` (`018`), `plugin_events` (`019`) and `fallback_reason`
/// (`023`). An
/// existing deployment applies those files by hand, possibly after it has
/// upgraded the control plane, and a select that names a column the table lacks
/// fails the whole list. `columns` says which of them the table has; one it
/// lacks is read as the default ClickHouse would have filled in for an older
/// row, under its own name, so the row has the same shape either way (#2903).
///
/// Paging is a keyset over `(ts, seq, request_id)`, never an offset. `request_logs`
/// is written continuously by the gateway, so rows land above the window
/// between one page and the next; counting from the top then shows a row twice
/// or skips it entirely, which for this screen is the normal case rather than
/// an edge case (#1394).
///
/// `request_id` is what makes that key total. The gateway stamps each row with
/// its own request time (#1210), but `ts` is a `DateTime64(3)` and a burst of
/// concurrent requests really does begin inside one millisecond, so `ts` alone
/// is not a total order and ClickHouse may return tied rows in any order it
/// likes (#1344). With `request_id` in both the sort and the cursor, tied
/// timestamps have one order and the cursor names exactly one row.
pub(crate) fn invocations_sql(status_expr: &str, columns: &Columns) -> String {
    let cursor = invocations_keyset_predicate(&columns.expr("seq"));
    let seq = columns.select("seq");
    let seq_order = columns.expr("seq");
    let cache_write_1h_tokens = columns.select("cache_write_1h_tokens");
    let upstream_status = columns.select("upstream_status");
    let attempts = columns.select("attempts");
    let lifecycle_operation = columns.select("lifecycle_operation");
    let guardrail_events = columns.select("guardrail_events");
    let plugin_events = columns.select("plugin_events");
    let fallback_reason = columns.select("fallback_reason");
    let param_adjustments = columns.select("param_adjustments");
    // the filter reads the column, which an older table lacks: it then matches
    // nothing, which is true, since no event was ever recorded there
    let guardrail_filter = columns.expr("guardrail_events");
    let plugin_filter = columns.expr("plugin_events");
    // without the column every row reads `unknown`, which is what the filter
    // then matches: true, since no outcome was ever recorded there
    let outcome = columns.select("outcome");
    let outcome_filter = columns.expr("outcome");
    // what the request asked for beyond its tokens (#3317)
    let has_tools = columns.select("has_tools");
    let has_image_input = columns.select("has_image_input");
    let has_audio_input = columns.select("has_audio_input");
    let has_response_schema = columns.select("has_response_schema");
    let turns = columns.select("turns");
    // the org that owns the provider (#1857), beside its name
    let provider_org_id = columns.select("provider_org_id");
    // whether the row's cache reads were stated at all (#3410)
    let cache_usage_reported = columns.select("cache_usage_reported");
    // the variant dialled first, beside the one that answered (#3505)
    let first_variant = columns.select("first_variant");
    // and the one the weighted split sampled (#3547)
    let sampled_variant = columns.select("sampled_variant");
    format!(
        "select ts, request_id, trace_id, org_id, team_id, project_id, virtual_key_id, \
                business_unit_id, customer_id, \
                model, provider, {provider_org_id}, target, variant, {first_variant}, {sampled_variant}, status, stream, cache_hit, cache_read_tokens, cache_write_tokens, \
                {cache_write_1h_tokens}, \
                prompt_tokens, completion_tokens, total_tokens, cost_usd, unpriced, withheld, usage_unknown, latency_ms, ttft_ms, error, \
                {upstream_status}, {attempts}, {lifecycle_operation}, {guardrail_events}, {plugin_events}, {seq}, \
                {fallback_reason}, {param_adjustments}, \
                {outcome}, {has_tools}, {has_image_input}, {has_audio_input}, {has_response_schema}, {turns}, \
                {cache_usage_reported}, \
                if({PAYLOAD_VISIBLE}, payload.request_payload, '') as request_payload, \
                if({PAYLOAD_VISIBLE}, payload.response_payload, '') as response_payload, \
                toUInt8(not {PAYLOAD_VISIBLE} \
                        and (notEmpty(payload.request_payload) or notEmpty(payload.response_payload))) \
                    as payload_withheld \
         from request_logs \
         left join ( \
             select {PAYLOAD_KEY} as payload_key, any(request_payload) as request_payload, \
                    any(response_payload) as response_payload \
             from request_payloads where {WHERE_WINDOW} group by payload_key \
         ) as payload on payload.payload_key = {PAYLOAD_KEY} \
         where {WHERE_WINDOW} \
           and {ROW_VISIBLE} \
           and {ROW_FILTERS} \
           and ({{request_id:String}} = '' or request_id = {{request_id:String}}) \
           and ({{trace_id:String}} = '' or trace_id = {{trace_id:String}}) \
           and ({{unpriced:UInt8}} = 0 or unpriced = 1) \
           and ({{withheld:UInt8}} = 0 or request_logs.withheld = 1) \
           and ({{usage_unknown:UInt8}} = 0 or request_logs.usage_unknown = 1) \
           and ({{guardrail:String}} = '' or has({guardrail_filter}, {{guardrail:String}})) \
           and ({{plugin:String}} = '' or has({plugin_filter}, {{plugin:String}})) \
           and ({{outcome:String}} = '' or {outcome_filter} = {{outcome:String}}) \
           and {status_expr} \
           and {cursor} \
         order by ts desc, {seq_order} desc, request_id desc \
         limit {{limit:UInt32}} format JSON"
    )
}

/// The values the invocation list's `outcome` filter accepts: every outcome a
/// gateway writes, then `unknown` for the rows from before it wrote one.
pub(crate) fn outcome_filter_values() -> Vec<&'static str> {
    use rolter_core::request_outcome::{RequestOutcome, UNKNOWN_OUTCOME};
    RequestOutcome::ALL
        .iter()
        .map(|outcome| outcome.as_str())
        .chain([UNKNOWN_OUTCOME])
        .collect()
}

/// Whether `outcome` is a value the `outcome` filter accepts; empty is no
/// filter. Checked before the query so a typo answers 400 rather than an
/// empty page that reads as "no such requests".
pub(crate) fn is_outcome_filter(outcome: &str) -> bool {
    outcome.is_empty() || outcome_filter_values().contains(&outcome)
}

/// Individual gateway invocations, newest first. Returns every persisted column
/// of `request_logs` plus any short-retention raw payload row, so the dashboard
/// can render both the list row and its optional detail bodies.
///
/// Answers `{data, next_cursor}`: feeding `next_cursor` back as `cursor` walks
/// the list without an offset, so rows the gateway writes while an operator
/// reads cannot shift the window under them.
async fn invocations(
    access: AnalyticsAccess,
    State(state): State<crate::ControlState>,
    Query(q): Query<InvocationsQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let status = q.status.as_deref().unwrap_or("all");
    let Some(status_expr) = status_predicate(status) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "status must be one of all|error|success"}})),
        )
            .into_response();
    };
    let outcome = q.outcome.clone().unwrap_or_default();
    if !is_outcome_filter(&outcome) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": format!(
                "outcome must be one of {}",
                outcome_filter_values().join("|")
            )}})),
        )
            .into_response();
    }
    // the id half is `<seq>|<request_id>`: room for the number, while the
    // request id keeps the limit it always had
    let (cursor_ts, cursor_tail) = match parse_keyset_cursor_within(
        q.cursor.as_deref(),
        "request_id",
        MAX_CURSOR_ID_LEN + MAX_CURSOR_SEQ_LEN,
    ) {
        Ok(cursor) => cursor,
        Err(message) => return InvalidParam::cursor(message).into_response(),
    };
    let (cursor_seq, cursor_id) = if cursor_tail.is_empty() {
        ("0".to_string(), String::new())
    } else {
        split_invocations_cursor(&cursor_tail)
    };
    if cursor_id.len() > MAX_CURSOR_ID_LEN {
        return InvalidParam::cursor("cursor must be timestamp|request_id".to_string())
            .into_response();
    }
    let limit = clamp_limit(q.limit);
    // after the request is known to be well formed, so a refused one costs
    // ClickHouse nothing
    let columns = match ch.request_log_columns().await {
        Ok(columns) => columns,
        Err(error) => return query_failed("analytics query failed", &error),
    };
    let sql = invocations_sql(status_expr, &columns);
    let request_id = q.request_id.clone().unwrap_or_default();
    let trace_id = q.trace_id.clone().unwrap_or_default();
    // a pasted id names one request wherever it sits in the retained log: the
    // 7-day default window would answer an older one with an empty page, which
    // reads as "no such request" (#1849)
    let since = match q.since.clone().filter(|since| !since.is_empty()) {
        None if !request_id.is_empty() || !trace_id.is_empty() => {
            Some("1970-01-01T00:00:00Z".to_string())
        }
        since => since,
    };
    let mut params = window_params(&WindowQuery {
        since,
        until: q.until.clone(),
        bucket: None,
    });
    params.push(("param_request_id".to_string(), request_id));
    params.push(("param_trace_id".to_string(), trace_id));
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
    params.push((
        "param_guardrail".to_string(),
        q.guardrail.clone().unwrap_or_default(),
    ));
    params.push((
        "param_plugin".to_string(),
        q.plugin.clone().unwrap_or_default(),
    ));
    params.push(("param_outcome".to_string(), outcome));
    params.extend(row_filter_params(
        q.model.as_deref(),
        q.key.as_deref(),
        q.business_unit.as_deref(),
        q.customer.as_deref(),
        q.project.as_deref(),
    ));
    params.push(("param_cursor_ts".to_string(), cursor_ts));
    params.push(("param_cursor_seq".to_string(), cursor_seq));
    params.push(("param_cursor_id".to_string(), cursor_id));
    params.push(("param_limit".to_string(), limit.to_string()));
    match ch.query(&sql, &with_access(params, &access)).await {
        Ok(data) => {
            let next_cursor = next_invocations_cursor(&data);
            Json(json!({"data": data, "next_cursor": next_cursor})).into_response()
        }
        Err(error) => {
            // the table may have changed under the cached answer (a column
            // dropped, the table recreated): look again on the next request
            ch.forget_request_log_columns();
            query_failed("analytics query failed", &error)
        }
    }
}

/// The two `alter table … modify ttl` statements that put the admin-configured
/// retention clocks on the log tables.
///
/// ClickHouse does not accept query parameters in an `alter table`, so both
/// bounds are formatted straight into the DDL. They are `u32` and range-checked
/// by `logging_settings::validate_settings` before they get here, so no
/// caller-controlled text ever reaches this SQL (#537).
fn retention_statements(retention_days: u32, payload_retention_hours: u32) -> [String; 2] {
    [
        format!(
            "alter table request_logs modify ttl toDateTime(ts) + interval {retention_days} day"
        ),
        format!(
            "alter table request_payloads modify ttl toDateTime(ts) + interval {payload_retention_hours} hour"
        ),
    ]
}

/// Test support: a stand-in for a ClickHouse that has stopped answering.
#[cfg(test)]
pub(crate) mod testing {
    /// A listener that accepts connections and never replies, so a client
    /// without a request timeout waits forever. The accepted sockets are held
    /// open (dropping one would be a reset, not a stall).
    pub(crate) struct Stalled {
        pub(crate) url: String,
        _task: tokio::task::JoinHandle<()>,
    }

    /// The password in a stalled server's url. Interpolated rather than
    /// written into a url literal, so secret scanners see no credential
    pub(crate) const STALLED_USERINFO_SECRET: &str = "stalled-userinfo";

    impl Stalled {
        pub(crate) async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind a local port");
            let port = listener.local_addr().expect("a local address").port();
            let task = tokio::spawn(async move {
                let mut held = Vec::new();
                while let Ok((socket, _)) = listener.accept().await {
                    held.push(socket);
                }
            });
            Self {
                // userinfo on purpose: it must never reach an error or a log
                url: format!("http://ch:{STALLED_USERINFO_SECRET}@127.0.0.1:{port}"),
                _task: task,
            }
        }
    }
    /// A server that answers the first request on each connection, keeps the
    /// connection alive, and drops it unanswered when a second request arrives
    /// on it — what a client sees when it reuses a connection at the instant
    /// ClickHouse's `keep_alive_timeout` closes it (#1940).
    pub(crate) struct ClosesReusedConnections {
        pub(crate) url: String,
        pub(crate) connections: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        _task: tokio::task::JoinHandle<()>,
    }

    /// Read one HTTP/1.1 request (head plus a `content-length` body); `false`
    /// when the peer closed first.
    async fn read_request(socket: &mut tokio::net::TcpStream) -> bool {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
        let length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);
        while buf.len() < head_end + length {
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        true
    }

    impl ClosesReusedConnections {
        pub(crate) async fn start() -> Self {
            use std::sync::atomic::Ordering;
            use tokio::io::AsyncWriteExt;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind a local port");
            let port = listener.local_addr().expect("a local address").port();
            let connections = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counted = connections.clone();
            let task = tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    counted.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(async move {
                        if !read_request(&mut socket).await {
                            return;
                        }
                        let reply = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: keep-alive\r\n\r\n";
                        if socket.write_all(reply).await.is_err() {
                            return;
                        }
                        // the reused connection: closed with the request unanswered
                        read_request(&mut socket).await;
                    });
                }
            });
            Self {
                url: format!("http://127.0.0.1:{port}"),
                connections,
                _task: task,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::time::Duration;

    /// A regression must fail the test, not hang the suite.
    const CEILING: Duration = Duration::from_secs(10);

    #[tokio::test]
    async fn reads_ask_clickhouse_for_iso_timestamps_with_their_zone() {
        let seen: Arc<parking_lot::Mutex<Vec<String>>> = Arc::default();
        let sink = seen.clone();
        let app = axum::Router::new().fallback(axum::routing::post(
            move |axum::extract::RawQuery(q): axum::extract::RawQuery| {
                let sink = sink.clone();
                async move {
                    sink.lock().push(q.unwrap_or_default());
                    // what clickhouse answers under `date_time_output_format=iso`
                    Json(json!({"data": [{"ts": "2026-08-06T10:00:00.123Z"}]}))
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });

        let rows = ClickHouseClient::new(&url)
            .query("select ts from t format JSON", &[("p".into(), "v".into())])
            .await
            .expect("the stub answers");
        assert_eq!(rows[0]["ts"], "2026-08-06T10:00:00.123Z");
        let query = seen.lock()[0].clone();
        assert!(query.contains("date_time_output_format=iso"), "{query}");
        assert!(query.contains("default_format=JSON"), "{query}");
        assert!(query.contains("p=v"), "{query}");
    }

    #[tokio::test]
    async fn a_stalled_clickhouse_read_times_out_without_leaking_the_url() {
        let stalled = testing::Stalled::start().await;
        let ch = ClickHouseClient::with_timeouts(
            &stalled.url,
            Duration::from_secs(1),
            Duration::from_millis(300),
        );
        let result = tokio::time::timeout(CEILING, ch.query("select 1 format JSON", &[]))
            .await
            .expect("the client bounds the request itself");
        let err = result.expect_err("a stalled server is an error");
        let source = err
            .downcast_ref::<reqwest::Error>()
            .expect("a transport error");
        assert!(source.is_timeout(), "{err}");
        let text = format!("{err:#}");
        // the message carries neither the error text nor the secret, so a
        // failure cannot itself print the credential
        assert!(
            !text.contains(testing::STALLED_USERINFO_SECRET) && !text.contains("127.0.0.1"),
            "the error text names the stalled server's userinfo or host"
        );
        // the route-level mapping carries no driver text either
        let response = run(Err(err));
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "analytics_query_timeout");
    }

    #[tokio::test]
    async fn a_stalled_clickhouse_insert_times_out() {
        let stalled = testing::Stalled::start().await;
        let ch = ClickHouseClient::with_timeouts(
            &stalled.url,
            Duration::from_secs(1),
            Duration::from_millis(300),
        );
        let event = json!({"event_id": "e"});
        let ui = tokio::time::timeout(CEILING, ch.insert_ui_events(std::slice::from_ref(&event)))
            .await
            .expect("ui-event insert is bounded");
        assert!(ui.is_err());
        let mcp = tokio::time::timeout(CEILING, ch.insert_mcp_tool_call(&event))
            .await
            .expect("mcp insert is bounded");
        assert!(mcp.is_err());
    }

    /// A connection idle past the pool bound is retired rather than reused, so
    /// a batch sent after a pause never lands on a socket ClickHouse is closing
    /// (#1940). The fake server drops any second request on a connection, which
    /// is the race made deterministic: reuse fails the insert, a fresh
    /// connection succeeds.
    #[tokio::test]
    async fn an_idle_connection_is_retired_before_the_server_closes_it() {
        let server = testing::ClosesReusedConnections::start().await;
        let pool_idle = Duration::from_millis(200);
        let ch = ClickHouseClient::with_pool_idle(
            &server.url,
            Duration::from_secs(1),
            Duration::from_secs(5),
            pool_idle,
        );
        let event = json!({"event_id": "e"});
        tokio::time::timeout(CEILING, ch.insert_ui_events(std::slice::from_ref(&event)))
            .await
            .expect("bounded")
            .expect("the first insert is answered");
        // wall-clock, not paused tokio time: the pool stamps idle connections
        // with std's clock
        tokio::time::sleep(pool_idle * 3).await;
        tokio::time::timeout(CEILING, ch.insert_ui_events(std::slice::from_ref(&event)))
            .await
            .expect("bounded")
            .expect("the insert after a pause goes out on a fresh connection");
        assert_eq!(
            server.connections.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
    }

    /// The read share's measurement is selected only when the table can say
    /// which requests reported their cache reads (#3410), and is weighted like
    /// every other sum.
    #[test]
    fn the_summary_measures_the_prompt_cache_only_with_the_reported_column() {
        let terms = cache_measured_terms(&Columns::default());
        assert!(terms.contains("as cache_reported_prompt_tokens"), "{terms}");
        assert!(terms.contains("as cache_unmeasured_requests"), "{terms}");
        for expr in [CACHE_REPORTED_PROMPT_TOKENS, CACHE_UNMEASURED] {
            assert!(expr.contains("sample_weight"), "{expr}");
            assert!(expr.contains("cache_usage_reported"), "{expr}");
        }
        // an un-migrated table leaves both fields out rather than reading
        // every request as unmeasured
        assert_eq!(
            cache_measured_terms(&Columns::lacking(&["cache_usage_reported"])),
            ""
        );
    }

    #[test]
    fn counts_and_sums_scale_by_the_row_sample_weight() {
        // at a 50 % rate each kept row weighs 2, so no count()/sum() may read
        // a row once, and the mean and the percentiles are weighted too
        for expr in [
            REQUESTS,
            TOKENS,
            PROMPT_TOKENS,
            COMPLETION_TOKENS,
            COST,
            UNPRICED,
            ERRORS,
        ] {
            assert!(expr.contains("sample_weight"), "{expr}");
        }
        assert_eq!(REQUESTS, "round(sum(sample_weight))");
        let src = include_str!("analytics.rs");
        let handlers = &src[src.find("async fn summary(").unwrap()
            ..src.find("pub struct InvocationsQuery").unwrap()];
        assert!(!handlers.contains("count()"), "an unweighted count()");
        assert!(!handlers.contains("countIf("), "an unweighted countIf()");
        // the mean is weighted too (#3420), not a plain avg()
        assert!(handlers.contains("{AVG_LATENCY}"));
        assert!(!handlers.contains("avg(latency_ms)"), "an unweighted mean");
        assert!(
            AVG_LATENCY.contains("sum(latency_ms * sample_weight) / sum(sample_weight)"),
            "{AVG_LATENCY}"
        );
        // and the divisor is guarded, so an empty window is 0 and not nan
        assert!(
            AVG_LATENCY.contains("sum(sample_weight) > 0"),
            "{AVG_LATENCY}"
        );
        // the percentiles are weighted too (#3355), by an integer weight
        assert!(handlers.contains("{P95_LATENCY}"));
        assert!(!handlers.contains("quantile(0.95)(latency_ms)"));
        for expr in [P50_LATENCY, P95_LATENCY] {
            assert!(
                expr.contains("quantileExactWeighted(") && expr.contains("toUInt64("),
                "{expr}"
            );
            assert!(expr.contains("sample_weight"), "{expr}");
            // the timing variant clamps at 30 000 ms, below a slow completion
            assert!(!expr.contains("Timing"), "{expr}");
        }
    }

    #[test]
    fn by_variant_counts_first_attempts_beside_the_requests_that_answered() {
        // #3409: `requests` is every request the variant answered; the first
        // attempt count leaves out those another target failed over to it
        let sql = by_variant_sql(&Columns::default());
        assert!(
            sql.contains("round(sumIf(sample_weight, request_logs.fallback_reason = ''))"),
            "{sql}"
        );
        assert!(sql.contains("as first_attempt_requests"), "{sql}");
        // the column is qualified, and no count reads a row once
        assert!(!sql.contains("count()"), "{sql}");
        // the counts and the grouping are still the by-model ones
        assert!(
            sql.contains("group by request_logs.org_id, request_logs.model, request_logs.variant"),
            "{sql}"
        );
        assert!(sql.contains(&format!("{REQUESTS} as requests")), "{sql}");
    }

    #[test]
    fn by_variant_counts_the_requests_the_split_assigned_each_variant() {
        // #3505: grouped by the variant first dialled, beside the answered
        // counts, from one window and the same filters
        let sql = by_variant_sql(&Columns::default());
        assert!(sql.contains("as assigned_requests"), "{sql}");
        assert!(
            sql.contains(&format!(
                "group by request_logs.org_id, request_logs.model, {ASSIGNED_VARIANT}"
            )),
            "{sql}"
        );
        // a row from before `048` counts only when it never failed over, and
        // a classic row counts under the empty string
        assert!(
            sql.contains(
                "(request_logs.first_variant != '' or request_logs.variant = '' \
                 or request_logs.fallback_reason = '')"
            ),
            "{sql}"
        );
        assert_eq!(
            sql.matches(&format!(
                "{WHERE_WINDOW} and {ROW_VISIBLE} and {ROW_FILTERS}"
            ))
            .count(),
            3,
            "every half reads the same window and filters: {sql}"
        );
        // the figures are summed per key, so the answered half is unchanged
        assert!(sql.contains("sum(by_role.requests) as requests"), "{sql}");
        assert!(sql.contains("max(by_role.p95_latency_ms)"), "{sql}");
        assert!(sql.ends_with("order by requests desc, org_id, model, variant format JSON"));
    }

    #[test]
    fn by_variant_keeps_two_orgs_same_named_routes_apart() {
        // #3548: as by-model since #3486, every half is keyed on the org too,
        // with or without the assigned half
        for columns in [
            Columns::default(),
            Columns::lacking(&["first_variant", "sampled_variant"]),
        ] {
            let sql = by_variant_sql(&columns);
            assert!(sql.starts_with("select "), "{sql}");
            assert!(sql.contains("as org_id"), "{sql}");
            assert!(!sql.contains("group by request_logs.model"), "{sql}");
            for group in sql.split("group by ").skip(1) {
                assert!(
                    group.starts_with("request_logs.org_id, ")
                        || group.starts_with("by_role.org_id, "),
                    "{group}"
                );
            }
        }
    }

    #[test]
    fn by_variant_does_not_claim_an_assignment_a_table_without_first_variant_cannot_make() {
        let sql = by_variant_sql(&Columns::lacking(&["first_variant", "sampled_variant"]));
        assert!(
            sql.contains("CAST(NULL AS Nullable(Float64)) as assigned_requests"),
            "{sql}"
        );
        assert!(!sql.contains("first_variant"), "{sql}");
        assert!(!sql.contains("union all"), "{sql}");
        // with `050` but not `048` the sampled half still reads, and the
        // assigned count is null in every half
        let sql = by_variant_sql(&Columns::lacking(&["first_variant"]));
        assert!(!sql.contains("first_variant"), "{sql}");
        assert_eq!(sql.matches("union all").count(), 1, "{sql}");
        assert!(
            sql.contains("CAST(NULL AS Nullable(Float64)) as counted_assigned"),
            "{sql}"
        );
        // the first-attempt count is still read
        assert!(sql.contains(FIRST_ATTEMPT_REQUESTS), "{sql}");
    }

    #[test]
    fn by_variant_counts_the_requests_the_split_sampled_each_variant() {
        // #3547: grouped by the variant the split sampled, before skipping;
        // a classic row counts under the empty string and an old variant row
        // that records no sample is left out
        let sql = by_variant_sql(&Columns::default());
        assert!(
            sql.contains("sum(by_role.sampled_requests) as sampled_requests"),
            "{sql}"
        );
        assert!(
            sql.contains(&format!(
                "group by request_logs.org_id, request_logs.model, {SAMPLED_VARIANT}"
            )),
            "{sql}"
        );
        assert!(sql.contains(SAMPLED_ROWS), "{sql}");
        // the answered and assigned halves contribute nothing to it
        assert!(sql.contains("toFloat64(0) as sampled_requests"), "{sql}");
        assert!(sql.contains("toFloat64(0) as counted_sampled"), "{sql}");
    }

    #[test]
    fn by_variant_does_not_claim_a_sample_a_table_without_sampled_variant_cannot_make() {
        let sql = by_variant_sql(&Columns::lacking(&["sampled_variant"]));
        assert!(
            sql.contains("CAST(NULL AS Nullable(Float64)) as sampled_requests"),
            "{sql}"
        );
        assert!(!sql.contains("sampled_variant"), "{sql}");
        // the assigned half is still read
        assert_eq!(sql.matches("union all").count(), 1, "{sql}");
        assert!(sql.contains(ASSIGNED_VARIANT), "{sql}");
    }

    #[test]
    fn by_variant_places_no_old_failed_over_row_without_fallback_reason() {
        // with `048` but not `023`, a variant row from before `048` cannot be
        // told apart from a failover, so only rows that name their variant
        // and classic rows count
        let sql = by_variant_sql(&Columns::lacking(&["fallback_reason"]));
        assert!(
            sql.contains("(request_logs.first_variant != '' or request_logs.variant = '')"),
            "{sql}"
        );
        assert!(!sql.contains("fallback_reason"), "{sql}");
    }

    #[test]
    fn by_variant_does_not_claim_a_first_attempt_split_a_table_without_the_column_cannot_make() {
        // 023 is applied by hand: without it every row would read as a first
        // attempt, which is a claim, so the count is null and the select does
        // not name the column
        let sql = by_variant_sql(&Columns::lacking(&["fallback_reason"]));
        assert!(
            sql.contains("CAST(NULL AS Nullable(Float64)) as first_attempt_requests"),
            "{sql}"
        );
        assert!(!sql.contains("fallback_reason"), "{sql}");
        // and the rest of the select is unchanged
        assert!(sql.contains(&format!("{REQUESTS} as requests")), "{sql}");
    }

    #[test]
    fn the_error_totals_count_the_same_statuses_as_the_error_filter() {
        // a request that never got a status (0) is an error in the list and in
        // the totals, or the screens disagree about how many requests failed
        assert_eq!(
            ERRORS,
            format!("round(sumIf(sample_weight, {ERROR_STATUS}))")
        );
    }

    #[test]
    fn by_model_keeps_two_orgs_same_named_routes_apart() {
        // #3486: grouping by the bare name summed two tenants' routes
        let sql = by_model_sql();
        assert!(sql.starts_with("select org_id, model,"), "{sql}");
        assert!(sql.contains("group by org_id, model "), "{sql}");
        assert!(!sql.contains("group by model "), "{sql}");
        assert!(sql.contains(ROW_VISIBLE), "{sql}");
    }

    #[test]
    fn bucket_expr_whitelists() {
        assert_eq!(bucket_expr("hour"), Some("toStartOfHour(ts, 'UTC')"));
        assert_eq!(bucket_expr("day"), Some("toStartOfDay(ts, 'UTC')"));
        assert_eq!(bucket_expr("week"), Some("toStartOfWeek(ts, 0, 'UTC')"));
        assert_eq!(bucket_expr("month"), Some("toStartOfMonth(ts, 'UTC')"));
        // anything else is rejected, so it can never be spliced into SQL
        assert_eq!(bucket_expr("day; drop table request_logs"), None);
        assert_eq!(bucket_expr(""), None);
    }

    #[test]
    fn bucket_boundaries_and_window_bounds_are_computed_in_utc() {
        // a server on Europe/Moscow starts a day at 21:00Z unless the zone is
        // explicit (#2984)
        for bucket in ["hour", "day", "week", "month"] {
            let expr = bucket_expr(bucket).unwrap();
            assert!(expr.starts_with("toStartOf"), "{expr}");
            assert!(
                expr.ends_with(", 'UTC')"),
                "{bucket} bucket has no zone: {expr}"
            );
        }
        // a zone-less since/until is read as UTC, not as the server's zone
        assert_eq!(WHERE_WINDOW.matches("{since:String}, 3, 'UTC')").count(), 1);
        assert_eq!(WHERE_WINDOW.matches("{until:String}, 3, 'UTC')").count(), 1);
        assert_eq!(
            WHERE_WINDOW
                .matches("parseDateTime64BestEffortOrZero(")
                .count(),
            2
        );
    }

    #[test]
    fn window_bounds_never_strict_parse_an_optional_bound() {
        // clickhouse constant-folds both branches of `if`, so a strict parse of
        // an empty since/until aborts the whole query before the default branch
        // is ever selected (#1177)
        assert!(!WHERE_WINDOW.contains("parseDateTime64BestEffort("));
        assert_eq!(
            WHERE_WINDOW
                .matches("parseDateTime64BestEffortOrZero(")
                .count(),
            2
        );
        // the empty-string guards stay, so the defaults are unchanged
        assert!(WHERE_WINDOW.contains("if({since:String} = '', now64(3) - interval 7 day"));
        assert!(WHERE_WINDOW.contains("if({until:String} = '', now64(3)"));
    }

    #[test]
    fn invocations_sql_aliases_the_payload_columns() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        // an unaliased qualified column is named `payload.request_payload` in
        // clickhouse's JSON output, which the dashboard never reads (#1177)
        assert!(!sql.contains("payload.request_payload, payload.response_payload"));
        assert!(sql.contains("payload.request_payload, '') as request_payload"));
        assert!(sql.contains("payload.response_payload, '') as response_payload"));
    }

    #[test]
    fn invocations_sql_filters_rows_and_masks_bodies_the_caller_may_not_read() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        // every row is narrowed to the caller's tenancy, and each body is blanked
        // unless the payload floor is met at the row's own scope (#1820)
        assert!(sql.contains(&format!("and {ROW_VISIBLE}")));
        assert!(sql.contains(&format!(
            "if({PAYLOAD_VISIBLE}, payload.request_payload, '') as request_payload"
        )));
        assert!(sql.contains(&format!(
            "if({PAYLOAD_VISIBLE}, payload.response_payload, '') as response_payload"
        )));
        assert!(sql.contains("as payload_withheld"));
    }

    #[test]
    fn invocations_sql_binds_each_body_to_its_own_log_row() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        // request_id is whatever the caller sent as x-request-id, so joining on
        // it alone hands one tenant's captured body to another tenant's row
        // under the same id. the log row's own ts is what names the request
        assert!(sql.contains(&format!("on payload.payload_key = {PAYLOAD_KEY}")));
        assert!(!sql.contains("using (request_id"));
        assert!(sql.contains("group by payload_key"));
        // a keyed row joins on the gateway's own key and its tenancy, and only
        // an unkeyed (pre-#1937) row falls back to the caller's id and ts
        assert!(PAYLOAD_KEY.contains("log_id != ''"));
        assert!(PAYLOAD_KEY.contains("concat(log_id, '|', org_id, '|', project_id)"));
        assert!(PAYLOAD_KEY.contains("concat(request_id, '|', toString(ts))"));
        // no aggregate may reach across every row that shares an id
        assert!(!sql.contains("argMax(request_payload"));
        assert!(!sql.contains("group by request_id "));
    }

    #[test]
    fn every_request_log_rollup_is_filtered_to_the_caller() {
        // the rollups build their sql inline, so the guard reads the source: a
        // new rollup over request_logs that forgets the filter would answer
        // every tenant's traffic to anyone signed in (#1820)
        let source = include_str!("analytics.rs");
        let table = "request_logs";
        let rollups = source
            .matches(&format!("from {table} where {{WHERE_WINDOW}}"))
            .count();
        let filtered = source
            .matches(&format!(
                "from {table} where {{WHERE_WINDOW}} and {{ROW_VISIBLE}}"
            ))
            .count();
        assert_eq!(
            rollups, 6,
            "summary, timeseries, by-model, by-variant, by-attribution, param-adjustments"
        );
        assert_eq!(filtered, rollups);
    }

    #[test]
    fn the_param_adjustment_rollup_is_scoped_and_weighted() {
        let sql = param_adjustments_sql(&Columns::default());
        assert!(
            sql.contains(&format!("where {WHERE_WINDOW} and {ROW_VISIBLE}")),
            "{sql}"
        );
        assert!(sql.contains("sum(sample_weight) as total"), "{sql}");
        assert!(
            sql.contains("arrayMap(entry -> sample_weight, param_adjustments)"),
            "{sql}"
        );
        assert!(
            sql.contains("group by route, provider_org_id, provider"),
            "{sql}"
        );
        assert!(sql.contains("as share"), "{sql}");
        assert!(!sql.contains("count()"), "an unweighted count: {sql}");
    }

    /// #3441: two orgs' `openai` are two providers in the rollup, and a table
    /// without `037` reads every row's org as empty rather than failing.
    #[test]
    fn the_param_adjustment_rollup_keys_a_provider_by_its_org() {
        let sql = param_adjustments_sql(&Columns::default());
        assert!(
            sql.contains("provider_org_id as provider_org_id, provider,"),
            "{sql}"
        );
        assert!(sql.starts_with("select route, provider_org_id, provider,"));
        let old = param_adjustments_sql(&Columns::lacking(&["provider_org_id"]));
        assert!(old.contains("'' as provider_org_id, provider,"), "{old}");
        assert!(
            old.contains("group by route, provider_org_id, provider"),
            "{old}"
        );
    }

    #[test]
    fn the_invocation_list_reads_and_filters_the_outcome() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        assert!(sql.contains(" outcome, "), "{sql}");
        assert!(
            sql.contains("({outcome:String} = '' or outcome = {outcome:String})"),
            "{sql}"
        );
        // a table without 030 reads every row as unknown, selected and filtered
        let old = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::lacking(&["outcome"]),
        );
        assert!(old.contains("'unknown' as outcome"), "{old}");
        assert!(
            old.contains("({outcome:String} = '' or 'unknown' = {outcome:String})"),
            "{old}"
        );
    }

    #[test]
    fn the_outcome_filter_accepts_the_published_names_and_unknown() {
        assert!(is_outcome_filter(""));
        for name in [
            "served",
            "refused_budget",
            "refused_rate_limit",
            "upstream_error",
            "unknown",
        ] {
            assert!(is_outcome_filter(name), "{name}");
        }
        assert!(!is_outcome_filter("refused_budget' or 1=1"));
        assert!(!is_outcome_filter("Served"));
    }

    #[test]
    fn the_invocation_list_filters_through_the_shared_predicate() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        assert!(sql.contains(ROW_FILTERS), "{sql}");
    }

    #[test]
    fn every_dashboard_read_filters_beside_the_callers_visibility() {
        // and-ed next to ROW_VISIBLE, never in its place: a filter narrows what
        // the caller may read and cannot reach past it (#1820, #2453)
        // built with format! so this test's own text never matches itself
        let source = include_str!("analytics.rs");
        let table = "request_logs";
        let filtered = source
            .matches(&format!(
                "from {table} where {{WHERE_WINDOW}} and {{ROW_VISIBLE}} and {{ROW_FILTERS}}"
            ))
            .count();
        assert_eq!(filtered, 4, "summary, timeseries, by-model, by-variant");
    }

    #[test]
    fn dashboard_filters_bind_every_value_as_a_param() {
        for param in [
            "{model:String}",
            "{key:String}",
            "{business_unit:String}",
            "{customer:String}",
            "{project:String}",
        ] {
            assert!(ROW_FILTERS.contains(param), "{param}");
            // each one short-circuits when empty, so an omitted filter is no filter
            assert!(ROW_FILTERS.contains(&format!("{param} = ''")), "{param}");
        }
        assert!(
            ROW_FILTERS.contains("has(splitByChar(',', {business_unit:String}), business_unit_id)")
        );
        assert!(ROW_FILTERS.contains("has(splitByChar(',', {customer:String}), customer_id)"));
        // no quoted literal on the column side of a comparison
        for column in [
            "model",
            "virtual_key_id",
            "business_unit_id",
            "customer_id",
            "project_id",
        ] {
            assert!(!ROW_FILTERS.contains(&format!("{column} = '")), "{column}");
        }
    }

    fn dashboard_query(query: &str) -> DashboardQuery {
        let uri: axum::http::Uri = format!("/api/v1/analytics/summary?{query}")
            .parse()
            .expect("uri");
        axum::extract::Query::<DashboardQuery>::try_from_uri(&uri)
            .expect("parses")
            .0
    }

    fn param<'a>(params: &'a [(String, String)], name: &str) -> &'a str {
        params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("{name} is bound"))
    }

    #[test]
    fn a_dashboard_query_reads_the_filters_beside_the_window() {
        let q = dashboard_query(
            "since=2026-01-01T00:00:00Z&bucket=hour&model=gpt-4o&key=k-1\
             &business_unit=bu-1,bu-2&customer=c-1&project=p-1",
        );
        assert_eq!(q.window.bucket.as_deref(), Some("hour"));
        let params = q.params();
        assert_eq!(param(&params, "param_project"), "p-1");
        assert_eq!(param(&params, "param_since"), "2026-01-01T00:00:00Z");
        assert_eq!(param(&params, "param_until"), "");
        assert_eq!(param(&params, "param_model"), "gpt-4o");
        assert_eq!(param(&params, "param_key"), "k-1");
        // the set travels as one bound value; splitByChar splits it in sql
        assert_eq!(param(&params, "param_business_unit"), "bu-1,bu-2");
        assert_eq!(param(&params, "param_customer"), "c-1");
    }

    #[test]
    fn an_absent_or_empty_dashboard_filter_binds_the_empty_string() {
        for query in ["", "model=&key=&business_unit=&customer="] {
            let params = dashboard_query(query).params();
            for name in [
                "param_model",
                "param_key",
                "param_business_unit",
                "param_customer",
            ] {
                assert_eq!(param(&params, name), "", "{name} from {query:?}");
            }
        }
    }

    #[test]
    fn a_hostile_dashboard_filter_stays_a_value() {
        let hostile = "x') or 1=1 --";
        let q = dashboard_query(&format!("model={}", url_escape(hostile)));
        assert_eq!(param(&q.params(), "param_model"), hostile);
    }

    fn url_escape(raw: &str) -> String {
        raw.bytes().map(|b| format!("%{b:02X}")).collect()
    }

    #[tokio::test]
    async fn a_dashboard_query_still_refuses_a_malformed_window() {
        use axum::extract::FromRequestParts;
        let (mut parts, ()) = axum::http::Request::builder()
            .uri("/api/v1/analytics/summary?model=gpt-4o&since=yesterday")
            .body(())
            .expect("request")
            .into_parts();
        let refused = Query::<DashboardQuery>::from_request_parts(&mut parts, &()).await;
        assert!(refused.is_err(), "a bad since is a 400, filters or not");
    }

    #[test]
    fn invocations_sql_filters_both_attribution_dimensions_as_params() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        // both are bound, never spliced: they arrive as caller-supplied uuids
        assert!(sql.contains("has(splitByChar(',', {business_unit:String}), business_unit_id)"));
        assert!(sql.contains("has(splitByChar(',', {customer:String}), customer_id)"));
        // the empty-string guard is what makes an omitted filter mean "every
        // unit" rather than "the unit whose id is the empty string" — without
        // it an unattributed row (both columns default to '') would be the
        // only thing an empty filter matched
        assert!(sql.contains("{business_unit:String} = ''"));
        assert!(sql.contains("{customer:String} = ''"));
        // and no attribution value is ever formatted into the text
        assert!(!sql.contains("business_unit_id = '"));
        assert!(!sql.contains("customer_id = '"));
    }

    /// A request is found by the id its client was handed (#1849), bound as a
    /// parameter like every other filter and still inside `ROW_VISIBLE`, so an
    /// id from another tenant finds nothing rather than that tenant's row.
    #[test]
    fn invocations_sql_looks_a_request_up_by_its_ids_as_params() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        assert!(sql.contains("({request_id:String} = '' or request_id = {request_id:String})"));
        assert!(sql.contains("({trace_id:String} = '' or trace_id = {trace_id:String})"));
        assert!(sql.contains(ROW_VISIBLE));
        assert!(!sql.contains("request_id = '") && !sql.contains("trace_id = '"));
    }

    #[test]
    fn withheld_and_unknown_usage_are_weighted_sums_of_the_recorded_flags() {
        // sampled logs stand each row in for `sample_weight` requests (#2239),
        // and the flags are the ones the gateway recorded (#1774)
        assert_eq!(WITHHELD, "round(sumIf(sample_weight, withheld = 1))");
        assert_eq!(
            WITHHELD_COST,
            "round(sumIf(request_logs.cost_usd * sample_weight, withheld = 1), 6)"
        );
        assert_eq!(
            USAGE_UNKNOWN,
            "round(sumIf(sample_weight, usage_unknown = 1))"
        );
    }

    #[test]
    fn cache_hits_are_a_weighted_sum_of_the_recorded_flag() {
        // the numerator of the dashboard's cache-hit rate, scaled by the
        // sampling weight like the `requests` it is divided by (#1978)
        assert_eq!(CACHE_HITS, "round(sumIf(sample_weight, cache_hit = 1))");
    }

    #[test]
    fn invocations_sql_selects_the_unpriced_flag_next_to_cost() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        // a zero cost_usd is ambiguous on its own: free, or no price row at all.
        // the flag the gateway recorded per request has to travel with it, or
        // the dashboard re-derives it from the live catalogue and drifts (#1226)
        assert!(sql.contains("cost_usd, unpriced, withheld, usage_unknown"));
    }

    #[test]
    fn invocations_sql_selects_what_the_request_did_upstream_next_to_its_error() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        // `status` is what the caller was told. when the gateway answered with
        // an error of its own, only these two say which upstream status caused
        // it and how many attempts it took to get there (#2807)
        assert!(sql.contains("error, upstream_status, attempts"));
    }

    #[test]
    fn invocations_sql_selects_which_lifecycle_call_a_row_records() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        // a `GET` and a `DELETE` of one stored response share a model and a
        // provider, carry no tokens and cost nothing: only this column says
        // which call the row is (#2865). it is `String default ''`, so a row
        // older than the column and every model request read back as ''
        assert!(sql.contains("attempts, lifecycle_operation,"));
    }

    #[test]
    fn invocations_sql_filters_unpriced_on_the_recorded_flag_as_a_param() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        // the flag the gateway recorded, bound as a value: the filter must not
        // re-derive "unpriced" from the live catalogue (#1226), and an unset
        // filter has to let every row through
        assert!(sql.contains("({unpriced:UInt8} = 0 or unpriced = 1)"));
    }

    #[test]
    fn invocations_sql_filters_withheld_and_unknown_usage_on_the_recorded_flags() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        // bound values over the flags the gateway recorded (#3302), qualified
        // by the table because the select list names the same columns and a
        // bare name resolves to a select alias before a column. an unset
        // filter lets every row through, and the row scope still applies
        assert!(sql.contains("({withheld:UInt8} = 0 or request_logs.withheld = 1)"));
        assert!(sql.contains("({usage_unknown:UInt8} = 0 or request_logs.usage_unknown = 1)"));
        assert!(sql.contains(ROW_VISIBLE));
    }

    #[test]
    fn the_withheld_and_unknown_usage_filters_read_true_and_nothing_else_narrows() {
        let read = |query: &str| {
            let uri: axum::http::Uri = format!("/api/v1/analytics/invocations{query}")
                .parse()
                .expect("a valid uri");
            axum::extract::Query::<InvocationsQuery>::try_from_uri(&uri)
                .map(|q| (q.0.withheld, q.0.usage_unknown))
        };
        assert_eq!(read("").expect("no filter"), (None, None));
        assert_eq!(
            read("?withheld=true&usage_unknown=true").expect("both on"),
            (Some(true), Some(true))
        );
        assert_eq!(read("?withheld=false").expect("off"), (Some(false), None));
        assert!(read("?withheld=maybe").is_err());
        assert!(read("?usage_unknown=maybe").is_err());
    }

    #[test]
    fn the_unpriced_filter_reads_true_and_nothing_else_narrows() {
        let read = |query: &str| {
            let uri: axum::http::Uri = format!("/api/v1/analytics/invocations{query}")
                .parse()
                .expect("a valid uri");
            axum::extract::Query::<InvocationsQuery>::try_from_uri(&uri).map(|q| q.0.unpriced)
        };
        assert_eq!(read("").expect("no filter"), None);
        assert_eq!(read("?unpriced=true").expect("on"), Some(true));
        assert_eq!(read("?unpriced=false").expect("off"), Some(false));
        // a value that is neither is a refused query, not a filter left off
        assert!(read("?unpriced=maybe").is_err());
    }

    #[test]
    fn invocations_sql_orders_by_a_total_key_so_paging_cannot_repeat_a_row() {
        for status in ["all", "error", "success"] {
            let sql = invocations_sql(
                status_predicate(status).expect("status is whitelisted"),
                &Columns::default(),
            );
            // ts is a DateTime64(3): a burst of concurrent requests shares one
            // millisecond even though every row carries its own request time,
            // so ordering on ts alone leaves tied rows in an arbitrary order
            // that limit/offset paging can repeat or skip (#1344)
            assert!(
                sql.contains("order by ts desc, seq desc, request_id desc"),
                "invocations must order on a total key, got: {sql}"
            );
        }
    }

    #[test]
    fn invocations_sql_binds_filters_as_params_and_splices_only_the_status() {
        let sql = invocations_sql(
            status_predicate("error").expect("error is whitelisted"),
            &Columns::default(),
        );
        assert!(sql.contains("and (status = 0 or status >= 400)"));
        assert!(sql.contains("{model:String}"));
        assert!(sql.contains("{key:String}"));
        assert!(sql.contains("{limit:UInt32}"));
        assert!(sql.contains("{cursor_ts:String}"));
        assert!(sql.contains("{cursor_seq:UInt64}"));
        assert!(sql.contains("{cursor_id:String}"));
        // the shared window clause is inlined, so it must be empty-safe here too
        assert!(!sql.contains("parseDateTime64BestEffort("));
    }

    #[test]
    fn the_start_sequence_orders_requests_inside_one_millisecond() {
        // a table with the column sorts and resumes on it
        let sql = invocations_sql("1", &Columns::default());
        assert!(sql.contains("seq,"), "{sql}");
        assert!(sql.contains("order by ts desc, seq desc, request_id desc"));
        assert!(sql.contains("(seq < {cursor_seq:UInt64} or (seq = {cursor_seq:UInt64}"));
        // one without it reads every row as sequence 0, under its own name, so
        // the list still works before an operator applies 021
        let lacking = invocations_sql("1", &Columns::lacking(&["seq"]));
        assert!(lacking.contains("toUInt64(0) as seq"), "{lacking}");
        assert!(lacking.contains("order by ts desc, toUInt64(0) desc, request_id desc"));
    }

    #[test]
    fn the_sequence_does_not_shrink_the_request_id_a_cursor_may_name() {
        // a 256 character request id used to page; with `<seq>|` in front of it
        // the whole tail is longer than 256, and must still parse
        let id = "r".repeat(MAX_CURSOR_ID_LEN);
        let cursor = format!("2026-10-01T10:00:00.123Z|18446744073709551615|{id}");
        let (_, tail) = parse_keyset_cursor_within(
            Some(&cursor),
            "request_id",
            MAX_CURSOR_ID_LEN + MAX_CURSOR_SEQ_LEN,
        )
        .expect("the longest sequence and request id parse");
        assert_eq!(split_invocations_cursor(&tail).1, id);
        // the plain parse keeps its old limit for the lists that use it
        assert!(parse_keyset_cursor(Some(&cursor), "request_id").is_err());
    }

    #[test]
    fn a_cursor_carries_the_sequence_and_an_older_one_still_resumes() {
        let rows =
            vec![json!({"ts": "2026-10-01T10:00:00.123Z", "seq": "42", "request_id": "req-a"})];
        assert_eq!(
            next_invocations_cursor(&rows).as_deref(),
            Some("2026-10-01T10:00:00.123Z|42|req-a")
        );
        let numeric = vec![json!({"ts": "2026-10-01T10:00:00.123Z", "seq": 7, "request_id": "r"})];
        assert_eq!(
            next_invocations_cursor(&numeric).as_deref(),
            Some("2026-10-01T10:00:00.123Z|7|r")
        );
        assert_eq!(
            split_invocations_cursor("42|req-a"),
            ("42".to_string(), "req-a".to_string())
        );
        // a request id with a bar in it survives the round trip
        assert_eq!(
            split_invocations_cursor("9|a|b"),
            ("9".to_string(), "a|b".to_string())
        );
        // a cursor from before the sequence resumes past its whole millisecond
        // (repeating rows there at worst), and a request id that merely starts
        // with digits and a bar is not mistaken for a sequence too large to fit
        assert_eq!(
            split_invocations_cursor("req-a"),
            (LEGACY_CURSOR_SEQ.to_string(), "req-a".to_string())
        );
        assert_eq!(
            split_invocations_cursor("99999999999999999999|x").0,
            LEGACY_CURSOR_SEQ
        );
        assert_eq!(next_invocations_cursor(&[]), None);
    }

    #[test]
    fn invocations_sql_pages_on_a_keyset_over_a_total_sort_key() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        // an offset counts rows from the top of a table the gateway is still
        // writing to, so a row ingested between two pages repeats or hides one
        // (#1394)
        assert!(!sql.contains("offset"));
        assert!(sql.contains(&invocations_keyset_predicate("seq")));
        // request_id is what makes the sort key total: without it tied
        // timestamps have no defined order and the cursor names no single row
        assert!(sql.contains("order by ts desc, seq desc, request_id desc"));
    }

    #[test]
    fn an_absent_invocations_cursor_is_never_strict_parsed() {
        let sql = invocations_sql(
            status_predicate("all").expect("all is whitelisted"),
            &Columns::default(),
        );
        // clickhouse evaluates the parse of the bound constant even when the
        // `= ''` disjunct short-circuits it, so a strict parse fails the whole
        // first page (#1177)
        assert!(!sql.contains("parseDateTime64BestEffort("));
        // two for the rows' window bounds, two for the same bounds on the
        // payload side of the join, two for the keyset cursor
        assert_eq!(sql.matches("parseDateTime64BestEffortOrZero(").count(), 6);
        assert_eq!(
            parse_keyset_cursor(None, "request_id"),
            Ok((String::new(), String::new()))
        );
        assert_eq!(
            parse_keyset_cursor(Some(""), "request_id"),
            Ok((String::new(), String::new()))
        );
    }

    #[test]
    fn an_invocations_cursor_requires_a_timestamp_and_a_request_id() {
        assert_eq!(
            parse_keyset_cursor(Some("2026-07-19 12:00:00.000|req-7"), "request_id"),
            Ok(("2026-07-19 12:00:00.000".to_string(), "req-7".to_string()))
        );
        // the rejection names the column this list's cursor is over
        assert_eq!(
            parse_keyset_cursor(Some("not-a-cursor"), "request_id"),
            Err("cursor must be timestamp|request_id".to_string())
        );
        assert!(parse_keyset_cursor(Some("|req-7"), "request_id").is_err());
        assert!(parse_keyset_cursor(Some("2026-07-19 12:00:00.000|"), "request_id").is_err());
    }

    #[test]
    fn an_invocations_cursor_timestamp_must_parse() {
        // the timestamp reaches the same `OrZero` parse as a window bound, so a
        // bad one used to resume from the epoch instead of failing (#1192)
        for timestamp in [
            "not-a-date",
            "2026-02-30 12:00:00.000",
            "2026-07-19t12:00:00z",
        ] {
            let error = parse_keyset_cursor(Some(&format!("{timestamp}|req-7")), "request_id")
                .expect_err(timestamp);
            assert!(error.contains("timestamp|request_id"), "{error}");
            assert!(error.contains("does not parse"), "{error}");
        }
        // every form a page hands out, and the rfc 3339 a caller may write
        for timestamp in ["2026-07-19 12:00:00.000", "2026-07-19T12:00:00.000Z"] {
            assert!(
                parse_keyset_cursor(Some(&format!("{timestamp}|req-7")), "request_id").is_ok(),
                "{timestamp}"
            );
        }
    }

    #[test]
    fn the_next_cursor_names_the_last_row_of_the_page() {
        let rows = vec![
            json!({"ts": "2026-07-19 12:00:01.000", "request_id": "req-2"}),
            json!({"ts": "2026-07-19 12:00:00.000", "request_id": "req-1"}),
        ];
        assert_eq!(
            next_keyset_cursor(&rows, "request_id").as_deref(),
            Some("2026-07-19 12:00:00.000|req-1")
        );
        // an empty page has nothing to resume from
        assert_eq!(next_keyset_cursor(&[], "request_id"), None);
    }

    #[test]
    fn window_params_default_to_empty_strings() {
        let q = WindowQuery {
            since: None,
            until: None,
            bucket: None,
        };
        let params = window_params(&q);
        assert_eq!(params[0], ("param_since".to_string(), String::new()));
        assert_eq!(params[1], ("param_until".to_string(), String::new()));
    }

    #[test]
    fn window_params_pass_bounds_through() {
        let q = WindowQuery {
            since: Some("2026-07-01T00:00:00Z".to_string()),
            until: Some("2026-07-08T00:00:00Z".to_string()),
            bucket: Some("hour".to_string()),
        };
        let params = window_params(&q);
        assert_eq!(params[0].1, "2026-07-01T00:00:00Z");
        assert_eq!(params[1].1, "2026-07-08T00:00:00Z");
    }

    #[test]
    fn status_predicate_whitelists() {
        assert_eq!(status_predicate("all"), Some("1"));
        assert_eq!(status_predicate("error"), Some(ERROR_STATUS));
        assert_eq!(ERROR_STATUS, "(status = 0 or status >= 400)");
        assert_eq!(
            status_predicate("success"),
            Some("status > 0 and status < 400")
        );
        // anything else is rejected, so it can never be spliced into SQL
        assert_eq!(status_predicate("error; drop table request_logs"), None);
        assert_eq!(status_predicate(""), None);
    }

    #[test]
    fn attribution_column_whitelists() {
        assert_eq!(
            attribution_column("business_unit"),
            Some("business_unit_id")
        );
        assert_eq!(attribution_column("customer"), Some("customer_id"));
        // anything else is rejected, so it can never be spliced into SQL
        assert_eq!(
            attribution_column("business_unit_id; drop table request_logs"),
            None
        );
        assert_eq!(attribution_column(""), None);
    }

    #[test]
    fn retention_statements_carry_both_clocks() {
        let [logs, payloads] = retention_statements(30, 24);
        assert_eq!(
            logs,
            "alter table request_logs modify ttl toDateTime(ts) + interval 30 day"
        );
        assert_eq!(
            payloads,
            "alter table request_payloads modify ttl toDateTime(ts) + interval 24 hour"
        );
        // the bounds are numeric all the way down, so there is no shape of
        // input that could close the interval and append another statement
        let [logs, _] = retention_statements(u32::MAX, 1);
        assert!(logs.ends_with(&format!("interval {} day", u32::MAX)));
    }

    /// One row of the sort key `invocations_sql` pages on.
    type KeyRow = (&'static str, &'static str);

    /// The page `invocations_sql` asks ClickHouse for, modelled: order by
    /// `ts desc, request_id desc`, keep only what sorts strictly after the
    /// cursor, take `limit`. Rust's tuple comparison stands in for ClickHouse's
    /// because the fixture's timestamps are fixed-width, so lexicographic order
    /// and chronological order agree.
    fn keyset_page(table: &[KeyRow], cursor: Option<KeyRow>, limit: usize) -> Vec<KeyRow> {
        let mut rows = table.to_vec();
        rows.sort_by(|a, b| b.cmp(a));
        rows.into_iter()
            .filter(|(ts, id)| match cursor {
                None => true,
                Some((cursor_ts, cursor_id)) => {
                    *ts < cursor_ts || (*ts == cursor_ts && *id < cursor_id)
                }
            })
            .take(limit)
            .collect()
    }

    /// The same page the way it used to be asked for: count `offset` rows down
    /// from the top of the same ordering.
    fn offset_page(table: &[KeyRow], offset: usize, limit: usize) -> Vec<KeyRow> {
        let mut rows = table.to_vec();
        rows.sort_by(|a, b| b.cmp(a));
        rows.into_iter().skip(offset).take(limit).collect()
    }

    fn four_logged_requests() -> Vec<KeyRow> {
        vec![
            ("2026-07-19 12:00:04.000", "req-4"),
            ("2026-07-19 12:00:03.000", "req-3"),
            ("2026-07-19 12:00:02.000", "req-2"),
            ("2026-07-19 12:00:01.000", "req-1"),
        ]
    }

    #[test]
    fn keyset_paging_neither_repeats_nor_skips_a_row_under_ingestion() {
        let mut table = four_logged_requests();
        let page = keyset_page(&table, None, 2);
        assert_eq!(
            page,
            vec![
                ("2026-07-19 12:00:04.000", "req-4"),
                ("2026-07-19 12:00:03.000", "req-3"),
            ]
        );

        // the gateway keeps logging while the operator reads the first page
        table.push(("2026-07-19 12:00:06.000", "req-6"));
        table.push(("2026-07-19 12:00:05.000", "req-5"));

        let cursor = *page.last().expect("the first page is not empty");
        let next = keyset_page(&table, Some(cursor), 2);
        // the bound is a position in the sort key, so the two new rows above it
        // move nothing: paging resumes exactly where it stopped
        assert_eq!(
            next,
            vec![
                ("2026-07-19 12:00:02.000", "req-2"),
                ("2026-07-19 12:00:01.000", "req-1"),
            ]
        );

        let seen: Vec<KeyRow> = page.iter().chain(next.iter()).copied().collect();
        let unique: BTreeSet<KeyRow> = seen.iter().copied().collect();
        assert_eq!(
            seen.len(),
            unique.len(),
            "a row was returned twice: {seen:?}"
        );
        assert_eq!(
            unique,
            four_logged_requests().into_iter().collect::<BTreeSet<_>>(),
            "every row that existed when paging started came back exactly once"
        );
    }

    #[test]
    fn offset_paging_would_have_repeated_the_rows_the_keyset_walks_past() {
        let mut table = four_logged_requests();
        let page = offset_page(&table, 0, 2);
        table.push(("2026-07-19 12:00:06.000", "req-6"));
        table.push(("2026-07-19 12:00:05.000", "req-5"));
        // two rows landed above the window, so counting two down from the top
        // lands on the page just served — and req-2/req-1 are never reachable
        assert_eq!(offset_page(&table, 2, 2), page);
    }

    #[test]
    fn a_tied_timestamp_still_pages_forward() {
        // two requests logged in the same millisecond: ts alone cannot separate
        // them, so the cursor would stall on its own timestamp forever without
        // request_id in the key
        let table = vec![
            ("2026-07-19 12:00:00.000", "req-c"),
            ("2026-07-19 12:00:00.000", "req-b"),
            ("2026-07-19 12:00:00.000", "req-a"),
        ];
        let page = keyset_page(&table, None, 2);
        let cursor = *page.last().expect("the first page is not empty");
        assert_eq!(
            keyset_page(&table, Some(cursor), 2),
            vec![("2026-07-19 12:00:00.000", "req-a")]
        );
    }

    #[test]
    fn clamp_limit_bounds() {
        assert_eq!(clamp_limit(None), 50);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(25)), 25);
        assert_eq!(clamp_limit(Some(10_000)), 200);
    }
}

#[cfg(test)]
mod query_failed_tests {
    use super::query_failed;
    use axum::body::to_bytes;
    use parking_lot::Mutex;
    use std::sync::Arc;
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Clone, Default)]
    struct WarnLogs(Arc<Mutex<Vec<String>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WarnLogs {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Visitor(String);
            impl tracing::field::Visit for Visitor {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    self.0.push_str(&format!("{}={value:?} ", field.name()));
                }
            }
            if *event.metadata().level() == tracing::Level::WARN {
                let mut visitor = Visitor(String::new());
                event.record(&mut visitor);
                self.0.lock().push(visitor.0);
            }
        }
    }

    // the database's message is for the log, never for the browser
    #[tokio::test]
    async fn the_response_carries_no_database_detail() {
        let err = anyhow::anyhow!(
            "clickhouse query failed (400 Bad Request): SELECT ts FROM request_logs WHERE host = 10.0.0.9"
        );
        let response = query_failed("analytics query failed", &err);
        assert_eq!(response.status(), axum::http::StatusCode::BAD_GATEWAY);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["message"], "analytics query failed");
        assert_eq!(json["error"]["code"], "analytics_query_failed");
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !text.contains("request_logs") && !text.contains("10.0.0.9"),
            "{text}"
        );
    }

    #[test]
    fn query_failed_redacts_urls_in_warn_log() {
        let logs = WarnLogs::default();
        let subscriber = tracing_subscriber::registry().with(logs.clone());
        let secret = "secret_pass_123";
        let err = anyhow::anyhow!(
            "clickhouse query failed (400 Bad Request): http://user:{secret}@clickhouse.internal:8123/?query=SELECT%201"
        );
        tracing::subscriber::with_default(subscriber, || {
            let _ = query_failed("analytics query failed", &err);
        });
        let seen = logs.0.lock().clone();
        assert_eq!(seen.len(), 1);
        assert!(
            !seen[0].contains(secret),
            "secret leaked in log: {}",
            seen[0]
        );
        assert!(
            seen[0].contains("http://***@clickhouse.internal:8123/"),
            "url was not redacted: {}",
            seen[0]
        );
    }
}

/// The invocation list against a table that may lack the hand-applied columns
/// (#2903). The stub answers the way ClickHouse does for the two statements the
/// list sends: a `system.columns` probe, and a select that fails on a column
/// the table does not have, so a regression to naming one unconditionally is a
/// `502` here as it is in front of a deployment that has not run the file.
#[cfg(test)]
mod optional_column_tests {
    use super::*;
    use crate::request_log_columns::{ColumnCache, OPTIONAL_COLUMNS};
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tower::ServiceExt;

    /// What a table from before `015` has: no optional column.
    const REQUIRED: &[&str] = &["ts", "request_id", "status", "cache_write_tokens"];

    fn every_optional() -> Vec<&'static str> {
        OPTIONAL_COLUMNS.iter().map(|column| column.name).collect()
    }

    #[derive(Clone)]
    struct Stub {
        /// the columns the stub's `request_logs` has
        columns: Arc<Mutex<Vec<&'static str>>>,
        /// every statement received, in order
        statements: Arc<Mutex<Vec<String>>>,
        refuse_probe: Arc<AtomicBool>,
        fail_list: Arc<AtomicBool>,
        url: String,
    }

    impl Stub {
        async fn start(columns: &[&'static str]) -> Self {
            let mut stub = Self {
                columns: Arc::new(Mutex::new(columns.to_vec())),
                statements: Arc::default(),
                refuse_probe: Arc::default(),
                fail_list: Arc::default(),
                url: String::new(),
            };
            let app = Router::new()
                .fallback(axum::routing::post(answer))
                .with_state(stub.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind a local port");
            stub.url = format!("http://{}", listener.local_addr().expect("a local address"));
            tokio::spawn(async move { axum::serve(listener, app).await });
            stub
        }

        fn probes(&self) -> usize {
            self.statements
                .lock()
                .iter()
                .filter(|sql| sql.contains("system.columns"))
                .count()
        }

        /// the last select the list sent
        fn list_sql(&self) -> String {
            self.statements
                .lock()
                .iter()
                .rfind(|sql| sql.contains("from request_logs"))
                .cloned()
                .expect("the list reached clickhouse")
        }
    }

    async fn answer(State(stub): State<Stub>, sql: String) -> Response {
        stub.statements.lock().push(sql.clone());
        if sql.contains("system.columns") {
            if stub.refuse_probe.load(Ordering::SeqCst) {
                return (
                    StatusCode::FORBIDDEN,
                    "Code: 497. DB::Exception: Not enough privileges",
                )
                    .into_response();
            }
            let rows: Vec<Value> = stub
                .columns
                .lock()
                .iter()
                .map(|name| json!({ "name": name }))
                .collect();
            return Json(json!({ "data": rows })).into_response();
        }
        // a select that names a column the table lacks is an error, as it is
        // in clickhouse; it only does so when the name stands bare in the list
        // a bare select-list entry, or a predicate that reads the column
        let missing = every_optional().into_iter().find(|name| {
            !stub.columns.lock().contains(name)
                && (sql.contains(&format!(", {name},")) || sql.contains(&format!("{name} = ")))
        });
        if let Some(name) = missing {
            return (
                StatusCode::NOT_FOUND,
                format!("Code: 47. DB::Exception: Unknown expression identifier `{name}`"),
            )
                .into_response();
        }
        if stub.fail_list.swap(false, Ordering::SeqCst) {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Code: 241. Memory limit").into_response();
        }
        Json(json!({ "data": [] })).into_response()
    }

    fn app_over(ch: ClickHouseClient) -> Router {
        let mut state = crate::tests::state_with_token(None);
        state.clickhouse = Some(ch);
        router().with_state(state)
    }

    /// #3410: a probe that cannot read `system.columns` assumes every column,
    /// so on a table without `cache_usage_reported` the summary's first select
    /// fails; it answers without the prompt-cache measurement instead of 502.
    #[tokio::test]
    async fn the_summary_degrades_when_the_probe_misses_a_lacking_column() {
        let stub = Stub::start(REQUIRED).await;
        stub.refuse_probe.store(true, Ordering::SeqCst);
        let app = app_over(ClickHouseClient::new(&stub.url));
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/v1/analytics/summary")
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::OK);
        let selects: Vec<String> = stub
            .statements
            .lock()
            .iter()
            .filter(|sql| sql.contains("from request_logs"))
            .cloned()
            .collect();
        assert_eq!(selects.len(), 2, "{selects:?}");
        assert!(selects[0].contains("cache_reported_prompt_tokens"));
        assert!(
            !selects[1].contains("cache_usage_reported"),
            "{}",
            selects[1]
        );
    }

    async fn list(app: &Router, query: &str) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/analytics/invocations{query}"))
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        let status = response.status();
        let body = to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("a readable body");
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[test]
    fn every_optional_column_is_named_through_the_probe_result() {
        let present = invocations_sql("true", &Columns::default());
        let absent = invocations_sql("true", &Columns::lacking(&every_optional()));
        for column in OPTIONAL_COLUMNS {
            let bare = format!(", {},", column.name);
            let stood_in = format!(", {} as {},", column.default, column.name);
            assert!(present.contains(&bare), "{} is not selected", column.name);
            assert!(!present.contains(&stood_in), "{}", column.name);
            assert!(absent.contains(&stood_in), "{} has no default", column.name);
            assert!(!absent.contains(&bare), "{} is still named", column.name);
        }
    }

    #[test]
    fn invocations_sql_selects_the_one_hour_share_of_a_cache_write() {
        let sql = invocations_sql("true", &Columns::default());
        // beside the total it is part of, so a reader sees the pair (#2891)
        assert!(sql.contains("cache_write_tokens, cache_write_1h_tokens,"));
    }

    async fn rollup(app: &Router) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/v1/analytics/guardrails")
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        let status = response.status();
        let body = to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("a readable body");
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn the_guardrail_rollup_splits_events_and_is_scoped_to_the_callers_rows() {
        let mut columns = REQUIRED.to_vec();
        columns.extend(every_optional());
        let stub = Stub::start(&columns).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, body) = rollup(&app).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["recorded"], true, "{body}");
        let sql = stub.list_sql();
        assert!(
            sql.contains("array join guardrail_events as event"),
            "{sql}"
        );
        assert!(sql.contains(&format!("and {ROW_VISIBLE}")), "{sql}");
        assert!(sql.contains("round(sum(sample_weight))"), "{sql}");
    }

    #[tokio::test]
    async fn the_guardrail_rollup_says_so_when_the_table_does_not_record_events() {
        let stub = Stub::start(REQUIRED).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, body) = rollup(&app).await;

        // not a 502: the file is not a condition of upgrading the control plane
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["recorded"], false, "{body}");
        assert_eq!(body["data"], json!([]), "{body}");
    }

    /// An outcome the gateway never writes is a 400 on the list and on its
    /// export alike, before ClickHouse is asked anything (#3307, #3386).
    #[tokio::test]
    async fn an_unknown_outcome_is_refused_by_the_list_and_the_export() {
        let stub = Stub::start(REQUIRED).await;
        let app = app_over(ClickHouseClient::new(&stub.url));
        for path in [
            "/api/v1/analytics/invocations",
            "/api/v1/analytics/invocations/export",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::get(format!("{path}?outcome=refused_everything"))
                        .body(Body::empty())
                        .expect("a valid request"),
                )
                .await
                .expect("the router answers");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
            let body = to_bytes(response.into_body(), 1 << 20)
                .await
                .expect("a readable body");
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let message = body["error"]["message"].as_str().unwrap_or_default();
            assert!(message.contains("refused_budget"), "{path}: {body}");
        }
    }

    async fn adjustment_rollup(app: &Router) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/v1/analytics/param-adjustments")
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        let status = response.status();
        let body = to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("a readable body");
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn the_param_adjustment_rollup_reads_the_column_when_the_table_has_it() {
        let mut columns = REQUIRED.to_vec();
        columns.extend(every_optional());
        let stub = Stub::start(&columns).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, body) = adjustment_rollup(&app).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["recorded"], true, "{body}");
        let sql = stub.list_sql();
        assert!(sql.contains("sumMap(param_adjustments"), "{sql}");
        assert!(sql.contains(&format!("and {ROW_VISIBLE}")), "{sql}");
    }

    #[tokio::test]
    async fn the_param_adjustment_rollup_says_so_when_the_table_does_not_record_them() {
        let stub = Stub::start(REQUIRED).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, body) = adjustment_rollup(&app).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["recorded"], false, "{body}");
        assert_eq!(body["data"], json!([]), "{body}");
    }

    async fn plugin_rollup(app: &Router) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/v1/analytics/plugins")
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        let status = response.status();
        let body = to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("a readable body");
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn the_plugin_rollup_splits_events_and_is_scoped_to_the_callers_rows() {
        let mut columns = REQUIRED.to_vec();
        columns.extend(every_optional());
        let stub = Stub::start(&columns).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, body) = plugin_rollup(&app).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["recorded"], true, "{body}");
        let sql = stub.list_sql();
        assert!(sql.contains("array join plugin_events as event"), "{sql}");
        assert!(sql.contains(&format!("and {ROW_VISIBLE}")), "{sql}");
        assert!(sql.contains("sample_weight as weight"), "{sql}");
        assert!(
            sql.contains("argMaxIf(kind, ts, outcome = 'error')"),
            "{sql}"
        );
    }

    #[tokio::test]
    async fn the_plugin_rollup_says_so_when_the_table_does_not_record_events() {
        let stub = Stub::start(REQUIRED).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, body) = plugin_rollup(&app).await;

        // not a 502: the file is not a condition of upgrading the control plane
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["recorded"], false, "{body}");
        assert_eq!(body["data"], json!([]), "{body}");
    }

    #[tokio::test]
    async fn the_invocation_filter_on_a_guardrail_event_survives_a_table_without_the_column() {
        let stub = Stub::start(REQUIRED).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, body) = list(&app, "?guardrail=blocked%3Aemail").await;

        assert_eq!(status, StatusCode::OK, "{body}");
        let sql = stub.list_sql();
        assert!(
            sql.contains("has(emptyArrayString(), {guardrail:String})"),
            "{sql}"
        );
    }

    #[tokio::test]
    async fn the_invocation_filter_on_a_plugin_event_survives_a_table_without_the_column() {
        let stub = Stub::start(REQUIRED).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, body) = list(&app, "?plugin=blocked%3Apii-scan").await;

        assert_eq!(status, StatusCode::OK, "{body}");
        let sql = stub.list_sql();
        assert!(
            sql.contains("has(emptyArrayString(), {plugin:String})"),
            "{sql}"
        );
    }

    #[tokio::test]
    async fn a_table_with_every_column_is_selected_in_full() {
        let mut columns = REQUIRED.to_vec();
        columns.extend(every_optional());
        let stub = Stub::start(&columns).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, body) = list(&app, "").await;

        assert_eq!(status, StatusCode::OK, "{body}");
        let sql = stub.list_sql();
        for name in every_optional() {
            assert!(sql.contains(&format!(", {name},")), "{name}: {sql}");
        }
        assert!(!sql.contains("toUInt32(0) as"), "{sql}");
    }

    #[tokio::test]
    async fn a_table_without_the_columns_still_lists_requests() {
        let stub = Stub::start(REQUIRED).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, body) = list(&app, "").await;

        assert_eq!(status, StatusCode::OK, "the list failed: {body}");
        let sql = stub.list_sql();
        assert!(
            sql.contains("toUInt32(0) as cache_write_1h_tokens"),
            "{sql}"
        );
        assert!(sql.contains("toUInt16(0) as upstream_status"), "{sql}");
        assert!(sql.contains("toUInt8(0) as attempts"), "{sql}");
        assert!(sql.contains("'' as lifecycle_operation"), "{sql}");
    }

    #[tokio::test]
    async fn only_the_column_the_table_lacks_is_replaced() {
        // a deployment that applied 015 and 016 but not 017
        let stub = Stub::start(&["ts", "upstream_status", "attempts", "lifecycle_operation"]).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, body) = list(&app, "").await;

        assert_eq!(status, StatusCode::OK, "{body}");
        let sql = stub.list_sql();
        assert!(sql.contains(", upstream_status, attempts, lifecycle_operation,"));
        assert!(sql.contains("toUInt32(0) as cache_write_1h_tokens"));
    }

    #[tokio::test]
    async fn the_probe_is_asked_once_for_many_lists() {
        let stub = Stub::start(REQUIRED).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        for _ in 0..3 {
            let (status, body) = list(&app, "").await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }

        assert_eq!(stub.probes(), 1);
    }

    #[tokio::test]
    async fn applying_the_migration_later_shows_without_a_restart() {
        let stub = Stub::start(REQUIRED).await;
        let mut ch = ClickHouseClient::new(&stub.url);
        // an answer that names a missing column is looked at again at once
        // instead of a minute on, which is the only difference from production
        ch.columns = Arc::new(ColumnCache::with_lifetimes(
            Duration::from_secs(600),
            Duration::ZERO,
        ));
        let app = app_over(ch);

        let (status, _) = list(&app, "").await;
        assert_eq!(status, StatusCode::OK);
        assert!(stub
            .list_sql()
            .contains("toUInt32(0) as cache_write_1h_tokens"));

        // the operator runs the files against ClickHouse
        let mut columns = REQUIRED.to_vec();
        columns.extend(every_optional());
        *stub.columns.lock() = columns;

        let (status, body) = list(&app, "").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(stub.list_sql().contains(", cache_write_1h_tokens,"));
        assert_eq!(stub.probes(), 2);
    }

    #[tokio::test]
    async fn a_server_that_will_not_say_is_read_as_having_every_column() {
        // the behaviour before the probe existed, so a user with no grant on
        // `system.columns` is no worse off than it was
        let mut columns = REQUIRED.to_vec();
        columns.extend(every_optional());
        let stub = Stub::start(&columns).await;
        stub.refuse_probe.store(true, Ordering::SeqCst);
        let app = app_over(ClickHouseClient::new(&stub.url));

        for _ in 0..2 {
            let (status, body) = list(&app, "").await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }

        assert!(stub.list_sql().contains(", cache_write_1h_tokens,"));
        // and it is not asked again for every page of the list
        assert_eq!(stub.probes(), 1);
    }

    #[tokio::test]
    async fn a_failed_list_makes_the_next_one_probe_again() {
        let stub = Stub::start(REQUIRED).await;
        let app = app_over(ClickHouseClient::new(&stub.url));
        stub.fail_list.store(true, Ordering::SeqCst);

        let (status, _) = list(&app, "").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(stub.probes(), 1);

        let (status, body) = list(&app, "").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(stub.probes(), 2, "the failed list left the answer in place");
    }

    #[tokio::test]
    async fn a_refused_request_does_not_probe() {
        let stub = Stub::start(REQUIRED).await;
        let app = app_over(ClickHouseClient::new(&stub.url));

        let (status, _) = list(&app, "?status=bogus").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = list(&app, "?since=not-a-date").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        assert!(stub.statements.lock().is_empty());
    }

    #[tokio::test]
    async fn an_unreachable_clickhouse_is_an_error_and_not_a_default() {
        // a closed port: nothing answers, so assuming a table would hide it
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let url = format!("http://{}", listener.local_addr().expect("a local address"));
        drop(listener);
        let ch = ClickHouseClient::new(&url);

        let err = ch
            .request_log_columns()
            .await
            .expect_err("nothing is listening");
        assert!(is_transport(&err), "{err:#}");
        // and the failure is not remembered as an answer
        assert!(ch.columns.fresh(Instant::now()).is_none());

        let (status, body) = list(&app_over(ch), "").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
        assert_eq!(body["error"]["code"], "analytics_query_failed");
    }
}
