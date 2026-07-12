//! End-to-end gateway integration tests. Each test spins one or more real mock
//! upstream servers on ephemeral ports, serves the gateway router in-process on
//! another ephemeral port, and drives it over HTTP with reqwest — exercising the
//! full parse → auth → balance → forward → stream-back pipeline.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
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
