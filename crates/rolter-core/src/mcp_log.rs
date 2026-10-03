//! Pure helpers for the MCP tool-call log, shared by the control plane's ingest
//! route and the gateway's direct writer so both apply one capture, redaction
//! and error-category policy.

use serde_json::Value;

/// The closed set of `status` values the `mcp_tool_call_logs` table accepts.
pub const MCP_STATUSES: &[&str] = &[
    "success",
    "timeout",
    "auth_denied",
    "transport_error",
    "error",
];

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
/// survive at the edge of the retained prefix.
pub fn capture(value: Option<Value>, enabled: bool, max_bytes: usize, fields: &[String]) -> String {
    if !enabled || max_bytes == 0 {
        return String::new();
    }
    let Some(mut value) = value else {
        return String::new();
    };
    redact(&mut value, fields);
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
            &["token".to_string()],
        );
        assert!(captured.contains("[REDACTED]"));
        assert!(!captured.contains("secret"));
    }

    #[test]
    fn capture_is_empty_when_disabled_or_zero_sized() {
        let value = Some(json!({"a": 1}));
        assert_eq!(capture(value.clone(), false, 1024, &[]), "");
        assert_eq!(capture(value, true, 0, &[]), "");
    }

    #[test]
    fn capture_truncates_on_a_char_boundary() {
        let captured = capture(Some(json!("ééééé")), true, 5, &[]);
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
