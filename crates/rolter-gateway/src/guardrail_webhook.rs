//! Request-path client for the custom guardrail webhook (ROL-257).
//!
//! Consults a self-hosted HTTP guardrail service before proxying a request. The
//! call is bounded (timeout, retries, content-size cap) and its transport-failure
//! behaviour is operator-chosen (fail-open vs fail-closed). Only the explicitly
//! assembled envelope is sent; prompt content is never logged here.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::OnceLock;

use rolter_core::guardrail_webhook::{
    FailureMode, GuardrailWebhookConfig, WebhookAuth, WebhookDecision, WebhookRequest,
    WebhookStage, WebhookTenant,
};
use serde_json::Value;

use crate::metrics::Metrics;

/// Shared client for webhook calls: connection pooling across requests, no
/// per-call setup cost. Per-call timeouts are applied on the request builder.
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// What the gateway should do after consulting the webhook.
#[derive(Debug, Clone, PartialEq)]
pub enum WebhookOutcome {
    /// forward unchanged (allow, annotate, or a fail-open transport failure)
    Allow,
    /// reject the request with an OpenAI-compatible error; carries the reason
    Block(Option<String>),
    /// replace the inspected content with this value before forwarding
    Transform(Value),
}

/// Consult the pre-call webhook for a request body. `content` is the body under
/// inspection; it is size-capped before sending. Transport failures resolve per
/// `config.failure_mode`. Never returns an error: the outcome always tells the
/// caller exactly what to do.
#[allow(clippy::too_many_arguments)]
pub async fn consult_pre_call(
    config: &GuardrailWebhookConfig,
    metrics: &Metrics,
    model: &str,
    route: &str,
    trace_id: &str,
    tenant: WebhookTenant,
    content: &Value,
) -> WebhookOutcome {
    if !config.enabled || config.stage != WebhookStage::PreCall {
        return WebhookOutcome::Allow;
    }

    // bound the content forwarded to the webhook. an oversized body is replaced
    // with a truncated preview and flagged, rather than streamed in full.
    let serialized = serde_json::to_vec(content).unwrap_or_default();
    let (payload, truncated) = if serialized.len() > config.max_body_bytes {
        let preview: String = content
            .to_string()
            .chars()
            .take(config.max_body_bytes)
            .collect();
        (Value::String(preview), true)
    } else {
        (content.clone(), false)
    };

    let envelope = WebhookRequest {
        direction: "request",
        stage: WebhookStage::PreCall,
        model,
        route,
        trace_id,
        tenant,
        truncated,
        content: &payload,
    };

    match call_with_retries(config, &envelope).await {
        Ok(decision) => match decision {
            WebhookDecision::Allow | WebhookDecision::Annotate { .. } => WebhookOutcome::Allow,
            WebhookDecision::Block { reason } => {
                metrics.guardrail_webhook_blocks_total.fetch_add(1, Relaxed);
                WebhookOutcome::Block(reason)
            }
            WebhookDecision::Transform { content } => {
                metrics
                    .guardrail_webhook_transforms_total
                    .fetch_add(1, Relaxed);
                WebhookOutcome::Transform(content)
            }
        },
        // transport failure: honour the operator's availability/enforcement choice
        Err(()) => {
            metrics.guardrail_webhook_errors_total.fetch_add(1, Relaxed);
            match config.failure_mode {
                FailureMode::FailOpen => WebhookOutcome::Allow,
                FailureMode::FailClosed => {
                    WebhookOutcome::Block(Some("guardrail service unavailable".to_string()))
                }
            }
        }
    }
}

/// POST the envelope, retrying transient failures up to `max_retries` extra times.
/// Returns `Err(())` when every attempt fails (caller applies the failure mode).
async fn call_with_retries(
    config: &GuardrailWebhookConfig,
    envelope: &WebhookRequest<'_>,
) -> Result<WebhookDecision, ()> {
    let attempts = config.max_retries.saturating_add(1);
    for _ in 0..attempts {
        if let Some(decision) = call_once(config, envelope).await {
            return Ok(decision);
        }
    }
    Err(())
}

/// A single webhook call. `None` signals a transient failure (connect, timeout,
/// non-2xx, or unreadable body) that the retry loop may re-attempt.
async fn call_once(
    config: &GuardrailWebhookConfig,
    envelope: &WebhookRequest<'_>,
) -> Option<WebhookDecision> {
    let mut req = client()
        .post(config.url.trim())
        .timeout(std::time::Duration::from_millis(config.timeout_ms))
        .header("X-Rolter-Trace-Id", envelope.trace_id)
        .json(envelope);

    if let Some(auth) = &config.auth {
        // secrets are resolved from the environment at call time, never inlined
        // in config; a missing/empty var means the header is simply omitted
        match auth {
            WebhookAuth::Bearer { token_env } => {
                if let Ok(token) = std::env::var(token_env) {
                    req = req.bearer_auth(token);
                }
            }
            WebhookAuth::SharedSecret { secret_env } => {
                if let Ok(secret) = std::env::var(secret_env) {
                    req = req.header("X-Rolter-Guardrail-Secret", secret);
                }
            }
        }
    }

    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.bytes().await.ok()?;
    Some(WebhookDecision::parse(&body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::guardrail_webhook::GuardrailWebhookConfig;
    use serde_json::json;

    fn disabled() -> GuardrailWebhookConfig {
        GuardrailWebhookConfig::default()
    }

    #[tokio::test]
    async fn disabled_webhook_allows_without_calling() {
        let out = consult_pre_call(
            &disabled(),
            &Metrics::default(),
            "gpt-4",
            "route",
            "trace",
            WebhookTenant::default(),
            &json!({"messages": []}),
        )
        .await;
        assert_eq!(out, WebhookOutcome::Allow);
    }

    #[tokio::test]
    async fn unreachable_webhook_fails_open_by_default() {
        let cfg = GuardrailWebhookConfig {
            enabled: true,
            // reserved TEST-NET-1 address; connection is refused/timed out fast
            url: "http://192.0.2.1:1/check".to_string(),
            timeout_ms: 150,
            ..GuardrailWebhookConfig::default()
        };
        let out = consult_pre_call(
            &cfg,
            &Metrics::default(),
            "gpt-4",
            "route",
            "trace",
            WebhookTenant::default(),
            &json!({"messages": []}),
        )
        .await;
        assert_eq!(out, WebhookOutcome::Allow);
    }

    async fn serve(decision: Value) -> String {
        use axum::routing::post;
        use axum::{Json, Router};
        let app = Router::new().route(
            "/check",
            post(move || {
                let decision = decision.clone();
                async move { Json(decision) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/check")
    }

    fn enabled(url: String) -> GuardrailWebhookConfig {
        GuardrailWebhookConfig {
            enabled: true,
            url,
            timeout_ms: 2_000,
            ..GuardrailWebhookConfig::default()
        }
    }

    #[tokio::test]
    async fn allow_decision_forwards_unchanged() {
        let url = serve(json!({"action": "allow"})).await;
        let out = consult_pre_call(
            &enabled(url),
            &Metrics::default(),
            "gpt-4",
            "route",
            "trace",
            WebhookTenant::default(),
            &json!({"messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
        assert_eq!(out, WebhookOutcome::Allow);
    }

    #[tokio::test]
    async fn block_decision_rejects_with_reason_and_counts() {
        let url = serve(json!({"action": "block", "reason": "pii detected"})).await;
        let metrics = Metrics::default();
        let out = consult_pre_call(
            &enabled(url),
            &metrics,
            "gpt-4",
            "route",
            "trace",
            WebhookTenant::default(),
            &json!({"messages": []}),
        )
        .await;
        assert_eq!(out, WebhookOutcome::Block(Some("pii detected".to_string())));
        assert_eq!(metrics.guardrail_webhook_blocks_total.load(Relaxed), 1);
    }

    #[tokio::test]
    async fn transform_decision_replaces_content_and_counts() {
        let url = serve(json!({
            "action": "transform",
            "content": {"messages": [{"role": "user", "content": "[cleaned]"}]}
        }))
        .await;
        let metrics = Metrics::default();
        let out = consult_pre_call(
            &enabled(url),
            &metrics,
            "gpt-4",
            "route",
            "trace",
            WebhookTenant::default(),
            &json!({"messages": [{"role": "user", "content": "dirty"}]}),
        )
        .await;
        match out {
            WebhookOutcome::Transform(content) => {
                assert_eq!(content["messages"][0]["content"], json!("[cleaned]"));
            }
            other => panic!("expected transform, got {other:?}"),
        }
        assert_eq!(metrics.guardrail_webhook_transforms_total.load(Relaxed), 1);
    }

    #[tokio::test]
    async fn unreachable_webhook_fails_closed_when_configured() {
        let cfg = GuardrailWebhookConfig {
            enabled: true,
            url: "http://192.0.2.1:1/check".to_string(),
            timeout_ms: 150,
            failure_mode: FailureMode::FailClosed,
            ..GuardrailWebhookConfig::default()
        };
        let out = consult_pre_call(
            &cfg,
            &Metrics::default(),
            "gpt-4",
            "route",
            "trace",
            WebhookTenant::default(),
            &json!({"messages": []}),
        )
        .await;
        assert!(matches!(out, WebhookOutcome::Block(_)));
    }
}
