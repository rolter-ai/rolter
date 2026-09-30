//! The time bounds the ClickHouse read endpoints accept, checked before a query
//! is built from them (#1192).
//!
//! Every analytics, provider-health, usage and MCP-log read narrows its scan
//! with a caller-supplied `since`/`until`, and the two keyset-paged lists resume
//! from the timestamp inside a caller-supplied cursor. ClickHouse reads all of
//! them with `parseDateTime64BestEffortOrZero`, which the empty-bound defaults
//! need (#1177) but which answers the epoch for anything it cannot read. A typo
//! was therefore never an error: `?since=not-a-date` became a scan from 1970,
//! and so did an RFC 3339 offset whose `+` the client forgot to percent-encode,
//! because a query string decodes `+` to a space.
//!
//! [`is_time_bound`] accepts exactly the shapes ClickHouse reads back to the
//! instant the caller wrote, and [`Query`] applies it to `since` and `until`
//! before a handler runs. A bound outside the grammar is a `400` in the
//! OpenAI-style error envelope, naming the parameter. A bound inside it reaches
//! ClickHouse byte for byte as it arrived, so a valid window means exactly what
//! it meant before.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{NaiveDate, NaiveTime};
use serde::de::DeserializeOwned;
use serde_json::json;

/// Whether `value` is a time bound ClickHouse reads back to the instant it
/// names.
///
/// The grammar, all ASCII:
///
/// ```text
/// bound = date [ ("T" / " ") time [ zone ] ]
/// date  = YYYY "-" MM "-" DD          a real calendar day, year 0001-9999
/// time  = hh ":" mm [ ":" ss [ "." 1*9 digit ] ]
/// zone  = "Z" / ("+" / "-") hh ":" mm
/// ```
///
/// That covers RFC 3339 the way the dashboard and the common SDKs write it, the
/// `YYYY-MM-DD hh:mm:ss.sss` form ClickHouse itself returns (and so the form
/// inside every cursor this API hands out), and a bare date. A value with no
/// zone is read in the ClickHouse server's time zone, as it always was.
///
/// It is narrower than RFC 3339 exactly where ClickHouse misreads it: a
/// lowercase `t` or `z`, year `0000` and a day the calendar does not have all
/// parse to the epoch, so all of them are refused here. Years outside the
/// `1900`-`2299` range `DateTime64` stores are accepted, because ClickHouse
/// clamps them to the nearest end of that range rather than to the epoch.
pub(crate) fn is_time_bound(value: &str) -> bool {
    scan(value.as_bytes()).is_some()
}

/// [`is_time_bound`]'s grammar, walked left to right.
fn scan(mut rest: &[u8]) -> Option<()> {
    let year = digits(&mut rest, 4)?;
    literal(&mut rest, b'-')?;
    let month = digits(&mut rest, 2)?;
    literal(&mut rest, b'-')?;
    let day = digits(&mut rest, 2)?;
    // chrono has a year zero; ClickHouse reads it as the epoch
    if year == 0 {
        return None;
    }
    NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)?;
    let Some((&separator, time)) = rest.split_first() else {
        return Some(());
    };
    if separator != b'T' && separator != b' ' {
        return None;
    }
    rest = time;
    let hour = digits(&mut rest, 2)?;
    literal(&mut rest, b':')?;
    let minute = digits(&mut rest, 2)?;
    let mut second = 0;
    if literal(&mut rest, b':').is_some() {
        second = digits(&mut rest, 2)?;
        if literal(&mut rest, b'.').is_some() {
            let fraction = rest.iter().take_while(|b| b.is_ascii_digit()).count();
            if !(1..=9).contains(&fraction) {
                return None;
            }
            rest = &rest[fraction..];
        }
    }
    // no leap second: ClickHouse rolls `:60` into the next minute
    NaiveTime::from_hms_opt(hour, minute, second)?;
    match rest {
        [] | [b'Z'] => Some(()),
        [b'+' | b'-', offset @ ..] => {
            let mut offset = offset;
            let hours = digits(&mut offset, 2)?;
            literal(&mut offset, b':')?;
            let minutes = digits(&mut offset, 2)?;
            (hours <= 23 && minutes <= 59 && offset.is_empty()).then_some(())
        }
        _ => None,
    }
}

/// Consume exactly `width` ASCII digits and return their value.
fn digits(rest: &mut &[u8], width: usize) -> Option<u32> {
    let field = rest.get(..width)?;
    if !field.iter().all(u8::is_ascii_digit) {
        return None;
    }
    *rest = &rest[width..];
    Some(
        field
            .iter()
            .fold(0, |value, digit| value * 10 + u32::from(digit - b'0')),
    )
}

/// Consume `byte` if it is next.
fn literal(rest: &mut &[u8], byte: u8) -> Option<()> {
    let (&next, tail) = rest.split_first()?;
    if next != byte {
        return None;
    }
    *rest = tail;
    Some(())
}

/// Why `value` is not a time bound, worded for the caller who sent it as
/// `param`.
fn bound_message(param: &str, value: &str) -> String {
    // `+` in a query string decodes to a space, so an offset that was not
    // percent-encoded arrives as `... 02:00` and would otherwise be a mystery
    let plus_became_space = value
        .rfind(' ')
        .is_some_and(|at| is_time_bound(&format!("{}+{}", &value[..at], &value[at + 1..])));
    if plus_became_space {
        format!(
            "{param} must be an RFC 3339 timestamp; its `+` offset arrived as a space, \
             so send it percent-encoded as %2B"
        )
    } else {
        format!("{param} must be an RFC 3339 timestamp such as 2026-07-01T00:00:00Z")
    }
}

/// A `400` in the OpenAI-style error envelope the gateway uses, naming the
/// query parameter that was refused when there is one to name.
#[derive(Debug)]
pub(crate) struct InvalidParam {
    param: Option<&'static str>,
    code: &'static str,
    message: String,
}

impl InvalidParam {
    /// A keyset cursor that is malformed or carries no readable timestamp.
    pub(crate) fn cursor(message: String) -> Self {
        Self {
            param: Some("cursor"),
            code: "invalid_cursor",
            message,
        }
    }
}

impl IntoResponse for InvalidParam {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {
                "message": self.message,
                "type": "invalid_request_error",
                "param": self.param,
                "code": self.code,
            }})),
        )
            .into_response()
    }
}

/// A query string carrying a `since`/`until` window, which [`Query`] checks
/// before a handler sees it.
pub(crate) trait TimeBounds {
    /// The `since` and `until` the caller sent, in that order.
    fn time_bounds(&self) -> (Option<&str>, Option<&str>);
}

/// axum's `Query` for a query string that carries a time window.
///
/// Both bounds are checked with [`is_time_bound`] before the handler runs, and
/// every failure, a malformed bound or a query string that does not deserialize
/// at all, is an [`InvalidParam`] rather than axum's plain-text rejection. An
/// empty bound is the same as an absent one: both mean the default.
///
/// The modules that read ClickHouse import this in place of axum's extractor,
/// so a handler added there is checked without having to remember to be, and
/// the `T: TimeBounds` bound makes a query type that never said where its
/// window is a compile error rather than an unchecked scan.
pub(crate) struct Query<T>(pub(crate) T);

impl<S, T> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned + TimeBounds,
    S: Send + Sync,
{
    type Rejection = InvalidParam;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let axum::extract::Query(query) = axum::extract::Query::<T>::try_from_uri(&parts.uri)
            .map_err(|rejection| InvalidParam {
                param: None,
                code: "invalid_query",
                message: rejection.body_text(),
            })?;
        let (since, until) = query.time_bounds();
        for (param, value) in [("since", since), ("until", until)] {
            let Some(value) = value.filter(|value| !value.is_empty()) else {
                continue;
            };
            if !is_time_bound(value) {
                return Err(InvalidParam {
                    param: Some(param),
                    code: "invalid_time_bound",
                    message: bound_message(param, value),
                });
            }
        }
        Ok(Self(query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use axum::Router;
    use serde::Deserialize;
    use serde_json::Value;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;
    use tower::ServiceExt as _;

    #[test]
    fn every_shape_clickhouse_reads_to_the_written_instant_is_a_bound() {
        for value in [
            // what the dashboard sends: `Date.prototype.toISOString()`
            "2026-07-01T00:00:00.000Z",
            "2026-07-01T00:00:00Z",
            "2026-07-01T00:00:00.123456789Z",
            "2026-07-01T00:00:00+02:00",
            "2026-07-01T00:00:00-05:30",
            "2026-07-01 00:00:00Z",
            // what clickhouse returns for a DateTime64(3), so every cursor
            "2026-07-19 12:00:01.000",
            "2026-07-01T12:00:01",
            "2026-07-01T00:00Z",
            "2026-07-01",
            "2024-02-29T00:00:00Z",
            // outside DateTime64's range, which clickhouse clamps rather than
            // reading as the epoch
            "0001-01-01T00:00:00Z",
            "9999-12-31T23:59:59Z",
        ] {
            assert!(is_time_bound(value), "{value} should be a time bound");
        }
    }

    #[test]
    fn every_shape_clickhouse_would_widen_to_the_epoch_is_refused() {
        for value in [
            "",
            "not-a-date",
            // each of these parses to 1970-01-01 in ClickHouse's best-effort
            // reader, which is the scan-everything bug this module exists for
            "2026-07-01t00:00:00z",
            "2026-07-01T00:00:00z",
            "0000-01-01T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2025-02-29",
            // the unencoded `+` of an offset, decoded to a space
            "2026-07-01T00:00:00 02:00",
            // not dates at all, or not ones this grammar names
            "1719792000",
            "20260701",
            "2026-7-1",
            "2026-13-01",
            "2026-07-01T24:00:00Z",
            "2026-07-01T23:60:00Z",
            "2026-07-01T23:59:60Z",
            "2026-07-01T00:00:00+0200",
            "2026-07-01T00:00:00+24:00",
            "2026-07-01T00:00:00.Z",
            "2026-07-01T00:00:00.1234567891Z",
            "2026-07-01Z",
            " 2026-07-01",
            "2026-07-01 ",
            "2026-07-01T",
            "2026-07-01T00",
            "2026-07-01T00:00:00Z; drop table request_logs",
            "\u{ff12}026-07-01",
        ] {
            assert!(!is_time_bound(value), "{value:?} should be refused");
        }
    }

    #[test]
    fn the_message_names_the_parameter_and_spots_a_plus_decoded_to_a_space() {
        assert_eq!(
            bound_message("since", "not-a-date"),
            "since must be an RFC 3339 timestamp such as 2026-07-01T00:00:00Z"
        );
        let message = bound_message("until", "2026-07-01T00:00:00 02:00");
        assert!(message.starts_with("until must be"), "{message}");
        assert!(message.contains("%2B"), "{message}");
        // a space that is not a lost `+` gets the plain message
        assert!(!bound_message("since", "2026-07-01 junk").contains("%2B"));
    }

    #[derive(Debug, Deserialize)]
    struct Window {
        since: Option<String>,
        until: Option<String>,
        limit: Option<u32>,
    }

    impl TimeBounds for Window {
        fn time_bounds(&self) -> (Option<&str>, Option<&str>) {
            (self.since.as_deref(), self.until.as_deref())
        }
    }

    async fn extract(uri: &str) -> Result<Window, Value> {
        let (mut parts, _) = Request::get(uri)
            .body(())
            .expect("a valid request")
            .into_parts();
        match Query::<Window>::from_request_parts(&mut parts, &()).await {
            Ok(Query(window)) => Ok(window),
            Err(rejection) => {
                let response = rejection.into_response();
                assert_eq!(response.status(), StatusCode::BAD_REQUEST);
                let body = to_bytes(response.into_body(), 1 << 16)
                    .await
                    .expect("a readable body");
                Err(serde_json::from_slice(&body).expect("a json body"))
            }
        }
    }

    #[tokio::test]
    async fn a_valid_or_empty_window_passes_through_untouched() {
        let window = extract("/?since=2026-07-01T00:00:00.000Z&until=2026-07-08%2000:00:00")
            .await
            .expect("a valid window");
        assert_eq!(window.since.as_deref(), Some("2026-07-01T00:00:00.000Z"));
        assert_eq!(window.until.as_deref(), Some("2026-07-08 00:00:00"));
        // empty still means "the default", exactly as before
        let window = extract("/?since=&until=").await.expect("an empty window");
        assert_eq!(window.since.as_deref(), Some(""));
        // and the rest of the query string deserializes as axum's would
        let window = extract("/?limit=5").await.expect("no window at all");
        assert_eq!((window.since, window.limit), (None, Some(5)));
    }

    #[tokio::test]
    async fn a_malformed_bound_is_an_openai_style_400_naming_it() {
        for param in ["since", "until"] {
            let error = extract(&format!("/?{param}=not-a-date"))
                .await
                .expect_err("a malformed bound");
            assert_eq!(error["error"]["param"], param);
            assert_eq!(error["error"]["type"], "invalid_request_error");
            assert_eq!(error["error"]["code"], "invalid_time_bound");
            assert!(
                error["error"]["message"]
                    .as_str()
                    .is_some_and(|m| m.starts_with(param)),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn a_query_string_that_does_not_deserialize_is_json_too() {
        // axum's own rejection is text/plain, which the dashboard cannot read
        let error = extract("/?limit=many").await.expect_err("a bad limit");
        assert_eq!(error["error"]["code"], "invalid_query");
        assert_eq!(error["error"]["type"], "invalid_request_error");
        assert!(error["error"]["param"].is_null());
        assert!(
            error["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("limit")),
            "{error}"
        );
    }

    /// Every route that reads a caller-supplied window from ClickHouse, beside
    /// the id column its keyset cursor is over when it takes one.
    const WINDOWED_ROUTES: &[(&str, Option<&str>)] = &[
        ("/api/v1/analytics/summary", None),
        ("/api/v1/analytics/timeseries", None),
        ("/api/v1/analytics/by-model", None),
        ("/api/v1/analytics/by-attribution", None),
        ("/api/v1/analytics/invocations", Some("request_id")),
        ("/api/v1/health/uptime", None),
        ("/api/v1/health/mttr", None),
        ("/api/v1/health/timeline", None),
        ("/api/v1/mcp/logs", Some("event_id")),
        ("/api/v1/mcp/logs/summary", None),
        ("/api/v1/me/usage", None),
    ];

    /// Routes that document a `since`, `until` or `cursor` but never hand it to
    /// a best-effort parse, each with the reason.
    const CHECKED_ELSEWHERE: &[(&str, &str)] = &[
        (
            "/api/v1/orgs/{org_id}/audit-log",
            "postgres-backed: `start_at`/`end_at` deserialize as `DateTime<Utc>` and the \
             cursor through `parse_audit_cursor`, so a malformed one is already refused",
        ),
        (
            "/api/v1/audit-log",
            "the deployment-wide read shares the per-org handler's query parsing",
        ),
    ];

    /// Whether this module's router harness can reach `path`. `/me/usage`
    /// authenticates a live session before its query string is read, so the
    /// postgres integration suite covers it; the MCP log routes only exist in
    /// a postgres build.
    fn reachable_here(path: &str) -> bool {
        path != "/api/v1/me/usage"
            && (cfg!(feature = "postgres") || !path.starts_with("/api/v1/mcp/"))
    }

    #[test]
    fn every_documented_time_bound_is_on_a_checked_route() {
        // the served document is the list a caller builds requests from, so a
        // new windowed read that is documented but missing from the table
        // above fails here instead of shipping unchecked
        let doc = crate::openapi::document();
        let paths = doc["paths"].as_object().expect("the document has paths");
        let known: BTreeSet<&str> = WINDOWED_ROUTES
            .iter()
            .map(|(path, _)| *path)
            .chain(CHECKED_ELSEWHERE.iter().map(|(path, _)| *path))
            .collect();
        let mut unchecked = Vec::new();
        let mut found = 0;
        for (path, item) in paths {
            let takes_a_bound = item["get"]["parameters"].as_array().is_some_and(|ps| {
                ps.iter().any(|p| {
                    p["in"] == "query"
                        && ["since", "until", "cursor"]
                            .contains(&p["name"].as_str().unwrap_or_default())
                })
            });
            if takes_a_bound {
                found += 1;
                if !known.contains(path.as_str()) {
                    unchecked.push(path.clone());
                }
            }
        }
        assert!(
            unchecked.is_empty(),
            "these routes document a time bound this module does not check: {unchecked:?}"
        );
        // and the scan found them, rather than passing by finding nothing
        assert_eq!(found, known.len());
    }

    /// Every `param_*` binding one ClickHouse query carried.
    type Bindings = BTreeMap<String, String>;

    /// A stand-in ClickHouse that answers every query with no rows and keeps
    /// the bindings it was sent, so a test can see what reached the database.
    async fn fake_clickhouse() -> (String, Arc<parking_lot::Mutex<Vec<Bindings>>>) {
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let record = seen.clone();
        let app = Router::new().fallback(
            move |axum::extract::Query(bindings): axum::extract::Query<Bindings>| {
                let record = record.clone();
                async move {
                    record.lock().push(bindings);
                    Json(json!({"data": []}))
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let url = format!("http://{}", listener.local_addr().expect("a local address"));
        tokio::spawn(async move { axum::serve(listener, app).await });
        (url, seen)
    }

    /// Every windowed router, in open mode, reading from `clickhouse_url`.
    fn windowed_app(clickhouse_url: &str) -> Router {
        let mut state = crate::tests::state_with_token(None);
        state.clickhouse = Some(crate::analytics::ClickHouseClient::new(clickhouse_url));
        let router = crate::analytics::router().merge(crate::health::router());
        #[cfg(feature = "postgres")]
        let router = router.merge(crate::mcp_logs::router());
        router.with_state(state)
    }

    async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::get(uri)
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
    async fn a_malformed_bound_is_refused_on_every_route_before_clickhouse_sees_it() {
        let (url, seen) = fake_clickhouse().await;
        let app = windowed_app(&url);
        let routes: Vec<&str> = WINDOWED_ROUTES
            .iter()
            .map(|(path, _)| *path)
            .filter(|path| reachable_here(path))
            .collect();
        assert!(routes.len() >= 8, "{routes:?}");
        for path in routes {
            for param in ["since", "until"] {
                for bad in ["not-a-date", "2026-07-01T00:00:00+02:00", "2026-02-30"] {
                    let (status, body) = get(&app, &format!("{path}?{param}={bad}")).await;
                    assert_eq!(status, StatusCode::BAD_REQUEST, "{path}?{param}={bad}");
                    assert_eq!(body["error"]["param"], param, "{path}: {body}");
                    assert_eq!(body["error"]["code"], "invalid_time_bound", "{path}");
                    assert_eq!(body["error"]["type"], "invalid_request_error", "{path}");
                }
            }
        }
        let seen = seen.lock();
        assert!(
            seen.is_empty(),
            "a refused bound still reached clickhouse: {seen:?}"
        );
    }

    #[tokio::test]
    async fn a_valid_bound_reaches_clickhouse_byte_for_byte() {
        let (url, seen) = fake_clickhouse().await;
        let app = windowed_app(&url);
        for (path, _) in WINDOWED_ROUTES.iter().filter(|(p, _)| reachable_here(p)) {
            let (status, body) = get(
                &app,
                &format!(
                    "{path}?since=2026-07-01T00:00:00.000%2B02:00&until=2026-07-08%2000:00:00"
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{path}: {body}");
            let bindings = seen.lock().pop().expect("the query reached clickhouse");
            assert_eq!(
                bindings["param_since"], "2026-07-01T00:00:00.000+02:00",
                "{path}"
            );
            assert_eq!(bindings["param_until"], "2026-07-08 00:00:00", "{path}");

            // no bounds still binds the empty strings the defaults key on
            let (status, _) = get(&app, path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            let bindings = seen.lock().pop().expect("the query reached clickhouse");
            assert_eq!(bindings["param_since"], "", "{path}");
            assert_eq!(bindings["param_until"], "", "{path}");
        }
    }

    #[tokio::test]
    async fn a_cursor_timestamp_is_checked_like_a_bound() {
        let (url, seen) = fake_clickhouse().await;
        let app = windowed_app(&url);
        let paged = WINDOWED_ROUTES
            .iter()
            .filter(|(path, _)| reachable_here(path))
            .filter_map(|(path, id)| id.map(|id| (*path, id)));
        let mut checked = 0;
        for (path, id) in paged {
            checked += 1;
            for bad in [
                "not-a-date%7Crow-1",
                "2026-02-30%2012:00:00.000%7Crow-1",
                "no-bar",
            ] {
                let (status, body) = get(&app, &format!("{path}?cursor={bad}")).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{path}?cursor={bad}");
                assert_eq!(body["error"]["param"], "cursor", "{path}: {body}");
                assert_eq!(body["error"]["code"], "invalid_cursor", "{path}");
                assert_eq!(body["error"]["type"], "invalid_request_error", "{path}");
                // the rejection still names the column this list pages over
                assert!(
                    body["error"]["message"]
                        .as_str()
                        .is_some_and(|m| m.contains(id)),
                    "{path}: {body}"
                );
            }
            assert!(
                seen.lock().is_empty(),
                "{path}: a refused cursor reached clickhouse"
            );

            // the form every page hands out resumes exactly as before
            let (status, body) = get(
                &app,
                &format!("{path}?cursor=2026-07-19%2012:00:01.000%7Crow-1"),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{path}: {body}");
            let bindings = seen.lock().pop().expect("the query reached clickhouse");
            assert_eq!(
                bindings["param_cursor_ts"], "2026-07-19 12:00:01.000",
                "{path}"
            );
            assert_eq!(bindings["param_cursor_id"], "row-1", "{path}");
        }
        let expected = if cfg!(feature = "postgres") { 2 } else { 1 };
        assert_eq!(checked, expected);
    }
}
