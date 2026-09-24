//! Provider stability rollups over the ClickHouse `provider_health_events` table
//! (ROL-198): uptime %, MTTR and a bucketed failure timeline, per provider and
//! target. All endpoints are read-only and window-bounded so scans stay cheap.
//!
//! # Two grains, named
//!
//! The gateway watches a provider at two different grains and writes both to
//! this one table. Probe and status-page events describe the *provider* and
//! carry its own name as the `target_id`; passive events describe one route
//! through it and carry the real target. Grouping by `(provider, target_id)`
//! alone therefore made a provider surface twice — once as itself, once per
//! target it serves — as two unrelated rows with wildly different counts
//! (#1257).
//!
//! Every row now reports `grain`: `provider` when the row is about the provider
//! as a whole, `target` when it is about one route through it. The counts are
//! deliberately *not* merged, because they measure different things — a probe
//! every `probe_interval_secs` against a request-driven passive observation —
//! so the dashboard nests the target rows under their provider instead of
//! adding them up. `uptime` also returns `sources`, the distinct event sources
//! behind the row, so a card can say what actually fed it.
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

use crate::analytics::{
    bucket_fn, client_or_503, run, window_params, with_access, WindowQuery, WHERE_WINDOW,
};
use crate::analytics_access::{AnalyticsAccess, PROVIDER_VISIBLE};

/// How wide a rollup row is. Probe and status-page events name the provider
/// itself as their `target_id`, passive events name the route's real target, so
/// this expression recovers which of the two a grouped row describes.
const GRAIN: &str = "if(target_id = provider, 'provider', 'target') as grain";

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
    access: AnalyticsAccess,
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
    run(ch
        .query(
            &uptime_sql(sla),
            &with_access(window_params(&q.window), &access),
        )
        .await)
}

/// The uptime rollup. `sla` must already be clamped by the caller — it is
/// spliced into the SQL text as a float literal, so it is never a raw query
/// string. Rows carry their [`grain`](self) and the distinct sources behind
/// them.
fn uptime_sql(sla: f64) -> String {
    format!(
        "select provider, target_id, \
                {GRAIN}, \
                arraySort(groupUniqArray(source)) as sources, \
                count() as events, \
                countIf(outcome = 'ok') as ok, \
                countIf(outcome = 'error') as errors, \
                countIf(outcome = 'timeout') as timeouts, \
                round(countIf(outcome = 'ok') / count(), 6) as uptime, \
                round(1 - countIf(outcome = 'ok') / count(), 6) as failure_rate, \
                round((1 - countIf(outcome = 'ok') / count()) / (1 - {sla}), 6) as error_budget_burn, \
                (1 - countIf(outcome = 'ok') / count()) > (1 - {sla}) as sla_breached, \
                max(ts) as last_event \
         from provider_health_events where {WHERE_WINDOW} and {PROVIDER_VISIBLE} \
         group by provider, target_id, grain order by uptime asc format JSON"
    )
}

/// Mean time to recovery per provider/target over the window.
///
/// A downtime episode is a maximal run of non-`ok` events bounded by `ok`
/// events. `good_before` (the count of `ok` events strictly before a row)
/// labels each episode: all bad rows of an episode and the single `ok` row that
/// recovers it share the same `good_before`, so grouping on it pairs the failure
/// onset (`min` bad `ts`) with its recovery (`min` `ok` `ts`). MTTR is the mean
/// recovery gap across episodes; `incidents` counts them.
async fn mttr(
    access: AnalyticsAccess,
    State(state): State<crate::ControlState>,
    Query(q): Query<WindowQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };
    run(ch
        .query(&mttr_sql(), &with_access(window_params(&q), &access))
        .await)
}

/// The MTTR rollup. Episodes are detected per `(provider, target_id)`, so a
/// probe outage and a passive outage on the same provider stay separate
/// incidents; each row names its [`grain`](self).
fn mttr_sql() -> String {
    format!(
        "select provider, target_id, \
                {GRAIN}, \
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
                 from provider_health_events where {WHERE_WINDOW} and {PROVIDER_VISIBLE} \
             ) \
             group by provider, target_id, good_before \
             having bad_n > 0 and good_n > 0 and mttr_seconds > 0 \
         ) \
         group by provider, target_id, grain order by mttr_seconds desc format JSON"
    )
}

/// Bucketed failure timeline per provider/target: ok/error/timeout counts per
/// time bucket for the dashboard's downtime strip.
async fn timeline(
    access: AnalyticsAccess,
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
    run(ch
        .query(
            &timeline_sql(bucket_expr),
            &with_access(window_params(&q), &access),
        )
        .await)
}

/// The bucketed failure timeline. `bucket_expr` has already been checked
/// against [`bucket_fn`]'s fixed whitelist by the caller, so no untrusted value
/// reaches the SQL text.
fn timeline_sql(bucket_expr: &str) -> String {
    format!(
        "select {bucket_expr}(ts) as bucket, provider, target_id, \
                {GRAIN}, \
                count() as events, \
                countIf(outcome = 'ok') as ok, \
                countIf(outcome = 'error') as errors, \
                countIf(outcome = 'timeout') as timeouts \
         from provider_health_events where {WHERE_WINDOW} and {PROVIDER_VISIBLE} \
         group by bucket, provider, target_id, grain order by bucket format JSON"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rollup_is_filtered_to_the_providers_the_caller_may_read() {
        // a health row names a provider and carries no org, so the provider
        // filter is the only thing between a rollup and every tenant (#1820)
        for sql in [uptime_sql(0.99), mttr_sql(), timeline_sql("toStartOfHour")] {
            assert!(
                sql.contains(&format!("{WHERE_WINDOW} and {PROVIDER_VISIBLE}")),
                "{sql}"
            );
        }
    }

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

    #[test]
    fn every_rollup_names_its_grain() {
        // without this the probe row (target_id = provider) and the passive
        // rows for the same provider come back as unrelated peers, which is
        // what made one provider render as several duplicate cards
        for sql in [uptime_sql(0.99), mttr_sql(), timeline_sql("toStartOfHour")] {
            assert!(
                sql.contains("if(target_id = provider, 'provider', 'target') as grain"),
                "missing grain in: {sql}"
            );
            assert!(sql.contains(", grain order by"), "grain not grouped: {sql}");
        }
    }

    #[test]
    fn uptime_reports_the_sources_behind_a_row() {
        let sql = uptime_sql(0.99);
        assert!(sql.contains("arraySort(groupUniqArray(source)) as sources"));
    }

    #[test]
    fn uptime_splices_only_the_clamped_sla_literal() {
        let sql = uptime_sql(0.995);
        assert!(sql.contains("(1 - 0.995)"));
        // the window bounds stay parameters, never text
        assert!(sql.contains("{since:String}"));
    }
}
