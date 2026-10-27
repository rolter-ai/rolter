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
//! * A spend figure is only final when every request in it was priced. Every
//!   export carries `unpriced_requests` and a `cost_is_partial` flag (the log
//!   carries `unpriced` per row and leaves the cost blank, as the screen does),
//!   so a total with a gap is never read as the whole.
//! * Every cell goes through [`csv_cell`], which defuses a value a spreadsheet
//!   would run as a formula (a leading `=`, `+`, `-`, `@`, tab or carriage return).

use std::collections::HashMap;
use std::io;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::stream::{self, StreamExt};
use serde_json::Value;

use crate::analytics::{
    attribution_column, bucket_expr, client_or_503, invocations_keyset_predicate,
    next_invocations_cursor, query_failed, row_filter_params, split_invocations_cursor,
    status_predicate, window_params, with_access, AttributionQuery, ClickHouseClient,
    DashboardQuery, InvocationsQuery, WindowQuery, COMPLETION_TOKENS, COST, ERRORS, PROMPT_TOKENS,
    REQUESTS, ROW_FILTERS, TOKENS, UNPRICED, WHERE_WINDOW,
};
use crate::analytics_access::{AnalyticsAccess, ROW_VISIBLE};
use crate::request_log_columns::Columns;
use crate::time_bounds::Query;
use crate::ControlState;

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

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({"error": {"message": message}})),
    )
        .into_response()
}

/// The names of the business units or customers a rollup refers to, by id.
/// An id with no row (deleted since) stays unnamed. A control plane with no
/// database has no names to give, and the id column still identifies the row.
#[cfg(feature = "postgres")]
async fn names_for(
    state: &ControlState,
    dimension: &str,
    ids: &[String],
) -> HashMap<String, String> {
    let ids: Vec<uuid::Uuid> = ids.iter().filter_map(|id| id.parse().ok()).collect();
    if ids.is_empty() {
        return HashMap::new();
    }
    let sql = match dimension {
        "customer" => "select id, name from customers where id = any($1)",
        _ => "select id, name from business_units where id = any($1)",
    };
    sqlx::query_as::<_, (uuid::Uuid, String)>(sql)
        .bind(ids)
        .fetch_all(crate::crud::pool(state))
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|(id, name)| (id.to_string(), name))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(not(feature = "postgres"))]
async fn names_for(_: &ControlState, _: &str, _: &[String]) -> HashMap<String, String> {
    HashMap::new()
}

/// `GET /api/v1/analytics/by-attribution/export`: the Cost Attribution rollup,
/// one row per business unit or customer.
async fn export_attribution(
    access: AnalyticsAccess,
    State(state): State<ControlState>,
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
    let names = names_for(&state, dimension, &ids).await;
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
}

impl LogWalk {
    /// The next chunk of the file, or `None` once the log is exhausted.
    async fn next_chunk(&mut self) -> anyhow::Result<Option<Bytes>> {
        if self.done {
            return Ok(None);
        }
        let mut params = self.params.clone();
        params.push(("param_cursor_ts".to_string(), self.cursor.0.clone()));
        params.push(("param_cursor_seq".to_string(), self.cursor.1.clone()));
        params.push(("param_cursor_id".to_string(), self.cursor.2.clone()));
        params.push(("param_limit".to_string(), PAGE_SIZE.to_string()));
        let rows = self.ch.query(&self.sql, &params).await?;
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
            // a full page may be followed by older rows; a short one is the end.
            // the cursor is a position in the sort key, so rows the gateway
            // writes while the file is built can neither repeat nor skip one
            Some(next) if rows.len() == PAGE_SIZE as usize => {
                self.cursor = next
                    .split_once('|')
                    .map(|(ts, tail)| {
                        let (seq, id) = split_invocations_cursor(tail);
                        (ts.to_string(), seq, id)
                    })
                    .unwrap_or_default();
            }
            _ => self.done = true,
        }
        Ok(Some(Bytes::from(out)))
    }
}

/// `GET /api/v1/analytics/invocations/export`: the LLM Logs list as a file,
/// metadata only, under the filters the screen applies (window, model, key,
/// project, business unit, customer, status and unpriced).
async fn export_invocations(
    access: AnalyticsAccess,
    State(state): State<ControlState>,
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
    let mut params = window_params(&WindowQuery {
        since: q.since.clone(),
        until: q.until.clone(),
        bucket: None,
    });
    params.push((
        "param_unpriced".to_string(),
        u8::from(q.unpriced.unwrap_or(false)).to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
