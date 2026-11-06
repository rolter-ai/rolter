//! A provider's credential, and whose failure it is when it is wrong (#3644).
//!
//! A provider that names a credential the gateway cannot resolve, such as an
//! unset environment variable, used to be called with none, and its `401`
//! reached the caller unchanged: a caller could not tell "my virtual key is
//! wrong" from "the operator never configured this provider". Now such a
//! provider is not called, the attempt fails over like any provider-side
//! failure, and with nothing left the request ends as a `503`
//! `provider_credential_missing`. A provider that refuses the credentials it
//! was sent (`401` or `403`) fails over the same way and ends as a `502`
//! `upstream_auth_failed`, never as the caller's own `401`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
};
use serde_json::{json, Value};

/// A variable no test sets, so a provider reading its key from it has none.
const UNSET: &str = "ROLTER_TEST_PROVIDER_KEY_NEVER_SET_3644";

#[derive(Clone)]
struct Canned {
    status: StatusCode,
    calls: Arc<AtomicU32>,
}

async fn canned(State(canned): State<Canned>) -> axum::response::Response {
    canned.calls.fetch_add(1, Ordering::SeqCst);
    if canned.status.is_success() {
        return Json(json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "model": "test-model",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }))
        .into_response();
    }
    (
        canned.status,
        Json(json!({"type": "error", "error": {"type": "authentication_error", "message": "x-api-key header is required"}})),
    )
        .into_response()
}

/// An upstream that answers every chat completion with `status`.
async fn upstream(status: StatusCode) -> (SocketAddr, Arc<AtomicU32>) {
    let calls = Arc::new(AtomicU32::new(0));
    let app = Router::new()
        .route("/v1/chat/completions", post(canned))
        .with_state(Canned {
            status,
            calls: calls.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, calls)
}

fn provider(name: &str, addr: SocketAddr, key: Option<&str>, env: Option<&str>) -> ProviderConfig {
    ProviderConfig {
        name: name.into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{addr}"),
        api_key: key.map(str::to_string),
        api_key_env: env.map(str::to_string),
        ..Default::default()
    }
}

/// `test-model` over `providers`, the first weighted so far above the rest
/// that it is the one dialled first, failing over to the next.
fn config(providers: Vec<ProviderConfig>) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    let targets = providers
        .iter()
        .enumerate()
        .map(|(i, p)| Target {
            provider: p.name.clone(),
            model: Some("test-model".into()),
            weight: if i == 0 { 1000 } else { 1 },
            ..Default::default()
        })
        .collect();
    config.providers = providers;
    config.routes.push(ModelRoute {
        model: "test-model".into(),
        strategy: BalancingStrategy::Weighted,
        targets,
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    config
}

async fn chat(config: &GatewayConfig) -> (StatusCode, Value) {
    let app = rolter_gateway::build_router_from_config(config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_provider_whose_credential_does_not_resolve_is_not_called() {
    assert!(std::env::var(UNSET).is_err(), "{UNSET} must stay unset");
    let (addr, calls) = upstream(StatusCode::OK).await;
    let (status, body) = chat(&config(vec![provider("openai", addr, None, Some(UNSET))])).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(
        body["error"]["code"], "provider_credential_missing",
        "{body}"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("'openai'"),
        "{body}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the provider was called with no key"
    );
}

#[tokio::test]
async fn a_missing_credential_fails_over_to_a_target_that_has_one() {
    let (first, first_calls) = upstream(StatusCode::OK).await;
    let (second, second_calls) = upstream(StatusCode::OK).await;
    let (status, body) = chat(&config(vec![
        provider("openai", first, None, Some(UNSET)),
        provider("backup", second, Some("sk-test-backup"), None),
    ]))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(first_calls.load(Ordering::SeqCst), 0);
    assert_eq!(second_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_provider_that_names_no_credential_is_still_called_bare() {
    // a local vLLM needs no key: naming none is not the same as one missing
    let (addr, calls) = upstream(StatusCode::OK).await;
    let (status, body) = chat(&config(vec![provider("vllm", addr, None, None)])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_upstream_401_is_a_502_of_its_own_and_not_the_callers() {
    let (addr, _) = upstream(StatusCode::UNAUTHORIZED).await;
    let (status, body) = chat(&config(vec![provider(
        "anthropic",
        addr,
        Some("sk-wrong"),
        None,
    )]))
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["error"]["code"], "upstream_auth_failed", "{body}");
    // the upstream's own words are the operator's, in the log row, not the caller's
    assert!(
        !body.to_string().contains("x-api-key header is required"),
        "{body}"
    );
}

#[tokio::test]
async fn an_upstream_403_that_is_no_content_refusal_is_one_too() {
    let (addr, _) = upstream(StatusCode::FORBIDDEN).await;
    let (status, body) = chat(&config(vec![provider(
        "anthropic",
        addr,
        Some("sk-wrong"),
        None,
    )]))
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["error"]["code"], "upstream_auth_failed", "{body}");
}

#[tokio::test]
async fn an_upstream_401_fails_over_to_the_next_target() {
    let (first, first_calls) = upstream(StatusCode::UNAUTHORIZED).await;
    let (second, second_calls) = upstream(StatusCode::OK).await;
    let (status, body) = chat(&config(vec![
        provider("anthropic", first, Some("sk-wrong"), None),
        provider("backup", second, Some("sk-right"), None),
    ]))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(first_calls.load(Ordering::SeqCst), 1);
    assert_eq!(second_calls.load(Ordering::SeqCst), 1);
}
