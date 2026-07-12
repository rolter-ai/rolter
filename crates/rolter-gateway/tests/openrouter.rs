//! Mocked OpenRouter coverage plus an opt-in credentialed live smoke test.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
};
use serde_json::{json, Value};

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

fn provider(name: &str, api_base: String) -> ProviderConfig {
    ProviderConfig {
        name: name.into(),
        kind: ProviderKind::Openrouter,
        api_base,
        api_key: Some("test-openrouter-key".into()),
        api_key_env: None,
        egress_proxy: None,
        api_keys: vec![],
        also_track_via_llm_call: false,
        llm_probe_model: None,
        status_page_url: None,
    }
}

fn config(providers: Vec<ProviderConfig>) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    let targets = providers
        .iter()
        .map(|provider| Target {
            provider: provider.name.clone(),
            model: Some("anthropic/claude-sonnet-4".into()),
            weight: 1,
        })
        .collect();
    config.providers = providers;
    config.routes.push(ModelRoute {
        model: "router-chat".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets,
        params: Default::default(),
        param_policy: Default::default(),
        cache: None,
        variants: Default::default(),
    });
    config
}

#[tokio::test]
async fn chat_preserves_routing_controls_usage_and_sse() {
    async fn mock(headers: HeaderMap, Json(body): Json<Value>) -> axum::response::Response {
        assert_eq!(headers["authorization"], "Bearer test-openrouter-key");
        assert_eq!(body["model"], "anthropic/claude-sonnet-4");
        assert_eq!(body["provider"]["order"], json!(["Anthropic", "Google"]));
        assert_eq!(body["provider"]["allow_fallbacks"], true);
        if body["stream"] == true {
            return (
                [("content-type", "text/event-stream")],
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n",
            )
                .into_response();
        }
        Json(json!({
            "id": "gen-openrouter",
            "choices": [{"message": {"role": "assistant", "content": "ok"}}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3, "cost": 0.00001},
            "provider": "Anthropic"
        }))
        .into_response()
    }

    let upstream = serve(Router::new().route("/api/v1/chat/completions", post(mock))).await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(vec![
        provider("openrouter", format!("http://{upstream}/api/v1")),
    ])))
    .await;
    let client = reqwest::Client::new();
    let mut request = json!({
        "model": "router-chat",
        "messages": [{"role": "user", "content": "hello"}],
        "provider": {"order": ["Anthropic", "Google"], "allow_fallbacks": true}
    });

    let response = client
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["usage"]["cost"], 0.00001);
    assert_eq!(body["provider"], "Anthropic");

    request["stream"] = Value::Bool(true);
    let response = client
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .contains("text/event-stream"));
    assert!(response.text().await.unwrap().contains("data: [DONE]"));
}

#[tokio::test]
async fn retryable_error_falls_back_and_preserves_openrouter_error_context() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let down = serve(Router::new().route(
        "/api/v1/chat/completions",
        post({
            let attempts = attempts.clone();
            move || async move {
                attempts.fetch_add(1, Ordering::Relaxed);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({"error": {"message": "provider rate limited", "code": 429, "metadata": {"provider_name": "Downstream"}}})),
                )
            }
        }),
    ))
    .await;
    let up = serve(Router::new().route(
        "/api/v1/chat/completions",
        post(|| async { Json(json!({"choices": [{"message": {"content": "fallback"}}]})) }),
    ))
    .await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(vec![
        provider("openrouter-a", format!("http://{down}/api/v1")),
        provider("openrouter-b", format!("http://{up}/api/v1")),
    ])))
    .await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({"model": "router-chat", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(attempts.load(Ordering::Relaxed) >= 1);
    assert_eq!(response.headers()["x-rolter-provider"], "openrouter-b");

    let rejected = serve(Router::new().route(
        "/api/v1/chat/completions",
        post(|| async {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": "model unavailable", "code": 400, "metadata": {"provider_name": "ExampleAI"}}})),
            )
        }),
    ))
    .await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(vec![
        provider("openrouter-error", format!("http://{rejected}/api/v1")),
    ])))
    .await;
    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({"model": "router-chat", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["metadata"]["provider_name"], "ExampleAI");
}

#[tokio::test]
#[ignore = "requires OPENROUTER_API_KEY and makes a billable network request"]
async fn live_openrouter_smoke() {
    let key = std::env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY");
    let model =
        std::env::var("ROLTER_OPENROUTER_LIVE_MODEL").expect("ROLTER_OPENROUTER_LIVE_MODEL");
    let mut provider = provider("openrouter", "https://openrouter.ai/api/v1".into());
    provider.api_key = Some(key);
    let mut config = config(vec![provider]);
    config.routes[0].targets[0].model = Some(model);
    let gateway = serve(rolter_gateway::build_router_from_config(&config)).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({"model": "router-chat", "messages": [{"role": "user", "content": "Reply OK"}], "max_tokens": 8}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
