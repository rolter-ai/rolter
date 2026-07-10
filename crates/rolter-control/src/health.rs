//! Provider stability rollups over the ClickHouse `provider_health_events` table
//! (ROL-198): uptime %, MTTR and a bucketed failure timeline, per provider and
//! target. All endpoints are read-only and window-bounded so scans stay cheap.
//!
//! Injection safety is the same as the analytics module: time bounds are passed
//! as ClickHouse query **parameters**, and the only value spliced into SQL text
//! (the timeline bucket) is validated against a fixed whitelist first. The SLA
//! target is a bound-checked float, never string-interpolated.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::analytics::{bucket_fn, client_or_503, run, window_params, WindowQuery, WHERE_WINDOW};

pub fn router() -> Router<crate::ControlState> {
    Router::new()
        .route("/api/v1/health/uptime", get(uptime))
        .route("/api/v1/health/mttr", get(mttr))
        .route("/api/v1/health/timeline", get(timeline))
}

#[derive(Debug, Deserialize)]
struct UptimeQuery {
    #[serde(flatten)]
    window: WindowQuery,
    /// SLA target as a fraction in (0, 1]; defaults to 0.99. Drives the reported
    /// error budget and breach flag.
    sla: Option<f64>,
}

/// Per provider/target uptime over the window: event counts, uptime %, and the
/// error budget consumed against the SLA target.
async fn uptime(
    State(state): State<crate::ControlState>,
    Query(q): Query<UptimeQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    // clamp the sla target into a sane open interval; it is a validated literal,
    // never a raw query string spliced into sql
    let sla = q.sla.unwrap_or(0.99).clamp(0.0001, 1.0);
    let sql = format!(
        "select provider, target_id, \
                count() as events, \
                countIf(outcome = 'ok') as ok, \
                countIf(outcome = 'error') as errors, \
                countIf(outcome = 'timeout') as timeouts, \
                round(countIf(outcome = 'ok') / count(), 6) as uptime, \
                round(1 - countIf(outcome = 'ok') / count(), 6) as failure_rate, \
                round((1 - countIf(outcome = 'ok') / count()) / (1 - {sla}), 6) as error_budget_burn, \
                (1 - countIf(outcome = 'ok') / count()) > (1 - {sla}) as sla_breached, \
                max(ts) as last_event \
         from provider_health_events where {WHERE_WINDOW} \
         group by provider, target_id order by uptime asc format JSON"
    );
    run(ch.query(&sql, &window_params(&q.window)).await)
}

/// Mean time to recovery per provider/target over the window.
///
/// A downtime episode is a maximal run of non-`ok` events bounded by `ok`
/// events. `good_before` (the count of `ok` events strictly before a row)
/// labels each episode: all bad rows of an episode and the single `ok` row that
/// recovers it share the same `good_before`, so grouping on it pairs the failure
/// onset (`min` bad `ts`) with its recovery (`min` `ok` `ts`). MTTR is the mean
/// recovery gap across episodes; `incidents` counts them.
async fn mttr(State(state): State<crate::ControlState>, Query(q): Query<WindowQuery>) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let sql = format!(
        "select provider, target_id, \
                round(avg(mttr_seconds), 1) as mttr_seconds, \
                count() as incidents \
         from ( \
             select provider, target_id, good_before, \
                    dateDiff('second', minIf(ts, outcome != 'ok'), minIf(ts, outcome = 'ok')) as mttr_seconds, \
                    countIf(outcome != 'ok') as bad_n, \
                    countIf(outcome = 'ok') as good_n \
             from ( \
                 select provider, target_id, ts, outcome, \
                        sum(outcome = 'ok') over ( \
                            partition by provider, target_id order by ts \
                            rows between unbounded preceding and 1 preceding \
                        ) as good_before \
                 from provider_health_events where {WHERE_WINDOW} \
             ) \
             group by provider, target_id, good_before \
             having bad_n > 0 and good_n > 0 and mttr_seconds > 0 \
         ) \
         group by provider, target_id order by mttr_seconds desc format JSON"
    );
    run(ch.query(&sql, &window_params(&q)).await)
}

/// Bucketed failure timeline per provider/target: ok/error/timeout counts per
/// time bucket for the dashboard's downtime strip.
async fn timeline(
    State(state): State<crate::ControlState>,
    Query(q): Query<WindowQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    let bucket = q.bucket.as_deref().unwrap_or("hour");
    let Some(bucket_expr) = bucket_fn(bucket) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "bucket must be one of hour|day|week|month"}})),
        )
            .into_response();
    };
    let sql = format!(
        "select {bucket_expr}(ts) as bucket, provider, target_id, \
                count() as events, \
                countIf(outcome = 'ok') as ok, \
                countIf(outcome = 'error') as errors, \
                countIf(outcome = 'timeout') as timeouts \
         from provider_health_events where {WHERE_WINDOW} \
         group by bucket, provider, target_id order by bucket format JSON"
    );
    run(ch.query(&sql, &window_params(&q)).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sla_clamps_into_open_interval() {
        assert_eq!(1.5_f64.clamp(0.0001, 1.0), 1.0);
        assert_eq!(0.0_f64.clamp(0.0001, 1.0), 0.0001);
        assert_eq!((-3.0_f64).clamp(0.0001, 1.0), 0.0001);
        assert_eq!(0.99_f64.clamp(0.0001, 1.0), 0.99);
    }

    #[test]
    fn timeline_bucket_defaults_and_whitelists() {
        // the shared whitelist rejects anything not in the fixed set, so no
        // untrusted bucket value ever reaches the SQL text
        assert!(bucket_fn("hour").is_some());
        assert!(bucket_fn("year").is_none());
        assert!(bucket_fn("hour; drop table provider_health_events").is_none());
    }
}
