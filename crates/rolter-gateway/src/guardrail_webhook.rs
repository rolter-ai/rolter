//! Request-path transport for webhook guardrail providers (ROL-257, #459); a
//! local provider runs in [`crate::local_classifier`] instead (#3329).
//!
//! One bounded call to a self-hosted guardrail service over the vendor-neutral
//! webhook contract: the envelope is size-capped, the call has its own timeout
//! and retry budget, and every way it can fail is named, so the policy runtime
//! in [`crate::semantic_guards`] can resolve it through the provider's
//! fail-open/fail-closed mode. Only the explicitly assembled envelope is sent;
//! content is never logged here.

use rolter_core::guardrail_webhook::{GuardrailWebhookConfig, WebhookAuth, WebhookRequest};
use rolter_core::ScoredDecision;
use serde_json::Value;

use crate::egress_client::EgressClient;

/// Why a provider call produced no verdict. Each has a fixed metric label,
/// never the provider's own text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallFailure {
    /// the egress policy refused the url, so nothing was sent
    Refused,
    /// the call outlived the provider's `timeout_ms`
    Timeout,
    /// the connection or the exchange failed before a status came back
    Connect,
    /// the provider shed the call: `429` or `503`
    Overloaded,
    /// any other non-2xx status
    Status,
    /// the body could not be read
    Body,
    /// the body is not a decision (see [`ScoredDecision::parse`])
    Unparsable,
    /// a local provider's model is not loaded, or its inference failed
    Model,
    /// the provider is of a kind this gateway does not know; unlike every
    /// other failure it blocks whatever the provider's failure mode says
    Unsupported,
    /// a local provider's text was longer than its `max_scored_bytes` under
    /// `on_truncated = "fail_closed"`; like `Unsupported` it blocks whatever
    /// the provider's failure mode says (#3513)
    Truncated,
}

impl CallFailure {
    /// The metric label for this failure.
    pub fn label(self) -> &'static str {
        match self {
            Self::Refused => "refused",
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::Overloaded => "overloaded",
            Self::Status => "status",
            Self::Body => "body",
            Self::Unparsable => "unparsable",
            Self::Model => "model",
            Self::Unsupported => "unsupported",
            Self::Truncated => "truncated",
        }
    }

    /// Whether another attempt can change the answer. A refused url, an
    /// unparsable answer and a `4xx` other than `429` say the same thing twice.
    fn is_transient(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Connect | Self::Overloaded | Self::Body
        )
    }
}

/// Bound `content` to `max_body_bytes`: an oversized body is replaced by a
/// truncated preview string, and the flag says so.
pub fn bounded_payload(content: &Value, max_body_bytes: usize) -> (Value, bool) {
    let serialized = serde_json::to_vec(content).unwrap_or_default();
    if serialized.len() > max_body_bytes {
        let preview: String = content.to_string().chars().take(max_body_bytes).collect();
        (Value::String(preview), true)
    } else {
        (content.clone(), false)
    }
}

/// POST `envelope` to the provider, retrying a transient failure up to
/// `max_retries` extra times. The error is the last attempt's failure; the
/// caller applies the provider's failure mode to it.
pub async fn consult(
    config: &GuardrailWebhookConfig,
    egress: &EgressClient,
    envelope: &WebhookRequest<'_>,
) -> Result<ScoredDecision, CallFailure> {
    let attempts = config.max_retries.saturating_add(1);
    let mut last = CallFailure::Connect;
    for _ in 0..attempts {
        match call_once(config, egress, envelope).await {
            Ok(decision) => return Ok(decision),
            Err(failure) if failure.is_transient() => last = failure,
            Err(failure) => return Err(failure),
        }
    }
    Err(last)
}

/// A single provider call.
async fn call_once(
    config: &GuardrailWebhookConfig,
    egress: &EgressClient,
    envelope: &WebhookRequest<'_>,
) -> Result<ScoredDecision, CallFailure> {
    // refused again here, not only at save: the policy may have been tightened
    // since, and a stored row is not a standing permission to egress
    let mut req = egress
        .post(config.url.trim())
        .ok_or(CallFailure::Refused)?
        .timeout(std::time::Duration::from_millis(config.timeout_ms))
        .header("X-Rolter-Trace-Id", envelope.trace_id)
        .json(envelope);

    if let Some(auth) = &config.auth {
        // secrets are resolved from the environment at call time, never inlined
        // in config; a missing/empty var means the header is simply omitted
        if let Some(secret) = auth.resolve_secret(&[]) {
            req = match auth {
                WebhookAuth::Bearer { .. } => req.bearer_auth(secret),
                WebhookAuth::SharedSecret { .. } => req.header("X-Rolter-Guardrail-Secret", secret),
            };
        } else if rolter_core::is_control_plane_secret(auth.env_var()) {
            // a stored row can name a deployment secret; sending it to the
            // webhook's url is what the refusal exists to stop
            tracing::warn!(
                env = auth.env_var(),
                "webhook auth names a deployment secret and was not sent"
            );
        }
    }

    let resp = req.send().await.map_err(|error| {
        if error.is_timeout() {
            CallFailure::Timeout
        } else {
            CallFailure::Connect
        }
    })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            429 | 503 => CallFailure::Overloaded,
            _ => CallFailure::Status,
        });
    }
    let body = resp.bytes().await.map_err(|error| {
        if error.is_timeout() {
            CallFailure::Timeout
        } else {
            CallFailure::Body
        }
    })?;
    ScoredDecision::parse(&body).ok_or(CallFailure::Unparsable)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rolter_core::guardrail_webhook::{WebhookDecision, WebhookStage};
    use rolter_core::WebhookTenant;
    use serde_json::json;

    pub(crate) fn envelope<'a>(content: &'a Value) -> WebhookRequest<'a> {
        WebhookRequest {
            direction: "request",
            stage: WebhookStage::PreCall,
            model: "gpt-4",
            route: "route",
            trace_id: "trace",
            tenant: WebhookTenant::default(),
            truncated: false,
            content,
            policy: None,
            streamed: false,
        }
    }

    fn enabled(url: String) -> GuardrailWebhookConfig {
        GuardrailWebhookConfig {
            enabled: true,
            url,
            timeout_ms: 2_000,
            ..GuardrailWebhookConfig::default()
        }
    }

    async fn ask(config: &GuardrailWebhookConfig) -> Result<ScoredDecision, CallFailure> {
        let content = json!({"messages": []});
        consult(
            config,
            &crate::egress_client::testing::permissive(),
            &envelope(&content),
        )
        .await
    }

    /// A provider that answers every call with `status` and `body`, counting
    /// the calls it took.
    pub(crate) async fn serve_raw(
        status: u16,
        body: &'static str,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use axum::routing::post;
        use axum::Router;
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let app = Router::new().route(
            "/check",
            post(move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move {
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        body,
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/check"), calls)
    }

    /// #3097: a stored webhook row naming a `ROLTER_` secret must not send it
    /// to the webhook's url
    #[tokio::test]
    async fn a_deployment_secret_named_as_the_bearer_env_is_not_sent() {
        use axum::routing::post;
        use axum::Router;
        // SAFETY: test-local env var, no concurrent access to this key elsewhere
        unsafe { std::env::set_var("ROLTER_TEST_GUARDRAIL_SECRET", "leaked") };
        let app = Router::new().route(
            "/check",
            post(|headers: axum::http::HeaderMap| async move {
                let sent = headers.contains_key(axum::http::header::AUTHORIZATION);
                axum::Json(json!({"action": if sent { "block" } else { "allow" }}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cfg = GuardrailWebhookConfig {
            auth: Some(WebhookAuth::Bearer {
                token_env: "ROLTER_TEST_GUARDRAIL_SECRET".to_string(),
            }),
            ..enabled(format!("http://{addr}/check"))
        };
        let out = ask(&cfg).await;
        // SAFETY: cleanup of the test-local var set above
        unsafe { std::env::remove_var("ROLTER_TEST_GUARDRAIL_SECRET") };
        assert_eq!(out.unwrap().decision, WebhookDecision::Allow);
    }

    #[tokio::test]
    async fn each_decision_and_its_scores_come_back() {
        let (url, _) = serve_raw(
            200,
            r#"{"action":"block","reason":"pii","categories":{"violence":0.4}}"#,
        )
        .await;
        let scored = ask(&enabled(url)).await.unwrap();
        assert_eq!(
            scored.decision,
            WebhookDecision::Block {
                reason: Some("pii".to_string())
            }
        );
        assert_eq!(scored.categories.get("violence"), Some(&0.4));
    }

    #[tokio::test]
    async fn an_unreachable_provider_is_a_connect_or_timeout_failure() {
        let cfg = GuardrailWebhookConfig {
            // reserved TEST-NET-1 address; connection is refused/timed out fast
            timeout_ms: 150,
            ..enabled("http://192.0.2.1:1/check".to_string())
        };
        let failure = ask(&cfg).await.unwrap_err();
        assert!(
            matches!(failure, CallFailure::Timeout | CallFailure::Connect),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_slow_provider_times_out() {
        use axum::routing::post;
        use axum::Router;
        let app = Router::new().route(
            "/check",
            post(|| async {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                axum::Json(json!({"action": "allow"}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cfg = GuardrailWebhookConfig {
            timeout_ms: 100,
            ..enabled(format!("http://{addr}/check"))
        };
        assert_eq!(ask(&cfg).await.unwrap_err(), CallFailure::Timeout);
    }

    #[tokio::test]
    async fn an_overloaded_provider_is_retried_then_named() {
        let (url, calls) = serve_raw(429, "{}").await;
        let cfg = GuardrailWebhookConfig {
            max_retries: 2,
            ..enabled(url)
        };
        assert_eq!(ask(&cfg).await.unwrap_err(), CallFailure::Overloaded);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_client_error_or_unparsable_answer_is_not_retried() {
        let (url, calls) = serve_raw(400, "{}").await;
        let cfg = GuardrailWebhookConfig {
            max_retries: 2,
            ..enabled(url)
        };
        assert_eq!(ask(&cfg).await.unwrap_err(), CallFailure::Status);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let (url, calls) = serve_raw(200, r#"{"action":"nuke"}"#).await;
        let cfg = GuardrailWebhookConfig {
            max_retries: 2,
            ..enabled(url)
        };
        assert_eq!(ask(&cfg).await.unwrap_err(), CallFailure::Unparsable);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn an_oversized_body_is_bounded_and_flagged() {
        let content = json!({"messages": [{"role": "user", "content": "x".repeat(100)}]});
        let (payload, truncated) = bounded_payload(&content, 20);
        assert!(truncated);
        assert_eq!(payload.as_str().map(str::len), Some(20));
        let (payload, truncated) = bounded_payload(&content, 10_000);
        assert!(!truncated);
        assert_eq!(payload, content);
    }

    /// #2383: a metadata literal is refused before any request is built.
    #[tokio::test]
    async fn a_metadata_literal_is_refused() {
        let cfg = enabled("https://169.254.169.254/latest/meta-data/".to_string());
        assert_eq!(ask(&cfg).await.unwrap_err(), CallFailure::Refused);
    }

    /// A hostname is not classified by the literal check, so only the
    /// connect-time resolver can stop one that resolves to a denied address.
    #[tokio::test]
    async fn a_name_resolving_to_a_denied_address_is_never_connected_to() {
        let listener = crate::egress_client::testing::Counter::start().await;
        let egress = EgressClient::new(crate::egress_client::testing::deny_loopback());
        let content = json!({"messages": []});
        let out = consult(
            &enabled(listener.url("/check")),
            &egress,
            &envelope(&content),
        )
        .await;
        assert!(out.is_err(), "{out:?}");
        assert_eq!(listener.accepted(), 0);
    }

    #[tokio::test]
    async fn the_default_policy_still_reaches_a_loopback_webhook() {
        // the counter is only evidence if it can count
        let listener = crate::egress_client::testing::Counter::start().await;
        let _ = ask(&enabled(listener.url("/check"))).await;
        assert_eq!(listener.accepted(), 1);
    }

    /// A policy tightened by a hot reload applies to the next request without
    /// rebuilding the client.
    #[tokio::test]
    async fn a_reload_that_tightens_the_policy_applies_to_the_next_call() {
        let listener = crate::egress_client::testing::Counter::start().await;
        let policy = crate::egress_client::testing::default_policy();
        let egress = EgressClient::new(policy.clone());
        policy.store(std::sync::Arc::new(rolter_core::EgressPolicy {
            block_loopback: true,
            ..rolter_core::EgressPolicy::default()
        }));
        let content = json!({"messages": []});
        let out = consult(
            &enabled(listener.url("/check")),
            &egress,
            &envelope(&content),
        )
        .await;
        assert!(out.is_err(), "{out:?}");
        assert_eq!(listener.accepted(), 0);
    }
}
