//! End-to-end gateway integration tests. Each test spins one or more real mock
//! upstream servers on ephemeral ports, serves the gateway router in-process on
//! another ephemeral port, and drives it over HTTP with reqwest — exercising the
//! full parse → auth → balance → forward → stream-back pipeline.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::extract::{OriginalUri, Path, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, RoleProfile,
    Target, VirtualKeyConfig,
};
use serde_json::{json, Value};

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
            api_key: None,
            api_key_env: None,
            egress_proxy: None,
            api_keys: Vec::new(),
            also_track_via_llm_call: false,
            llm_probe_model: None,
            status_page_url: None,
            role_profile: None,
            model_role_profiles: Default::default(),
        });
    }
    config.routes.push(ModelRoute {
        model: model.to_string(),
        strategy: BalancingStrategy::RoundRobin,
        targets,
        params: Default::default(),
        param_policy: Default::default(),
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
    async fn create() -> impl IntoResponse {
        Json(json!({"id":"resp_native","object":"response","status":"completed"}))
    }
    async fn retrieve(Path(id): Path<String>) -> impl IntoResponse {
        Json(json!({"id":id,"object":"response","provider":"pinned"}))
    }
    async fn cancel(Path(id): Path<String>) -> impl IntoResponse {
        Json(json!({"id":id,"object":"response","status":"cancelled"}))
    }
    async fn input_items(
        Path(id): Path<String>,
        OriginalUri(uri): OriginalUri,
    ) -> impl IntoResponse {
        assert_eq!(uri.query(), Some("limit=1"));
        Json(json!({"object":"list","response_id":id,"data":[{"id":"item_1"}]}))
    }
    async fn delete_response(Path(id): Path<String>) -> impl IntoResponse {
        Json(json!({"id":id,"object":"response.deleted","deleted":true}))
    }

    let upstream = serve(
        Router::new()
            .route("/v1/responses", post(create))
            .route("/v1/responses/{id}", get(retrieve).delete(delete_response))
            .route("/v1/responses/{id}/cancel", post(cancel))
            .route("/v1/responses/{id}/input_items", get(input_items)),
    )
    .await;
    let mut config = config_for("native-model", vec![("native", upstream)]);
    config.providers[0].kind = ProviderKind::Openai;
    config.virtual_keys = vec![
        VirtualKeyConfig {
            key: "sk-tenant-a".to_string(),
            name: None,
            models: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
        },
        VirtualKeyConfig {
            key: "sk-tenant-b".to_string(),
            name: None,
            models: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
        },
    ];
    let gw = serve_gateway(&config).await;
    let client = reqwest::Client::new();

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
    assert_eq!(created["id"], "resp_native");

    let cross_tenant = client
        .get(format!("http://{gw}/v1/responses/resp_native"))
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

    let retrieved: Value = client
        .get(format!("http://{gw}/v1/responses/resp_native"))
        .bearer_auth("sk-tenant-a")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retrieved["provider"], "pinned");
    let cancelled: Value = client
        .post(format!("http://{gw}/v1/responses/resp_native/cancel"))
        .bearer_auth("sk-tenant-a")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    let items: Value = client
        .get(format!(
            "http://{gw}/v1/responses/resp_native/input_items?limit=1"
        ))
        .bearer_auth("sk-tenant-a")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(items["data"][0]["id"], "item_1");
    let deleted = client
        .delete(format!("http://{gw}/v1/responses/resp_native"))
        .bearer_auth("sk-tenant-a")
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 200);
    let after_delete = client
        .get(format!("http://{gw}/v1/responses/resp_native"))
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
                disabled: false,
                expires_at: None,
                cache: None,
            },
            VirtualKeyConfig {
                key: "sk-tenant-b".to_string(),
                name: None,
                models: vec![],
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
            api_key: None,
            api_key_env: None,
            egress_proxy: None,
            api_keys: Vec::new(),
            also_track_via_llm_call: false,
            llm_probe_model: None,
            status_page_url: None,
            role_profile: None,
            model_role_profiles: Default::default(),
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
