//! Request-path client for enabled webhook plugin instances (#509).
//!
//! Generalizes the custom guardrail webhook's request/response contract
//! (#257, `crate::guardrail_webhook`) from one deployment-wide webhook to an
//! ordered, org/project-scoped list: every plugin enabled for the caller's
//! tenant at a given [`PluginStage`] is consulted in `position` order, each
//! seeing the content the previous one left behind. A `block` from any plugin
//! stops the chain immediately; the wire contract, timeout/size bounds and
//! fail-open/fail-closed semantics are otherwise identical to the guardrail
//! webhook.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::OnceLock;

use rolter_core::guardrail_webhook::{WebhookAuth, WebhookDecision};
use rolter_core::{FailureMode, PluginInstanceConfig, PluginRequest, PluginStage, WebhookTenant};
use serde_json::Value;

use crate::metrics::Metrics;

/// Per-call timeout. The registry table carries no per-instance timeout
/// column (unlike the guardrail webhook), so every plugin gets the same
/// bound the guardrail webhook defaults to.
const TIMEOUT_MS: u64 = rolter_core::guardrail_webhook::DEFAULT_TIMEOUT_MS;
/// Cap on the content bytes forwarded to a plugin; oversized content is
/// truncated and flagged, same policy as the guardrail webhook.
const MAX_BODY_BYTES: usize = rolter_core::guardrail_webhook::DEFAULT_MAX_BODY_BYTES;

/// Shared client for plugin calls: connection pooling across requests, no
/// per-call setup cost. Separate from the guardrail webhook's client so a
/// burst of plugin traffic cannot starve guardrail connections or vice versa.
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// What the gateway should do after running a stage's plugin chain.
#[derive(Debug, Clone, PartialEq)]
pub enum DispatchOutcome {
    /// forward with `content` (unchanged if no plugin transformed it)
    Allow(Value),
    /// reject the request with an OpenAI-compatible error; carries the reason
    Block(Option<String>),
}

/// Run every plugin in `plugins` (already the caller's org/project matched at
/// `stage`, in dispatch order — see [`rolter_core::PluginsConfig::for_stage`])
/// against `content`, threading each plugin's output into the next. Never
/// returns an error: a transport failure resolves per that plugin's own
/// `failure_mode`, exactly like the guardrail webhook.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch(
    plugins: &[&PluginInstanceConfig],
    stage: PluginStage,
    metrics: &Metrics,
    model: &str,
    route: &str,
    trace_id: &str,
    tenant: &WebhookTenant,
    content: &Value,
) -> DispatchOutcome {
    let mut current = content.clone();
    for plugin in plugins {
        match consult_one(
            plugin, stage, metrics, model, route, trace_id, tenant, &current,
        )
        .await
        {
            WebhookDecision::Allow | WebhookDecision::Annotate { .. } => {}
            WebhookDecision::Block { reason } => {
                metrics.plugin_blocks_total.fetch_add(1, Relaxed);
                return DispatchOutcome::Block(reason);
            }
            WebhookDecision::Transform { content } => {
                metrics.plugin_transforms_total.fetch_add(1, Relaxed);
                current = content;
            }
        }
    }
    DispatchOutcome::Allow(current)
}

/// Consult a single plugin instance, resolving a transport failure to its own
/// `failure_mode` (fail-open forwards unchanged, fail-closed blocks).
#[allow(clippy::too_many_arguments)]
async fn consult_one(
    plugin: &PluginInstanceConfig,
    stage: PluginStage,
    metrics: &Metrics,
    model: &str,
    route: &str,
    trace_id: &str,
    tenant: &WebhookTenant,
    content: &Value,
) -> WebhookDecision {
    let serialized = serde_json::to_vec(content).unwrap_or_default();
    let (payload, truncated) = if serialized.len() > MAX_BODY_BYTES {
        let preview: String = content.to_string().chars().take(MAX_BODY_BYTES).collect();
        (Value::String(preview), true)
    } else {
        (content.clone(), false)
    };

    let envelope = PluginRequest {
        plugin: &plugin.slug,
        stage,
        model,
        route,
        trace_id,
        tenant: tenant.clone(),
        truncated,
        content: &payload,
    };

    match call_once(plugin, &envelope).await {
        Some(decision) => decision,
        None => {
            metrics.plugin_errors_total.fetch_add(1, Relaxed);
            match plugin.failure_mode {
                FailureMode::FailOpen => WebhookDecision::Allow,
                FailureMode::FailClosed => WebhookDecision::Block {
                    reason: Some(format!("plugin '{}' is unavailable", plugin.slug)),
                },
            }
        }
    }
}

/// Everything [`apply_post_response`] needs, bundled once at the point the
/// route/scope are known so the two non-streaming response call sites in
/// `handlers.rs` don't each carry a long parameter list.
#[derive(Clone)]
pub struct PostResponsePlugins<'a> {
    pub plugins: Vec<&'a PluginInstanceConfig>,
    pub metrics: &'a Metrics,
    /// owned rather than borrowed: the request path moves `model`/`trace_id`
    /// into the request log before this reaches [`apply_post_response`]
    pub model: String,
    pub route: String,
    pub trace_id: String,
    pub tenant: WebhookTenant,
}

/// What the caller should do with a non-streaming response body after running
/// the `post_response` plugin chain.
#[derive(Debug, Clone, PartialEq)]
pub enum PostResponseOutcome {
    /// forward `bytes` (unchanged if no plugin transformed it, or if the body
    /// was not valid JSON — a plugin has nothing structured to inspect then,
    /// so the stage is skipped rather than treated as a failure)
    Bytes(Vec<u8>),
    Block(Option<String>),
}

/// Run the `post_response` plugin chain over a non-streaming response body.
/// Bodies that are not valid JSON pass through unexamined: every plugin's
/// contract is JSON in, JSON out, so there is nothing to send.
pub async fn apply_post_response(
    ctx: &PostResponsePlugins<'_>,
    bytes: &[u8],
) -> PostResponseOutcome {
    if ctx.plugins.is_empty() {
        return PostResponseOutcome::Bytes(bytes.to_vec());
    }
    let Ok(content) = serde_json::from_slice::<Value>(bytes) else {
        return PostResponseOutcome::Bytes(bytes.to_vec());
    };
    match dispatch(
        &ctx.plugins,
        PluginStage::PostResponse,
        ctx.metrics,
        &ctx.model,
        &ctx.route,
        &ctx.trace_id,
        &ctx.tenant,
        &content,
    )
    .await
    {
        DispatchOutcome::Allow(content) => PostResponseOutcome::Bytes(
            serde_json::to_vec(&content).unwrap_or_else(|_| bytes.to_vec()),
        ),
        DispatchOutcome::Block(reason) => PostResponseOutcome::Block(reason),
    }
}

/// A single call to `plugin.endpoint`. `None` signals a transport failure
/// (connect, timeout, non-2xx, or unreadable body); the caller applies the
/// plugin's `failure_mode`. No retries: the registry has no per-instance
/// retry count, and the guardrail webhook's default is zero anyway.
async fn call_once(
    plugin: &PluginInstanceConfig,
    envelope: &PluginRequest<'_>,
) -> Option<WebhookDecision> {
    let mut req = client()
        .post(plugin.endpoint.trim())
        .timeout(std::time::Duration::from_millis(TIMEOUT_MS))
        .header("X-Rolter-Trace-Id", envelope.trace_id)
        .json(envelope);

    if let Some(WebhookAuth::Bearer { token_env }) = &plugin.auth {
        if let Ok(token) = std::env::var(token_env) {
            req = req.bearer_auth(token);
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

    fn plugin(slug: &str, endpoint: String, failure_mode: FailureMode) -> PluginInstanceConfig {
        PluginInstanceConfig {
            slug: slug.to_string(),
            org_id: "org-a".to_string(),
            project_id: None,
            stage: PluginStage::PreUpstream,
            position: 0,
            failure_mode,
            endpoint,
            auth: None,
        }
    }

    #[tokio::test]
    async fn no_plugins_allows_content_unchanged() {
        let metrics = Metrics::default();
        let content = serde_json::json!({"messages": []});
        let out = dispatch(
            &[],
            PluginStage::PreUpstream,
            &metrics,
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &content,
        )
        .await;
        assert_eq!(out, DispatchOutcome::Allow(content));
    }

    #[tokio::test]
    async fn unreachable_plugin_fails_open_by_default() {
        let metrics = Metrics::default();
        let p = plugin(
            "audit",
            "http://192.0.2.1:1/hook".to_string(),
            FailureMode::FailOpen,
        );
        let content = serde_json::json!({"messages": []});
        let out = dispatch(
            &[&p],
            PluginStage::PreUpstream,
            &metrics,
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &content,
        )
        .await;
        assert_eq!(out, DispatchOutcome::Allow(content));
        assert_eq!(metrics.plugin_errors_total.load(Relaxed), 1);
    }

    #[tokio::test]
    async fn unreachable_plugin_fails_closed_when_configured() {
        let metrics = Metrics::default();
        let p = plugin(
            "audit",
            "http://192.0.2.1:1/hook".to_string(),
            FailureMode::FailClosed,
        );
        let out = dispatch(
            &[&p],
            PluginStage::PreUpstream,
            &metrics,
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &serde_json::json!({"messages": []}),
        )
        .await;
        assert!(matches!(out, DispatchOutcome::Block(_)));
    }

    async fn serve(decision: Value) -> String {
        use axum::routing::post;
        use axum::{Json, Router};
        let app = Router::new().route(
            "/hook",
            post(move || {
                let decision = decision.clone();
                async move { Json(decision) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/hook")
    }

    #[tokio::test]
    async fn block_from_any_plugin_stops_the_chain() {
        let url = serve(serde_json::json!({"action": "block", "reason": "denied"})).await;
        let metrics = Metrics::default();
        let p = plugin("audit", url, FailureMode::FailOpen);
        let out = dispatch(
            &[&p],
            PluginStage::PreUpstream,
            &metrics,
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &serde_json::json!({"messages": []}),
        )
        .await;
        assert_eq!(out, DispatchOutcome::Block(Some("denied".to_string())));
        assert_eq!(metrics.plugin_blocks_total.load(Relaxed), 1);
    }

    #[tokio::test]
    async fn transforms_thread_through_multiple_plugins_in_order() {
        let first_url = serve(serde_json::json!({
            "action": "transform",
            "content": {"messages": ["first"]}
        }))
        .await;
        let second_url = serve(serde_json::json!({
            "action": "transform",
            "content": {"messages": ["first", "second"]}
        }))
        .await;
        let metrics = Metrics::default();
        let first = plugin("first", first_url, FailureMode::FailOpen);
        let second = plugin("second", second_url, FailureMode::FailOpen);
        let out = dispatch(
            &[&first, &second],
            PluginStage::PreUpstream,
            &metrics,
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &serde_json::json!({"messages": []}),
        )
        .await;
        assert_eq!(
            out,
            DispatchOutcome::Allow(serde_json::json!({"messages": ["first", "second"]}))
        );
        assert_eq!(metrics.plugin_transforms_total.load(Relaxed), 2);
    }

    #[tokio::test]
    async fn a_block_prevents_later_plugins_from_running() {
        let block_url = serve(serde_json::json!({"action": "block"})).await;
        let metrics = Metrics::default();
        let first = plugin("first", block_url, FailureMode::FailOpen);
        // second would transform, but must never run since first blocked
        let second = plugin(
            "second",
            "http://192.0.2.1:1/unreachable".to_string(),
            FailureMode::FailClosed,
        );
        let out = dispatch(
            &[&first, &second],
            PluginStage::PreUpstream,
            &metrics,
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &serde_json::json!({"messages": []}),
        )
        .await;
        assert!(matches!(out, DispatchOutcome::Block(_)));
        assert_eq!(metrics.plugin_errors_total.load(Relaxed), 0);
    }

    #[tokio::test]
    async fn post_response_passes_through_non_json_bodies_unexamined() {
        let metrics = Metrics::default();
        let p = plugin(
            "audit",
            "http://192.0.2.1:1/hook".to_string(),
            FailureMode::FailClosed,
        );
        let ctx = PostResponsePlugins {
            plugins: vec![&p],
            metrics: &metrics,
            model: "gpt-4".to_string(),
            route: "route".to_string(),
            trace_id: "trace".to_string(),
            tenant: WebhookTenant::default(),
        };
        let body = b"not json";
        let out = apply_post_response(&ctx, body).await;
        assert_eq!(out, PostResponseOutcome::Bytes(body.to_vec()));
        // never dispatched, so no error is recorded even though fail_closed
        assert_eq!(metrics.plugin_errors_total.load(Relaxed), 0);
    }

    #[tokio::test]
    async fn post_response_transforms_a_json_body() {
        let url = serve(serde_json::json!({
            "action": "transform",
            "content": {"choices": ["redacted"]}
        }))
        .await;
        let metrics = Metrics::default();
        let p = plugin("audit", url, FailureMode::FailOpen);
        let ctx = PostResponsePlugins {
            plugins: vec![&p],
            metrics: &metrics,
            model: "gpt-4".to_string(),
            route: "route".to_string(),
            trace_id: "trace".to_string(),
            tenant: WebhookTenant::default(),
        };
        let out = apply_post_response(&ctx, br#"{"choices": ["raw"]}"#).await;
        assert_eq!(
            out,
            PostResponseOutcome::Bytes(
                serde_json::to_vec(&serde_json::json!({"choices": ["redacted"]})).unwrap()
            )
        );
    }

    #[tokio::test]
    async fn post_response_block_is_reported() {
        let url = serve(serde_json::json!({"action": "block", "reason": "leaked pii"})).await;
        let metrics = Metrics::default();
        let p = plugin("audit", url, FailureMode::FailOpen);
        let ctx = PostResponsePlugins {
            plugins: vec![&p],
            metrics: &metrics,
            model: "gpt-4".to_string(),
            route: "route".to_string(),
            trace_id: "trace".to_string(),
            tenant: WebhookTenant::default(),
        };
        let out = apply_post_response(&ctx, br#"{"choices": []}"#).await;
        assert_eq!(
            out,
            PostResponseOutcome::Block(Some("leaked pii".to_string()))
        );
    }

    #[tokio::test]
    async fn bearer_auth_is_sent_when_the_env_var_is_set() {
        use axum::routing::post;
        use axum::{Json, Router};
        // SAFETY: test-local env var, no concurrent access to this key elsewhere
        unsafe { std::env::set_var("ROLTER_TEST_PLUGIN_TOKEN", "secret-token") };
        let app = Router::new().route(
            "/hook",
            post(|headers: axum::http::HeaderMap| async move {
                let saw_auth = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    == Some("Bearer secret-token");
                Json(serde_json::json!({"action": if saw_auth { "allow" } else { "block" }}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut p = plugin(
            "audit",
            format!("http://{addr}/hook"),
            FailureMode::FailOpen,
        );
        p.auth = Some(WebhookAuth::Bearer {
            token_env: "ROLTER_TEST_PLUGIN_TOKEN".to_string(),
        });
        let metrics = Metrics::default();
        let out = dispatch(
            &[&p],
            PluginStage::PreUpstream,
            &metrics,
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &serde_json::json!({"messages": []}),
        )
        .await;
        assert_eq!(
            out,
            DispatchOutcome::Allow(serde_json::json!({"messages": []}))
        );
        // SAFETY: cleanup of the test-local var set above
        unsafe { std::env::remove_var("ROLTER_TEST_PLUGIN_TOKEN") };
    }
}
