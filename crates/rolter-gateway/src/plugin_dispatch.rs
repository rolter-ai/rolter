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

use rolter_core::guardrail_webhook::{WebhookAuth, WebhookDecision};
use rolter_core::{
    FailureMode, PluginInstanceConfig, PluginRequest, PluginStage, WebhookTenant,
    PLUGIN_CONTRACT_VERSION,
};
use serde_json::Value;

use crate::egress_client::EgressClient;
use crate::metrics::Metrics;

/// Per-call timeout. The registry table carries no per-instance timeout
/// column (unlike the guardrail webhook), so every plugin gets the same
/// bound the guardrail webhook defaults to.
const TIMEOUT_MS: u64 = rolter_core::guardrail_webhook::DEFAULT_TIMEOUT_MS;
/// Cap on the content bytes forwarded to a plugin; oversized content is
/// truncated and flagged, same policy as the guardrail webhook.
const MAX_BODY_BYTES: usize = rolter_core::guardrail_webhook::DEFAULT_MAX_BODY_BYTES;

/// What the gateway should do after running a stage's plugin chain.
#[derive(Debug, Clone, PartialEq)]
pub enum DispatchOutcome {
    /// forward with `content` (unchanged if no plugin transformed it)
    Allow(Value),
    /// reject the request with an OpenAI-compatible error; carries the reason
    Block(Option<String>),
}

/// What one plugin call came to, as the `<outcome>:<slug>` entry the request
/// log keeps in `plugin_events` (#2243). An `error` entry carries a third part,
/// the failure kind (see [`CallFailure::kind`]), because a transport failure
/// has no verdict to name and the kind is what tells an operator whether the
/// endpoint is down, slow or refused. Slugs are `[a-z0-9-]`, so neither `:`
/// splitting nor the entry can carry request content.
fn event(outcome: &str, slug: &str) -> String {
    format!("{outcome}:{slug}")
}

/// Why a plugin call produced no verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CallFailure {
    /// the egress policy refused the endpoint, so nothing was sent
    Refused,
    /// the request timed out
    Timeout,
    /// the connection or the exchange failed before a status came back
    Connect,
    /// the endpoint answered with a non-2xx status
    Status(u16),
    /// the response body could not be read
    Body,
}

impl CallFailure {
    /// A short, fixed-vocabulary label: never the upstream's own text.
    fn kind(&self) -> String {
        match self {
            Self::Refused => "refused".to_string(),
            Self::Timeout => "timeout".to_string(),
            Self::Connect => "connect".to_string(),
            Self::Status(code) => format!("status_{code}"),
            Self::Body => "body".to_string(),
        }
    }
}

/// Run every plugin in `plugins` (already the caller's org/project matched at
/// `stage`, in dispatch order — see [`rolter_core::PluginsConfig::for_stage`])
/// against `content`, threading each plugin's output into the next. Never
/// returns an error: a transport failure resolves per that plugin's own
/// `failure_mode`, exactly like the guardrail webhook.
///
/// Every call leaves one entry in `events` (`allowed`, `blocked`,
/// `transformed`, `annotated` or `error`, each `:<slug>`), so the request log
/// can say per plugin what it did. A fail-closed plugin that could not be
/// reached counts as an `error`, not a `blocked`: it gave no verdict.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch(
    plugins: &[&PluginInstanceConfig],
    stage: PluginStage,
    metrics: &Metrics,
    egress: &EgressClient,
    model: &str,
    route: &str,
    trace_id: &str,
    tenant: &WebhookTenant,
    events: &mut Vec<String>,
    content: &Value,
) -> DispatchOutcome {
    let mut current = content.clone();
    for plugin in plugins {
        let (decision, failure) = consult_one(
            plugin, stage, metrics, egress, model, route, trace_id, tenant, &current,
        )
        .await;
        // a call that gave no verdict is an `error` whatever its failure mode
        // then did with the request
        events.push(match (&failure, &decision) {
            (Some(failure), _) => format!("{}:{}", event("error", &plugin.slug), failure.kind()),
            (None, WebhookDecision::Allow) => event("allowed", &plugin.slug),
            (None, WebhookDecision::Annotate { .. }) => event("annotated", &plugin.slug),
            (None, WebhookDecision::Block { .. }) => event("blocked", &plugin.slug),
            (None, WebhookDecision::Transform { .. }) => event("transformed", &plugin.slug),
        });
        match decision {
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
/// `failure_mode` (fail-open forwards unchanged, fail-closed blocks). The
/// second half says why the call failed, when it did.
#[allow(clippy::too_many_arguments)]
async fn consult_one(
    plugin: &PluginInstanceConfig,
    stage: PluginStage,
    metrics: &Metrics,
    egress: &EgressClient,
    model: &str,
    route: &str,
    trace_id: &str,
    tenant: &WebhookTenant,
    content: &Value,
) -> (WebhookDecision, Option<CallFailure>) {
    let serialized = serde_json::to_vec(content).unwrap_or_default();
    let (payload, truncated) = if serialized.len() > MAX_BODY_BYTES {
        let preview: String = content.to_string().chars().take(MAX_BODY_BYTES).collect();
        (Value::String(preview), true)
    } else {
        (content.clone(), false)
    };

    let envelope = PluginRequest {
        contract_version: PLUGIN_CONTRACT_VERSION,
        plugin: &plugin.slug,
        stage,
        model,
        route,
        trace_id,
        tenant: tenant.clone(),
        truncated,
        content: &payload,
    };

    match call_once(egress, plugin, &envelope).await {
        Ok(decision) => (decision, None),
        Err(failure) => {
            metrics.plugin_errors_total.fetch_add(1, Relaxed);
            let decision = match plugin.failure_mode {
                FailureMode::FailOpen => WebhookDecision::Allow,
                FailureMode::FailClosed | FailureMode::Unknown => WebhookDecision::Block {
                    reason: Some(format!("plugin '{}' is unavailable", plugin.slug)),
                },
            };
            (decision, Some(failure))
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
    pub egress: &'a EgressClient,
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
///
/// Each plugin call leaves an entry in `events`, as for [`dispatch`].
pub async fn apply_post_response(
    ctx: &PostResponsePlugins<'_>,
    events: &mut Vec<String>,
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
        ctx.egress,
        &ctx.model,
        &ctx.route,
        &ctx.trace_id,
        &ctx.tenant,
        events,
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

/// A single call to `plugin.endpoint`. `Err` signals a transport failure
/// (refused, connect, timeout, non-2xx, or unreadable body); the caller applies the
/// plugin's `failure_mode`. No retries: the registry has no per-instance
/// retry count, and the guardrail webhook's default is zero anyway.
async fn call_once(
    egress: &EgressClient,
    plugin: &PluginInstanceConfig,
    envelope: &PluginRequest<'_>,
) -> Result<WebhookDecision, CallFailure> {
    // refused again here, not only at save: the policy may have been tightened
    // since, and a stored row is not a standing permission to egress
    let mut req = egress
        .post(plugin.endpoint.trim())
        .ok_or(CallFailure::Refused)?
        .timeout(std::time::Duration::from_millis(TIMEOUT_MS))
        .header("X-Rolter-Trace-Id", envelope.trace_id)
        .json(envelope);

    if let Some(auth @ WebhookAuth::Bearer { token_env }) = &plugin.auth {
        // the plugin token is this purpose's documented credential; any other
        // deployment secret a stored row names is not sent
        if let Some(token) = auth.resolve_secret(&["ROLTER_PLUGIN_TOKEN"]) {
            req = req.bearer_auth(token);
        } else if rolter_core::is_secret_unless_allowed(token_env, &["ROLTER_PLUGIN_TOKEN"]) {
            tracing::warn!(env = %token_env, "plugin auth names a deployment secret and was not sent");
        }
    }

    let resp = req.send().await.map_err(|error| {
        if error.is_timeout() {
            CallFailure::Timeout
        } else {
            CallFailure::Connect
        }
    })?;
    if !resp.status().is_success() {
        return Err(CallFailure::Status(resp.status().as_u16()));
    }
    let body = resp.bytes().await.map_err(|_| CallFailure::Body)?;
    Ok(WebhookDecision::parse(&body))
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
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut Vec::new(),
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
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut Vec::new(),
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
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut Vec::new(),
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

    /// Every body carries the contract version, so a plugin author can tell
    /// which shape they were sent and rolter can change it compatibly (#1405).
    #[tokio::test]
    async fn the_posted_body_carries_the_contract_version() {
        use axum::routing::post;
        use axum::{Json, Router};
        let seen: std::sync::Arc<parking_lot::Mutex<Option<Value>>> = Default::default();
        let sink = seen.clone();
        let app = Router::new().route(
            "/hook",
            post(move |Json(body): Json<Value>| {
                let sink = sink.clone();
                async move {
                    *sink.lock() = Some(body);
                    Json(serde_json::json!({"action": "allow"}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let p = plugin(
            "audit",
            format!("http://{addr}/hook"),
            FailureMode::FailClosed,
        );
        let out = dispatch(
            &[&p],
            PluginStage::PreRoute,
            &Metrics::default(),
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut Vec::new(),
            &serde_json::json!({"messages": []}),
        )
        .await;
        assert!(matches!(out, DispatchOutcome::Allow(_)));
        let body = seen.lock().take().expect("the plugin received a body");
        assert_eq!(body["contract_version"], PLUGIN_CONTRACT_VERSION);
        assert_eq!(body["plugin"], "audit");
        assert_eq!(body["stage"], "pre_route");
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
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut Vec::new(),
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
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut Vec::new(),
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
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut Vec::new(),
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
            egress: &crate::egress_client::testing::permissive(),
            model: "gpt-4".to_string(),
            route: "route".to_string(),
            trace_id: "trace".to_string(),
            tenant: WebhookTenant::default(),
        };
        let body = b"not json";
        let out = apply_post_response(&ctx, &mut Vec::new(), body).await;
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
            egress: &crate::egress_client::testing::permissive(),
            model: "gpt-4".to_string(),
            route: "route".to_string(),
            trace_id: "trace".to_string(),
            tenant: WebhookTenant::default(),
        };
        let out = apply_post_response(&ctx, &mut Vec::new(), br#"{"choices": ["raw"]}"#).await;
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
            egress: &crate::egress_client::testing::permissive(),
            model: "gpt-4".to_string(),
            route: "route".to_string(),
            trace_id: "trace".to_string(),
            tenant: WebhookTenant::default(),
        };
        let out = apply_post_response(&ctx, &mut Vec::new(), br#"{"choices": []}"#).await;
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
        unsafe { std::env::set_var("PLUGIN_TEST_BEARER_TOKEN", "secret-token") };
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
            token_env: "PLUGIN_TEST_BEARER_TOKEN".to_string(),
        });
        let metrics = Metrics::default();
        let out = dispatch(
            &[&p],
            PluginStage::PreUpstream,
            &metrics,
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut Vec::new(),
            &serde_json::json!({"messages": []}),
        )
        .await;
        assert_eq!(
            out,
            DispatchOutcome::Allow(serde_json::json!({"messages": []}))
        );
        // SAFETY: cleanup of the test-local var set above
        unsafe { std::env::remove_var("PLUGIN_TEST_BEARER_TOKEN") };
    }

    /// #3097: a stored row naming a `ROLTER_` secret must not send it out
    #[tokio::test]
    async fn a_deployment_secret_named_as_the_bearer_env_is_not_sent() {
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
                Json(serde_json::json!({"action": if saw_auth { "block" } else { "allow" }}))
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
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut Vec::new(),
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

    /// #2383: a fail-closed plugin whose endpoint the policy denies blocks
    /// rather than reaching it, for a literal and for a name that resolves to
    /// a denied address.
    #[tokio::test]
    async fn a_denied_plugin_endpoint_is_never_called() {
        let metrics = Metrics::default();
        let content = serde_json::json!({"messages": []});
        let listener = crate::egress_client::testing::Counter::start().await;
        let denied = EgressClient::new(crate::egress_client::testing::deny_loopback());
        for (egress, endpoint) in [
            (
                crate::egress_client::testing::permissive(),
                "https://169.254.169.254/hook".to_string(),
            ),
            (denied, listener.url("/hook")),
        ] {
            let p = plugin("audit", endpoint, FailureMode::FailClosed);
            let out = dispatch(
                &[&p],
                PluginStage::PreUpstream,
                &metrics,
                &egress,
                "gpt-4",
                "route",
                "trace",
                &WebhookTenant::default(),
                &mut Vec::new(),
                &content,
            )
            .await;
            assert!(matches!(out, DispatchOutcome::Block(_)), "{out:?}");
        }
        assert_eq!(listener.accepted(), 0);
    }

    /// A plugin keeps its own failure mode when the policy refuses its
    /// endpoint: fail-open passes the content on unchanged.
    #[tokio::test]
    async fn a_denied_fail_open_plugin_endpoint_passes_the_request() {
        let content = serde_json::json!({"messages": []});
        let p = plugin(
            "audit",
            "https://169.254.169.254/hook".to_string(),
            FailureMode::FailOpen,
        );
        let out = dispatch(
            &[&p],
            PluginStage::PreUpstream,
            &Metrics::default(),
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut Vec::new(),
            &content,
        )
        .await;
        assert_eq!(out, DispatchOutcome::Allow(content));
    }

    #[tokio::test]
    async fn every_call_leaves_one_event_per_plugin_in_order() {
        let allow = serve(serde_json::json!({"action": "allow"})).await;
        let annotate = serve(serde_json::json!({"action": "annotate", "annotations": {}})).await;
        let transform = serve(serde_json::json!({"action": "transform", "content": {}})).await;
        let first = plugin("first", allow, FailureMode::FailOpen);
        let second = plugin("second", annotate, FailureMode::FailOpen);
        let third = plugin("third", transform, FailureMode::FailOpen);
        let mut events = Vec::new();
        dispatch(
            &[&first, &second, &third],
            PluginStage::PreUpstream,
            &Metrics::default(),
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut events,
            &serde_json::json!({"messages": []}),
        )
        .await;
        assert_eq!(
            events,
            ["allowed:first", "annotated:second", "transformed:third"]
        );
    }

    #[tokio::test]
    async fn a_failing_call_names_why_in_a_fixed_vocabulary() {
        use axum::routing::post;
        use axum::Router;
        let app = Router::new().route(
            "/hook",
            post(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let down = plugin("down", format!("http://{addr}/hook"), FailureMode::FailOpen);
        let refused = plugin(
            "refused",
            "https://169.254.169.254/hook".to_string(),
            FailureMode::FailOpen,
        );
        let mut events = Vec::new();
        dispatch(
            &[&down, &refused],
            PluginStage::PreUpstream,
            &Metrics::default(),
            &crate::egress_client::testing::permissive(),
            "gpt-4",
            "route",
            "trace",
            &WebhookTenant::default(),
            &mut events,
            &serde_json::json!({"messages": []}),
        )
        .await;
        assert_eq!(events, ["error:down:status_503", "error:refused:refused"]);
    }
}
