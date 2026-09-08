//! Usage/cost aggregation over the ClickHouse `request_logs` table for the
//! dashboard. All endpoints are read-only and return ClickHouse's `FORMAT JSON`
//! `data` array straight through.
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
    /// row offset for pagination (defaults to 0)
    pub(crate) offset: Option<u32>,
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
fn invocations_sql(status_expr: &str) -> String {
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
         order by ts desc \
         limit {{limit:UInt32}} offset {{offset:UInt32}} format JSON"
    )
}

/// Individual gateway invocations, newest first. Returns every persisted column
/// of `request_logs` plus any short-retention raw payload row, so the dashboard
/// can render both the list row and its optional detail bodies.
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
    let limit = clamp_limit(q.limit);
    let offset = q.offset.unwrap_or(0);
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
    params.push(("param_limit".to_string(), limit.to_string()));
    params.push(("param_offset".to_string(), offset.to_string()));
    run(ch.query(&sql, &params).await)
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
        assert!(sql.contains("{offset:UInt32}"));
        // the shared window clause is inlined, so it must be empty-safe here too
        assert!(!sql.contains("parseDateTime64BestEffort("));
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
