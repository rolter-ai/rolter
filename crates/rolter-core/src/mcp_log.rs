//! Pure helpers for the MCP tool-call log, shared by the control plane's ingest
//! route and the gateway's direct writer so both apply one capture, redaction
//! and error-category policy.

use serde_json::Value;

use crate::payload_redaction::{PayloadRedactor, RedactionReport};

/// The closed set of `status` values the `mcp_tool_call_logs` table accepts.
pub const MCP_STATUSES: &[&str] = &[
    "success",
    "timeout",
    "auth_denied",
    "transport_error",
    "error",
];

/// The outcomes that refine a `status` the HTTP layer gave, for a call whose
/// result nothing judged (#3529, #3528). Stored in the `outcome` column
/// (ClickHouse `051`), never in the closed `status` enum, so a writer against a
/// table without the column still has its rows accepted: there they read as
/// the `status` written beside them, `success`.
///
/// - `accepted`: a legacy `sse` server answered the `POST` `202 Accepted` with
///   no JSON-RPC reply in its body, so the reply comes on the stream the client
///   holds open and the relay does not correlate it.
/// - `unread`: the relay forwarded the body to an ungoverned server without
///   reading it (it was encoded, or it was not JSON), so the calls it held are
///   not itemised: the one row has no tool.
///
/// A row with one of these, like a notification, says nothing about whether a
/// tool succeeded, and the MCP summary leaves it out of the success and the
/// failure rate.
pub const MCP_OUTCOMES: &[&str] = &["accepted", "unread"];

/// Replace the value of every object key named in `fields` (case-insensitive,
/// recursively) with a placeholder.
pub fn redact(value: &mut Value, fields: &[String]) {
    match value {
        Value::Object(object) => {
            for (key, nested) in object.iter_mut() {
                if fields.iter().any(|field| field.eq_ignore_ascii_case(key)) {
                    *nested = Value::String("[REDACTED]".to_string());
                } else {
                    redact(nested, fields);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(|nested| redact(nested, fields)),
        _ => {}
    }
}

/// Render a payload for storage: empty unless capture is `enabled` with a
/// non-zero `max_bytes`, redacted before it is truncated so a secret can never
/// survive at the edge of the retained prefix. `redactor` is the
/// `[logging.payload_capture]` policy the request log applies too (#3277): the
/// named keys are blanked and every remaining string is scanned for credential
/// shapes, so a secret pasted into a tool argument or returned in a result is
/// replaced by its typed marker.
pub fn capture(
    value: Option<Value>,
    enabled: bool,
    max_bytes: usize,
    redactor: &PayloadRedactor,
) -> String {
    if !enabled || max_bytes == 0 {
        return String::new();
    }
    let Some(mut value) = value else {
        return String::new();
    };
    redactor.redact_json(&mut value, &mut RedactionReport::default());
    let mut rendered = serde_json::to_string(&value).unwrap_or_default();
    if rendered.len() > max_bytes {
        let end = rendered.floor_char_boundary(max_bytes);
        rendered.truncate(end);
        rendered.push_str("…[truncated]");
    }
    rendered
}

/// Never persist a caller-supplied diagnostic verbatim. The status is the
/// durable machine-readable signal; this optional display string is a bounded,
/// secret-free category for the log detail viewer.
pub fn safe_error(status: &str, error: Option<&str>) -> String {
    if status == "success" || error.is_none() {
        return String::new();
    }
    let error = error.unwrap_or_default().to_ascii_lowercase();
    if error.contains("timeout") {
        "timeout".to_string()
    } else if error.contains("auth") || error.contains("forbidden") || error.contains("denied") {
        "authentication denied".to_string()
    } else if error.contains("connect") || error.contains("transport") || error.contains("dns") {
        "transport failure".to_string()
    } else {
        "tool invocation failed".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn capture_redacts_nested_sensitive_arguments_before_truncation() {
        let captured = capture(
            Some(json!({"nested": {"token": "secret"}, "query": "hello"})),
            true,
            1024,
            &PayloadRedactor::fields_only(&["token".to_string()]),
        );
        assert!(captured.contains("[REDACTED]"));
        assert!(!captured.contains("secret"));
    }

    fn default_policy() -> PayloadRedactor {
        let (redactor, error) = crate::PayloadCaptureConfig::default().redactor();
        assert!(error.is_none());
        redactor
    }

    #[test]
    fn capture_replaces_a_credential_pasted_into_free_text() {
        let key = format!("sk-ant-{}", "a1B2c3D4".repeat(4));
        let captured = capture(
            Some(json!({"query": format!("use {key} to log in"), "nested": [format!("x {key}")]})),
            true,
            1024,
            &default_policy(),
        );
        assert!(!captured.contains(&key), "{captured}");
        assert!(captured.contains("[redacted:anthropic_key]"), "{captured}");
    }

    #[test]
    fn capture_applies_an_operator_pattern() {
        let redactor = PayloadRedactor::compile(
            &[],
            &[],
            &[crate::payload_redaction::RedactPattern {
                name: "ticket".into(),
                pattern: r"TKT-\d{6}".into(),
            }],
        )
        .unwrap();
        let captured = capture(Some(json!("see TKT-123456")), true, 1024, &redactor);
        assert_eq!(captured, "\"see [redacted:ticket]\"");
    }

    #[test]
    fn capture_is_empty_when_disabled_or_zero_sized() {
        let value = Some(json!({"a": 1}));
        assert_eq!(
            capture(value.clone(), false, 1024, &PayloadRedactor::default()),
            ""
        );
        assert_eq!(capture(value, true, 0, &PayloadRedactor::default()), "");
    }

    #[test]
    fn capture_truncates_on_a_char_boundary() {
        let captured = capture(Some(json!("ééééé")), true, 5, &PayloadRedactor::default());
        assert!(captured.ends_with("…[truncated]"));
    }

    #[test]
    fn error_message_is_reduced_to_a_safe_category() {
        assert_eq!(
            safe_error("transport_error", Some("connect failed: bearer secret")),
            "transport failure"
        );
        assert_eq!(safe_error("success", Some("anything")), "");
    }
}
