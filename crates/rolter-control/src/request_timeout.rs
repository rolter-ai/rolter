//! The bound on how long the control plane works on one API request (#1925).
//!
//! Nothing between a dashboard button and Postgres used to give up: a write
//! stuck behind another transaction's row lock hung until the browser tab was
//! reloaded, with the spinner never stopping. The pool's `lock_timeout` and
//! `statement_timeout` cut the database wait short with a coded `store_timeout`;
//! this layer is the outer bound for everything else a handler can wait on, and
//! it answers in the same JSON envelope the rest of the API uses so the
//! dashboard shows it like any other error.

use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};

/// stable code of the `504` for a request that outlived the bound. Part of the
/// API and never renamed
pub(crate) const REQUEST_TIMEOUT: &str = "request_timeout";

/// Paths that are streams, not requests: the playground proxy relays model
/// output for as long as the model produces it, and bounding it would cut every
/// long completion off.
const UNBOUNDED_PREFIXES: &[&str] = &["/gw/"];

/// Seconds to a bound; `0` means no bound.
pub(crate) fn bound(secs: u64) -> Option<Duration> {
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Wrap `router` so no request outlives `bound`. `None` leaves it as it is.
pub(crate) fn apply(router: Router, bound: Option<Duration>) -> Router {
    match bound {
        Some(bound) => router.layer(axum::middleware::from_fn_with_state(bound, enforce)),
        None => router,
    }
}

async fn enforce(State(bound): State<Duration>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if UNBOUNDED_PREFIXES.iter().any(|p| path.starts_with(p)) {
        return next.run(request).await;
    }
    match tokio::time::timeout(bound, next.run(request)).await {
        Ok(response) => response,
        Err(_) => timed_out(bound),
    }
}

fn timed_out(bound: Duration) -> Response {
    (
        StatusCode::GATEWAY_TIMEOUT,
        Json(serde_json::json!({
            "error": {
                "code": REQUEST_TIMEOUT,
                "message": format!(
                    "the control plane did not finish this request within {} seconds; it may \
                     be waiting on another change to the same record, so try again",
                    bound.as_secs()
                ),
            }
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use tower::ServiceExt;

    const BOUND: Duration = Duration::from_millis(100);

    fn app(bound: Option<Duration>) -> Router {
        // outlives BOUND by a wide margin; the timeout drops the handler, so
        // the long sleep never actually runs to its end
        let stuck = || async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            "never"
        };
        let slow = || async {
            tokio::time::sleep(BOUND * 3).await;
            "streamed"
        };
        apply(
            Router::new()
                .route("/api/v1/stuck", get(stuck))
                .route("/gw/v1/slow", get(slow))
                .route("/api/v1/quick", get(|| async { "done" })),
            bound,
        )
    }

    async fn call(app: Router, path: &str) -> Response {
        app.oneshot(Request::get(path).body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_stuck_request_is_answered_with_a_coded_gateway_timeout() {
        let response = call(app(Some(BOUND)), "/api/v1/stuck").await;
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], REQUEST_TIMEOUT);
        assert!(body["error"]["message"].is_string());
    }

    #[tokio::test]
    async fn a_request_inside_the_bound_is_untouched() {
        let response = call(app(Some(BOUND)), "/api/v1/quick").await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn the_playground_proxy_is_not_bound() {
        // slower than the bound, so a bound that applied would answer 504
        let response = call(app(Some(BOUND)), "/gw/v1/slow").await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn zero_disables_the_bound() {
        assert_eq!(bound(0), None);
        assert_eq!(bound(60), Some(Duration::from_secs(60)));
    }
}
