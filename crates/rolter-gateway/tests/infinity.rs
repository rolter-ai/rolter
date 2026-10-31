//! Mock Infinity coverage (#463).
//!
//! The fake below reproduces the michaelfeil/infinity server closely enough to
//! pin what rolter depends on, as read from `infinity_emb/infinity_server.py`
//! and `fastapi_schemas/pymodels.py`:
//!
//! - its routes sit under `--url-prefix`, empty by default, so `/embeddings`,
//!   `/rerank` and `/models` with no `/v1`;
//! - one process serves several models, picked by the body's `model`; an
//!   unknown one is a `400` in OpenAI's error shape (`Invalid model: ...`), and
//!   a model asked for a task it does not do is a `400` too;
//! - a key, when set, is read as `Authorization: Bearer`, and a wrong one is
//!   FastAPI's bare `401 {"detail": "Unauthorized"}`;
//! - a body that fails validation is FastAPI's `422 {"detail": [...]}`;
//! - a rerank answer is sorted by descending score, cut to `top_n`, and carries
//!   each document as a bare string when `return_documents` is set.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::{
    AdvancedModelConfig, BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig,
    ProviderKind, Target,
};
use serde_json::{json, Value};

const EMBED_SMALL: &str = "BAAI/bge-small-en-v1.5";
const EMBED_E5: &str = "intfloat/multilingual-e5-small";
const RERANKER: &str = "mixedbread-ai/mxbai-rerank-xsmall-v1";

#[derive(Clone, Default)]
struct Infinity {
    /// `--api-key`, when the server was started with one
    api_key: Option<String>,
    /// how long to hold every answer back
    delay: Duration,
    /// answer every rerank with this body instead of a ranking
    raw_rerank: Option<&'static str>,
    /// every `(path, body)` the server was sent
    seen: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Infinity {
    /// the 401 a missing or wrong key earns, or `None` when the key is good
    fn refusal(&self, headers: &HeaderMap) -> Option<Response> {
        let Some(key) = &self.api_key else {
            return None;
        };
        let presented = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if presented == Some(key.as_str()) {
            return None;
        }
        Some(
            (
                StatusCode::UNAUTHORIZED,
                [("www-authenticate", "Bearer")],
                Json(json!({"detail": "Unauthorized"})),
            )
                .into_response(),
        )
    }
}

/// `errors.OpenAIException(...).json()`
fn openai_error(status: StatusCode, message: String) -> Response {
    (
        status,
        Json(json!({"error": {
            "message": message,
            "type": "invalid_request_error",
            "param": null,
            "code": status.as_u16(),
        }})),
    )
        .into_response()
}

/// `_resolve_engine` plus the capability check each endpoint makes: the 400 an
/// unknown model or the wrong task earns, or `None` to serve it
fn engine_refusal(model: &str, capability: &str) -> Option<Response> {
    let capabilities = match model {
        EMBED_SMALL | EMBED_E5 => "embed",
        RERANKER => "rerank",
        other => {
            return Some(openai_error(
                StatusCode::BAD_REQUEST,
                format!("Invalid model: Engine for model name `{other}` not found."),
            ))
        }
    };
    if capabilities != capability {
        return Some(openai_error(
            StatusCode::BAD_REQUEST,
            format!("ModelNotDeployedError: model=`{model}` does not support `{capability}`."),
        ));
    }
    None
}

fn unprocessable(loc: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({"detail": [{"type": "string_type", "loc": ["body", loc], "msg": "Input should be a valid string"}]})),
    )
        .into_response()
}

fn words(text: &str) -> u64 {
    text.split_whitespace().count().max(1) as u64
}

async fn embeddings(
    State(up): State<Infinity>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    tokio::time::sleep(up.delay).await;
    up.seen.lock().push(("embeddings".into(), body.clone()));
    if let Some(refused) = up.refusal(&headers) {
        return refused;
    }
    let model = body["model"].as_str().unwrap_or("default/not-specified");
    if let Some(refused) = engine_refusal(model, "embed") {
        return refused;
    }
    let inputs: Vec<String> = match &body["input"] {
        Value::String(text) => vec![text.clone()],
        Value::Array(items) if items.iter().all(Value::is_string) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => return unprocessable("input"),
    };
    // a vector per model, so a test can tell which engine answered
    let first = if model == EMBED_E5 { 0.5 } else { 0.25 };
    let data: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(index, _)| json!({"object": "embedding", "embedding": [first, index as f64], "index": index}))
        .collect();
    let tokens: u64 = inputs.iter().map(|text| words(text)).sum();
    Json(json!({
        "object": "list",
        "data": data,
        "model": model,
        "usage": {"prompt_tokens": tokens, "total_tokens": tokens},
        "id": "infinity-embed",
        "created": 1_700_000_000,
    }))
    .into_response()
}

async fn rerank(
    State(up): State<Infinity>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    tokio::time::sleep(up.delay).await;
    up.seen.lock().push(("rerank".into(), body.clone()));
    if let Some(refused) = up.refusal(&headers) {
        return refused;
    }
    if let Some(raw) = up.raw_rerank {
        return (StatusCode::OK, raw).into_response();
    }
    let model = body["model"].as_str().unwrap_or("default/not-specified");
    if let Some(refused) = engine_refusal(model, "rerank") {
        return refused;
    }
    let Some(query) = body["query"].as_str() else {
        return unprocessable("query");
    };
    let Some(documents) = body["documents"].as_array() else {
        return unprocessable("documents");
    };
    let Some(documents) = documents
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<&str>>>()
    else {
        return unprocessable("documents");
    };
    // the share of the query's words a document contains, so the order is known
    let query_words: Vec<&str> = query.split_whitespace().collect();
    let mut ranked: Vec<(usize, f64)> = documents
        .iter()
        .enumerate()
        .map(|(index, doc)| {
            let hits = query_words.iter().filter(|w| doc.contains(**w)).count();
            (index, hits as f64 / query_words.len().max(1) as f64)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    if let Some(top_n) = body["top_n"].as_u64() {
        ranked.truncate(top_n as usize);
    }
    let return_documents = body["return_documents"].as_bool().unwrap_or(false);
    let results: Vec<Value> = ranked
        .iter()
        .map(|&(index, score)| {
            if return_documents {
                json!({"relevance_score": score, "index": index, "document": documents[index]})
            } else {
                json!({"relevance_score": score, "index": index})
            }
        })
        .collect();
    let tokens: u64 = words(query) + documents.iter().map(|doc| words(doc)).sum::<u64>();
    Json(json!({
        "object": "rerank",
        "results": results,
        "model": model,
        "usage": {"prompt_tokens": tokens, "total_tokens": tokens},
        "id": "infinity-rerank",
        "created": 1_700_000_000,
    }))
    .into_response()
}

async fn models(State(up): State<Infinity>, headers: HeaderMap) -> Response {
    if let Some(refused) = up.refusal(&headers) {
        return refused;
    }
    Json(json!({"object": "list", "data": [
        {"id": EMBED_SMALL, "object": "model", "capabilities": ["embed"]},
        {"id": EMBED_E5, "object": "model", "capabilities": ["embed"]},
        {"id": RERANKER, "object": "model", "capabilities": ["rerank"]},
    ]}))
    .into_response()
}

/// Start the fake under `url_prefix` (`""` is Infinity's default).
async fn infinity(up: Infinity, url_prefix: &str) -> SocketAddr {
    let app = Router::new()
        .route(&format!("{url_prefix}/embeddings"), post(embeddings))
        .route(&format!("{url_prefix}/rerank"), post(rerank))
        .route(&format!("{url_prefix}/models"), get(models))
        .route("/health", get(|| async { Json(json!({"unix": 1.0})) }))
        .with_state(up);
    serve(app).await
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

fn route(model: &str, upstream_model: &str, advanced: AdvancedModelConfig) -> ModelRoute {
    ModelRoute {
        model: model.into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "infinity-local".into(),
            model: Some(upstream_model.into()),
            ..Default::default()
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced,
        cache: None,
        variants: Default::default(),
        tenancy: None,
    }
}

/// One Infinity provider at `api_base`, and a route per model it serves.
fn config(api_base: String, api_key: Option<&str>, advanced: AdvancedModelConfig) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "infinity-local".into(),
        kind: ProviderKind::Infinity,
        api_base,
        api_key: api_key.map(str::to_string),
        ..Default::default()
    });
    config
        .routes
        .push(route("embed-small", EMBED_SMALL, advanced.clone()));
    config
        .routes
        .push(route("embed-e5", EMBED_E5, advanced.clone()));
    config
        .routes
        .push(route("rerank", RERANKER, advanced.clone()));
    config
        .routes
        .push(route("missing", "BAAI/not-served", advanced));
    config
}

async fn gateway(config: &GatewayConfig) -> SocketAddr {
    serve(rolter_gateway::build_router_from_config(config)).await
}

async fn post_json(gateway: SocketAddr, path: &str, body: Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gateway}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn embeddings_and_rerank_reach_their_own_models_on_one_server() {
    let up = Infinity::default();
    let addr = infinity(up.clone(), "").await;
    let gw = gateway(&config(format!("http://{addr}"), None, Default::default())).await;

    // two embedding models from the one process, each with its own vector
    for (route, upstream_model, first) in [
        ("embed-small", EMBED_SMALL, 0.25),
        ("embed-e5", EMBED_E5, 0.5),
    ] {
        let response = post_json(
            gw,
            "/v1/embeddings",
            json!({"model": route, "input": ["hello world", "bye"], "encoding_format": "float"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-rolter-provider"], "infinity-local");
        assert_eq!(response.headers()["x-rolter-target"], upstream_model);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["object"], "list");
        assert_eq!(body["model"], upstream_model);
        assert_eq!(body["data"][0]["embedding"], json!([first, 0.0]));
        assert_eq!(body["data"][1]["index"], 1);
        assert_eq!(body["usage"]["prompt_tokens"], 3);
    }

    let response = post_json(
        gw,
        "/v1/rerank",
        json!({
            "model": "rerank",
            "query": "fast rust gateway",
            "documents": ["a python script", {"text": "a fast rust gateway"}, "rust"],
            "top_n": 2,
            "return_documents": true,
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-rolter-target"], RERANKER);
    let body: Value = response.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2, "top_n reached the upstream: {body}");
    assert_eq!(results[0]["index"], 1);
    assert_eq!(results[0]["relevance_score"], 1.0);
    // infinity's bare string comes back as cohere/jina's object
    assert_eq!(
        results[0]["document"],
        json!({"text": "a fast rust gateway"})
    );
    assert_eq!(results[1]["document"], json!({"text": "rust"}));
    // 3 query words plus 3 + 4 + 1 document words
    assert_eq!(body["usage"]["prompt_tokens"], 11);

    let seen = up.seen.lock().clone();
    let paths: Vec<&str> = seen.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(paths, ["embeddings", "embeddings", "rerank"]);
    // the route's upstream model went up, and the object document as its text
    assert_eq!(seen[0].1["model"], EMBED_SMALL);
    assert_eq!(seen[1].1["model"], EMBED_E5);
    assert_eq!(seen[2].1["model"], RERANKER);
    assert_eq!(
        seen[2].1["documents"],
        json!(["a python script", "a fast rust gateway", "rust"])
    );
}

#[tokio::test]
async fn a_rerank_without_documents_returned_omits_them() {
    let addr = infinity(Infinity::default(), "").await;
    let gw = gateway(&config(format!("http://{addr}"), None, Default::default())).await;
    let response = post_json(
        gw,
        "/v1/rerank",
        json!({"model": "rerank", "query": "rust", "documents": ["go", "rust"]}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["results"][0]["index"], 1);
    assert!(body["results"][0].get("document").is_none(), "{body}");
}

/// an operator who started infinity with `--url-prefix /v1` puts it in the base
#[tokio::test]
async fn a_url_prefix_travels_in_the_base() {
    let up = Infinity::default();
    let addr = infinity(up.clone(), "/v1").await;
    let gw = gateway(&config(
        format!("http://{addr}/v1/"),
        None,
        Default::default(),
    ))
    .await;
    let embedded = post_json(
        gw,
        "/v1/embeddings",
        json!({"model": "embed-small", "input": "x"}),
    )
    .await;
    assert_eq!(embedded.status(), StatusCode::OK);
    let ranked = post_json(
        gw,
        "/v1/rerank",
        json!({"model": "rerank", "query": "x", "documents": ["x"]}),
    )
    .await;
    assert_eq!(ranked.status(), StatusCode::OK);
    assert_eq!(up.seen.lock().len(), 2);
}

#[tokio::test]
async fn the_key_travels_as_a_bearer_token_and_a_refusal_reaches_the_caller() {
    let up = Infinity {
        api_key: Some("inf-secret".into()),
        ..Default::default()
    };
    let addr = infinity(up, "").await;

    let keyed = gateway(&config(
        format!("http://{addr}"),
        Some("inf-secret"),
        Default::default(),
    ))
    .await;
    let response = post_json(
        keyed,
        "/v1/embeddings",
        json!({"model": "embed-small", "input": "x"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = post_json(
        keyed,
        "/v1/rerank",
        json!({"model": "rerank", "query": "x", "documents": ["x"]}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // a provider configured without the key the server wants: infinity's 401
    // is handed on rather than turned into a gateway error
    let keyless = gateway(&config(format!("http://{addr}"), None, Default::default())).await;
    let response = post_json(
        keyless,
        "/v1/embeddings",
        json!({"model": "embed-small", "input": "x"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"], "Unauthorized");
}

#[tokio::test]
async fn upstream_errors_keep_their_status_and_body() {
    let addr = infinity(Infinity::default(), "").await;
    let gw = gateway(&config(format!("http://{addr}"), None, Default::default())).await;

    // a model the server does not run
    let response = post_json(
        gw,
        "/v1/embeddings",
        json!({"model": "missing", "input": "x"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Invalid model"),
        "{body}"
    );

    // an embedding model asked to rerank; the error body is not reshaped
    let mut cfg = config(format!("http://{addr}"), None, Default::default());
    cfg.routes
        .push(route("wrong-task", EMBED_SMALL, Default::default()));
    let gw = gateway(&cfg).await;
    let response = post_json(
        gw,
        "/v1/rerank",
        json!({"model": "wrong-task", "query": "x", "documents": ["x"]}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ModelNotDeployedError"),
        "{body}"
    );

    // token ids are openai's but not infinity's: its validation 422 is handed on
    let response = post_json(
        gw,
        "/v1/embeddings",
        json!({"model": "embed-small", "input": [1, 2, 3]}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"][0]["loc"], json!(["body", "input"]));
}

#[tokio::test]
async fn a_document_with_no_text_is_refused_before_the_upstream() {
    let up = Infinity::default();
    let addr = infinity(up.clone(), "").await;
    let gw = gateway(&config(format!("http://{addr}"), None, Default::default())).await;
    let response = post_json(
        gw,
        "/v1/rerank",
        json!({"model": "rerank", "query": "x", "documents": ["x", {"title": "no text"}]}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unsupported_content_part", "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("documents[1]"),
        "{body}"
    );
    assert!(up.seen.lock().is_empty(), "nothing was forwarded");
}

#[tokio::test]
async fn a_malformed_answer_is_handed_on_unchanged() {
    let up = Infinity {
        raw_rerank: Some("not json at all"),
        ..Default::default()
    };
    let addr = infinity(up, "").await;
    let gw = gateway(&config(format!("http://{addr}"), None, Default::default())).await;
    let response = post_json(
        gw,
        "/v1/rerank",
        json!({"model": "rerank", "query": "x", "documents": ["x"]}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), "not json at all");
}

#[tokio::test]
async fn a_slow_server_is_cut_short_by_the_route_timeout() {
    let up = Infinity {
        delay: Duration::from_secs(4),
        ..Default::default()
    };
    let addr = infinity(up, "").await;
    let mut advanced = AdvancedModelConfig::default();
    advanced.limits.timeout_secs = Some(1);
    advanced.limits.retries = Some(0);
    let gw = gateway(&config(format!("http://{addr}"), None, advanced)).await;

    for (path, body) in [
        (
            "/v1/embeddings",
            json!({"model": "embed-small", "input": "x"}),
        ),
        (
            "/v1/rerank",
            json!({"model": "rerank", "query": "x", "documents": ["x"]}),
        ),
    ] {
        let started = Instant::now();
        let response = post_json(gw, path, body).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "{path}");
        let body: Value = response.json().await.unwrap();
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("timed out after 1s"),
            "{path}: {body}"
        );
        assert!(started.elapsed() < Duration::from_secs(3), "{path}");
    }
}
