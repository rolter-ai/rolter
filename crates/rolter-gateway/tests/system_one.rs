//! System One decisions through the gateway (#3150): a mock decision server
//! stands in for TypeSafe Jev or a vLLM Semantic Router engine.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{GatewayConfig, ModelRoute, ProviderConfig, ProviderKind};
use serde_json::{json, Value};

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

fn provider(name: &str, kind: ProviderKind, api_base: String) -> ProviderConfig {
    ProviderConfig {
        name: name.into(),
        kind,
        api_base,
        api_key: Some("ts-secret".into()),
        ..Default::default()
    }
}

fn route(model: &str, strategy: &str, targets: &[(&str, &str)]) -> ModelRoute {
    let targets: Vec<Value> = targets
        .iter()
        .map(|(provider, model)| json!({ "provider": provider, "model": model }))
        .collect();
    serde_json::from_value(json!({ "model": model, "strategy": strategy, "targets": targets }))
        .expect("a route")
}

fn ticket(model: &str) -> Value {
    json!({
        "model": model,
        "state": "Our checkout has returned 500 errors since 9am.",
        "questions": {
            "department": {
                "type": "choice",
                "instructions": "Which team owns this?",
                "criteria": {"billing": "payments", "engineering": "software errors"}
            },
            "is_urgent": {"type": "noul", "instructions": "Is it urgent?"}
        }
    })
}

/// A decision server that counts its calls and answers the way TypeSafe
/// documents, echoing the model it was asked for.
#[derive(Clone, Default)]
struct Upstream {
    calls: Arc<AtomicUsize>,
}

async fn decide(
    State(upstream): State<Upstream>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    upstream.calls.fetch_add(1, Ordering::SeqCst);
    assert_eq!(headers["authorization"], "Bearer ts-secret");
    assert!(body["questions"]["department"].is_object());
    Json(json!({
        "model": body["model"],
        "answers": {
            "department": {
                "type": "choice",
                "choice": "engineering",
                "probabilities": {"billing": 0.03, "engineering": 0.97},
                "confidence": 0.91
            },
            "is_urgent": {"type": "noul", "noul": 0.96}
        },
        "usage": {"input_tokens": 120, "output_tokens": 0}
    }))
}

async fn upstream() -> (SocketAddr, Upstream) {
    let state = Upstream::default();
    let app = Router::new()
        .route("/v1/systemone", post(decide))
        .with_state(state.clone());
    (serve(app).await, state)
}

async fn post_json(gateway: SocketAddr, path: &str, body: &Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gateway}{path}"))
        .json(body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn openjev_pixels_and_structured_state_survive_the_proxy() {
    let request = json!({
        "model": "openjev-v5",
        "state": {"text": "An image: <<IMG>>", "metadata": [1, 2]},
        "image_data": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
        "questions": {"parcel/risk": {"type": "noul", "instructions": "Is the parcel damaged?", "extension": true}}
    });
    let expected = request.clone();
    let addr = serve(Router::new().route(
        "/v1/systemone",
        post(move |Json(body): Json<Value>| {
            let expected = expected.clone();
            async move {
                assert_eq!(body, expected);
                Json(json!({"answers": {"parcel/risk": {"type": "noul", "noul": 0.9}}}))
            }
        }),
    ))
    .await;
    let mut config = GatewayConfig::default();
    config.providers.push(provider(
        "openjev",
        ProviderKind::SystemOne,
        format!("http://{addr}"),
    ));
    config
        .routes
        .push(route("parcel", "round_robin", &[("openjev", "openjev-v5")]));
    let gateway = serve(rolter_gateway::build_router_from_config(&config)).await;
    let mut public_request = request;
    public_request["model"] = json!("parcel");
    let response = post_json(gateway, "/v1/systemone", &public_request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["answers"]["parcel/risk"]["noul"], 0.9);
}

#[tokio::test]
async fn a_decision_reaches_the_upstream_and_comes_back_typed() {
    let (addr, upstream) = upstream().await;
    let mut config = GatewayConfig::default();
    config.providers.push(provider(
        "typesafe",
        ProviderKind::SystemOne,
        format!("http://{addr}"),
    ));
    config
        .routes
        .push(route("jev", "round_robin", &[("typesafe", "jev-1.13.0")]));
    let gateway = serve(rolter_gateway::build_router_from_config(&config)).await;

    let response = post_json(gateway, "/v1/systemone", &ticket("jev")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-rolter-provider"], "typesafe");
    assert_eq!(response.headers()["x-rolter-target"], "jev-1.13.0");
    let body: Value = response.json().await.unwrap();
    // the target's upstream model replaced the public name on the way out
    assert_eq!(body["model"], "jev-1.13.0");
    assert_eq!(body["answers"]["department"]["choice"], "engineering");
    assert_eq!(body["answers"]["is_urgent"]["noul"], 0.96);
    assert_eq!(body["usage"]["input_tokens"], 120);
    assert_eq!(upstream.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_decision_model_refuses_other_endpoints_without_calling_upstream() {
    let (addr, upstream) = upstream().await;
    let mut config = GatewayConfig::default();
    config.providers.push(provider(
        "typesafe",
        ProviderKind::SystemOne,
        format!("http://{addr}"),
    ));
    config
        .routes
        .push(route("jev", "round_robin", &[("typesafe", "jev-latest")]));
    let gateway = serve(rolter_gateway::build_router_from_config(&config)).await;

    for (path, body) in [
        (
            "/v1/chat/completions",
            json!({"model": "jev", "messages": [{"role": "user", "content": "hi"}]}),
        ),
        ("/v1/embeddings", json!({"model": "jev", "input": "hi"})),
    ] {
        let response = post_json(gateway, path, &body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "unsupported_endpoint", "{path}");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("/v1/systemone"),
            "{body}"
        );
    }
    assert_eq!(upstream.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_chat_api_is_not_sent_a_decision() {
    let mut config = GatewayConfig::default();
    config.providers.push(provider(
        "anthropic",
        ProviderKind::Anthropic,
        "http://127.0.0.1:1".to_string(),
    ));
    config.routes.push(route(
        "claude",
        "round_robin",
        &[("anthropic", "claude-sonnet-5-5")],
    ));
    let gateway = serve(rolter_gateway::build_router_from_config(&config)).await;

    let response = post_json(gateway, "/v1/systemone", &ticket("claude")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unsupported_endpoint");
}

#[tokio::test]
async fn decisions_balance_across_two_decision_servers() {
    let (first, first_calls) = upstream().await;
    let (second, second_calls) = upstream().await;
    let mut config = GatewayConfig::default();
    config.providers.push(provider(
        "typesafe-a",
        ProviderKind::SystemOne,
        format!("http://{first}"),
    ));
    config.providers.push(provider(
        "typesafe-b",
        ProviderKind::SystemOne,
        format!("http://{second}"),
    ));
    config.routes.push(route(
        "jev",
        "round_robin",
        &[("typesafe-a", "jev-latest"), ("typesafe-b", "jev-latest")],
    ));
    let gateway = serve(rolter_gateway::build_router_from_config(&config)).await;

    for _ in 0..4 {
        let response = post_json(gateway, "/v1/systemone", &ticket("jev")).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_eq!(first_calls.calls.load(Ordering::SeqCst), 2);
    assert_eq!(second_calls.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_upstream_validation_error_reaches_the_caller() {
    async fn reject() -> (StatusCode, Json<Value>) {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"detail": "questions.department.criteria: at least 2 options"})),
        )
    }
    let addr = serve(Router::new().route("/v1/systemone", post(reject))).await;
    let mut config = GatewayConfig::default();
    config.providers.push(provider(
        "typesafe",
        ProviderKind::SystemOne,
        format!("http://{addr}"),
    ));
    config
        .routes
        .push(route("jev", "round_robin", &[("typesafe", "jev-latest")]));
    let gateway = serve(rolter_gateway::build_router_from_config(&config)).await;

    let response = post_json(gateway, "/v1/systemone", &ticket("jev")).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json().await.unwrap();
    assert!(body["detail"].as_str().unwrap().contains("criteria"));
}
