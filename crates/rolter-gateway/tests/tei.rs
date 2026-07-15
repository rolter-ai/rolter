//! Mock TEI coverage. Real-server coverage is opt-in under `integration/tei`.

use std::net::SocketAddr;

use axum::http::{HeaderMap, StatusCode};
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

fn config(addr: SocketAddr, api_key: Option<&str>) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "tei-local".into(),
        slug: None,
        kind: ProviderKind::Tei,
        api_base: format!("http://{addr}"),
        api_key: api_key.map(str::to_string),
        api_key_env: None,
        egress_proxy: None,
        ca_bundles: None,
        api_keys: vec![],
        also_track_via_llm_call: false,
        llm_probe_model: None,
        status_page_url: None,
        role_profile: None,
        model_role_profiles: Default::default(),
    });
    config.routes.push(ModelRoute {
        model: "embed-local".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "tei-local".into(),
            model: Some("sentence-transformers/all-MiniLM-L6-v2".into()),
            weight: 1,
        }],
        params: Default::default(),
        param_policy: Default::default(),
        cache: None,
        variants: Default::default(),
    });
    config
}

#[tokio::test]
async fn all_openai_input_forms_and_optional_fields_round_trip() {
    async fn mock(headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
        assert!(!headers.contains_key("authorization"));
        assert_eq!(body["model"], "sentence-transformers/all-MiniLM-L6-v2");
        assert_eq!(body["encoding_format"], "float");
        assert_eq!(body["dimensions"], 3);
        assert_eq!(body["user"], "tenant-1");
        Json(json!({
            "object": "list",
            "data": [{"object": "embedding", "index": 0, "embedding": [0.1, 0.2, 0.3]}],
            "model": body["model"],
            "usage": {"prompt_tokens": 4, "total_tokens": 4}
        }))
    }
    let upstream = serve(Router::new().route("/v1/embeddings", post(mock))).await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(
        upstream, None,
    )))
    .await;
    let client = reqwest::Client::new();

    for input in [
        json!("hello"),
        json!(["hello", "world"]),
        json!([1, 2, 3]),
        json!([[1, 2], [3, 4]]),
    ] {
        let response = client
            .post(format!("http://{gateway}/v1/embeddings"))
            .json(&json!({
                "model": "embed-local",
                "input": input,
                "encoding_format": "float",
                "dimensions": 3,
                "user": "tenant-1"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-rolter-provider"], "tei-local");
        assert_eq!(
            response.headers()["x-rolter-target"],
            "sentence-transformers/all-MiniLM-L6-v2"
        );
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["usage"]["total_tokens"], 4);
        assert_eq!(body["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));
    }
}

#[tokio::test]
async fn optional_bearer_auth_and_tei_errors_are_preserved() {
    async fn mock(headers: HeaderMap) -> (StatusCode, Json<Value>) {
        assert_eq!(headers["authorization"], "Bearer tei-secret");
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "input must not be empty", "error_type": "validation"})),
        )
    }
    let upstream = serve(Router::new().route("/v1/embeddings", post(mock))).await;
    let gateway = serve(rolter_gateway::build_router_from_config(&config(
        upstream,
        Some("tei-secret"),
    )))
    .await;
    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/embeddings"))
        .json(&json!({"model": "embed-local", "input": ""}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "input must not be empty");
    assert_eq!(body["error_type"], "validation");
}
