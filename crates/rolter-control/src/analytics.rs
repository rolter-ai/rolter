//! Usage/cost aggregation over the ClickHouse `request_logs` table for the
//! dashboard. All endpoints are read-only and return ClickHouse's `FORMAT JSON`
//! `data` array straight through; the keyset-paged invocation list adds a
//! `next_cursor` beside it.
//!
//! Injection safety: time bounds are passed as ClickHouse query **parameters**
//! (`{since:DateTime64}` / `param_since=…`), never interpolated into SQL. The
//! only value spliced into SQL text is the time bucket, which is validated
//! against a fixed whitelist first.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

/// Minimal ClickHouse HTTP read client.
#[derive(Clone)]
pub struct ClickHouseClient {
    base: String,
    client: reqwest::Client,
}

impl ClickHouseClient {
    pub fn new(url: &str) -> Self {
        Self {
            base: url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
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
            .query(&[("default_format", "JSON")]);
        for (k, v) in params {
            req = req.query(&[(k.as_str(), v.as_str())]);
        }
        let resp = req.body(sql.to_string()).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("clickhouse query failed ({status}): {body}");
        }
        let value: Value = resp.json().await?;
        Ok(value
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default())
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
                .send()
                .await?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                anyhow::bail!("clickhouse retention change failed ({status}): {body}");
            }
        }
        Ok(())
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
                // DateTime64(3) column instead of being rejected (#1210)
                "{}/?query=INSERT%20INTO%20mcp_tool_call_logs%20FORMAT%20JSONEachRow\
                 &date_time_input_format=best_effort",
                self.base
            ))
            .body(serde_json::to_vec(event)?)
            .send()
            .await?;
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
                "{}/?query=INSERT%20INTO%20ui_events%20FORMAT%20JSONEachRow",
                self.base
            ))
            .body(body)
            .send()
            .await?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("clickhouse UX event insert failed ({status}): {text}")
    }
}

/// Map a bucket name to a ClickHouse start-of-interval function. Whitelisted so
/// the returned string is safe to splice into SQL.
pub(crate) fn bucket_fn(bucket: &str) -> Option<&'static str> {
    match bucket {
        "hour" => Some("toStartOfHour"),
        "day" => Some("toStartOfDay"),
        "week" => Some("toStartOfWeek"),
        "month" => Some("toStartOfMonth"),
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

/// The shared `where` clause. Empty since/until fall back to a default range so
/// callers can omit either bound.
///
/// The parse is the `OrZero` variant on purpose: ClickHouse constant-folds both
/// branches of `if` over constant arguments, so the strict
/// `parseDateTime64BestEffort('')` throws "Cannot read DateTime" before the
/// empty-string branch is ever selected (#1177). With `OrZero` the failed parse
/// yields an unused epoch value and the `if` still picks the default bound.
pub(crate) const WHERE_WINDOW: &str = "ts >= if({since:String} = '', now64(3) - interval 7 day, parseDateTime64BestEffortOrZero({since:String})) \
     and ts < if({until:String} = '', now64(3), parseDateTime64BestEffortOrZero({until:String}))";

pub fn router() -> Router<crate::ControlState> {
    Router::new()
        .route("/api/v1/analytics/summary", get(summary))
        .route("/api/v1/analytics/timeseries", get(timeseries))
        .route("/api/v1/analytics/by-model", get(by_model))
        .route("/api/v1/analytics/by-attribution", get(by_attribution))
        .route("/api/v1/analytics/invocations", get(invocations))
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

/// Map a status filter name to a whitelisted SQL predicate. Anything else is
/// rejected so it can never be spliced into SQL. `all` applies no filter.
pub(crate) fn status_predicate(status: &str) -> Option<&'static str> {
    match status {
        "all" => Some("1"),
        "error" => Some("status >= 400"),
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
pub(crate) fn parse_keyset_cursor(
    cursor: Option<&str>,
    id_label: &str,
) -> Result<(String, String), String> {
    let Some(cursor) = cursor.filter(|value| !value.is_empty()) else {
        return Ok((String::new(), String::new()));
    };
    let malformed = || format!("cursor must be timestamp|{id_label}");
    let Some((timestamp, id)) = cursor.split_once('|') else {
        return Err(malformed());
    };
    if timestamp.is_empty() || id.is_empty() || timestamp.len() > 64 || id.len() > 256 {
        return Err(malformed());
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
pub(crate) fn keyset_predicate(id_column: &str) -> String {
    format!(
        "({{cursor_ts:String}} = '' or ts < parseDateTime64BestEffortOrZero({{cursor_ts:String}}) \
              or (ts = parseDateTime64BestEffortOrZero({{cursor_ts:String}}) \
                  and {id_column} < {{cursor_id:String}}))"
    )
}

/// The cursor that resumes paging after the last row of `rows`, or `None` when
/// the page came back empty and there is nothing left to resume from.
pub(crate) fn next_keyset_cursor(rows: &[Value], id_column: &str) -> Option<String> {
    let row = rows.last()?;
    Some(format!(
        "{}|{}",
        row.get("ts")?.as_str()?,
        row.get(id_column)?.as_str()?
    ))
}

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
pub(crate) fn query_failed(
    what: &'static str,
    err: &(impl std::fmt::Display + ?Sized),
) -> Response {
    tracing::warn!(error = %err, "{what}");
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"error": {"message": what, "code": "analytics_query_failed"}})),
    )
        .into_response()
}

/// Totals over the window: request count, tokens, cost, error count, avg latency.
/// Totals for the window, plus how much of the window is **unpriced**.
///
/// `cost_usd` alone cannot say whether it is a complete figure: traffic against
/// a model with no price row contributes zero, which is indistinguishable from
/// traffic that genuinely cost nothing. `unpriced_requests` and
/// `unpriced_models` are what let a caller present a partial total as partial
/// rather than as final (#969).
async fn summary(
    State(state): State<crate::ControlState>,
    Query(q): Query<WindowQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let sql = format!(
        "select count() as requests, \
                sum(total_tokens) as tokens, \
                sum(prompt_tokens) as prompt_tokens, \
                sum(completion_tokens) as completion_tokens, \
                round(sum(cost_usd), 6) as cost_usd, \
                countIf(unpriced = 1) as unpriced_requests, \
                uniqIf(model, unpriced = 1) as unpriced_models, \
                countIf(status >= 400) as errors, \
                round(avg(latency_ms), 1) as avg_latency_ms \
         from request_logs where {WHERE_WINDOW} format JSON"
    );
    run(ch.query(&sql, &window_params(&q)).await)
}

/// Per-bucket time series of requests, tokens and cost.
async fn timeseries(
    State(state): State<crate::ControlState>,
    Query(q): Query<WindowQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let bucket = q.bucket.as_deref().unwrap_or("day");
    let Some(bucket_expr) = bucket_fn(bucket) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "bucket must be one of hour|day|week|month"}})),
        )
            .into_response();
    };
    let sql = format!(
        "select {bucket_expr}(ts) as bucket, \
                count() as requests, \
                sum(total_tokens) as tokens, \
                round(sum(cost_usd), 6) as cost_usd \
         from request_logs where {WHERE_WINDOW} \
         group by bucket order by bucket format JSON"
    );
    run(ch.query(&sql, &window_params(&q)).await)
}

/// Per-model aggregates: requests, tokens, cost, error rate, latency percentiles.
async fn by_model(
    State(state): State<crate::ControlState>,
    Query(q): Query<WindowQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let sql = format!(
        "select model, \
                count() as requests, \
                sum(total_tokens) as tokens, \
                round(sum(cost_usd), 6) as cost_usd, \
                countIf(unpriced = 1) as unpriced_requests, \
                countIf(status >= 400) as errors, \
                round(quantile(0.5)(latency_ms), 1) as p50_latency_ms, \
                round(quantile(0.95)(latency_ms), 1) as p95_latency_ms \
         from request_logs where {WHERE_WINDOW} \
         group by model order by cost_usd desc format JSON"
    );
    run(ch.query(&sql, &window_params(&q)).await)
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

/// Spend and usage grouped by a governance dimension. Rows the key left
/// unattributed are excluded unless `include_unattributed=true`, so a
/// business-unit chargeback report is not skewed by an empty bucket.
async fn by_attribution(
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
                count() as requests, \
                sum(total_tokens) as tokens, \
                sum(prompt_tokens) as prompt_tokens, \
                sum(completion_tokens) as completion_tokens, \
                round(sum(cost_usd), 6) as cost_usd, \
                countIf(status >= 400) as errors \
         from request_logs where {WHERE_WINDOW} \
         group by id having {attributed} order by cost_usd desc format JSON"
    );
    run(ch
        .query(
            &sql,
            &window_params(&WindowQuery {
                since: q.since.clone(),
                until: q.until.clone(),
                bucket: None,
            }),
        )
        .await)
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
    /// status class: all|error|success (defaults to all)
    pub(crate) status: Option<String>,
    /// page size, 1..=200 (defaults to 50)
    pub(crate) limit: Option<u32>,
    /// opaque `timestamp|request_id` cursor returned as the preceding page's
    /// `next_cursor`; omitted for the first page
    pub(crate) cursor: Option<String>,
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
/// `unpriced` rides along with `cost_usd` because the two are only meaningful
/// together: a zero cost means "free" when the flag is clear and "unknown" when
/// it is set. The gateway decides that per request, against the catalogue that
/// applied at the time, so a caller that instead re-derives it from today's
/// model prices re-judges old rows against new prices and drifts (#1226).
///
/// Paging is a keyset over `(ts, request_id)`, never an offset. `request_logs`
/// is written continuously by the gateway, so rows land above the window
/// between one page and the next; counting from the top then shows a row twice
/// or skips it entirely, which for this screen is the normal case rather than
/// an edge case (#1394). `request_id` makes the sort key total, so tied
/// timestamps have one order and the cursor names exactly one row.
fn invocations_sql(status_expr: &str) -> String {
    let cursor = keyset_predicate("request_id");
    format!(
        "select ts, request_id, trace_id, org_id, team_id, project_id, virtual_key_id, \
                business_unit_id, customer_id, \
                model, provider, target, variant, status, stream, cache_hit, cache_read_tokens, cache_write_tokens, \
                prompt_tokens, completion_tokens, total_tokens, cost_usd, unpriced, latency_ms, ttft_ms, error, \
                payload.request_payload as request_payload, payload.response_payload as response_payload \
         from request_logs \
         left join ( \
             select request_id, argMax(request_payload, ts) as request_payload, \
                    argMax(response_payload, ts) as response_payload \
             from request_payloads group by request_id \
         ) as payload using (request_id) \
         where {WHERE_WINDOW} \
           and ({{model:String}} = '' or model = {{model:String}}) \
           and ({{key:String}} = '' or virtual_key_id = {{key:String}}) \
           and ({{business_unit:String}} = '' \
                or has(splitByChar(',', {{business_unit:String}}), business_unit_id)) \
           and ({{customer:String}} = '' \
                or has(splitByChar(',', {{customer:String}}), customer_id)) \
           and {status_expr} \
           and {cursor} \
         order by ts desc, request_id desc \
         limit {{limit:UInt32}} format JSON"
    )
}

/// Individual gateway invocations, newest first. Returns every persisted column
/// of `request_logs` plus any short-retention raw payload row, so the dashboard
/// can render both the list row and its optional detail bodies.
///
/// Answers `{data, next_cursor}`: feeding `next_cursor` back as `cursor` walks
/// the list without an offset, so rows the gateway writes while an operator
/// reads cannot shift the window under them.
async fn invocations(
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
    let (cursor_ts, cursor_id) = match parse_keyset_cursor(q.cursor.as_deref(), "request_id") {
        Ok(cursor) => cursor,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": message}})),
            )
                .into_response()
        }
    };
    let limit = clamp_limit(q.limit);
    let sql = invocations_sql(status_expr);
    let mut params = window_params(&WindowQuery {
        since: q.since.clone(),
        until: q.until.clone(),
        bucket: None,
    });
    params.push((
        "param_model".to_string(),
        q.model.clone().unwrap_or_default(),
    ));
    params.push(("param_key".to_string(), q.key.clone().unwrap_or_default()));
    params.push((
        "param_business_unit".to_string(),
        q.business_unit.clone().unwrap_or_default(),
    ));
    params.push((
        "param_customer".to_string(),
        q.customer.clone().unwrap_or_default(),
    ));
    params.push(("param_cursor_ts".to_string(), cursor_ts));
    params.push(("param_cursor_id".to_string(), cursor_id));
    params.push(("param_limit".to_string(), limit.to_string()));
    match ch.query(&sql, &params).await {
        Ok(data) => {
            let next_cursor = next_keyset_cursor(&data, "request_id");
            Json(json!({"data": data, "next_cursor": next_cursor})).into_response()
        }
        Err(error) => query_failed("analytics query failed", &error),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn bucket_fn_whitelists() {
        assert_eq!(bucket_fn("hour"), Some("toStartOfHour"));
        assert_eq!(bucket_fn("day"), Some("toStartOfDay"));
        assert_eq!(bucket_fn("week"), Some("toStartOfWeek"));
        assert_eq!(bucket_fn("month"), Some("toStartOfMonth"));
        // anything else is rejected, so it can never be spliced into SQL
        assert_eq!(bucket_fn("day; drop table request_logs"), None);
        assert_eq!(bucket_fn(""), None);
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
        let sql = invocations_sql(status_predicate("all").expect("all is whitelisted"));
        // an unaliased qualified column is named `payload.request_payload` in
        // clickhouse's JSON output, which the dashboard never reads (#1177)
        assert!(!sql.contains("payload.request_payload, payload.response_payload"));
        assert!(sql.contains("payload.request_payload as request_payload"));
        assert!(sql.contains("payload.response_payload as response_payload"));
    }

    #[test]
    fn invocations_sql_filters_both_attribution_dimensions_as_params() {
        let sql = invocations_sql(status_predicate("all").expect("all is whitelisted"));
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

    #[test]
    fn invocations_sql_selects_the_unpriced_flag_next_to_cost() {
        let sql = invocations_sql(status_predicate("all").expect("all is whitelisted"));
        // a zero cost_usd is ambiguous on its own: free, or no price row at all.
        // the flag the gateway recorded per request has to travel with it, or
        // the dashboard re-derives it from the live catalogue and drifts (#1226)
        assert!(sql.contains("cost_usd, unpriced"));
    }

    #[test]
    fn invocations_sql_binds_filters_as_params_and_splices_only_the_status() {
        let sql = invocations_sql(status_predicate("error").expect("error is whitelisted"));
        assert!(sql.contains("and status >= 400"));
        assert!(sql.contains("{model:String}"));
        assert!(sql.contains("{key:String}"));
        assert!(sql.contains("{limit:UInt32}"));
        assert!(sql.contains("{cursor_ts:String}"));
        assert!(sql.contains("{cursor_id:String}"));
        // the shared window clause is inlined, so it must be empty-safe here too
        assert!(!sql.contains("parseDateTime64BestEffort("));
    }

    #[test]
    fn invocations_sql_pages_on_a_keyset_over_a_total_sort_key() {
        let sql = invocations_sql(status_predicate("all").expect("all is whitelisted"));
        // an offset counts rows from the top of a table the gateway is still
        // writing to, so a row ingested between two pages repeats or hides one
        // (#1394)
        assert!(!sql.contains("offset"));
        assert!(sql.contains(&keyset_predicate("request_id")));
        // request_id is what makes the sort key total: without it tied
        // timestamps have no defined order and the cursor names no single row
        assert!(sql.contains("order by ts desc, request_id desc"));
    }

    #[test]
    fn an_absent_invocations_cursor_is_never_strict_parsed() {
        let sql = invocations_sql(status_predicate("all").expect("all is whitelisted"));
        // clickhouse evaluates the parse of the bound constant even when the
        // `= ''` disjunct short-circuits it, so a strict parse fails the whole
        // first page (#1177)
        assert!(!sql.contains("parseDateTime64BestEffort("));
        // two for the shared window bounds, two for the keyset cursor
        assert_eq!(sql.matches("parseDateTime64BestEffortOrZero(").count(), 4);
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
        assert_eq!(status_predicate("error"), Some("status >= 400"));
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
}
