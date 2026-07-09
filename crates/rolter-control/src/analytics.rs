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
    async fn query(&self, sql: &str, params: &[(String, String)]) -> anyhow::Result<Vec<Value>> {
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
}

/// Map a bucket name to a ClickHouse start-of-interval function. Whitelisted so
/// the returned string is safe to splice into SQL.
fn bucket_fn(bucket: &str) -> Option<&'static str> {
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
    since: Option<String>,
    /// exclusive upper bound (RFC3339); defaults to now
    until: Option<String>,
    /// time bucket for the timeseries endpoint: hour|day|week|month
    bucket: Option<String>,
}

/// Build the `param_*` bindings for the time window, applying defaults.
fn window_params(q: &WindowQuery) -> Vec<(String, String)> {
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
const WHERE_WINDOW: &str = "ts >= if({since:String} = '', now64(3) - interval 7 day, parseDateTime64BestEffort({since:String})) \
     and ts < if({until:String} = '', now64(3), parseDateTime64BestEffort({until:String}))";

pub fn router() -> Router<crate::ControlState> {
    Router::new()
        .route("/api/v1/analytics/summary", get(summary))
        .route("/api/v1/analytics/timeseries", get(timeseries))
        .route("/api/v1/analytics/by-model", get(by_model))
}

#[allow(clippy::result_large_err)]
fn client_or_503(state: &crate::ControlState) -> Result<&ClickHouseClient, Response> {
    state.clickhouse.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": {"message": "analytics unavailable: no clickhouse_url configured"}})),
        )
            .into_response()
    })
}

fn run(rows: anyhow::Result<Vec<Value>>) -> Response {
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
}
