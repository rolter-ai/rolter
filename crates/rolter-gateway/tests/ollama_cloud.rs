use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
};
use serde_json::{json, Value};

async fn serve(app: Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

async fn mock_cloud(headers: HeaderMap, Json(body): Json<Value>) -> axum::response::Response {
    if headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some("Bearer mock-ollama-cloud-key")
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        )
            .into_response();
    }
    if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        return (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            "data: {\"choices\":[{\"delta\":{\"content\":\"pong\"}}]}\n\ndata: [DONE]\n\n",
        )
            .into_response();
    }
    Json(json!({"choices":[{"message":{"role":"assistant","content":"pong"}}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})).into_response()
}

fn config(api_base: String, key_env: &str, upstream_model: &str) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "cloud".into(),
        kind: ProviderKind::OllamaCloud,
        api_base,
        api_key: None,
        api_key_env: Some(key_env.into()),
        egress_proxy: None,
        api_keys: vec![],
        also_track_via_llm_call: false,
        llm_probe_model: None,
        status_page_url: None,
        role_profile: None,
        model_role_profiles: Default::default(),
    });
    config.routes.push(ModelRoute {
        model: "cloud-model".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "cloud".into(),
            model: Some(upstream_model.into()),
            weight: 1,
        }],
        params: Default::default(),
        param_policy: Default::default(),
        cache: None,
        variants: vec![],
    });
    config
}

#[tokio::test]
async fn chat_and_stream_use_environment_bearer_key() {
    const KEY_ENV: &str = "ROLTER_TEST_OLLAMA_CLOUD_KEY";
    std::env::set_var(KEY_ENV, "mock-ollama-cloud-key");
    let upstream = serve(Router::new().route("/v1/chat/completions", post(mock_cloud))).await;
    let gw = serve(rolter_gateway::build_router_from_config(&config(
        format!("http://{upstream}"),
        KEY_ENV,
        "upstream-model",
    )))
    .await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model":"cloud-model","messages":[]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["usage"]["total_tokens"],
        2
    );
    let response = client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model":"cloud-model","stream":true,"messages":[]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.text().await.unwrap().contains("data: [DONE]"));
    std::env::remove_var(KEY_ENV);
}

#[tokio::test]
#[ignore = "requires OLLAMA_API_KEY and ROLTER_OLLAMA_LIVE_MODEL"]
async fn live_smoke() {
    let (Ok(_), Ok(model)) = (
        std::env::var("OLLAMA_API_KEY"),
        std::env::var("ROLTER_OLLAMA_LIVE_MODEL"),
    ) else {
        return;
    };
    let gw = serve(rolter_gateway::build_router_from_config(&config(
        "https://ollama.com".into(),
        "OLLAMA_API_KEY",
        &model,
    )))
    .await;
    let response = reqwest::Client::new().post(format!("http://{gw}/v1/chat/completions")).json(&json!({"model":"cloud-model","messages":[{"role":"user","content":"Reply with OK"}],"max_tokens":8})).send().await.unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
}
