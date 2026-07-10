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
