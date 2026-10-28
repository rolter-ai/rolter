//! Routes that reach only text-embeddings servers (`tei`, `infinity`) refuse
//! the endpoints those servers lack without calling them (#3247).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
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

/// An embeddings server that counts every request it receives, whatever the
/// path, and answers embeddings and rerank the way TEI and Infinity do.
async fn upstream() -> (SocketAddr, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    async fn embed(State(calls): State<Arc<AtomicUsize>>, Json(_): Json<Value>) -> Json<Value> {
        calls.fetch_add(1, Ordering::SeqCst);
        Json(json!({
            "object": "list",
            "data": [{"object": "embedding", "index": 0, "embedding": [0.1, 0.2]}],
            "model": "bge",
            "usage": {"prompt_tokens": 2, "total_tokens": 2}
        }))
    }
    async fn rerank(State(calls): State<Arc<AtomicUsize>>, Json(_): Json<Value>) -> Json<Value> {
        calls.fetch_add(1, Ordering::SeqCst);
        Json(json!({"results": [{"index": 0, "relevance_score": 0.9}]}))
    }
    async fn anything(State(calls): State<Arc<AtomicUsize>>) -> StatusCode {
        calls.fetch_add(1, Ordering::SeqCst);
        StatusCode::NOT_FOUND
    }
    let app = Router::new()
        .route("/v1/embeddings", post(embed))
        .route("/embeddings", post(embed))
        .route("/v1/rerank", post(rerank))
        .route("/rerank", post(rerank))
        .fallback(anything)
        .with_state(calls.clone());
    (serve(app).await, calls)
}

async fn gateway(kind: ProviderKind, upstream: SocketAddr) -> SocketAddr {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "embed".into(),
        kind,
        api_base: format!("http://{upstream}"),
        ..Default::default()
    });
    let route: ModelRoute = serde_json::from_value(json!({
        "model": "bge",
        "targets": [{"provider": "embed", "model": "bge-small"}]
    }))
    .expect("a route");
    config.routes.push(route);
    serve(rolter_gateway::build_router_from_config(&config)).await
}

async fn post_json(gateway: SocketAddr, path: &str, body: &Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gateway}{path}"))
        .json(body)
        .send()
        .await
        .unwrap()
}

fn chat_shaped() -> Vec<(&'static str, Value)> {
    let messages = json!([{"role": "user", "content": "hi"}]);
    vec![
        (
            "/v1/chat/completions",
            json!({"model": "bge", "messages": messages}),
        ),
        (
            "/v1/messages",
            json!({"model": "bge", "max_tokens": 8, "messages": messages}),
        ),
        ("/v1/responses", json!({"model": "bge", "input": "hi"})),
        (
            "/v1/images/generations",
            json!({"model": "bge", "prompt": "a cat"}),
        ),
        (
            "/v1/audio/speech",
            json!({"model": "bge", "input": "hi", "voice": "alloy"}),
        ),
    ]
}

#[tokio::test]
async fn an_embedding_route_refuses_chat_shaped_endpoints_without_calling_upstream() {
    for kind in [ProviderKind::Tei, ProviderKind::Infinity] {
        let (addr, calls) = upstream().await;
        let gw = gateway(kind, addr).await;
        for (path, body) in chat_shaped() {
            let response = post_json(gw, path, &body).await;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "{kind:?} {path}"
            );
            let body: Value = response.json().await.unwrap();
            assert_eq!(
                body["error"]["code"], "unsupported_endpoint",
                "{kind:?} {path}"
            );
            assert!(
                body["error"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("/v1/embeddings"),
                "{body}"
            );
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "{kind:?}: nothing may reach the upstream"
        );
    }
}

#[tokio::test]
async fn an_embedding_route_still_embeds() {
    for kind in [ProviderKind::Tei, ProviderKind::Infinity] {
        let (addr, calls) = upstream().await;
        let gw = gateway(kind, addr).await;
        let response = post_json(
            gw,
            "/v1/embeddings",
            &json!({"model": "bge", "input": "hi"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{kind:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{kind:?}");
    }
}

#[tokio::test]
async fn an_infinity_route_still_reranks() {
    let (addr, calls) = upstream().await;
    let gw = gateway(ProviderKind::Infinity, addr).await;
    let response = post_json(
        gw,
        "/v1/rerank",
        &json!({"model": "bge", "query": "q", "documents": ["a"]}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
