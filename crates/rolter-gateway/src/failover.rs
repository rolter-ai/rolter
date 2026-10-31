//! Reading a refused attempt for the failure class a fallback target takes
//! over (#1584).
//!
//! A `4xx` is final on a route with no target that takes refusals over, and
//! its response goes to the caller untouched. On a route with one, the forward
//! loop has to know whether the answer was a content-policy refusal before it
//! decides, which means reading the body. [`peek_refusal`] reads a bounded
//! prefix, classifies it with [`Failure::refusal`], and hands back a response
//! that still carries the whole body, so an answer that is not taken over
//! reaches the caller byte for byte as it would have.

use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use rolter_core::failover::Failure;

/// The most bytes of a refused response read to classify it. A refusal
/// envelope is a few hundred bytes; anything past this is passed on unread.
const PEEK_MAX_BYTES: usize = 16 * 1024;

/// The longest the body is waited on before it is classified on what arrived.
/// The rest is still forwarded, so a slow body costs the classification, never
/// the caller's answer.
const PEEK_TIMEOUT: Duration = Duration::from_millis(1000);

/// Whether a response with `status` could be a refusal, which is what decides
/// whether its body is read at all.
pub(crate) fn may_be_refusal(status: u16) -> bool {
    matches!(status, 400 | 403 | 422 | 451)
}

/// A refused response after [`peek_refusal`] read it.
pub(crate) struct Peeked {
    /// the response, rebuilt with every byte of its body
    pub(crate) response: reqwest::Response,
    /// the refusal it is, if it is one
    pub(crate) refusal: Option<Failure>,
    /// what the upstream said, for the log row when the attempt is superseded
    pub(crate) reason: String,
}

/// Read up to [`PEEK_MAX_BYTES`] of `response`'s body and classify it.
pub(crate) async fn peek_refusal(response: reqwest::Response) -> Peeked {
    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    let mut stream = response.bytes_stream();
    let mut prefix = Vec::new();
    let mut ended = false;
    let _ = tokio::time::timeout(PEEK_TIMEOUT, async {
        while prefix.len() < PEEK_MAX_BYTES {
            match stream.next().await {
                Some(Ok(chunk)) => prefix.extend_from_slice(&chunk),
                // a body that broke off ends here, as it would have for the
                // caller
                Some(Err(_)) | None => {
                    ended = true;
                    break;
                }
            }
        }
    })
    .await;
    let refusal = Failure::refusal(status.as_u16(), &prefix);
    let reason = crate::upstream_failure::reason_from_body(&prefix);
    let prefix = Bytes::from(prefix);
    let body = if ended {
        reqwest::Body::from(prefix)
    } else {
        // what was read goes first, then whatever the upstream still sends
        reqwest::Body::wrap_stream(futures_util::stream::once(async { Ok(prefix) }).chain(stream))
    };
    let mut rebuilt = axum::http::Response::new(body);
    *rebuilt.status_mut() = status;
    *rebuilt.version_mut() = version;
    *rebuilt.headers_mut() = headers;
    Peeked {
        response: reqwest::Response::from(rebuilt),
        refusal,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream(status: u16, body: &'static str) -> reqwest::Response {
        let mut response = axum::http::Response::new(reqwest::Body::from(body));
        *response.status_mut() = axum::http::StatusCode::from_u16(status).unwrap();
        response
            .headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        reqwest::Response::from(response)
    }

    #[tokio::test]
    async fn a_refusal_is_classified_and_its_body_survives() {
        let body = r#"{"error":{"message":"blocked","code":"content_policy_violation"}}"#;
        let peeked = peek_refusal(upstream(400, body)).await;
        assert_eq!(
            peeked.refusal.as_ref().map(Failure::token).as_deref(),
            Some("refusal:content_policy_violation")
        );
        assert_eq!(peeked.reason, "blocked");
        assert_eq!(peeked.response.status(), 400);
        assert_eq!(
            peeked.response.headers()["content-type"],
            "application/json"
        );
        assert_eq!(peeked.response.text().await.unwrap(), body);
    }

    #[tokio::test]
    async fn an_ordinary_client_error_passes_through_unchanged() {
        let body = r#"{"error":{"message":"bad","code":"invalid_value"}}"#;
        let peeked = peek_refusal(upstream(400, body)).await;
        assert!(peeked.refusal.is_none());
        assert_eq!(peeked.response.text().await.unwrap(), body);
    }

    #[tokio::test]
    async fn a_body_past_the_peek_is_forwarded_whole() {
        let long: &'static str = Box::leak("x".repeat(PEEK_MAX_BYTES * 3).into_boxed_str());
        let peeked = peek_refusal(upstream(400, long)).await;
        assert!(peeked.refusal.is_none());
        assert_eq!(peeked.response.text().await.unwrap().len(), long.len());
    }

    #[test]
    fn only_the_refusal_statuses_are_read() {
        for status in [400, 403, 422, 451] {
            assert!(may_be_refusal(status));
        }
        for status in [200, 401, 404, 429, 500] {
            assert!(!may_be_refusal(status));
        }
    }
}
