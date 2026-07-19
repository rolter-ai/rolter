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

    /// Persist one already-sanitized MCP tool-call event. The table and insert
    /// statement are fixed here rather than supplied by a caller, so event
    /// metadata can never alter ClickHouse SQL.
    pub(crate) async fn insert_mcp_tool_call(&self, event: &Value) -> anyhow::Result<()> {
        let response = self
            .client
            .post(format!(
                "{}/?query=INSERT%20INTO%20mcp_tool_call_logs%20FORMAT%20JSONEachRow",
                self.base
            ))
            .body(serde_json::to_string(event)?)
            .send()
            .await?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("clickhouse MCP event insert failed ({status}): {body}")
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
pub(crate) const WHERE_WINDOW: &str = "ts >= if({since:String} = '', now64(3) - interval 7 day, parseDateTime64BestEffort({since:String})) \
     and ts < if({until:String} = '', now64(3), parseDateTime64BestEffort({until:String}))";

pub fn router() -> Router<crate::ControlState> {
    Router::new()
        .route("/api/v1/analytics/summary", get(summary))
        .route("/api/v1/analytics/timeseries", get(timeseries))
        .route("/api/v1/analytics/by-model", get(by_model))
        .route("/api/v1/analytics/invocations", get(invocations))
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
        Err(err) => {
            tracing::warn!(error = %err, "analytics query failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"message": err.to_string()}})),
            )
                .into_response()
        }
    }
}

/// Totals over the window: request count, tokens, cost, error count, avg latency.
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
                countIf(status >= 400) as errors, \
                round(quantile(0.5)(latency_ms), 1) as p50_latency_ms, \
                round(quantile(0.95)(latency_ms), 1) as p95_latency_ms \
         from request_logs where {WHERE_WINDOW} \
         group by model order by cost_usd desc format JSON"
    );
    run(ch.query(&sql, &window_params(&q)).await)
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
    /// status class: all|error|success (defaults to all)
    pub(crate) status: Option<String>,
    /// page size, 1..=200 (defaults to 50)
    pub(crate) limit: Option<u32>,
    /// row offset for pagination (defaults to 0)
    pub(crate) offset: Option<u32>,
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
    // all user-supplied values are passed as clickhouse params; the only spliced
    // text is the status predicate, which is whitelisted above.
    let sql = format!(
        "select ts, request_id, trace_id, org_id, team_id, project_id, virtual_key_id, \
                model, provider, target, variant, status, stream, cache_hit, cache_read_tokens, cache_write_tokens, \
                prompt_tokens, completion_tokens, total_tokens, cost_usd, latency_ms, ttft_ms, error, \
                payload.request_payload, payload.response_payload \
         from request_logs \
         left join ( \
             select request_id, argMax(request_payload, ts) as request_payload, \
                    argMax(response_payload, ts) as response_payload \
             from request_payloads group by request_id \
         ) as payload using (request_id) \
         where {WHERE_WINDOW} \
           and ({{model:String}} = '' or model = {{model:String}}) \
           and ({{key:String}} = '' or virtual_key_id = {{key:String}}) \
           and {status_expr} \
         order by ts desc \
         limit {{limit:UInt32}} offset {{offset:UInt32}} format JSON"
    );
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
    params.push(("param_limit".to_string(), limit.to_string()));
    params.push(("param_offset".to_string(), offset.to_string()));
    run(ch.query(&sql, &params).await)
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
    fn clamp_limit_bounds() {
        assert_eq!(clamp_limit(None), 50);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(25)), 25);
        assert_eq!(clamp_limit(Some(10_000)), 200);
    }
}
