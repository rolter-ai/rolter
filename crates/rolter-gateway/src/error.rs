//! OpenAI-compatible error responses. Every error the gateway returns on a
//! request path is shaped like the OpenAI/Anthropic error envelope so existing
//! client SDKs surface a useful `type`/`code`/`param` instead of an opaque blob:
//!
//! ```json
//! { "error": { "message": "...", "type": "invalid_request_error",
//!              "param": "model", "code": "model_not_found" } }
//! ```
//!
//! `type` is inferred from the HTTP status via [`openai_error_type`] so callers
//! cannot forget it; `code` and `param` are optional refinements.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Map an HTTP status to the OpenAI error `type` string clients expect.
pub fn openai_error_type(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "invalid_request_error",
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::NOT_FOUND => "not_found_error",
        StatusCode::PAYMENT_REQUIRED => "insufficient_quota",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        StatusCode::SERVICE_UNAVAILABLE | StatusCode::BAD_GATEWAY | StatusCode::GATEWAY_TIMEOUT => {
            "overloaded_error"
        }
        s if s.is_server_error() => "api_error",
        s if s.is_client_error() => "invalid_request_error",
        _ => "api_error",
    }
}

/// A structured, OpenAI-compatible error ready to be returned from a handler.
/// The `type` is always derived from `status`; `code` and `param` are optional.
pub struct ApiError {
    status: StatusCode,
    message: String,
    code: Option<&'static str>,
    param: Option<&'static str>,
}

impl ApiError {
    /// An error carrying just a status and human-readable message.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            code: None,
            param: None,
        }
    }

    /// Attach a machine-readable `code` slug (e.g. `model_not_found`).
    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    /// Name the offending request field (e.g. `model`).
    pub fn with_param(mut self, param: &'static str) -> Self {
        self.param = Some(param);
        self
    }

    /// Render the OpenAI-style JSON body (without the HTTP status), exposed for
    /// callers that need to attach extra headers before responding.
    pub fn body(&self) -> serde_json::Value {
        json!({
            "error": {
                "message": self.message,
                "type": openai_error_type(self.status),
                "param": self.param,
                "code": self.code,
            }
        })
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body())).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_is_inferred_from_status() {
        assert_eq!(
            openai_error_type(StatusCode::BAD_REQUEST),
            "invalid_request_error"
        );
        assert_eq!(
            openai_error_type(StatusCode::UNAUTHORIZED),
            "authentication_error"
        );
        assert_eq!(
            openai_error_type(StatusCode::TOO_MANY_REQUESTS),
            "rate_limit_error"
        );
        assert_eq!(
            openai_error_type(StatusCode::PAYMENT_REQUIRED),
            "insufficient_quota"
        );
        assert_eq!(
            openai_error_type(StatusCode::BAD_GATEWAY),
            "overloaded_error"
        );
        assert_eq!(
            openai_error_type(StatusCode::INTERNAL_SERVER_ERROR),
            "api_error"
        );
    }

    #[test]
    fn body_carries_code_and_param() {
        let err = ApiError::new(StatusCode::NOT_FOUND, "no such model")
            .with_code("model_not_found")
            .with_param("model");
        let body = err.body();
        let e = &body["error"];
        assert_eq!(e["type"], "not_found_error");
        assert_eq!(e["code"], "model_not_found");
        assert_eq!(e["param"], "model");
        assert_eq!(e["message"], "no such model");
    }

    #[test]
    fn body_nulls_absent_fields() {
        let body = ApiError::new(StatusCode::BAD_REQUEST, "bad").body();
        assert!(body["error"]["code"].is_null());
        assert!(body["error"]["param"].is_null());
    }
}
