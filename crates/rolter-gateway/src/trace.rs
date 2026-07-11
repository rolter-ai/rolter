//! Request correlation: an end-to-end `x-request-id` and inbound distributed-
//! trace continuation (ROL-60).
//!
//! [`ensure_request_id`] is a middleware that guarantees every request carries an
//! `x-request-id` — reusing the caller's when present, else minting a UUID — and
//! echoes it on the response so a client can correlate its call with the gateway
//! logs. [`inbound_trace_id`] pulls the trace id out of a W3C `traceparent` or a
//! B3 header so the request log adopts the caller's trace instead of starting a
//! disconnected one; the id is stored on each [`RequestLog`](crate::logging::RequestLog)
//! and surfaces in ClickHouse for cross-service correlation.

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// header carrying the end-to-end request id
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Ensure the request has an `x-request-id` (generating one when absent or
/// blank), expose it to downstream handlers via the request headers, and mirror
/// it onto the response.
pub async fn ensure_request_id(mut req: Request, next: Next) -> Response {
    let id = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(new_request_id);

    // a generated uuid is always a valid header value; a caller-supplied one that
    // isn't is dropped rather than failing the request
    let Ok(header) = HeaderValue::from_str(&id) else {
        return next.run(req).await;
    };
    req.headers_mut().insert(REQUEST_ID_HEADER, header.clone());
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(REQUEST_ID_HEADER, header);
    resp
}

fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Extract an inbound trace id from a W3C `traceparent`, a B3 single header, or
/// the `x-b3-traceid` multi-header, normalized to lowercase hex. Returns an
/// empty string when no well-formed trace id is present.
pub fn inbound_trace_id(headers: &HeaderMap) -> String {
    // W3C traceparent: `version-traceid-spanid-flags`; trace id is 32 hex chars
    if let Some(tp) = headers.get("traceparent").and_then(|v| v.to_str().ok()) {
        let parts: Vec<&str> = tp.split('-').collect();
        if parts.len() >= 3 && is_hex(parts[1], 32) {
            return parts[1].to_lowercase();
        }
    }
    // B3 single header: `traceid-spanid[-sampled[-parentspanid]]` (64- or 128-bit)
    if let Some(b3) = headers.get("b3").and_then(|v| v.to_str().ok()) {
        let first = b3.split('-').next().unwrap_or("");
        if is_hex(first, 32) || is_hex(first, 16) {
            return first.to_lowercase();
        }
    }
    // B3 multi header
    if let Some(tid) = headers.get("x-b3-traceid").and_then(|v| v.to_str().ok()) {
        if is_hex(tid, 32) || is_hex(tid, 16) {
            return tid.to_lowercase();
        }
    }
    String::new()
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// standard distributed-trace headers propagated verbatim to the upstream
const PROPAGATED_TRACE_HEADERS: &[&str] = &[
    "traceparent",
    "tracestate",
    "b3",
    "x-b3-traceid",
    "x-b3-spanid",
    "x-b3-sampled",
    "x-b3-parentspanid",
    "x-b3-flags",
];

/// Collect the caller's inbound trace-context headers so the forwarder can
/// propagate them verbatim to the upstream, letting vLLM/SGLang/TGI continue the
/// same trace (ROL-61). Returns an empty vec when the caller sent none, so an
/// untraced request adds nothing to the wire.
pub fn outbound_trace_headers(headers: &HeaderMap) -> Vec<(&'static str, String)> {
    PROPAGATED_TRACE_HEADERS
        .iter()
        .filter_map(|&name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.is_empty())
                .map(|v| (name, v.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn parses_w3c_traceparent() {
        let h = headers(&[(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )]);
        assert_eq!(inbound_trace_id(&h), "4bf92f3577b34da6a3ce929d0e0e4736");
    }

    #[test]
    fn parses_b3_single_and_multi() {
        let single = headers(&[("b3", "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1")]);
        assert_eq!(
            inbound_trace_id(&single),
            "80f198ee56343ba864fe8b2a57d3eff7"
        );
        let multi = headers(&[("x-b3-traceid", "A3CE929D0E0E4736A3CE929D0E0E4736")]);
        assert_eq!(inbound_trace_id(&multi), "a3ce929d0e0e4736a3ce929d0e0e4736");
    }

    #[test]
    fn rejects_malformed_trace_ids() {
        assert_eq!(inbound_trace_id(&headers(&[])), "");
        // wrong length / non-hex are ignored
        assert_eq!(
            inbound_trace_id(&headers(&[("traceparent", "00-xyz-span-01")])),
            ""
        );
        assert_eq!(inbound_trace_id(&headers(&[("b3", "nothex-span")])), "");
    }

    #[test]
    fn traceparent_wins_over_b3() {
        let h = headers(&[
            (
                "traceparent",
                "00-11111111111111111111111111111111-2222222222222222-01",
            ),
            ("b3", "33333333333333333333333333333333-4444444444444444-1"),
        ]);
        assert_eq!(inbound_trace_id(&h), "11111111111111111111111111111111");
    }
}
