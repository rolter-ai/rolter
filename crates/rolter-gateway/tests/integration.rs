//! End-to-end gateway integration tests. Each test spins one or more real mock
//! upstream servers on ephemeral ports, serves the gateway router in-process on
//! another ephemeral port, and drives it over HTTP with reqwest — exercising the
//! full parse → auth → balance → forward → stream-back pipeline.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{OriginalUri, Path, State};
use axum::response::IntoResponse;
use axum::routing::{any, get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use rolter_core::{
    BalancingStrategy, GatewayConfig, McpOAuthSessionConfig, McpServerConfig, ModelPriceConfig,
    ModelRoute, ProviderConfig, ProviderKind, RoleProfile, Target, UnpricedPolicy,
    VirtualKeyConfig, VirtualKeyRecord,
};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message as WebSocketMessage;

/// Bind an axum app to an ephemeral port and serve it in the background,
/// returning the bound address.
async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// A mock upstream that answers `/v1/chat/completions` with a canned OpenAI
/// chat completion, or an SSE stream when the request asks for `stream: true`.
async fn mock_openai(body: Json<Value>) -> axum::response::Response {
    let streaming = body
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    if streaming {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"pong\"}}]}\n\n\
                   data: [DONE]\n\n";
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            sse,
        )
            .into_response()
    } else {
        Json(json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "pong"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }))
        .into_response()
    }
}

/// Build a gateway config with a single route pointing at the given upstreams.
fn config_for(model: &str, providers: Vec<(&str, SocketAddr)>) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    let targets = providers
        .iter()
        .map(|(name, _)| Target {
            provider: name.to_string(),
            model: None,
            weight: 1,
        })
        .collect();
    for (name, addr) in providers {
        config.providers.push(ProviderConfig {
            name: name.to_string(),
            kind: ProviderKind::OpenaiCompatible,
            api_base: format!("http://{addr}"),
            ..Default::default()
        });
    }
    config.routes.push(ModelRoute {
        model: model.to_string(),
        strategy: BalancingStrategy::RoundRobin,
        targets,
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
    });
    config
}

/// Serve the gateway from a config and return its address.
async fn serve_gateway(config: &GatewayConfig) -> SocketAddr {
    let app = rolter_gateway::build_router_from_config(config);
    serve(app).await
}

#[tokio::test]
async fn mcp_proxy_binds_virtual_key_owner_to_server_scopes_and_bearer() {
    async fn mcp_upstream(
        OriginalUri(uri): OriginalUri,
        headers: axum::http::HeaderMap,
        body: String,
    ) -> Json<Value> {
        Json(json!({
            "authorization": headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            "x_api_key": headers.get("x-api-key").and_then(|value| value.to_str().ok()),
            "cookie": headers.get("cookie").and_then(|value| value.to_str().ok()),
            "uri": uri.to_string(),
            "body": body,
        }))
    }

    let upstream = serve(Router::new().route("/{*path}", any(mcp_upstream))).await;
    let mut config = GatewayConfig::default();
    config.db_virtual_keys.push(VirtualKeyRecord {
        access_policy: None,
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), "sk-user-owned"),
        id: "key-1".to_string(),
        org_id: "org-1".to_string(),
        team_id: "team-1".to_string(),
        project_id: "project-1".to_string(),
        user_id: "user-1".to_string(),
        models: Vec::new(),
        providers: Vec::new(),
        disabled: false,
        expires_at: None,
        cache: None,
        business_unit_id: String::new(),
        customer_id: String::new(),
    });
    config.mcp_servers.push(McpServerConfig {
        id: "server-1".to_string(),
        org_id: "org-1".to_string(),
        slug: "docs".to_string(),
        url: format!("http://{upstream}/rpc"),
        transport: "streamable_http".to_string(),
        required_scopes: vec!["tools:execute".to_string()],
        auth_kind: rolter_core::McpAuthKind::Oauth,
        ..Default::default()
    });
    config.mcp_oauth_sessions.push(McpOAuthSessionConfig {
        id: "session-1".to_string(),
        server_id: "server-1".to_string(),
        user_id: "user-1".to_string(),
        scopes: vec!["tools:execute".to_string()],
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        access_token: "oauth-downstream".to_string(),
    });
    let gateway = serve_gateway(&config).await;

    let response: Value = reqwest::Client::new()
        .post(format!("http://{gateway}/mcp/docs/messages?cursor=next"))
        .header("x-api-key", "sk-user-owned")
        .header("cookie", "gateway-session=must-not-leak")
        .body(r#"{"jsonrpc":"2.0","method":"tools/list"}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        response,
        json!({
            "authorization": "Bearer oauth-downstream",
            "x_api_key": null,
            "cookie": null,
            "uri": "/rpc/messages?cursor=next",
            "body": "{\"jsonrpc\":\"2.0\",\"method\":\"tools/list\"}",
        })
    );
}

/// A static credential is the point of #952: before it, a `bearer`/`header`
/// server was refused for want of an OAuth session however it was configured,
/// so the stored credential could not be observed on the wire at all.
///
/// Both kinds are exercised against one upstream that echoes what it received,
/// because "the credential is sent" and "it is sent in the right place" are
/// separate claims and the second is the one a wrong implementation gets wrong.
#[tokio::test]
async fn mcp_proxy_presents_a_stored_static_credential_without_an_oauth_session() {
    async fn echo(headers: axum::http::HeaderMap) -> Json<Value> {
        Json(json!({
            "authorization": headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            "x_tenant_key": headers.get("x-tenant-key").and_then(|value| value.to_str().ok()),
        }))
    }

    let upstream = serve(Router::new().route("/{*path}", any(echo))).await;
    let mut config = GatewayConfig::default();
    config.db_virtual_keys.push(VirtualKeyRecord {
        access_policy: None,
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), "sk-user-owned"),
        id: "key-1".to_string(),
        org_id: "org-1".to_string(),
        team_id: "team-1".to_string(),
        project_id: "project-1".to_string(),
        user_id: "user-1".to_string(),
        models: Vec::new(),
        providers: Vec::new(),
        disabled: false,
        expires_at: None,
        cache: None,
        business_unit_id: String::new(),
        customer_id: String::new(),
    });
    config.mcp_servers.push(McpServerConfig {
        id: "server-bearer".to_string(),
        org_id: "org-1".to_string(),
        slug: "bearer-server".to_string(),
        url: format!("http://{upstream}/rpc"),
        transport: "streamable_http".to_string(),
        auth_kind: rolter_core::McpAuthKind::Bearer,
        credential: Some("static-bearer-token".to_string()),
        ..Default::default()
    });
    config.mcp_servers.push(McpServerConfig {
        id: "server-header".to_string(),
        org_id: "org-1".to_string(),
        slug: "header-server".to_string(),
        url: format!("http://{upstream}/rpc"),
        transport: "streamable_http".to_string(),
        auth_kind: rolter_core::McpAuthKind::Header,
        auth_header_name: Some("x-tenant-key".to_string()),
        credential: Some("static-api-key".to_string()),
        ..Default::default()
    });
    // deliberately no mcp_oauth_sessions: a static credential must not need one
    let gateway = serve_gateway(&config).await;
    let client = reqwest::Client::new();

    let bearer: Value = client
        .post(format!("http://{gateway}/mcp/bearer-server/messages"))
        .header("x-api-key", "sk-user-owned")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        bearer,
        json!({"authorization": "Bearer static-bearer-token", "x_tenant_key": null}),
        "a bearer credential belongs in Authorization and nowhere else"
    );

    let header: Value = client
        .post(format!("http://{gateway}/mcp/header-server/messages"))
        .header("x-api-key", "sk-user-owned")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        header,
        json!({"authorization": null, "x_tenant_key": "static-api-key"}),
        "a header credential must go in its own header and must not also set Authorization"
    );
}

/// A server configured to present a credential the gateway does not have is a
/// misconfiguration that has to say so. Forwarding with no credential would
/// surface as the upstream's own 401 and send the operator looking at their
/// token rather than at their KEK (#952).
#[tokio::test]
async fn mcp_proxy_refuses_a_credentialled_server_whose_secret_did_not_arrive() {
    let mut config = GatewayConfig::default();
    config.db_virtual_keys.push(VirtualKeyRecord {
        access_policy: None,
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), "sk-user-owned"),
        id: "key-1".to_string(),
        org_id: "org-1".to_string(),
        team_id: "team-1".to_string(),
        project_id: "project-1".to_string(),
        user_id: "user-1".to_string(),
        models: Vec::new(),
        providers: Vec::new(),
        disabled: false,
        expires_at: None,
        cache: None,
        business_unit_id: String::new(),
        customer_id: String::new(),
    });
    config.mcp_servers.push(McpServerConfig {
        id: "server-1".to_string(),
        org_id: "org-1".to_string(),
        slug: "sealed".to_string(),
        // 127.0.0.1:9 is the discard port: reaching it would be the bug
        url: "http://127.0.0.1:9/rpc".to_string(),
        transport: "streamable_http".to_string(),
        auth_kind: rolter_core::McpAuthKind::Bearer,
        credential: None,
        ..Default::default()
    });
    let gateway = serve_gateway(&config).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/mcp/sealed/messages"))
        .header("x-api-key", "sk-user-owned")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "mcp_credential_unavailable");
}

/// The per-server `request_timeout_ms` override has to bound a slow upstream,
/// and bound it at the server's value rather than the deployment's (#952).
#[tokio::test]
async fn mcp_proxy_applies_the_per_server_request_timeout() {
    async fn slow() -> Json<Value> {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        Json(json!({"never": "reached"}))
    }

    let upstream = serve(Router::new().route("/{*path}", any(slow))).await;
    let mut config = GatewayConfig::default();
    config.db_virtual_keys.push(VirtualKeyRecord {
        access_policy: None,
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), "sk-user-owned"),
        id: "key-1".to_string(),
        org_id: "org-1".to_string(),
        team_id: "team-1".to_string(),
        project_id: "project-1".to_string(),
        user_id: "user-1".to_string(),
        models: Vec::new(),
        providers: Vec::new(),
        disabled: false,
        expires_at: None,
        cache: None,
        business_unit_id: String::new(),
        customer_id: String::new(),
    });
    config.mcp_servers.push(McpServerConfig {
        id: "server-1".to_string(),
        org_id: "org-1".to_string(),
        slug: "slow".to_string(),
        url: format!("http://{upstream}/rpc"),
        transport: "streamable_http".to_string(),
        auth_kind: rolter_core::McpAuthKind::None,
        request_timeout_ms: Some(250),
        ..Default::default()
    });
    let gateway = serve_gateway(&config).await;

    let started = std::time::Instant::now();
    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/mcp/slow/messages"))
        .header("x-api-key", "sk-user-owned")
        .send()
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(response.status(), 502);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "mcp_upstream_error");
    // the upstream sleeps for 30s; anything near that means the override was
    // ignored and the deployment default applied instead
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the 250ms override should have ended this, took {elapsed:?}"
    );
}

#[derive(Debug, PartialEq, Eq)]
struct CapturedRealtimeRequest {
    uri: String,
    authorization: Option<String>,
    beta: Option<String>,
}

#[expect(
    clippy::result_large_err,
    reason = "tungstenite's handshake callback fixes the large HTTP error response type"
)]
async fn serve_realtime_echo() -> (
    SocketAddr,
    tokio::sync::mpsc::UnboundedReceiver<CapturedRealtimeRequest>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (captured_tx, captured_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let captured_tx = captured_tx.clone();
            tokio::spawn(async move {
                let callback = move |
                    request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                    response: tokio_tungstenite::tungstenite::handshake::server::Response,
                | {
                    let _ = captured_tx.send(CapturedRealtimeRequest {
                        uri: request.uri().to_string(),
                        authorization: request
                            .headers()
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string),
                        beta: request
                            .headers()
                            .get("openai-beta")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string),
                    });
                    Ok(response)
                };
                let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(stream, callback).await
                else {
                    return;
                };
                while let Some(Ok(message)) = socket.next().await {
                    let close = message.is_close();
                    if socket.send(message).await.is_err() || close {
                        break;
                    }
                }
            });
        }
    });
    (addr, captured_rx)
}

fn realtime_client_request(
    gateway: SocketAddr,
    model: &str,
    authorization: Option<&str>,
    beta: Option<&str>,
) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut request =
        tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!(
            "ws://{gateway}/v1/realtime?model={model}"
        ))
        .unwrap();
    if let Some(authorization) = authorization {
        request.headers_mut().insert(
            axum::http::header::AUTHORIZATION,
            authorization.parse().unwrap(),
        );
    }
    if let Some(beta) = beta {
        request
            .headers_mut()
            .insert("openai-beta", beta.parse().unwrap());
    }
    request
}

fn websocket_handshake_status(error: tokio_tungstenite::tungstenite::Error) -> u16 {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => response.status().as_u16(),
        other => panic!("expected HTTP handshake rejection, got {other}"),
    }
}

#[tokio::test]
async fn non_streaming_request_proxies_upstream_body() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_openai))).await;
    let gw = serve_gateway(&config_for("test-model", vec![("up", upstream)])).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "pong");
}

#[tokio::test]
async fn models_endpoint_lists_route_ids_and_provider_slug_model_ids() {
    // a provider with an explicit slug, and a route that renames the upstream
    // model on its target: /v1/models should surface both the route id and the
    // provider-slug/model address (ADR-0017)
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "vLLM SPB".to_string(),
        slug: Some("vllm-spb".to_string()),
        kind: ProviderKind::OpenaiCompatible,
        api_base: "http://127.0.0.1:1".to_string(),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: "chat".to_string(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "vLLM SPB".to_string(),
            model: Some("qwen3".to_string()),
            weight: 1,
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
    });
    let gw = serve_gateway(&config).await;

    let models: Value = reqwest::Client::new()
        .get(format!("http://{gw}/v1/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    // bare route id
    assert!(ids.contains(&"chat"), "route id missing: {ids:?}");
    // provider-slug/model id built from the target's upstream model
    assert!(
        ids.contains(&"vllm-spb/qwen3"),
        "slug/model id missing: {ids:?}"
    );
    // the slug id is owned_by the provider, so a client can group by it
    let slug_entry = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "vllm-spb/qwen3")
        .unwrap();
    assert_eq!(slug_entry["owned_by"], "vLLM SPB");
}

#[tokio::test]
async fn provider_slug_model_pins_provider_and_rewrites_upstream_model() {
    // a mock that echoes back the `model` field it received, so the test can
    // assert the right segment of `slug/model` became the upstream model
    async fn echo_model(Json(body): Json<Value>) -> impl IntoResponse {
        let model = body.get("model").and_then(Value::as_str).unwrap_or("");
        Json(json!({
            "id": "chatcmpl-mock",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": model},
                "finish_reason": "stop"
            }]
        }))
    }

    let upstream = serve(Router::new().route("/v1/chat/completions", post(echo_model))).await;
    // a provider with an explicit slug, and no configured route: the request is
    // resolved purely through `provider-slug/model` addressing (ADR-0017)
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "vLLM SPB".to_string(),
        slug: Some("vllm-spb".to_string()),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{upstream}"),
        ..Default::default()
    });
    let gw = serve_gateway(&config).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(
            &json!({"model": "vllm-spb/qwen3", "messages": [{"role": "user", "content": "ping"}]}),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    // the pinned provider received the rewritten upstream model, not the address
    assert_eq!(body["choices"][0]["message"]["content"], "qwen3");

    // an unknown slug is a normal model-not-found, not a pinned forward
    let missing = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "nope/qwen3", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn system_only_profile_normalizes_developer_and_rejects_mid_conversation_roles() {
    async fn upstream(Json(body): Json<Value>) -> impl IntoResponse {
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "follow policy");
        Json(
            json!({"id":"chat_1","choices":[{"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}]}),
        )
    }

    let upstream = serve(Router::new().route("/v1/chat/completions", post(upstream))).await;
    let gw = serve_gateway(&config_for("test-model", vec![("up", upstream)])).await;
    let client = reqwest::Client::new();
    let normalized = client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model":"test-model","messages":[{"role":"developer","content":"follow policy"},{"role":"user","content":"hello"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(normalized.status(), 200);

    let rejected = client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model":"test-model","messages":[{"role":"user","content":"hello"},{"role":"developer","content":"override"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 400);
    let error: Value = rejected.json().await.unwrap();
    assert_eq!(error["error"]["code"], "role_capability_unsupported");
}

/// #882: a content part the Gemini dialects cannot carry must stop at the
/// gateway. The upstream here fails the test if it is ever called, because the
/// bug being fixed was precisely that a shortened body reached the provider and
/// came back `200`.
#[tokio::test]
async fn unsupported_content_parts_never_reach_a_gemini_upstream() {
    async fn upstream(Json(body): Json<Value>) -> Json<Value> {
        panic!("gateway forwarded a request with a dropped content part: {body}");
    }

    let upstream = serve(
        Router::new()
            .route("/interactions", post(upstream))
            .route("/v1beta/models/{model}", post(upstream))
            .fallback(post(upstream)),
    )
    .await;
    for kind in [ProviderKind::GeminiInteractions, ProviderKind::GeminiNative] {
        let mut config = config_for("test-model", vec![("up", upstream)]);
        config.providers[0].kind = kind;
        config.providers[0].api_key = Some("test-key".to_string());
        let gw = serve_gateway(&config).await;
        let rejected = reqwest::Client::new()
            .post(format!("http://{gw}/v1/chat/completions"))
            .json(
                &json!({"model":"test-model","messages":[{"role":"user","content":[
                    {"type":"text","text":"what is in this recording?"},
                    {"type":"input_audio","input_audio":{"data":"AAAA","format":"wav"}}
                ]}]}),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), 400, "{kind:?} did not fail closed");
        let error: Value = rejected.json().await.unwrap();
        assert_eq!(error["error"]["code"], "unsupported_content_part");
        let message = error["error"]["message"].as_str().unwrap();
        assert!(message.contains("input_audio"), "{message}");
    }
}

#[tokio::test]
async fn openai_profile_override_preserves_developer() {
    async fn upstream(Json(body): Json<Value>) -> impl IntoResponse {
        assert_eq!(body["messages"][0]["role"], "developer");
        Json(
            json!({"id":"chat_1","choices":[{"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}]}),
        )
    }

    let upstream = serve(Router::new().route("/v1/chat/completions", post(upstream))).await;
    let mut config = config_for("test-model", vec![("up", upstream)]);
    config.providers[0].role_profile = Some(RoleProfile::Openai);
    let gw = serve_gateway(&config).await;
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model":"test-model","messages":[{"role":"developer","content":"follow policy"},{"role":"user","content":"hello"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn responses_passthrough_preserves_body_and_sse_events() {
    async fn responses_upstream(Json(body): Json<Value>) -> axum::response::Response {
        assert_eq!(body["tools"][0]["type"], "web_search_preview");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_image");
        assert_eq!(body["reasoning"]["effort"], "high");
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"pong\"}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_stream\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        )
            .into_response()
    }

    async fn retrieve(Path(id): Path<String>) -> impl IntoResponse {
        Json(json!({"id":id,"object":"response"}))
    }
    let upstream = serve(
        Router::new()
            .route("/v1/responses", post(responses_upstream))
            .route("/v1/responses/{id}", get(retrieve)),
    )
    .await;
    let mut config = config_for("test-model", vec![("up", upstream)]);
    config.providers[0].kind = ProviderKind::Openai;
    let gw = serve_gateway(&config).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/responses"))
        .json(&json!({
            "model": "test-model",
            "stream": true,
            "input": [{"role": "user", "content": [{"type": "input_image", "image_url": "https://example.com/a.png"}]}],
            "tools": [{"type": "web_search_preview"}],
            "reasoning": {"effort": "high"},
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert!(resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .contains("text/event-stream"));
    let body = resp.text().await.unwrap();
    assert!(body.contains("event: response.output_text.delta"));
    assert!(body.contains("event: response.completed"));
    let lifecycle = reqwest::Client::new()
        .get(format!("http://{gw}/v1/responses/resp_stream"))
        .send()
        .await
        .unwrap();
    assert_eq!(lifecycle.status(), 200);
}

#[tokio::test]
async fn responses_translates_to_chat_completions() {
    async fn chat_upstream(Json(body): Json<Value>) -> impl IntoResponse {
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"][0]["text"], "inspect");
        assert_eq!(
            body["messages"][1]["content"][1]["image_url"]["url"],
            "https://example.com/diagram.png"
        );
        assert_eq!(
            body["messages"][1]["content"][1]["image_url"]["detail"],
            "high"
        );
        assert_eq!(
            body["messages"][1]["content"][2]["input_file"]["filename"],
            "brief.pdf"
        );
        assert_eq!(body["messages"][2]["role"], "tool");
        assert_eq!(body["messages"][2]["tool_call_id"], "call_1");
        assert_eq!(body["messages"][2]["content"], "lookup result");
        assert_eq!(body["tools"][0]["function"]["name"], "lookup");
        Json(json!({
            "id":"chat_1", "object":"chat.completion", "model":"upstream-model",
            "choices":[{"message":{"role":"assistant","content":"pong","tool_calls":[{"id":"call_2","type":"function","function":{"name":"lookup","arguments":"{\"q\":\"x\"}"}}]},"finish_reason":"tool_calls"}],
            "usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}
        }))
    }

    let upstream = serve(Router::new().route("/v1/chat/completions", post(chat_upstream))).await;
    let gw = serve_gateway(&config_for("test-model", vec![("up", upstream)])).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/responses"))
        .json(&json!({
            "model":"test-model", "instructions":"be concise",
            "input":[
                {"role":"user","content":[
                    {"type":"input_text","text":"inspect"},
                    {"type":"input_image","image_url":"https://example.com/diagram.png","detail":"high"},
                    {"type":"input_file","filename":"brief.pdf","file_data":"data:application/pdf;base64,JVBERi0xLjQ="}
                ]},
                {"type":"function_call_output","call_id":"call_1","output":"lookup result"}
            ],
            "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["output"][0]["content"][0]["text"], "pong");
    assert_eq!(body["output"][0]["content"][1]["type"], "function_call");
    assert_eq!(body["output"][0]["content"][1]["name"], "lookup");
    assert_eq!(body["usage"]["input_tokens"], 2);

    let lifecycle = reqwest::Client::new()
        .get(format!("http://{gw}/v1/responses/chat_1"))
        .send()
        .await
        .unwrap();
    assert_eq!(lifecycle.status(), 501);
    assert_eq!(
        lifecycle.json::<Value>().await.unwrap()["error"]["code"],
        "response_lifecycle_unsupported"
    );
}

#[tokio::test]
async fn responses_translates_to_anthropic_messages() {
    async fn messages_upstream(Json(body): Json<Value>) -> impl IntoResponse {
        assert_eq!(body["system"][0]["text"], "be concise");
        assert_eq!(body["messages"][0]["content"][0]["text"], "inspect");
        assert_eq!(body["messages"][0]["content"][1]["type"], "image");
        assert_eq!(
            body["messages"][0]["content"][1]["source"]["url"],
            "https://example.com/diagram.png"
        );
        assert_eq!(body["messages"][0]["content"][2]["type"], "document");
        assert_eq!(body["messages"][0]["content"][2]["title"], "brief.pdf");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][1]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(body["tools"][0]["name"], "lookup");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        Json(json!({
            "id":"msg_1", "type":"message", "model":"claude", "role":"assistant",
            "content":[{"type":"text","text":"pong"},{"type":"tool_use","id":"call_2","name":"lookup","input":{"q":"x"}}], "stop_reason":"tool_use",
            "usage":{"input_tokens":2,"output_tokens":1}
        }))
    }

    let upstream = serve(Router::new().route("/v1/messages", post(messages_upstream))).await;
    let mut config = config_for("test-model", vec![("up", upstream)]);
    config.providers[0].kind = ProviderKind::Anthropic;
    let gw = serve_gateway(&config).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/responses"))
        .json(&json!({
            "model":"test-model", "instructions":"be concise",
            "input":[
                {"role":"user","content":[
                    {"type":"input_text","text":"inspect"},
                    {"type":"input_image","image_url":"https://example.com/diagram.png","detail":"high"},
                    {"type":"input_file","filename":"brief.pdf","file_data":"data:application/pdf;base64,JVBERi0xLjQ="}
                ]},
                {"type":"function_call_output","call_id":"call_1","output":"lookup result"}
            ],
            "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["output"][0]["content"][0]["text"], "pong");
    assert_eq!(body["output"][0]["content"][1]["type"], "function_call");
    assert_eq!(body["output"][0]["content"][1]["name"], "lookup");
    assert_eq!(body["usage"]["output_tokens"], 1);
}

#[tokio::test]
async fn responses_lifecycle_operations_are_uniformly_unsupported() {
    let gw = serve_gateway(&GatewayConfig::default()).await;
    let client = reqwest::Client::new();
    for request in [
        client.get(format!("http://{gw}/v1/responses/resp_a")),
        client.delete(format!("http://{gw}/v1/responses/resp_other_tenant")),
        client.post(format!("http://{gw}/v1/responses/resp_a/cancel")),
        client.get(format!("http://{gw}/v1/responses/resp_a/input_items")),
    ] {
        let resp = request.send().await.unwrap();
        assert_eq!(resp.status(), 404);
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], "response_not_found");
    }
    for request in [
        client.post(format!("http://{gw}/v1/responses/resp_a/compact")),
        client.get(format!("http://{gw}/v1/responses/resp_a/input_tokens")),
    ] {
        let resp = request.send().await.unwrap();
        assert_eq!(resp.status(), 501);
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], "response_lifecycle_unsupported");
    }
}

#[tokio::test]
async fn native_responses_lifecycle_is_tenant_scoped_and_pinned() {
    async fn create(State(provider): State<String>) -> impl IntoResponse {
        Json(json!({
            "id":format!("resp_{provider}"),
            "object":"response",
            "status":"completed"
        }))
    }
    async fn retrieve(State(provider): State<String>, Path(id): Path<String>) -> impl IntoResponse {
        assert_eq!(id, format!("resp_{provider}"));
        Json(json!({"id":id,"object":"response","provider":provider}))
    }
    async fn cancel(State(provider): State<String>, Path(id): Path<String>) -> impl IntoResponse {
        assert_eq!(id, format!("resp_{provider}"));
        Json(json!({"id":id,"object":"response","status":"cancelled","provider":provider}))
    }
    async fn input_items(
        State(provider): State<String>,
        Path(id): Path<String>,
        OriginalUri(uri): OriginalUri,
    ) -> impl IntoResponse {
        assert_eq!(id, format!("resp_{provider}"));
        assert_eq!(uri.query(), Some("limit=1"));
        Json(json!({"object":"list","response_id":id,"provider":provider,"data":[{"id":"item_1"}]}))
    }
    async fn delete_response(
        State(provider): State<String>,
        Path(id): Path<String>,
    ) -> impl IntoResponse {
        assert_eq!(id, format!("resp_{provider}"));
        Json(json!({"id":id,"object":"response.deleted","deleted":true,"provider":provider}))
    }

    async fn lifecycle_upstream(provider: &str) -> SocketAddr {
        serve(
            Router::new()
                .route("/v1/responses", post(create))
                .route("/v1/responses/{id}", get(retrieve).delete(delete_response))
                .route("/v1/responses/{id}/cancel", post(cancel))
                .route("/v1/responses/{id}/input_items", get(input_items))
                .with_state(provider.to_string()),
        )
        .await
    }

    let upstream_a = lifecycle_upstream("a").await;
    let upstream_b = lifecycle_upstream("b").await;
    let mut config = config_for(
        "native-model",
        vec![("native-a", upstream_a), ("native-b", upstream_b)],
    );
    for provider in &mut config.providers {
        provider.kind = ProviderKind::Openai;
    }
    config.virtual_keys = vec![
        VirtualKeyConfig {
            key: "sk-tenant-a".to_string(),
            name: None,
            models: vec![],
            providers: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
        },
        VirtualKeyConfig {
            key: "sk-tenant-b".to_string(),
            name: None,
            models: vec![],
            providers: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
        },
    ];
    let gw = serve_gateway(&config).await;
    let client = reqwest::Client::new();

    let mut response_ids = Vec::new();
    for _ in 0..2 {
        let created: Value = client
            .post(format!("http://{gw}/v1/responses"))
            .bearer_auth("sk-tenant-a")
            .json(&json!({"model":"native-model","input":"hello"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        response_ids.push(created["id"].as_str().unwrap().to_string());
    }
    response_ids.sort();
    assert_eq!(response_ids, ["resp_a", "resp_b"]);

    for response_id in &response_ids {
        let retrieved: Value = client
            .get(format!("http://{gw}/v1/responses/{response_id}"))
            .bearer_auth("sk-tenant-a")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            retrieved["provider"],
            response_id.trim_start_matches("resp_")
        );
    }

    let response_id = &response_ids[0];

    let cross_tenant = client
        .get(format!("http://{gw}/v1/responses/{response_id}"))
        .bearer_auth("sk-tenant-b")
        .send()
        .await
        .unwrap();
    assert_eq!(cross_tenant.status(), 404);
    let unknown = client
        .get(format!("http://{gw}/v1/responses/unknown"))
        .bearer_auth("sk-tenant-a")
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);

    let cancelled: Value = client
        .post(format!("http://{gw}/v1/responses/{response_id}/cancel"))
        .bearer_auth("sk-tenant-a")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(cancelled["provider"], "a");
    let items: Value = client
        .get(format!(
            "http://{gw}/v1/responses/{response_id}/input_items?limit=1"
        ))
        .bearer_auth("sk-tenant-a")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(items["data"][0]["id"], "item_1");
    assert_eq!(items["provider"], "a");
    let deleted = client
        .delete(format!("http://{gw}/v1/responses/{response_id}"))
        .bearer_auth("sk-tenant-a")
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 200);
    let after_delete = client
        .get(format!("http://{gw}/v1/responses/{response_id}"))
        .bearer_auth("sk-tenant-a")
        .send()
        .await
        .unwrap();
    assert_eq!(after_delete.status(), 404);
}

#[tokio::test]
async fn response_lifecycle_operations_require_auth_without_leaking_key_scope() {
    let config = GatewayConfig {
        virtual_keys: vec![
            VirtualKeyConfig {
                key: "sk-tenant-a".to_string(),
                name: None,
                models: vec![],
                providers: vec![],
                disabled: false,
                expires_at: None,
                cache: None,
            },
            VirtualKeyConfig {
                key: "sk-tenant-b".to_string(),
                name: None,
                models: vec![],
                providers: vec![],
                disabled: false,
                expires_at: None,
                cache: None,
            },
        ],
        ..Default::default()
    };
    let gw = serve_gateway(&config).await;
    let client = reqwest::Client::new();

    let unauthenticated = client
        .get(format!("http://{gw}/v1/responses/resp_a/input_tokens"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), 401);

    let tenant_a = client
        .post(format!("http://{gw}/v1/responses/resp_a/compact"))
        .bearer_auth("sk-tenant-a")
        .send()
        .await
        .unwrap();
    assert_eq!(tenant_a.status(), 501);
    let tenant_a: Value = tenant_a.json().await.unwrap();
    let tenant_b = client
        .get(format!(
            "http://{gw}/v1/responses/resp_other_tenant/input_tokens"
        ))
        .bearer_auth("sk-tenant-b")
        .send()
        .await
        .unwrap();
    assert_eq!(tenant_b.status(), 501);
    let tenant_b: Value = tenant_b.json().await.unwrap();

    assert_eq!(tenant_a, tenant_b);
    assert_eq!(tenant_a["error"]["code"], "response_lifecycle_unsupported");
}

#[tokio::test]
async fn ollama_preserves_openai_compatible_fields_and_rewrites_model() {
    async fn inspect(Json(body): Json<Value>) -> impl IntoResponse {
        assert_eq!(body["model"], "qwen2.5:0.5b");
        assert_eq!(body["seed"], 42);
        assert_eq!(body["response_format"]["type"], "json_object");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["messages"][0]["content"][0]["type"], "image_url");
        assert_eq!(body["stream_options"]["include_usage"], true);
        Json(json!({"choices": [], "usage": {"total_tokens": 0}}))
    }

    let upstream = serve(Router::new().route("/v1/chat/completions", post(inspect))).await;
    let mut config = config_for("local-qwen", vec![("ollama", upstream)]);
    config.providers[0].kind = ProviderKind::Ollama;
    config.routes[0].targets[0].model = Some("qwen2.5:0.5b".to_string());
    let gw = serve_gateway(&config).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "local-qwen",
            "messages": [{"role": "user", "content": [{"type": "image_url", "image_url": {"url": "data:image/png;base64,AA=="}}]}],
            "seed": 42,
            "response_format": {"type": "json_object"},
            "tools": [{"type": "function", "function": {"name": "ping", "parameters": {"type": "object"}}}],
            "stream_options": {"include_usage": true}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["usage"]["total_tokens"],
        0
    );
}

#[tokio::test]
async fn openai_multimodal_content_is_forwarded_byte_for_byte() {
    let payload = serde_json::to_vec(&json!({
        "model": "multimodal-model",
        "modalities": ["text", "audio"],
        "audio": {"voice": "alloy", "format": "wav"},
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": "describe these inputs"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}},
                {"type": "input_audio", "input_audio": {"data": "UklGRiQAAABXQVZF", "format": "wav"}},
                {"type": "input_file", "input_file": {"filename": "notes.pdf", "file_data": "data:application/pdf;base64,JVBERi0xLjQ="}}
            ]
        }]
    }))
    .unwrap();
    let expected = payload.clone();
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post(move |body: bytes::Bytes| {
            let expected = expected.clone();
            async move {
                assert_eq!(body, expected, "multimodal payload must not be re-encoded");
                Json(json!({
                    "choices": [{"message": {"role": "assistant", "content": "done", "audio": {"id": "audio_1", "data": "UklGRg==", "expires_at": 0, "transcript": "done"}}}]
                }))
            }
        }),
    ))
    .await;
    let gw = serve_gateway(&config_for("multimodal-model", vec![("up", upstream)])).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(payload)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["choices"][0]["message"]["audio"]["data"],
        "UklGRg=="
    );
}

#[tokio::test]
async fn anthropic_multimodal_content_is_forwarded_byte_for_byte() {
    let payload = serde_json::to_vec(&json!({
        "model": "claude-multimodal",
        "max_tokens": 64,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}},
                {"type": "image", "source": {"type": "url", "url": "https://example.test/image.png"}},
                {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBERi0xLjQ="}},
                {"type": "document", "source": {"type": "url", "url": "https://example.test/report.pdf"}}
            ]
        }]
    }))
    .unwrap();
    let expected = payload.clone();
    let upstream = serve(Router::new().route(
        "/v1/messages",
        post(move |headers: axum::http::HeaderMap, body: bytes::Bytes| {
            let expected = expected.clone();
            async move {
                assert_eq!(body, expected, "multimodal payload must not be re-encoded");
                assert_eq!(headers["anthropic-version"], "2023-06-01");
                Json(json!({
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "text", "text": "done"}],
                    "stop_reason": "end_turn"
                }))
            }
        }),
    ))
    .await;
    let mut config = config_for("claude-multimodal", vec![("anthropic", upstream)]);
    config.providers[0].kind = ProviderKind::Anthropic;
    let gw = serve_gateway(&config).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/messages"))
        .header("content-type", "application/json")
        .body(payload)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.unwrap()["type"], "message");
}

#[tokio::test]
async fn openai_client_translates_multimodal_tools_through_anthropic() {
    async fn anthropic_upstream(Json(body): Json<Value>) -> impl IntoResponse {
        assert_eq!(body["model"], "claude-native");
        assert_eq!(body["system"][0]["text"], "be precise");
        assert_eq!(body["messages"][0]["content"][1]["type"], "image");
        assert_eq!(
            body["messages"][0]["content"][1]["source"]["media_type"],
            "image/png"
        );
        assert_eq!(body["messages"][0]["content"][2]["type"], "document");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        Json(json!({
            "id":"msg_native","type":"message","role":"assistant","model":"claude-native",
            "content":[{"type":"text","text":"done"},{"type":"tool_use","id":"tool_1","name":"lookup","input":{"q":"x"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":8,"output_tokens":3}
        }))
    }

    let upstream = serve(Router::new().route("/v1/messages", post(anthropic_upstream))).await;
    let mut config = config_for("public-model", vec![("anthropic", upstream)]);
    config.providers[0].kind = ProviderKind::Anthropic;
    config.routes[0].targets[0].model = Some("claude-native".to_string());
    let gw = serve_gateway(&config).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model":"public-model","max_tokens":64,
            "messages":[
                {"role":"system","content":"be precise"},
                {"role":"user","content":[
                    {"type":"text","text":"inspect"},
                    {"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}},
                    {"type":"input_file","input_file":{"filename":"report.pdf","file_data":"data:application/pdf;base64,BB=="}}
                ]}
            ],
            "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}]
        }))
        .send().await.unwrap();

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "done");
    assert_eq!(
        body["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "lookup"
    );
    assert_eq!(body["usage"]["total_tokens"], 11);
}

#[tokio::test]
async fn anthropic_client_translates_multimodal_tools_through_openai() {
    async fn openai_upstream(Json(body): Json<Value>) -> impl IntoResponse {
        assert_eq!(body["model"], "gpt-native");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"][0]["type"], "image_url");
        assert_eq!(body["messages"][1]["content"][1]["type"], "input_file");
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
        Json(json!({
            "id":"chatcmpl-native","object":"chat.completion","model":"gpt-native",
            "choices":[{"index":0,"message":{"role":"assistant","content":"done","tool_calls":[{"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"q\":\"x\"}"}}]},"finish_reason":"tool_calls"}],
            "usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}
        }))
    }

    let upstream = serve(Router::new().route("/v1/chat/completions", post(openai_upstream))).await;
    let mut config = config_for("public-model", vec![("openai", upstream)]);
    config.routes[0].targets[0].model = Some("gpt-native".to_string());
    let gw = serve_gateway(&config).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/messages"))
        .json(&json!({
            "model":"public-model","max_tokens":64,"system":"be precise",
            "messages":[{"role":"user","content":[
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}},
                {"type":"document","source":{"type":"url","url":"https://example.test/report.pdf"},"title":"report"}
            ]}],
            "tools":[{"name":"lookup","input_schema":{"type":"object"}}]
        }))
        .send().await.unwrap();

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["text"], "done");
    assert_eq!(body["content"][1]["type"], "tool_use");
    assert_eq!(body["stop_reason"], "tool_use");
    assert_eq!(body["usage"]["input_tokens"], 5);
}

#[tokio::test]
async fn cross_protocol_sse_is_translated_incrementally_both_ways() {
    async fn anthropic_stream() -> impl IntoResponse {
        let sse = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude\",\"usage\":{\"input_tokens\":2}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"pong\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        );
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            sse,
        )
    }
    let anthropic = serve(Router::new().route("/v1/messages", post(anthropic_stream))).await;
    let mut config = config_for("to-anthropic", vec![("anthropic", anthropic)]);
    config.providers[0].kind = ProviderKind::Anthropic;
    let gw = serve_gateway(&config).await;
    let openai_text = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model":"to-anthropic","stream":true,"messages":[]}))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(openai_text.contains("chat.completion.chunk"));
    assert!(openai_text.contains("\"content\":\"pong\""));
    assert!(openai_text.ends_with("data: [DONE]\n\n"));

    async fn openai_stream() -> impl IntoResponse {
        let sse = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt\",\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt\",\"choices\":[{\"delta\":{\"content\":\"pong\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n"
        );
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            sse,
        )
    }
    let openai = serve(Router::new().route("/v1/chat/completions", post(openai_stream))).await;
    let config = config_for("to-openai", vec![("openai", openai)]);
    let gw = serve_gateway(&config).await;
    let anthropic_text = reqwest::Client::new()
        .post(format!("http://{gw}/v1/messages"))
        .json(&json!({"model":"to-openai","max_tokens":16,"stream":true,"messages":[]}))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(anthropic_text.matches("event: message_start").count(), 1);
    assert!(anthropic_text.contains("event: content_block_delta"));
    assert!(anthropic_text.contains("\"text\":\"pong\""));
    assert!(anthropic_text.contains("\"input_tokens\":2"));
    assert!(anthropic_text.contains("\"output_tokens\":1"));
    assert!(anthropic_text.ends_with("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
}

#[tokio::test]
async fn response_carries_routing_decision_headers() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_openai))).await;
    let gw = serve_gateway(&config_for("test-model", vec![("up", upstream)])).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    // the client can see which target served the request without ClickHouse
    assert_eq!(header("x-rolter-provider"), "up");
    assert_eq!(header("x-rolter-model"), "test-model");
    // no per-target upstream model override, so the resolved target model is the
    // requested model
    assert_eq!(header("x-rolter-target"), "test-model");
    // no cache yet (ROL-56), so every response is a miss for now
    assert_eq!(header("x-rolter-cache"), "MISS");
    // no A/B variant on the classic single-pool path: header omitted, not blank
    assert!(!resp.headers().contains_key("x-rolter-variant"));
}

#[tokio::test]
async fn complexity_policy_selects_routes_for_openai_and_anthropic_requests() {
    async fn openai_response() -> Json<Value> {
        Json(json!({"choices":[{"message":{"role":"assistant","content":"pong"}}]}))
    }
    async fn anthropic_response() -> Json<Value> {
        Json(json!({"id":"msg_1","type":"message","content":[{"type":"text","text":"pong"}]}))
    }

    let fast = serve(
        Router::new()
            .route("/v1/chat/completions", post(openai_response))
            .route("/v1/messages", post(anthropic_response)),
    )
    .await;
    let capable = serve(
        Router::new()
            .route("/v1/chat/completions", post(openai_response))
            .route("/v1/messages", post(anthropic_response)),
    )
    .await;
    let mut config = config_for("router", vec![("fast", fast), ("capable", capable)]);
    config.routes[0].params.insert(
        "_rolter_complexity".to_string(),
        json!({"tiers": [
            {"name": "simple", "max_input_bytes": 512, "route": "fast-route"},
            {"name": "complex", "route": "capable-route"}
        ]}),
    );
    for (model, provider) in [("fast-route", "fast"), ("capable-route", "capable")] {
        config.routes.push(ModelRoute {
            model: model.to_string(),
            strategy: BalancingStrategy::RoundRobin,
            targets: vec![Target {
                provider: provider.to_string(),
                model: None,
                weight: 1,
            }],
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
        });
    }
    let gw = serve_gateway(&config).await;
    let client = reqwest::Client::new();

    let short = client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "router", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(short.status(), 200);
    assert_eq!(short.headers()["x-rolter-provider"], "fast");

    let long = client
        .post(format!("http://{gw}/v1/messages"))
        .json(&json!({
            "model": "router",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "x".repeat(1024)}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(long.status(), 200);
    assert_eq!(long.headers()["x-rolter-provider"], "capable");
}

#[tokio::test]
async fn streaming_request_passes_through_sse() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_openai))).await;
    let gw = serve_gateway(&config_for("test-model", vec![("up", upstream)])).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "stream": true, "messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.contains("event-stream"), "expected SSE, got {ct}");
    let text = resp.text().await.unwrap();
    assert!(text.contains("data:"), "missing SSE data frames");
    assert!(text.contains("[DONE]"), "missing SSE terminator");
    assert!(text.contains("pong"), "missing streamed content");
}

#[tokio::test]
async fn streaming_response_is_forwarded_incrementally() {
    async fn delayed_sse(
        State(release): State<Arc<tokio::sync::Notify>>,
    ) -> axum::response::Response {
        let chunks = futures_util::stream::unfold((0, release), |(step, release)| async move {
            match step {
                0 => Some((
                    Ok::<_, std::convert::Infallible>(bytes::Bytes::from_static(
                        b"data: {\"choices\":[{\"delta\":{\"content\":\"first\"}}]}\n\n",
                    )),
                    (1, release),
                )),
                1 => {
                    release.notified().await;
                    Some((
                        Ok(bytes::Bytes::from_static(
                            b"data: {\"choices\":[{\"delta\":{\"content\":\"second\"}}]}\n\ndata: [DONE]\n\n",
                        )),
                        (2, release),
                    ))
                }
                _ => None,
            }
        });
        axum::response::Response::builder()
            .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap()
    }

    let release = Arc::new(tokio::sync::Notify::new());
    let upstream = serve(
        Router::new()
            .route("/v1/chat/completions", post(delayed_sse))
            .with_state(release.clone()),
    )
    .await;
    let gw = serve_gateway(&config_for("test-model", vec![("up", upstream)])).await;

    let mut response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "stream": true, "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let first = tokio::time::timeout(std::time::Duration::from_secs(1), response.chunk())
        .await
        .expect("gateway buffered the first SSE event")
        .unwrap()
        .expect("upstream ended before its first SSE event");
    let first = String::from_utf8(first.to_vec()).unwrap();
    assert!(first.contains("first"), "unexpected first chunk: {first}");
    assert!(
        !first.contains("second"),
        "gateway buffered SSE events: {first}"
    );

    release.notify_one();
    let rest = response.text().await.unwrap();
    assert!(rest.contains("second"), "missing second SSE event: {rest}");
    assert!(rest.contains("[DONE]"), "missing SSE terminator: {rest}");
}

#[tokio::test]
async fn retry_policy_distinguishes_client_and_transient_upstream_errors() {
    async fn failing(
        State((status, hits)): State<(axum::http::StatusCode, Arc<AtomicU32>)>,
    ) -> axum::response::Response {
        hits.fetch_add(1, Ordering::SeqCst);
        (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            format!(r#"{{"error":{{"message":"upstream {status}"}}}}"#),
        )
            .into_response()
    }

    async fn healthy(State(hits): State<Arc<AtomicU32>>) -> axum::response::Response {
        hits.fetch_add(1, Ordering::SeqCst);
        Json(json!({
            "id": "chatcmpl-fallback",
            "object": "chat.completion",
            "choices": [{"message": {"role": "assistant", "content": "fallback"}}]
        }))
        .into_response()
    }

    for (status, should_retry) in [
        (axum::http::StatusCode::BAD_REQUEST, false),
        (axum::http::StatusCode::NOT_FOUND, false),
        (axum::http::StatusCode::UNPROCESSABLE_ENTITY, false),
        (axum::http::StatusCode::REQUEST_TIMEOUT, true),
        (axum::http::StatusCode::TOO_MANY_REQUESTS, true),
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, true),
    ] {
        let failing_hits = Arc::new(AtomicU32::new(0));
        let healthy_hits = Arc::new(AtomicU32::new(0));
        let down = serve(
            Router::new()
                .route("/v1/chat/completions", post(failing))
                .with_state((status, failing_hits.clone())),
        )
        .await;
        let up = serve(
            Router::new()
                .route("/v1/chat/completions", post(healthy))
                .with_state(healthy_hits.clone()),
        )
        .await;
        let mut config = config_for("test-model", vec![("down", down), ("up", up)]);
        config.retry.base_backoff_ms = 0;
        config.retry.max_backoff_ms = 0;
        let gw = serve_gateway(&config).await;

        let response = reqwest::Client::new()
            .post(format!("http://{gw}/v1/chat/completions"))
            .json(&json!({"model": "test-model", "messages": []}))
            .send()
            .await
            .unwrap();

        assert_eq!(failing_hits.load(Ordering::SeqCst), 1, "status {status}");
        if should_retry {
            assert_eq!(response.status(), 200, "status {status} was not retried");
            assert_eq!(healthy_hits.load(Ordering::SeqCst), 1, "status {status}");
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["choices"][0]["message"]["content"], "fallback");
        } else {
            assert_eq!(response.status(), status, "status {status} was retried");
            assert_eq!(healthy_hits.load(Ordering::SeqCst), 0, "status {status}");
            let body: Value = response.json().await.unwrap();
            assert_eq!(
                body["error"]["message"],
                format!("upstream {status}"),
                "status {status} error body changed"
            );
        }
    }
}

#[tokio::test]
async fn realtime_websocket_rewrites_model_and_relays_frames_with_upstream_auth() {
    let (upstream_addr, mut captured_rx) = serve_realtime_echo().await;

    let mut config = config_for("realtime-alias", vec![("up", upstream_addr)]);
    config.providers[0].api_key = Some("provider-secret".to_string());
    config.routes[0].targets[0].model = Some("gpt-realtime-upstream".to_string());
    let gw = serve_gateway(&config).await;

    let request = realtime_client_request(gw, "realtime-alias", None, Some("realtime=v1"));
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();

    let event = r#"{"type":"session.update","session":{"modalities":["text"]}}"#;
    socket
        .send(WebSocketMessage::Text(event.into()))
        .await
        .unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap().into_text().unwrap(),
        event
    );
    let audio = bytes::Bytes::from_static(&[0, 1, 2, 3]);
    socket
        .send(WebSocketMessage::Binary(audio.clone()))
        .await
        .unwrap();
    assert_eq!(socket.next().await.unwrap().unwrap().into_data(), audio);
    socket.close(None).await.unwrap();

    assert_eq!(
        captured_rx.recv().await.unwrap(),
        CapturedRealtimeRequest {
            uri: "/v1/realtime?model=gpt-realtime-upstream".to_string(),
            authorization: Some("Bearer provider-secret".to_string()),
            beta: Some("realtime=v1".to_string()),
        }
    );
}

#[tokio::test]
async fn realtime_websocket_rejects_missing_auth_disallowed_and_unknown_models() {
    let mut config = config_for("realtime-alias", vec![]);
    config.virtual_keys = vec![
        VirtualKeyConfig {
            key: "sk-restricted".to_string(),
            name: None,
            models: vec!["other-model".to_string()],
            providers: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
        },
        VirtualKeyConfig {
            key: "sk-all".to_string(),
            name: None,
            models: vec![],
            providers: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
        },
    ];
    let gw = serve_gateway(&config).await;

    let missing_auth =
        tokio_tungstenite::connect_async(realtime_client_request(gw, "realtime-alias", None, None))
            .await
            .unwrap_err();
    assert_eq!(websocket_handshake_status(missing_auth), 401);

    let disallowed = tokio_tungstenite::connect_async(realtime_client_request(
        gw,
        "realtime-alias",
        Some("Bearer sk-restricted"),
        None,
    ))
    .await
    .unwrap_err();
    assert_eq!(websocket_handshake_status(disallowed), 403);

    let unknown = tokio_tungstenite::connect_async(realtime_client_request(
        gw,
        "missing-model",
        Some("Bearer sk-all"),
        None,
    ))
    .await
    .unwrap_err();
    assert_eq!(websocket_handshake_status(unknown), 404);
}

#[tokio::test]
async fn realtime_websocket_fails_over_during_connection_establishment() {
    let unavailable = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unavailable_addr = unavailable.local_addr().unwrap();
    drop(unavailable);
    let (upstream_addr, mut captured_rx) = serve_realtime_echo().await;

    let config = config_for(
        "realtime-alias",
        vec![
            ("unavailable", unavailable_addr),
            ("healthy", upstream_addr),
        ],
    );
    let gw = serve_gateway(&config).await;
    let (mut socket, _) =
        tokio_tungstenite::connect_async(realtime_client_request(gw, "realtime-alias", None, None))
            .await
            .unwrap();

    socket
        .send(WebSocketMessage::Text("ping".into()))
        .await
        .unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap().into_text().unwrap(),
        "ping"
    );
    socket.close(None).await.unwrap();
    assert_eq!(
        captured_rx.recv().await.unwrap().uri,
        "/v1/realtime?model=realtime-alias"
    );
}

#[tokio::test]
async fn realtime_websocket_enforces_concurrent_session_limit() {
    let (upstream_addr, mut captured_rx) = serve_realtime_echo().await;
    let mut config = config_for("realtime-alias", vec![("up", upstream_addr)]);
    config.realtime.max_connections = 1;
    let gw = serve_gateway(&config).await;

    let (mut first, _) =
        tokio_tungstenite::connect_async(realtime_client_request(gw, "realtime-alias", None, None))
            .await
            .unwrap();
    captured_rx
        .recv()
        .await
        .expect("first session never reached upstream");

    let second =
        tokio_tungstenite::connect_async(realtime_client_request(gw, "realtime-alias", None, None))
            .await
            .unwrap_err();
    assert_eq!(websocket_handshake_status(second), 429);
    assert!(captured_rx.try_recv().is_err());

    first.close(None).await.unwrap();
}

#[tokio::test]
async fn oversized_body_is_rejected_with_json_413() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_openai))).await;
    let mut config = config_for("test-model", vec![("up", upstream)]);
    // tiny limit so a normal-looking request trips it without shipping megabytes
    config.server.max_body_bytes = 256;
    let gw = serve_gateway(&config).await;
    let client = reqwest::Client::new();

    // a body over the limit: axum's DefaultBodyLimit rejects it before routing,
    // and our mapper rewrites the plain-text 413 into openai-style json
    let big = "x".repeat(4096);
    let resp = client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": big}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 413);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "request_too_large");

    // a request under the limit still flows through to the upstream
    let ok = client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
}

#[tokio::test]
async fn missing_model_field_is_rejected() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_openai))).await;
    let gw = serve_gateway(&config_for("test-model", vec![("up", upstream)])).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "missing_required_parameter");
}

#[tokio::test]
async fn metrics_served_on_configured_path() {
    let mut config = config_for("test-model", vec![]);
    config.server.metrics_path = "/internal/metrics".to_string();
    let gw = serve_gateway(&config).await;
    let client = reqwest::Client::new();

    // the configured path serves prometheus text
    let resp = client
        .get(format!("http://{gw}/internal/metrics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("rolter_requests_total"));

    // the default /metrics no longer exists
    let resp = client
        .get(format!("http://{gw}/metrics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn adaptive_routing_decisions_are_exported() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_openai))).await;
    let mut config = config_for("test-model", vec![("up", upstream)]);
    config.routes[0].strategy = rolter_core::BalancingStrategy::Adaptive;
    // the policy stays off: the point is that a disabled kill switch is still
    // observable, so an operator can see the fallback carrying every pick
    let gw = serve_gateway(&config).await;
    let client = reqwest::Client::new();

    client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();

    let body = client
        .get(format!("http://{gw}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains(
            "rolter_adaptive_routing_decisions_total{model=\"test-model\",mode=\"fallback\"} 1"
        ),
        "fallback pick not exported: {body}"
    );
    assert!(body.contains("rolter_adaptive_routing_engaged{model=\"test-model\"} 0"));
}

#[tokio::test]
async fn builtin_fake_llm_serves_embeddings() {
    // no routes configured: the built-in fake-llm answers /v1/embeddings locally
    let gw = serve_gateway(&GatewayConfig::default()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{gw}/v1/embeddings"))
        .json(&json!({"model": "fake-llm", "input": ["hello", "world"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    assert_eq!(body["data"][0]["object"], "embedding");
    assert!(!body["data"][0]["embedding"].as_array().unwrap().is_empty());
    assert!(body["usage"]["prompt_tokens"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn builtin_fake_llm_serves_rerank() {
    // no routes configured: the built-in fake-llm answers /v1/rerank locally
    let gw = serve_gateway(&GatewayConfig::default()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{gw}/v1/rerank"))
        .json(&json!({
            "model": "fake-llm",
            "query": "capital of france",
            "documents": ["paris", "berlin", "rome"],
            "top_n": 2,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(
        results[0]["relevance_score"].as_f64().unwrap()
            >= results[1]["relevance_score"].as_f64().unwrap()
    );
    assert!(body["usage"]["prompt_tokens"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn builtin_fake_llm_serves_images() {
    // no routes configured: the built-in fake-llm answers image generations locally
    let gw = serve_gateway(&GatewayConfig::default()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{gw}/v1/images/generations"))
        .json(&json!({"model": "fake-llm", "prompt": "a red circle", "n": 2}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert!(data[0]["url"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
}

#[tokio::test]
async fn builtin_fake_llm_serves_audio_speech() {
    // no routes configured: the built-in fake-llm returns a silent wav clip
    let gw = serve_gateway(&GatewayConfig::default()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{gw}/v1/audio/speech"))
        .json(&json!({"model": "fake-llm", "input": "hello world", "voice": "alloy"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.contains("audio/wav"), "expected wav, got {ct}");
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
}

#[tokio::test]
async fn serves_openapi_document() {
    let gw = serve_gateway(&GatewayConfig::default()).await;
    let client = reqwest::Client::new();

    // openapi spec is valid JSON describing the endpoints, no external assets
    let resp = client
        .get(format!("http://{gw}/openapi.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let doc: Value = resp.json().await.unwrap();
    assert_eq!(doc["openapi"], "3.1.0");
    assert!(doc["paths"]["/v1/embeddings"].is_object());
    assert!(doc["paths"]["/v1/audio/transcriptions"].is_object());
}

#[tokio::test]
async fn root_serves_service_info() {
    let gw = serve_gateway(&GatewayConfig::default()).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{gw}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["service"], "rolter-gateway");
    assert_eq!(body["docs"], "/docs");
}

#[tokio::test]
async fn serves_scalar_docs_air_gapped() {
    let gw = serve_gateway(&GatewayConfig::default()).await;
    let client = reqwest::Client::new();

    // the docs page loads its bundle from this gateway, never a cdn
    let resp = client
        .get(format!("http://{gw}/docs"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.unwrap();
    assert!(html.contains("/docs/scalar.js"));
    assert!(!html.contains("cdn.jsdelivr.net"));

    // the embedded bundle is actually served
    let resp = client
        .get(format!("http://{gw}/docs/scalar.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.contains("javascript"), "expected js, got {ct}");
    assert!(!resp.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn builtin_fake_llm_serves_audio_transcriptions() {
    // no routes configured: the built-in fake-llm answers multipart transcriptions
    let gw = serve_gateway(&GatewayConfig::default()).await;
    let boundary = "ROLTERBOUND";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nfake-llm\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\nRIFFxxxxWAVE\r\n--{b}--\r\n",
        b = boundary
    );
    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/audio/transcriptions"))
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let value: Value = resp.json().await.unwrap();
    assert!(!value["text"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn audio_translations_rejects_non_multipart() {
    let gw = serve_gateway(&GatewayConfig::default()).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/audio/translations"))
        .json(&json!({"model": "fake-llm"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_content_type");
}

#[tokio::test]
async fn variant_routing_fails_over_to_next_variant() {
    use rolter_core::Variant;
    // primary variant's target always 500 (retryable); the fallback variant is
    // healthy. the request should fall over across variants and succeed.
    let down = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
    ))
    .await;
    let up = serve(Router::new().route("/v1/chat/completions", post(mock_openai))).await;

    let mut config = GatewayConfig::default();
    for (name, addr) in [("down", down), ("up", up)] {
        config.providers.push(ProviderConfig {
            name: name.to_string(),
            kind: ProviderKind::OpenaiCompatible,
            api_base: format!("http://{addr}"),
            ..Default::default()
        });
    }
    let mk_variant = |name: &str, provider: &str, weight: u32| Variant {
        name: name.to_string(),
        weight,
        targets: vec![Target {
            provider: provider.to_string(),
            model: None,
            weight: 1,
        }],
        params: Default::default(),
    };
    config.routes.push(ModelRoute {
        model: "ab-model".to_string(),
        strategy: BalancingStrategy::RoundRobin,
        targets: Default::default(),
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        // heavily weight the failing variant as primary so failover is exercised
        variants: vec![
            mk_variant("control", "down", 100),
            mk_variant("canary", "up", 1),
        ],
    });
    let gw = serve_gateway(&config).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "ab-model", "messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "pong");
}

#[tokio::test]
async fn unknown_model_returns_404() {
    let gw = serve_gateway(&config_for("test-model", vec![])).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "does-not-exist", "messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn transient_upstream_failure_fails_over_to_healthy_target() {
    // first target always 500 (retryable), second target always 200
    let down = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
    ))
    .await;
    let up = serve(Router::new().route("/v1/chat/completions", post(mock_openai))).await;

    let gw = serve_gateway(&config_for("test-model", vec![("down", down), ("up", up)])).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": []}))
        .send()
        .await
        .unwrap();

    // the retry path should fail over from the 500 target to the healthy one
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "pong");
}

#[tokio::test]
async fn round_robin_spreads_across_healthy_targets() {
    // two healthy targets, each counting its hits; round-robin over several
    // requests should exercise both rather than pinning to one
    fn counting_upstream(hits: Arc<AtomicU32>) -> Router {
        Router::new()
            .route(
                "/v1/chat/completions",
                post(
                    |State(hits): State<Arc<AtomicU32>>, body: Json<Value>| async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        mock_openai(body).await
                    },
                ),
            )
            .with_state(hits)
    }
    let a_hits = Arc::new(AtomicU32::new(0));
    let b_hits = Arc::new(AtomicU32::new(0));
    let a = serve(counting_upstream(a_hits.clone())).await;
    let b = serve(counting_upstream(b_hits.clone())).await;

    let gw = serve_gateway(&config_for("test-model", vec![("a", a), ("b", b)])).await;
    let client = reqwest::Client::new();
    for _ in 0..4 {
        let resp = client
            .post(format!("http://{gw}/v1/chat/completions"))
            .json(&json!({"model": "test-model", "messages": []}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    // both targets served at least one request (round-robin, not pinned)
    assert!(a_hits.load(Ordering::SeqCst) > 0, "target a never hit");
    assert!(b_hits.load(Ordering::SeqCst) > 0, "target b never hit");
}

#[tokio::test]
async fn revoked_key_fails_over_to_sibling_key_in_request() {
    use axum::http::HeaderMap;
    use rolter_core::ApiKeyConfig;

    // an upstream that 401s the bad key and answers 200 for the good one
    async fn key_gate(headers: HeaderMap, body: Json<Value>) -> axum::response::Response {
        let auth = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if auth == "Bearer good-key" {
            mock_openai(body).await
        } else {
            (axum::http::StatusCode::UNAUTHORIZED, "bad key").into_response()
        }
    }
    let upstream = serve(Router::new().route("/v1/chat/completions", post(key_gate))).await;

    let mut config = config_for("test-model", vec![("up", upstream)]);
    // the bad key's weight dwarfs the good one, so the first pick is always
    // the bad key (the jitter draw never reaches the good key's sliver) and a
    // 200 can only come from the in-request sibling-key failover
    config.providers[0].api_keys = vec![
        ApiKeyConfig {
            key: Some("bad-key".to_string()),
            env: None,
            weight: 999_999,
        },
        ApiKeyConfig {
            key: Some("good-key".to_string()),
            env: None,
            weight: 1,
        },
    ];
    let gw = serve_gateway(&config).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "sibling key failover did not happen");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "pong");
}

// ── built-in fake-llm: end-to-end with zero providers/secrets ────────────────

#[tokio::test]
async fn fake_llm_chat_completions_without_any_config() {
    // an empty config still serves the built-in fake-llm model locally
    let gw = serve_gateway(&GatewayConfig::default()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "fake-llm", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["model"], "fake-llm");
    assert!(
        body["choices"][0]["message"]["content"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "fake-llm returned no content: {body}"
    );
}

#[tokio::test]
async fn fake_llm_chat_completions_streams_sse() {
    let gw = serve_gateway(&GatewayConfig::default()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "fake-llm", "stream": true, "messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.contains("event-stream"), "expected SSE, got {ct}");
    let text = resp.text().await.unwrap();
    assert!(text.contains("data:"), "missing SSE data frames");
    assert!(text.contains("[DONE]"), "missing SSE terminator");
}

#[tokio::test]
async fn fake_llm_anthropic_messages_without_any_config() {
    let gw = serve_gateway(&GatewayConfig::default()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/messages"))
        .json(&json!({
            "model": "fake-llm",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    // anthropic messages shape: top-level content array of blocks
    assert!(
        body["content"][0]["text"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "fake-llm messages returned no content: {body}"
    );
}

#[tokio::test]
async fn fake_llm_anthropic_messages_streams_sse() {
    let gw = serve_gateway(&GatewayConfig::default()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/messages"))
        .json(&json!({
            "model": "fake-llm",
            "max_tokens": 16,
            "stream": true,
            "messages": []
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.contains("event-stream"), "expected SSE, got {ct}");
    let text = resp.text().await.unwrap();
    assert!(text.contains("data:"), "missing SSE data frames");
}

// ── outbound trace propagation to the upstream (ROL-61) ──────────────────────

#[tokio::test]
async fn propagates_inbound_traceparent_to_upstream() {
    use axum::http::HeaderMap;
    use std::sync::Mutex;

    // an upstream that records the traceparent header it received
    let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured = seen.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, _body: Json<Value>| {
            let captured = captured.clone();
            async move {
                *captured.lock().unwrap() = headers
                    .get("traceparent")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                Json(json!({"choices": [{"message": {"content": "ok"}}]}))
            }
        }),
    );
    let upstream = serve(app).await;
    let gw = serve_gateway(&config_for("test-model", vec![("up", upstream)])).await;

    let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0da902b7-01";
    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .header("traceparent", tp)
        .json(&json!({"model": "test-model", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    assert_eq!(
        seen.lock().unwrap().as_deref(),
        Some(tp),
        "gateway did not propagate the caller's traceparent to the upstream"
    );
}

#[tokio::test]
async fn untraced_request_sends_no_traceparent_upstream() {
    use axum::http::HeaderMap;
    use std::sync::Mutex;

    let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured = seen.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, _body: Json<Value>| {
            let captured = captured.clone();
            async move {
                *captured.lock().unwrap() = headers
                    .get("traceparent")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                Json(json!({"choices": [{"message": {"content": "ok"}}]}))
            }
        }),
    );
    let upstream = serve(app).await;
    let gw = serve_gateway(&config_for("test-model", vec![("up", upstream)])).await;

    reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": []}))
        .send()
        .await
        .unwrap();

    // no inbound trace context → nothing added to the upstream wire
    assert_eq!(seen.lock().unwrap().as_deref(), None);
}

// ── request id: generated when absent, echoed when supplied (ROL-60) ─────────

#[tokio::test]
async fn generates_and_echoes_request_id_when_absent() {
    let gw = serve_gateway(&GatewayConfig::default()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "fake-llm", "messages": []}))
        .send()
        .await
        .unwrap();

    let id = resp
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    // a v4 uuid is 36 chars; just assert a non-empty id was minted and returned
    assert!(!id.is_empty(), "gateway did not return an x-request-id");
    assert_eq!(id.len(), 36, "expected a uuid request id, got `{id}`");
}

#[tokio::test]
async fn preserves_caller_supplied_request_id() {
    let gw = serve_gateway(&GatewayConfig::default()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .header("x-request-id", "caller-abc-123")
        .json(&json!({"model": "fake-llm", "messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok()),
        Some("caller-abc-123"),
        "caller's x-request-id should be echoed unchanged"
    );
}

// ── config hot-reload: arc-swap snapshot swap serves new routing live ────────

#[tokio::test]
async fn config_hot_reload_swaps_routing_without_restart() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_openai))).await;

    // start serving with a route for `model-a` only
    let state = rolter_gateway::AppState::with_logging(
        &config_for("model-a", vec![("up", upstream)]),
        None,
    );
    let app = rolter_gateway::build_router(state.clone(), "/metrics", 32 * 1024 * 1024);
    let gw = serve(app).await;
    let client = reqwest::Client::new();

    let a = client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "model-a", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(a.status(), 200, "model-a should route before reload");

    // hot-swap the snapshot to a config that only knows `model-b`
    state.reload(&config_for("model-b", vec![("up", upstream)]), 1);

    let b = client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "model-b", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(b.status(), 200, "model-b should route after reload");

    // the old model is gone from the live snapshot
    let stale = client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "model-a", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 404, "model-a should 404 after reload");
}

/// A mock upstream whose completion carries an address, so an output guardrail
/// has something to mask. Streams the same text when asked.
async fn mock_leaky_openai(body: Json<Value>) -> axum::response::Response {
    let streaming = body
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    if streaming {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"ops@corp.com\"}}]}\n\n\
                   data: [DONE]\n\n";
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            sse,
        )
            .into_response()
    } else {
        Json(json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "write to ops@corp.com"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }))
        .into_response()
    }
}

/// A config with one output-stage rule over the leaky upstream.
fn config_with_output_rule(
    upstream: SocketAddr,
    action: rolter_core::GuardAction,
    streaming: rolter_core::StreamingPostCall,
) -> GatewayConfig {
    let mut config = config_for("test-model", vec![("up", upstream)]);
    config.guardrails = rolter_core::GuardrailsConfig {
        enabled: true,
        max_scan_bytes: None,
        streaming_post_call: streaming,
        rules: vec![rolter_core::GuardrailRule {
            name: "email".to_string(),
            builtin: Some(rolter_core::BuiltinRule::Email),
            pattern: None,
            stage: rolter_core::GuardStage::PostCall,
            action,
            replacement: None,
            include_system: false,
        }],
    };
    config
}

#[tokio::test]
async fn output_guardrail_masks_a_non_streaming_completion() {
    let upstream =
        serve(Router::new().route("/v1/chat/completions", post(mock_leaky_openai))).await;
    let gw = serve_gateway(&config_with_output_rule(
        upstream,
        rolter_core::GuardAction::Redact,
        Default::default(),
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "write to [REDACTED:EMAIL]"
    );
    // the rest of the completion survives the round-trip through the masker
    assert_eq!(body["usage"]["total_tokens"], 2);
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
}

/// A config with one enabled webhook plugin instance at `stage`, scoped to
/// the keyless test tenant (`org_id: ""`, matching a request with no virtual
/// key).
fn config_with_plugin(
    upstream: SocketAddr,
    plugin_endpoint: String,
    stage: rolter_core::PluginStage,
    failure_mode: rolter_core::FailureMode,
) -> GatewayConfig {
    let mut config = config_for("test-model", vec![("up", upstream)]);
    config.plugins = rolter_core::PluginsConfig {
        instances: vec![rolter_core::PluginInstanceConfig {
            slug: "audit".to_string(),
            org_id: String::new(),
            project_id: None,
            stage,
            position: 0,
            failure_mode,
            endpoint: plugin_endpoint,
            auth: None,
        }],
    };
    config
}

async fn plugin_transform_server(action_content: Value) -> SocketAddr {
    serve(Router::new().route(
        "/hook",
        post(move || {
            let content = action_content.clone();
            async move { Json(json!({"action": "transform", "content": content})) }
        }),
    ))
    .await
}

async fn plugin_block_server() -> SocketAddr {
    serve(Router::new().route(
        "/hook",
        post(|| async { Json(json!({"action": "block", "reason": "denied by plugin"})) }),
    ))
    .await
}

#[tokio::test]
async fn pre_upstream_plugin_transforms_the_request_body() {
    // the mock upstream echoes the request body back, so a transform applied
    // pre_upstream is directly observable in the response
    async fn echo(Json(body): Json<Value>) -> axum::response::Response {
        Json(json!({
            "id": "chatcmpl-echo",
            "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": body["messages"][0]["content"]}, "finish_reason": "stop"}],
            "usage": {"total_tokens": 1}
        }))
        .into_response()
    }
    let upstream = serve(Router::new().route("/v1/chat/completions", post(echo))).await;
    let plugin = plugin_transform_server(json!({
        "messages": [{"role": "user", "content": "rewritten by plugin"}]
    }))
    .await;
    let gw = serve_gateway(&config_with_plugin(
        upstream,
        format!("http://{plugin}/hook"),
        rolter_core::PluginStage::PreUpstream,
        rolter_core::FailureMode::FailOpen,
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(
            &json!({"model": "test-model", "messages": [{"role": "user", "content": "original"}]}),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "rewritten by plugin"
    );
}

#[tokio::test]
async fn pre_route_plugin_block_rejects_before_the_route_is_resolved() {
    let upstream =
        serve(Router::new().route("/v1/chat/completions", post(mock_leaky_openai))).await;
    let plugin = plugin_block_server().await;
    let gw = serve_gateway(&config_with_plugin(
        upstream,
        format!("http://{plugin}/hook"),
        rolter_core::PluginStage::PreRoute,
        rolter_core::FailureMode::FailOpen,
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        // an unroutable model still gets blocked at pre_route, before route
        // resolution would have 404'd it — proof the plugin ran first
        .json(&json!({"model": "no-such-model", "messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "plugin_blocked");
    assert_eq!(body["error"]["message"], "denied by plugin");
}

#[tokio::test]
async fn post_response_plugin_masks_a_non_streaming_completion() {
    let upstream =
        serve(Router::new().route("/v1/chat/completions", post(mock_leaky_openai))).await;
    let plugin = plugin_transform_server(json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "[redacted by plugin]"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }))
    .await;
    let gw = serve_gateway(&config_with_plugin(
        upstream,
        format!("http://{plugin}/hook"),
        rolter_core::PluginStage::PostResponse,
        rolter_core::FailureMode::FailOpen,
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "[redacted by plugin]"
    );
}

#[tokio::test]
async fn unreachable_plugin_fails_open_and_forwards_the_original_request() {
    let upstream =
        serve(Router::new().route("/v1/chat/completions", post(mock_leaky_openai))).await;
    let gw = serve_gateway(&config_with_plugin(
        upstream,
        // reserved TEST-NET-1 address: connection refused/timed out fast
        "http://192.0.2.1:1/hook".to_string(),
        rolter_core::PluginStage::PreUpstream,
        rolter_core::FailureMode::FailOpen,
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();

    // the plugin was unreachable but fail_open forwards unchanged
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn a_blocking_output_rule_withholds_the_completion() {
    let upstream =
        serve(Router::new().route("/v1/chat/completions", post(mock_leaky_openai))).await;
    let gw = serve_gateway(&config_with_output_rule(
        upstream,
        rolter_core::GuardAction::Block,
        Default::default(),
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 403);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "guardrail_blocked");
    // the rule name is safe to surface; the matched text never is
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("email"), "{message}");
    assert!(!message.contains("ops@corp.com"), "{message}");
}

#[tokio::test]
async fn a_streamed_request_is_refused_when_output_rules_apply() {
    // the default fails closed: masking cannot run on a stream, so the request
    // is refused rather than quietly served unmasked
    let upstream =
        serve(Router::new().route("/v1/chat/completions", post(mock_leaky_openai))).await;
    let gw = serve_gateway(&config_with_output_rule(
        upstream,
        rolter_core::GuardAction::Redact,
        rolter_core::StreamingPostCall::Reject,
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "test-model",
            "stream": true,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "guardrail_streaming_unsupported");
}

#[tokio::test]
async fn passthrough_serves_the_stream_with_output_rules_off() {
    // the opt-out an operator has to write deliberately
    let upstream =
        serve(Router::new().route("/v1/chat/completions", post(mock_leaky_openai))).await;
    let gw = serve_gateway(&config_with_output_rule(
        upstream,
        rolter_core::GuardAction::Redact,
        rolter_core::StreamingPostCall::Passthrough,
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "test-model",
            "stream": true,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    assert!(text.contains("ops@corp.com"), "{text}");
}

#[tokio::test]
async fn a_route_without_output_rules_is_untouched() {
    // the guard must not exist for an unguarded route: input-only rules leave
    // the response path exactly as it was
    let upstream =
        serve(Router::new().route("/v1/chat/completions", post(mock_leaky_openai))).await;
    let mut config = config_with_output_rule(
        upstream,
        rolter_core::GuardAction::Redact,
        rolter_core::StreamingPostCall::Reject,
    );
    config.guardrails.rules[0].stage = rolter_core::GuardStage::PreCall;
    let gw = serve_gateway(&config).await;

    let streamed = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "test-model",
            "stream": true,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(streamed.status(), 200, "no post_call rule, no rejection");

    let plain = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();
    let body: Value = plain.json().await.unwrap();
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "write to ops@corp.com"
    );
}

// ---------------------------------------------------------------------------
// external PII sanitizer (#848)
// ---------------------------------------------------------------------------

/// Recursively substitute every occurrence of `from` with `to` in every JSON
/// string of `value`. The stub sanitizer's whole detection engine — enough to
/// prove the gateway forwards, substitutes and restores the right bodies.
fn replace_in_strings(value: &mut Value, from: &str, to: &str) -> usize {
    match value {
        Value::String(text) => {
            let hits = text.matches(from).count();
            if hits > 0 {
                *text = text.replace(from, to);
            }
            hits
        }
        Value::Array(items) => items
            .iter_mut()
            .map(|item| replace_in_strings(item, from, to))
            .sum(),
        Value::Object(map) => map
            .values_mut()
            .map(|item| replace_in_strings(item, from, to))
            .sum(),
        _ => 0,
    }
}

const LEAKED_EMAIL: &str = "ops@corp.com";
const PLACEHOLDER: &str = "<EMAIL_ADDRESS_1>";

/// A stub PII sanitizer service shaped like the reference Presidio adapter:
/// `POST /sanitize` swaps the email for a deterministic placeholder and hands
/// back an opaque token, `POST /restore` swaps it back.
///
/// `calls` counts sanitize requests so a test can assert the gateway called the
/// service exactly as many times as the configured direction implies.
fn stub_sanitizer(calls: Arc<AtomicU32>) -> Router {
    let sanitize_calls = calls.clone();
    Router::new()
        .route(
            "/sanitize",
            post(move |Json(body): Json<Value>| {
                let calls = sanitize_calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let mut content = body.get("content").cloned().unwrap_or(Value::Null);
                    let hits = replace_in_strings(&mut content, LEAKED_EMAIL, PLACEHOLDER);
                    let reversible = body
                        .get("reversible")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let mut out = json!({
                        "content": content,
                        "findings": [{
                            "entity_type": "EMAIL_ADDRESS",
                            "count": hits,
                            "placeholders": [PLACEHOLDER],
                        }],
                    });
                    if reversible && hits > 0 {
                        out["restoration_token"] = json!("tok-abc123");
                    }
                    Json(out)
                }
            }),
        )
        .route(
            "/restore",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(
                    body["restoration_token"], "tok-abc123",
                    "gateway must echo the token it was handed"
                );
                let mut content = body.get("content").cloned().unwrap_or(Value::Null);
                let restored = replace_in_strings(&mut content, PLACEHOLDER, LEAKED_EMAIL);
                Json(json!({"content": content, "restored": restored}))
            }),
        )
}

/// An upstream that echoes back whatever content it was sent, so a test can see
/// exactly what the provider received after the request leg ran. Streams the
/// same text when asked.
async fn mock_echo_openai(Json(body): Json<Value>) -> axum::response::Response {
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let seen = body["messages"][0]["content"]
        .as_str()
        .or_else(|| body["messages"][0]["content"][0]["text"].as_str())
        .unwrap_or("")
        .to_string();
    if streaming {
        let sse = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices": [{"delta": {"content": seen}}]})
        );
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            sse,
        )
            .into_response()
    } else {
        Json(json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": seen},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }))
        .into_response()
    }
}

/// A config wiring the gateway to a sanitizer at `sanitizer` over `upstream`.
fn config_with_sanitizer(
    upstream: SocketAddr,
    sanitizer: SocketAddr,
    direction: rolter_core::SanitizeDirection,
    restoration: rolter_core::RestorationPolicy,
) -> GatewayConfig {
    let mut config = config_for("test-model", vec![("up", upstream)]);
    config.pii_sanitizer = rolter_core::PiiSanitizerConfig {
        enabled: true,
        url: format!("http://{sanitizer}/sanitize"),
        restore_url: format!("http://{sanitizer}/restore"),
        direction,
        restoration,
        ..Default::default()
    };
    config
}

#[tokio::test]
async fn sanitizer_strips_pii_before_it_reaches_the_provider() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_echo_openai))).await;
    let calls = Arc::new(AtomicU32::new(0));
    let sanitizer = serve(stub_sanitizer(calls.clone())).await;
    let gw = serve_gateway(&config_with_sanitizer(
        upstream,
        sanitizer,
        rolter_core::SanitizeDirection::Request,
        rolter_core::RestorationPolicy::Never,
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": format!("mail {LEAKED_EMAIL} now")}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    // the upstream echoes what it received: the placeholder, never the address
    let echoed = body["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(echoed.contains(PLACEHOLDER), "upstream saw {echoed}");
    assert!(
        !echoed.contains(LEAKED_EMAIL),
        "the address must not reach the provider: {echoed}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "request leg only");
}

#[tokio::test]
async fn sanitizer_restores_placeholders_for_a_trusted_downstream() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_echo_openai))).await;
    let calls = Arc::new(AtomicU32::new(0));
    let sanitizer = serve(stub_sanitizer(calls.clone())).await;
    let gw = serve_gateway(&config_with_sanitizer(
        upstream,
        sanitizer,
        rolter_core::SanitizeDirection::Request,
        rolter_core::RestorationPolicy::TrustedDownstream,
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": format!("mail {LEAKED_EMAIL} now")}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["choices"][0]["message"]["content"],
        format!("mail {LEAKED_EMAIL} now"),
        "a trusted downstream gets the plaintext back"
    );
}

#[tokio::test]
async fn sanitizer_restores_only_when_the_caller_asks_under_caller_authorized() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_echo_openai))).await;
    let calls = Arc::new(AtomicU32::new(0));
    let sanitizer = serve(stub_sanitizer(calls.clone())).await;
    let gw = serve_gateway(&config_with_sanitizer(
        upstream,
        sanitizer,
        rolter_core::SanitizeDirection::Request,
        rolter_core::RestorationPolicy::CallerAuthorized,
    ))
    .await;

    let payload = json!({
        "model": "test-model",
        "messages": [{"role": "user", "content": format!("mail {LEAKED_EMAIL} now")}]
    });

    let without: Value = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        without["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
            .contains(PLACEHOLDER),
        "no opt-in header: the caller keeps the placeholder"
    );

    let with: Value = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .header(rolter_core::pii_sanitizer::RESTORE_HEADER, "true")
        .json(&payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        with["choices"][0]["message"]["content"],
        format!("mail {LEAKED_EMAIL} now"),
        "opt-in header restores"
    );
}

#[tokio::test]
async fn sanitizer_scrubs_an_anthropic_payload_too() {
    async fn mock_echo_anthropic(Json(body): Json<Value>) -> Json<Value> {
        let seen = body["messages"][0]["content"]
            .as_str()
            .or_else(|| body["messages"][0]["content"][0]["text"].as_str())
            .unwrap_or("")
            .to_string();
        Json(json!({
            "id": "msg_mock",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": seen}],
            "model": "test-model",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }))
    }

    let upstream = serve(Router::new().route("/v1/messages", post(mock_echo_anthropic))).await;
    let calls = Arc::new(AtomicU32::new(0));
    let sanitizer = serve(stub_sanitizer(calls.clone())).await;
    let mut config = config_with_sanitizer(
        upstream,
        sanitizer,
        rolter_core::SanitizeDirection::Request,
        rolter_core::RestorationPolicy::Never,
    );
    config.providers[0].kind = ProviderKind::Anthropic;
    let gw = serve_gateway(&config).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/messages"))
        .json(&json!({
            "model": "test-model",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": format!("mail {LEAKED_EMAIL} now")}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let echoed = body["content"][0]["text"].as_str().unwrap();
    assert!(
        echoed.contains(PLACEHOLDER),
        "anthropic upstream saw {echoed}"
    );
    assert!(!echoed.contains(LEAKED_EMAIL));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_streamed_request_is_refused_when_the_response_leg_is_active() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_echo_openai))).await;
    let calls = Arc::new(AtomicU32::new(0));
    let sanitizer = serve(stub_sanitizer(calls.clone())).await;
    let gw = serve_gateway(&config_with_sanitizer(
        upstream,
        sanitizer,
        rolter_core::SanitizeDirection::Both,
        rolter_core::RestorationPolicy::Never,
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "test-model",
            "stream": true,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "pii_streaming_unsupported");
    assert_eq!(body["error"]["param"], "stream");
}

#[tokio::test]
async fn a_streamed_request_passes_through_when_only_the_request_leg_runs() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_echo_openai))).await;
    let calls = Arc::new(AtomicU32::new(0));
    let sanitizer = serve(stub_sanitizer(calls.clone())).await;
    let gw = serve_gateway(&config_with_sanitizer(
        upstream,
        sanitizer,
        rolter_core::SanitizeDirection::Request,
        rolter_core::RestorationPolicy::Never,
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "test-model",
            "stream": true,
            "messages": [{"role": "user", "content": format!("mail {LEAKED_EMAIL} now")}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "nothing has to touch the response");
    let text = resp.text().await.unwrap();
    assert!(
        text.contains(PLACEHOLDER),
        "sanitized upstream echo: {text}"
    );
    assert!(!text.contains(LEAKED_EMAIL));
}

#[tokio::test]
async fn a_streamed_request_passes_through_when_streaming_is_set_to_passthrough() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_echo_openai))).await;
    let calls = Arc::new(AtomicU32::new(0));
    let sanitizer = serve(stub_sanitizer(calls.clone())).await;
    let mut config = config_with_sanitizer(
        upstream,
        sanitizer,
        rolter_core::SanitizeDirection::Both,
        rolter_core::RestorationPolicy::Never,
    );
    config.pii_sanitizer.streaming = rolter_core::StreamingResponse::Passthrough;
    let gw = serve_gateway(&config).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "test-model",
            "stream": true,
            "messages": [{"role": "user", "content": format!("mail {LEAKED_EMAIL} now")}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    // the request leg still ran, so the stream carries the placeholder — what
    // passthrough waives is the *response* leg, not the request one
    assert!(text.contains(PLACEHOLDER), "{text}");
}

#[tokio::test]
async fn an_unreachable_sanitizer_fails_open_by_default() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_echo_openai))).await;
    // a listener that 500s on every path stands in for a broken service
    let broken = serve(Router::new().route(
        "/sanitize",
        post(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
    ))
    .await;
    let gw = serve_gateway(&config_with_sanitizer(
        upstream,
        broken,
        rolter_core::SanitizeDirection::Request,
        rolter_core::RestorationPolicy::Never,
    ))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": format!("mail {LEAKED_EMAIL} now")}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "fail_open forwards unchanged");
    let body: Value = resp.json().await.unwrap();
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains(LEAKED_EMAIL));
}

#[tokio::test]
async fn an_unreachable_sanitizer_blocks_the_request_when_configured_fail_closed() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_echo_openai))).await;
    let broken = serve(Router::new().route(
        "/sanitize",
        post(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
    ))
    .await;
    let mut config = config_with_sanitizer(
        upstream,
        broken,
        rolter_core::SanitizeDirection::Request,
        rolter_core::RestorationPolicy::Never,
    );
    config.pii_sanitizer.failure_mode = rolter_core::FailureMode::FailClosed;
    let gw = serve_gateway(&config).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": format!("mail {LEAKED_EMAIL} now")}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 502);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "pii_sanitizer_unavailable");
}

// ── unpriced-traffic budget policy (#974) ───────────────────────────────────
//
// A model with no price row accrues zero spend, so it can never exceed a
// budget however much is served. These cover the operator's three answers to
// that: serve it silently, serve it loudly, or refuse it.

/// Chat against a route whose model has no price, under `policy`.
async fn chat_unpriced(policy: UnpricedPolicy) -> reqwest::Response {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_openai))).await;
    let mut config = config_for("unpriced-model", vec![("up", upstream)]);
    config.unpriced_policy = policy;
    let gw = serve_gateway(&config).await;
    reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(
            &json!({"model": "unpriced-model", "messages": [{"role": "user", "content": "ping"}]}),
        )
        .send()
        .await
        .unwrap()
}

/// The default must reproduce today's behaviour exactly, so an upgrade does not
/// start refusing traffic for the deployments #969 measured as fully unpriced.
#[tokio::test]
async fn unpriced_ignore_serves_the_request() {
    assert_eq!(UnpricedPolicy::default(), UnpricedPolicy::Ignore);
    let resp = chat_unpriced(UnpricedPolicy::Ignore).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "pong");
}

/// `warn` is about visibility, not enforcement: the request is still served.
#[tokio::test]
async fn unpriced_warn_still_serves_the_request() {
    let resp = chat_unpriced(UnpricedPolicy::Warn).await;
    assert_eq!(resp.status(), 200);
}

/// `block` refuses with 402 and names the model, so the operator knows which
/// price to set rather than only that something was rejected.
#[tokio::test]
async fn unpriced_block_refuses_with_402_naming_the_model() {
    let resp = chat_unpriced(UnpricedPolicy::Block).await;
    assert_eq!(resp.status(), 402);
    let body: Value = resp.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("unpriced-model"),
        "402 must name the unpriced model: {message}"
    );
    assert_eq!(body["error"]["code"], "model_unpriced");
    assert_eq!(body["error"]["param"], "model");
}

/// `block` must not be a blanket refusal: a priced model is served exactly as
/// before, so the policy costs nothing once the catalogue is complete.
#[tokio::test]
async fn unpriced_block_serves_a_model_that_has_a_price() {
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_openai))).await;
    let mut config = config_for("priced-model", vec![("up", upstream)]);
    config.unpriced_policy = UnpricedPolicy::Block;
    config.model_prices.push(ModelPriceConfig {
        model: "priced-model".to_string(),
        input_per_mtok: 1.0,
        output_per_mtok: 2.0,
        cached_input_per_mtok: None,
        currency: "USD".to_string(),
    });
    let gw = serve_gateway(&config).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "priced-model", "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// The built-in fake-llm is answered locally before routing, so it never
/// reaches the price lookup. `block` must not take away the model the docs
/// point at for smoke-testing a deployment without secrets.
#[tokio::test]
async fn unpriced_block_leaves_the_builtin_fake_llm_reachable() {
    let config = GatewayConfig {
        unpriced_policy: UnpricedPolicy::Block,
        ..Default::default()
    };
    let gw = serve_gateway(&config).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/embeddings"))
        .json(&json!({"model": "fake-llm", "input": ["hello"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// `block` refuses before the upstream call, so no tokens are spent on traffic
/// that would never have been accounted for.
#[tokio::test]
async fn unpriced_block_never_reaches_the_upstream() {
    let hits = Arc::new(AtomicU32::new(0));
    let counter = hits.clone();
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post(move |body: Json<Value>| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                mock_openai(body).await
            }
        }),
    ))
    .await;
    let mut config = config_for("unpriced-model", vec![("up", upstream)]);
    config.unpriced_policy = UnpricedPolicy::Block;
    let gw = serve_gateway(&config).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(
            &json!({"model": "unpriced-model", "messages": [{"role": "user", "content": "ping"}]}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 402);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the upstream must not be called for traffic the budget cannot account for"
    );
}

/// #1162: every one of these settings was stored by the control plane,
/// rendered by the dashboard, and read by nothing. An operator who switched
/// one on was told, by the product, that the gateway now behaved differently.
/// It did not. These tests fail if that regresses — each asserts the *changed*
/// behaviour, and each also asserts the unset default, so a rule that is
/// simply always-on would not pass either.
#[tokio::test]
async fn a_required_header_is_enforced_at_ingress_and_names_only_itself() {
    let mut config = GatewayConfig::default();
    config
        .security
        .required_headers
        .insert("x-mesh-id".to_string(), "edge-42".to_string());
    let addr = serve_gateway(&config).await;
    let client = reqwest::Client::new();

    let missing = client
        .get(format!("http://{addr}/v1/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 403);
    let body: Value = missing.json().await.unwrap();
    assert_eq!(body["error"]["code"], "required_header_missing");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains("x-mesh-id"), "{message}");
    // the expected value may well be a shared secret; it is never echoed
    assert!(!message.contains("edge-42"), "{message}");

    // present but wrong is the same refusal as absent
    let wrong = client
        .get(format!("http://{addr}/v1/models"))
        .header("x-mesh-id", "somewhere-else")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 403);

    let ok = client
        .get(format!("http://{addr}/v1/models"))
        .header("x-mesh-id", "edge-42")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    // a probe is the deployment's own traffic: locking the orchestrator out of
    // /healthz would take the gateway down rather than protect it
    let probe = client
        .get(format!("http://{addr}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(probe.status(), 200);
}

#[tokio::test]
async fn an_ingress_filter_is_absent_until_the_operator_sets_one() {
    let addr = serve_gateway(&GatewayConfig::default()).await;
    let resp = reqwest::get(format!("http://{addr}/v1/models"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn enforcing_virtual_keys_closes_a_gateway_that_holds_no_keys() {
    let mut config = GatewayConfig::default();
    config.security.virtual_key_required = true;
    let addr = serve_gateway(&config).await;
    let closed = reqwest::get(format!("http://{addr}/v1/models"))
        .await
        .unwrap();
    assert_eq!(closed.status(), 401);

    // and the same deployment with the toggle off answers — so the test is
    // measuring the toggle, not the absence of keys
    let addr = serve_gateway(&GatewayConfig::default()).await;
    let open = reqwest::get(format!("http://{addr}/v1/models"))
        .await
        .unwrap();
    assert_eq!(open.status(), 200);
}

#[tokio::test]
async fn a_bypass_route_opens_exactly_the_path_it_names() {
    let mut config = GatewayConfig::default();
    config.security.virtual_key_required = true;
    config.security.auth_bypass_routes = vec!["/v1/models".to_string()];
    let addr = serve_gateway(&config).await;
    let client = reqwest::Client::new();

    let bypassed = client
        .get(format!("http://{addr}/v1/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(bypassed.status(), 200);

    // everything else still needs a key
    let guarded = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .json(&json!({"model": "fake-llm", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(guarded.status(), 401);
}

#[tokio::test]
async fn an_expired_virtual_key_is_refused_at_the_gateway() {
    // #945 gives the mint path a TTL. `KeyMeta::is_valid` already knew how to
    // reject an expired key, but nothing asserted it at the HTTP surface —
    // "expiry is enforced, not merely stored" needs a test that presents one
    let config = GatewayConfig {
        virtual_keys: vec![
            VirtualKeyConfig {
                key: "sk-rolter-expired".to_string(),
                name: Some("yesterday's key".to_string()),
                expires_at: Some(chrono::Utc::now() - chrono::Duration::hours(1)),
                ..Default::default()
            },
            VirtualKeyConfig {
                key: "sk-rolter-live".to_string(),
                name: Some("still good".to_string()),
                expires_at: Some(chrono::Utc::now() + chrono::Duration::days(30)),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let gw = serve_gateway(&config).await;
    let client = reqwest::Client::new();

    let call = |key: &'static str| {
        let client = client.clone();
        async move {
            client
                .post(format!("http://{gw}/v1/chat/completions"))
                .bearer_auth(key)
                .json(&json!({"model": "fake-llm", "messages": [{"role":"user","content":"hi"}]}))
                .send()
                .await
                .unwrap()
        }
    };

    let expired = call("sk-rolter-expired").await;
    assert_eq!(expired.status(), 401);
    let body = expired.text().await.unwrap();
    // the rejection must not leak which key it was or when it lapsed
    assert!(!body.contains("yesterday"), "{body}");

    // a key with a future expiry is untouched: the check is the instant, not
    // the mere presence of a TTL
    assert_eq!(call("sk-rolter-live").await.status(), 200);
}
