//! Mock llama-server coverage. The real-server smoke test lives in
//! `integration/llama-cpp-smoke.sh` and is opt-in because it requires a GGUF.

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

fn config(addr: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "local-llama".into(),
        kind: ProviderKind::LlamaCpp,
        api_base: format!("http://{addr}"),
        api_key: None,
        api_key_env: None,
        egress_proxy: None,
        api_keys: vec![],
        also_track_via_llm_call: false,
        llm_probe_model: None,
        status_page_url: None,
        role_profile: None,
        model_role_profiles: Default::default(),
    });
    config.routes.push(ModelRoute {
        model: "local-chat".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "local-llama".into(),
            model: Some("tiny.gguf".into()),
            weight: 1,
        }],
        params: Default::default(),
        param_policy: Default::default(),
        cache: None,
        variants: Default::default(),
    });
    config
}

async fn mock_llama(headers: HeaderMap, Json(body): Json<Value>) -> axum::response::Response {
    assert!(!headers.contains_key("authorization"));
    assert_eq!(body["model"], "tiny.gguf");
    assert_eq!(body["temperature"], 0.2);
    assert_eq!(body["grammar"], "root ::= \"yes\"");
    assert_eq!(body["response_format"]["type"], "json_object");
    if body["stream"] == true {
        return (
            [("content-type", "text/event-stream")],
            "data: {\"choices\":[{\"text\":\"yes\"}]}\n\ndata: [DONE]\n\n",
        )
            .into_response();
    }
    Json(json!({"choices": [{"text": "yes"}]})).into_response()
}

#[tokio::test]
async fn llama_cpp_alias_sampling_json_and_sse_pass_through_without_auth() {
    let upstream = serve(Router::new().route("/v1/completions", post(mock_llama))).await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(upstream))).await;
    let client = reqwest::Client::new();
    let request = json!({
        "model": "local-chat",
        "prompt": "answer yes",
        "temperature": 0.2,
        "grammar": "root ::= \"yes\"",
        "response_format": {"type": "json_object"}
    });

    let response = client
        .post(format!("http://{gateway}/v1/completions"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-rolter-provider"], "local-llama");
    assert_eq!(response.headers()["x-rolter-target"], "tiny.gguf");

    let mut stream_request = request;
    stream_request["stream"] = Value::Bool(true);
    let response = client
        .post(format!("http://{gateway}/v1/completions"))
        .json(&stream_request)
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
async fn llama_cpp_models_are_listed_and_upstream_errors_propagate() {
    let calls = Arc::new(AtomicUsize::new(0));
    let upstream = serve(Router::new().route(
        "/v1/completions",
        post({
            let calls = calls.clone();
            move || async move {
                calls.fetch_add(1, Ordering::Relaxed);
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": {"message": "model loading"}})),
                )
            }
        }),
    ))
    .await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(upstream))).await;
    let client = reqwest::Client::new();

    let models: Value = client
        .get(format!("http://{gateway}/v1/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"][0]["id"], "local-chat");

    let response = client
        .post(format!("http://{gateway}/v1/completions"))
        .json(&json!({"model": "local-chat", "prompt": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(calls.load(Ordering::Relaxed) >= 1);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["message"],
        "model loading"
    );
}
