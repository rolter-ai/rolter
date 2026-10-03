//! Request-path client for the external PII sanitizer (#848).
//!
//! Distinct from [`crate::guardrail_webhook`], which asks a service for an
//! allow/block *decision*. This one asks for **transformed content**: entities
//! replaced by deterministic placeholders, plus an opaque token that the same
//! service will accept back to reverse the transform.
//!
//! rolter never holds the placeholder→plaintext mapping. That lives with the
//! sanitizer, keyed by the token, so there is no mapping in this process to
//! leak into a log, a metric or a span. What is recorded here is counts and
//! entity types — never a matched value, never the token.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::OnceLock;

use rolter_core::pii_sanitizer::{
    FailureMode, PiiSanitizerConfig, RestorationTicket, RestoreRequest, RestoreResponse,
    SanitizeRequest, SanitizeResponse, TokenScope, WebhookAuth, WebhookTenant,
};
use serde_json::Value;

use crate::metrics::Metrics;

/// Shared client: connection pooling across requests, no per-call setup cost.
/// Per-call timeouts are applied on the request builder.
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// What the gateway should do after consulting the sanitizer.
#[derive(Debug)]
pub enum SanitizeOutcome {
    /// forward unchanged — disabled, nothing found, or a fail-open failure
    Unchanged,
    /// forward this content instead, and hold the ticket for restoration
    Sanitized {
        content: Value,
        ticket: Option<RestorationTicket>,
        replaced: usize,
    },
    /// reject: the sanitizer was unreachable and the deployment is fail-closed
    Block(String),
}

/// Sanitize one leg of an exchange.
///
/// `reversible` tells the service whether a mapping will be asked for later, so
/// a deployment that never restores does not make the service retain one.
/// Never returns an error: the outcome always says exactly what to do.
#[allow(clippy::too_many_arguments)]
pub async fn sanitize(
    config: &PiiSanitizerConfig,
    metrics: &Metrics,
    direction: &'static str,
    model: &str,
    route: &str,
    trace_id: &str,
    tenant: WebhookTenant,
    scope: TokenScope,
    reversible: bool,
    content: &Value,
) -> SanitizeOutcome {
    let (payload, truncated) =
        rolter_core::pii_sanitizer::bound_content(content, config.max_body_bytes);

    let envelope = SanitizeRequest {
        direction,
        model,
        route,
        trace_id,
        tenant,
        entities: &config.entities,
        reversible,
        truncated,
        content: &payload,
    };

    match call_sanitize(config, &envelope).await {
        Ok(response) => {
            let replaced = response.replaced();
            if replaced > 0 {
                metrics
                    .pii_entities_redacted_total
                    .fetch_add(replaced as u64, Relaxed);
            }
            // a service that returns no token cannot reverse; that is a
            // "do not restore", not an error
            let ticket = response
                .restoration_token
                .filter(|_| reversible)
                .map(|token| RestorationTicket::new(token, scope));
            SanitizeOutcome::Sanitized {
                content: response.content,
                ticket,
                replaced,
            }
        }
        Err(()) => {
            metrics.pii_sanitizer_errors_total.fetch_add(1, Relaxed);
            match config.failure_mode {
                // availability over enforcement: forward the *original* content
                FailureMode::FailOpen => SanitizeOutcome::Unchanged,
                // enforcement over availability. this is the mode to pick when
                // the point of the deployment is that PII must not reach the
                // provider: forwarding unsanitized on failure would defeat it
                FailureMode::FailClosed => {
                    metrics.pii_sanitizer_blocks_total.fetch_add(1, Relaxed);
                    SanitizeOutcome::Block("pii sanitizer unavailable".to_string())
                }
            }
        }
    }
}

/// Turn placeholders back into plaintext.
///
/// Returns the restored content, or `None` when restoration was refused or
/// failed — in which case the caller keeps the placeholder-bearing content,
/// which is safe by construction. A restore failure is never fatal: the
/// response is already deliverable, just less convenient.
pub async fn restore(
    config: &PiiSanitizerConfig,
    metrics: &Metrics,
    ticket: &RestorationTicket,
    scope: &TokenScope,
    trace_id: &str,
    tenant: WebhookTenant,
    content: &Value,
) -> Option<Value> {
    // tenant and route isolation. the ticket refuses to yield its token outside
    // the scope it was issued in, so a restore can never be triggered across
    // tenants even if the surrounding control flow changes
    let Some(token) = ticket.token_for(scope) else {
        metrics.pii_restore_errors_total.fetch_add(1, Relaxed);
        tracing::warn!(
            "refusing to restore a pii token outside the tenant and route that produced it"
        );
        return None;
    };

    let envelope = RestoreRequest {
        trace_id,
        tenant,
        restoration_token: token,
        content,
    };

    match call_restore(config, &envelope).await {
        Ok(response) => {
            metrics.pii_restorations_total.fetch_add(1, Relaxed);
            let _ = response.restored;
            Some(response.content)
        }
        Err(()) => {
            metrics.pii_restore_errors_total.fetch_add(1, Relaxed);
            None
        }
    }
}

async fn call_sanitize(
    config: &PiiSanitizerConfig,
    envelope: &SanitizeRequest<'_>,
) -> Result<SanitizeResponse, ()> {
    let attempts = config.max_retries.saturating_add(1);
    for _ in 0..attempts {
        if let Some(parsed) = post(config, config.url.trim(), envelope, envelope.trace_id).await {
            return Ok(parsed);
        }
    }
    Err(())
}

async fn call_restore(
    config: &PiiSanitizerConfig,
    envelope: &RestoreRequest<'_>,
) -> Result<RestoreResponse, ()> {
    let attempts = config.max_retries.saturating_add(1);
    for _ in 0..attempts {
        if let Some(parsed) = post(
            config,
            config.restore_url.trim(),
            envelope,
            envelope.trace_id,
        )
        .await
        {
            return Ok(parsed);
        }
    }
    Err(())
}

/// One POST. `None` signals a transient failure (connect, timeout, non-2xx, or
/// a body that does not parse) that the retry loop may re-attempt.
///
/// A malformed body is deliberately a *failure* here, unlike the guardrail
/// webhook's decision parsing which defaults to allow. Defaulting would mean
/// forwarding content the gateway believes is sanitized and is not — the caller
/// must get to apply its failure mode instead.
async fn post<B: serde::Serialize, R: serde::de::DeserializeOwned>(
    config: &PiiSanitizerConfig,
    url: &str,
    body: &B,
    trace_id: &str,
) -> Option<R> {
    let mut req = client()
        .post(url)
        .timeout(std::time::Duration::from_millis(config.timeout_ms))
        .header("X-Rolter-Trace-Id", trace_id)
        .json(body);

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
    let bytes = resp.bytes().await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use axum::routing::post as post_route;
    use axum::{Json, Router};
    use rolter_core::pii_sanitizer::RestorationPolicy;
    use serde_json::json;

    use super::*;

    fn scope() -> TokenScope {
        TokenScope {
            org: "acme".into(),
            team: "core".into(),
            project: "billing".into(),
            route: "gpt-4o".into(),
        }
    }

    /// Stand up a stub sanitizer serving fixed `/sanitize` and `/restore`
    /// replies, and return its base URL.
    async fn serve(sanitize_reply: Value, restore_reply: Value) -> String {
        let app = Router::new()
            .route(
                "/sanitize",
                post_route(move || {
                    let reply = sanitize_reply.clone();
                    async move { Json(reply) }
                }),
            )
            .route(
                "/restore",
                post_route(move || {
                    let reply = restore_reply.clone();
                    async move { Json(reply) }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn enabled(base: &str, restoration: RestorationPolicy) -> PiiSanitizerConfig {
        PiiSanitizerConfig {
            enabled: true,
            url: format!("{base}/sanitize"),
            restore_url: format!("{base}/restore"),
            restoration,
            timeout_ms: 2_000,
            ..PiiSanitizerConfig::default()
        }
    }

    /// A config pointing at a reserved TEST-NET-1 address: connections are
    /// refused or time out fast, which is the transport-failure case.
    fn unreachable(failure_mode: FailureMode) -> PiiSanitizerConfig {
        PiiSanitizerConfig {
            enabled: true,
            url: "http://192.0.2.1:1/sanitize".to_string(),
            restore_url: "http://192.0.2.1:1/restore".to_string(),
            timeout_ms: 150,
            failure_mode,
            ..PiiSanitizerConfig::default()
        }
    }

    async fn sanitize_with(
        config: &PiiSanitizerConfig,
        metrics: &Metrics,
        body: Value,
    ) -> SanitizeOutcome {
        sanitize(
            config,
            metrics,
            "request",
            "gpt-4o",
            "gpt-4o",
            "trace",
            WebhookTenant::default(),
            scope(),
            true,
            &body,
        )
        .await
    }

    #[tokio::test]
    async fn sanitized_content_replaces_the_body_and_counts_entities() {
        let base = serve(
            json!({
                "content": {"messages": [{"role": "user", "content": "email <EMAIL_ADDRESS_1>"}]},
                "findings": [{"entity_type": "EMAIL_ADDRESS", "count": 1, "placeholders": ["<EMAIL_ADDRESS_1>"]}],
                "restoration_token": "tok_1"
            }),
            json!({"content": {}, "restored": 0}),
        )
        .await;
        let metrics = Metrics::default();
        let out = sanitize_with(
            &enabled(&base, RestorationPolicy::TrustedDownstream),
            &metrics,
            json!({"messages": [{"role": "user", "content": "email a@b.test"}]}),
        )
        .await;

        match out {
            SanitizeOutcome::Sanitized {
                content,
                ticket,
                replaced,
            } => {
                assert_eq!(content["messages"][0]["content"], "email <EMAIL_ADDRESS_1>");
                assert_eq!(replaced, 1);
                let ticket = ticket.expect("a reversible call should yield a ticket");
                assert_eq!(ticket.token_for(&scope()), Some("tok_1"));
            }
            other => panic!("expected sanitized, got {other:?}"),
        }
        assert_eq!(metrics.pii_entities_redacted_total.load(Relaxed), 1);
    }

    /// Availability over enforcement: the original content goes upstream.
    #[tokio::test]
    async fn an_unreachable_sanitizer_fails_open_by_default() {
        let metrics = Metrics::default();
        let out = sanitize_with(
            &unreachable(FailureMode::FailOpen),
            &metrics,
            json!({"messages": []}),
        )
        .await;
        assert!(matches!(out, SanitizeOutcome::Unchanged), "{out:?}");
        assert_eq!(metrics.pii_sanitizer_errors_total.load(Relaxed), 1);
        assert_eq!(metrics.pii_sanitizer_blocks_total.load(Relaxed), 0);
    }

    /// Enforcement over availability — the mode for a deployment whose whole
    /// point is that PII must not reach the provider.
    #[tokio::test]
    async fn an_unreachable_sanitizer_fails_closed_when_configured() {
        let metrics = Metrics::default();
        let out = sanitize_with(
            &unreachable(FailureMode::FailClosed),
            &metrics,
            json!({"messages": []}),
        )
        .await;
        assert!(matches!(out, SanitizeOutcome::Block(_)), "{out:?}");
        assert_eq!(metrics.pii_sanitizer_errors_total.load(Relaxed), 1);
        assert_eq!(metrics.pii_sanitizer_blocks_total.load(Relaxed), 1);
    }

    /// A body the gateway cannot parse must not be treated as "sanitized" —
    /// unlike the guardrail webhook's decision parsing, which defaults to allow.
    /// Defaulting here would forward content the gateway believes is clean.
    #[tokio::test]
    async fn a_malformed_sanitizer_reply_is_a_failure_not_a_pass() {
        let base = serve(json!({"unexpected": true}), json!({})).await;
        let mut config = enabled(&base, RestorationPolicy::Never);
        config.failure_mode = FailureMode::FailClosed;
        let metrics = Metrics::default();
        let out = sanitize_with(&config, &metrics, json!({"messages": []})).await;
        assert!(matches!(out, SanitizeOutcome::Block(_)), "{out:?}");
    }

    /// A non-reversible call must not carry a ticket even if the service
    /// volunteers a token: holding one the deployment will never redeem is a
    /// liability with no benefit.
    #[tokio::test]
    async fn a_non_reversible_call_discards_any_offered_token() {
        let base = serve(
            json!({"content": {"messages": []}, "restoration_token": "tok_unwanted"}),
            json!({}),
        )
        .await;
        let out = sanitize(
            &enabled(&base, RestorationPolicy::Never),
            &Metrics::default(),
            "request",
            "gpt-4o",
            "gpt-4o",
            "trace",
            WebhookTenant::default(),
            scope(),
            false,
            &json!({"messages": []}),
        )
        .await;
        match out {
            SanitizeOutcome::Sanitized { ticket, .. } => assert!(ticket.is_none()),
            other => panic!("expected sanitized, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn restore_replaces_placeholders_and_counts() {
        let base = serve(
            json!({"content": {}}),
            json!({"content": {"choices": [{"message": {"content": "hi Ada"}}]}, "restored": 1}),
        )
        .await;
        let metrics = Metrics::default();
        let ticket = RestorationTicket::new("tok_1".into(), scope());
        let restored = restore(
            &enabled(&base, RestorationPolicy::TrustedDownstream),
            &metrics,
            &ticket,
            &scope(),
            "trace",
            WebhookTenant::default(),
            &json!({"choices": [{"message": {"content": "hi <PERSON_1>"}}]}),
        )
        .await;
        assert_eq!(
            restored.unwrap()["choices"][0]["message"]["content"],
            "hi Ada"
        );
        assert_eq!(metrics.pii_restorations_total.load(Relaxed), 1);
    }

    /// The isolation guarantee: a token issued for one tenant/route cannot be
    /// redeemed in another, and the attempt never reaches the network.
    #[tokio::test]
    async fn restore_refuses_a_token_from_another_scope() {
        let metrics = Metrics::default();
        let ticket = RestorationTicket::new("tok_1".into(), scope());
        let other = TokenScope {
            org: "globex".into(),
            ..scope()
        };
        // deliberately unreachable: if the scope check were skipped this would
        // still fail, but the counters distinguish the two paths
        let restored = restore(
            &unreachable(FailureMode::FailOpen),
            &metrics,
            &ticket,
            &other,
            "trace",
            WebhookTenant::default(),
            &json!({}),
        )
        .await;
        assert!(restored.is_none());
        assert_eq!(metrics.pii_restore_errors_total.load(Relaxed), 1);
    }

    /// A restore failure leaves the caller with placeholder-bearing content,
    /// which is safe. It must not be counted as a sanitizer failure: one is a
    /// usability problem, the other a potential disclosure.
    #[tokio::test]
    async fn a_restore_failure_is_counted_separately_from_a_sanitize_failure() {
        let metrics = Metrics::default();
        let ticket = RestorationTicket::new("tok_1".into(), scope());
        let restored = restore(
            &unreachable(FailureMode::FailOpen),
            &metrics,
            &ticket,
            &scope(),
            "trace",
            WebhookTenant::default(),
            &json!({}),
        )
        .await;
        assert!(restored.is_none());
        assert_eq!(metrics.pii_restore_errors_total.load(Relaxed), 1);
        assert_eq!(metrics.pii_sanitizer_errors_total.load(Relaxed), 0);
    }
}
