//! What a request knows about the upstream that failed it (#2807).
//!
//! The forward loops retry a retryable upstream status on the next target and
//! drop the failed response. That is right while there is a next target, and
//! wrong once there is not: the request used to end as `503 no target
//! selected`, which points at routing config when the cause was an upstream
//! that answered `429`, and left no request-log row at all.
//!
//! This module keeps the last failure alive past the loop. A response the loop
//! is about to supersede is read for its reason ([`StatusFailure::read`]), the
//! loop carries it out as the request's last failure, and when nothing is left
//! to try the caller is answered with [`exhausted`], which keeps a rate limit a
//! rate limit and names the upstream status.
//!
//! What is stored is the same text on both sides of the wire, bounded and with
//! embedded URLs masked, never the raw upstream body. A reason is a sentence
//! for a log row; a body can be megabytes, and an upstream's own error text is
//! not ours to relay beyond that.

use std::time::Duration;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

/// The most characters of an upstream's reason a log row keeps.
pub(crate) const REASON_MAX_CHARS: usize = 300;

/// The most bytes of a superseded response's body read looking for its reason.
const BODY_READ_MAX_BYTES: usize = 8 * 1024;

/// The longest a superseded response's body is waited on. The attempt is
/// already lost and the caller is not waiting on the body, so a stalled one
/// costs a log row its reason rather than the request its latency.
const BODY_READ_TIMEOUT: Duration = Duration::from_millis(500);

/// The last retryable upstream response a request failed over from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusFailure {
    /// The upstream's HTTP status.
    pub status: u16,
    /// What the upstream said, bounded and with URLs masked; empty when its
    /// body held nothing readable.
    pub reason: String,
    /// How long the upstream asked callers to wait, in whole seconds.
    pub retry_after: Option<u64>,
}

impl StatusFailure {
    /// Read the reason off `response`, which the caller is about to drop.
    ///
    /// Bounded in bytes and in time (see [`BODY_READ_MAX_BYTES`] and
    /// [`BODY_READ_TIMEOUT`]); a body that cannot be read in that budget gives
    /// an empty reason rather than a slower failover.
    pub(crate) async fn read(mut response: reqwest::Response, retry_after: Option<u64>) -> Self {
        let status = response.status().as_u16();
        let mut body = Vec::new();
        let _ = tokio::time::timeout(BODY_READ_TIMEOUT, async {
            while body.len() < BODY_READ_MAX_BYTES {
                match response.chunk().await {
                    Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                    // the end of the body, or a body that broke off: either
                    // way what was read is all there will be
                    Ok(None) | Err(_) => break,
                }
            }
        })
        .await;
        Self {
            status,
            reason: reason_from_body(&body),
            retry_after,
        }
    }
}

/// The reason an upstream gave for an error response, from its body.
///
/// Understands the envelopes providers use for an error (`error.message` for
/// OpenAI, Anthropic and most compatibles, a bare `message` or `detail`), and
/// the provider's own words when a router nests them one level down
/// (`error.metadata.raw`, which is where OpenRouter says that the model behind
/// it is rate limited). Anything else, such as a proxy's HTML page, is taken
/// as text. The result is one bounded line with URLs masked.
pub(crate) fn reason_from_body(body: &[u8]) -> String {
    let text = match serde_json::from_slice::<Value>(body) {
        Ok(value) => json_reason(&value).unwrap_or_default(),
        Err(_) => String::from_utf8_lossy(body).into_owned(),
    };
    tidy(&text)
}

fn json_reason(value: &Value) -> Option<String> {
    let error = value.get("error");
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| error.and_then(Value::as_str))
        .or_else(|| value.get("message").and_then(Value::as_str))
        .or_else(|| value.get("detail").and_then(Value::as_str))?;
    let raw = error
        .and_then(|error| error.pointer("/metadata/raw"))
        .and_then(Value::as_str)
        .filter(|raw| !raw.is_empty() && *raw != message);
    Some(match raw {
        Some(raw) => format!("{message}: {raw}"),
        None => message.to_string(),
    })
}

/// One line, URLs masked, at most [`REASON_MAX_CHARS`] characters.
///
/// Masked before it is cut, so a URL the cut would have split is never half
/// shown.
fn tidy(text: &str) -> String {
    let masked = rolter_core::redact::redact_urls_in_text(text);
    let line = masked
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    match line.char_indices().nth(REASON_MAX_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    }
}

/// The `error` text of a row for an upstream that answered with an error
/// status the caller received as it was.
///
/// The row's `status` is the same number, which is why this names the
/// upstream anyway: a reader of the error column alone is told who said it.
pub(crate) fn upstream_error_text(status: u16, body: &[u8]) -> String {
    with_reason(
        format!("upstream returned {status}"),
        &reason_from_body(body),
    )
}

fn with_reason(lead: String, reason: &str) -> String {
    if reason.is_empty() {
        lead
    } else {
        format!("{lead}: {reason}")
    }
}

/// What a request ends with when its last attempt got a retryable status and
/// there is no target left to try.
pub(crate) struct Exhausted {
    /// The answer for the caller.
    pub response: Response,
    /// The status of that answer, which is also the row's `status`.
    pub status: u16,
    /// The row's `error`: the upstream's status and reason, and how many
    /// attempts it took to get there.
    pub error: String,
}

/// Answer a request whose targets all failed with a retryable status.
///
/// A `429` stays a `429` with `rate_limit_error` and the upstream's
/// `Retry-After`, so a client that backs off on rate limits still does. Any
/// other status becomes a `503`: the gateway has no healthy target to give
/// the caller, and says which upstream status it was last told. `cooling_down`
/// is whether the failed targets were parked, so the message is only claiming
/// a cooldown that exists.
///
/// The upstream's reason is in the row, not in this answer: the caller is told
/// what happened, and the operator reads why.
pub(crate) fn exhausted(
    model: &str,
    failure: &StatusFailure,
    attempts: u8,
    cooling_down: bool,
) -> Exhausted {
    let rate_limited = failure.status == 429;
    let (status, code, message) = if rate_limited {
        (
            StatusCode::TOO_MANY_REQUESTS,
            "upstream_rate_limited",
            format!(
                "every upstream target for model '{model}' is rate limited (last upstream status {})",
                failure.status
            ),
        )
    } else {
        let state = if cooling_down {
            "failed and are cooling down after upstream errors"
        } else {
            "failed"
        };
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream_unavailable",
            format!(
                "every upstream target for model '{model}' {state} (last upstream status {})",
                failure.status
            ),
        )
    };
    let mut response = crate::error::ApiError::new(status, message)
        .with_code(code)
        .into_response();
    if rate_limited {
        if let Some(secs) = failure.retry_after {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
    }
    let plural = if attempts == 1 { "" } else { "s" };
    Exhausted {
        response,
        status: status.as_u16(),
        error: with_reason(
            format!(
                "upstream returned {} after {attempts} attempt{plural}",
                failure.status
            ),
            &failure.reason,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn failure(status: u16, reason: &str) -> StatusFailure {
        StatusFailure {
            status,
            reason: reason.to_string(),
            retry_after: None,
        }
    }

    #[test]
    fn an_openai_error_envelope_gives_its_message() {
        let body = br#"{"error":{"message":"Rate limit reached","type":"rate_limit_error"}}"#;
        assert_eq!(reason_from_body(body), "Rate limit reached");
    }

    #[test]
    fn an_anthropic_error_envelope_gives_its_message() {
        let body =
            br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(reason_from_body(body), "Overloaded");
    }

    #[test]
    fn a_routers_nested_provider_text_is_kept() {
        // what OpenRouter answers when the model behind it is rate limited
        let body = br#"{"error":{"message":"Provider returned error","code":429,
            "metadata":{"raw":"gemma is temporarily rate-limited upstream","provider_name":"x"}}}"#;
        assert_eq!(
            reason_from_body(body),
            "Provider returned error: gemma is temporarily rate-limited upstream"
        );
    }

    #[test]
    fn bare_message_detail_and_string_error_shapes_are_read() {
        assert_eq!(reason_from_body(br#"{"message":"slow down"}"#), "slow down");
        assert_eq!(
            reason_from_body(br#"{"detail":"queue full"}"#),
            "queue full"
        );
        assert_eq!(reason_from_body(br#"{"error":"busy"}"#), "busy");
    }

    #[test]
    fn a_body_that_is_not_json_is_taken_as_one_line_of_text() {
        assert_eq!(
            reason_from_body(b"upstream\n  connect   error\r\n"),
            "upstream connect error"
        );
    }

    #[test]
    fn json_with_no_message_has_no_reason_rather_than_a_dump_of_itself() {
        assert_eq!(reason_from_body(br#"{"unexpected":{"shape":1}}"#), "");
        assert_eq!(reason_from_body(b""), "");
    }

    #[test]
    fn a_reason_is_bounded_and_marked_as_cut() {
        let body = format!(r#"{{"error":{{"message":"{}"}}}}"#, "x".repeat(2_000));
        let reason = reason_from_body(body.as_bytes());
        assert_eq!(reason.chars().count(), REASON_MAX_CHARS + 1);
        assert!(reason.ends_with('…'));
    }

    #[test]
    fn a_reason_is_cut_on_a_character_boundary() {
        let body = format!(r#"{{"error":{{"message":"{}"}}}}"#, "é".repeat(2_000));
        let reason = reason_from_body(body.as_bytes());
        assert_eq!(reason.chars().count(), REASON_MAX_CHARS + 1);
    }

    #[test]
    fn urls_in_a_reason_are_masked_before_it_is_stored() {
        // generated per run, so no test writes a credential out
        let password = format!("pw-{}", uuid::Uuid::new_v4());
        let token = format!("tok-{}", uuid::Uuid::new_v4());
        let body = json!({"error": {"message": format!(
            "cannot reach https://user:{password}@internal.example/v1?api_key={token} now"
        )}});
        let reason = reason_from_body(body.to_string().as_bytes());
        assert!(!reason.contains(&password), "{reason}");
        assert!(!reason.contains(&token), "{reason}");
        assert!(reason.contains("internal.example"), "{reason}");
    }

    #[test]
    fn the_error_text_names_the_upstream_status_and_its_reason() {
        let body = br#"{"error":{"message":"busy"}}"#;
        assert_eq!(
            upstream_error_text(503, body),
            "upstream returned 503: busy"
        );
        assert_eq!(upstream_error_text(503, b""), "upstream returned 503");
    }

    #[tokio::test]
    async fn a_rate_limited_upstream_stays_a_rate_limit_for_the_caller() {
        let mut failed = failure(429, "Provider returned error");
        failed.retry_after = Some(7);
        let answer = exhausted("gemma", &failed, 2, true);

        assert_eq!(answer.status, 429);
        assert_eq!(answer.response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(answer.response.headers()[header::RETRY_AFTER], "7");
        assert_eq!(
            answer.error,
            "upstream returned 429 after 2 attempts: Provider returned error"
        );
        let body = axum::body::to_bytes(answer.response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["type"], "rate_limit_error");
        assert_eq!(body["error"]["code"], "upstream_rate_limited");
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.contains("429"), "{message}");
        assert!(message.contains("'gemma'"), "{message}");
        // the upstream's own words belong to the log row, not to the answer
        assert!(!message.contains("Provider returned error"), "{message}");
    }

    #[tokio::test]
    async fn any_other_upstream_status_is_a_503_that_names_it() {
        let answer = exhausted("gemma", &failure(502, ""), 1, true);

        assert_eq!(answer.status, 503);
        assert!(answer.response.headers().get(header::RETRY_AFTER).is_none());
        assert_eq!(answer.error, "upstream returned 502 after 1 attempt");
        let body = axum::body::to_bytes(answer.response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["type"], "overloaded_error");
        assert_eq!(body["error"]["code"], "upstream_unavailable");
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.contains("cooling down"), "{message}");
        assert!(message.contains("502"), "{message}");
    }

    #[tokio::test]
    async fn a_cooldown_is_only_claimed_when_there_is_one() {
        let answer = exhausted("gemma", &failure(500, ""), 3, false);
        let body = axum::body::to_bytes(answer.response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        let message = body["error"]["message"].as_str().unwrap();
        assert!(!message.contains("cooling down"), "{message}");
        assert!(message.contains("500"), "{message}");
    }

    #[test]
    fn a_retry_after_is_not_sent_for_anything_but_a_rate_limit() {
        let mut failed = failure(503, "");
        failed.retry_after = Some(30);
        let answer = exhausted("m", &failed, 2, true);
        assert!(answer.response.headers().get(header::RETRY_AFTER).is_none());
    }
}
