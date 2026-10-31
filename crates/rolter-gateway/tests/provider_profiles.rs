//! Conformance fixtures for each distinct provider auth and path profile
//! (#3336).
//!
//! Each case replays a request and a response shaped after the vendor's own
//! documentation (the `doc` url of the case) through the gateway against a mock
//! upstream mounted at the vendor's documented base path, and checks the
//! three things a profile consists of: the path the upstream receives, the
//! header that carries the credential, and that the vendor's response reaches
//! the client with every field it sent, vendor extensions included. They prove
//! what rolter sends and reads, not that the vendor still accepts it; the
//! registry record of each kind says so (`conformance`).
//!
//! The kinds that share a profile (every hosted kind whose base ends in `/v1`,
//! say) are covered by the one case per profile; the walk-every-kind tests in
//! `rolter_core` and `rolter_proxy` prove each kind's own base and header.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::Request;
use axum::response::IntoResponse;
use axum::{Json, Router};
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// What the mock upstream saw.
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

/// How a case was grounded in the vendor's documentation.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// base, path, credential header and request read from the documentation
    /// page `doc` on the given date (`YYYY-MM-DD`). A page that prints no
    /// response example leaves the response the OpenAI chat completion shape
    /// its client examples read
    Fetched(&'static str),
    /// written from memory of the documentation; not checked against the page.
    /// A kind whose page cannot be read records its case honestly here, and the
    /// registry test in `rolter_core` refuses such a case as the only evidence
    /// of `provider-profile` (#3495)
    #[expect(
        dead_code,
        reason = "no case is recalled today; the variant stays so one can say so (#3488)"
    )]
    Recalled,
}

struct Profile {
    kind: ProviderKind,
    source: Source,
    /// the vendor documentation the request and response come from
    doc: &'static str,
    /// the documented base, relative to the host
    base: &'static str,
    /// the path the upstream must receive
    path: &'static str,
    /// the header that carries the credential, and the text before the key
    auth: (&'static str, &'static str),
    /// the model as the vendor's example names it
    model: &'static str,
    request: Value,
    response: Value,
}

/// The credential, built at run time so no source line reads as a key.
fn key() -> String {
    ["fixture", "credential", "0001"].join("-")
}

async fn replay(profile: &Profile) {
    let seen: Arc<Mutex<Option<Seen>>> = Arc::new(Mutex::new(None));
    let response = profile.response.clone();
    let upstream = serve(Router::new().fallback({
        let seen = seen.clone();
        move |request: Request| {
            let seen = seen.clone();
            let response = response.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                *seen.lock().await = Some(Seen {
                    path: parts.uri.path().to_string(),
                    headers: parts
                        .headers
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
                        .collect(),
                    body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
                });
                Json(response).into_response()
            }
        }
    }))
    .await;

    let mut config = GatewayConfig {
        providers: vec![ProviderConfig {
            name: "vendor".into(),
            kind: profile.kind,
            api_base: format!("http://{upstream}{}", profile.base),
            api_key: Some(key()),
            ..Default::default()
        }],
        ..Default::default()
    };
    config.routes.push(ModelRoute {
        model: "profile-chat".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "vendor".into(),
            model: Some(profile.model.into()),
            ..Default::default()
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    let gateway = serve(rolter_gateway::build_router_from_config(&config)).await;

    // the client names the route; the vendor's documented body is otherwise
    // sent as is
    let mut request = profile.request.clone();
    request["model"] = json!("profile-chat");
    let answered = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&request)
        .send()
        .await
        .unwrap();
    let kind = (profile.kind, profile.source);
    if let Source::Fetched(date) = profile.source {
        let shaped = date.len() == 10
            && date.char_indices().all(|(i, c)| {
                if i == 4 || i == 7 {
                    c == '-'
                } else {
                    c.is_ascii_digit()
                }
            });
        assert!(shaped, "{kind:?}: fetched on '{date}', not YYYY-MM-DD");
    }
    assert_eq!(answered.status(), 200, "{kind:?}");
    let body: Value = answered.json().await.unwrap();
    assert_eq!(
        body, profile.response,
        "{kind:?}: the response is forwarded whole"
    );

    let seen = seen.lock().await.take().expect("the upstream was called");
    assert_eq!(
        seen.path,
        format!("{}{}", profile.base, profile.path),
        "{kind:?} ({})",
        profile.doc
    );
    let header = |name: &str| {
        seen.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let (name, prefix) = profile.auth;
    assert_eq!(
        header(name),
        Some(format!("{prefix}{}", key()).as_str()),
        "{kind:?}: the credential travels in `{name}`"
    );
    for other in ["authorization", "api-key", "x-api-key", "x-goog-api-key"] {
        if other != name {
            assert!(
                header(other).is_none(),
                "{kind:?}: the credential must not also travel in `{other}`"
            );
        }
    }
    assert_eq!(
        seen.body["model"], profile.model,
        "{kind:?}: model rewritten"
    );
    let mut sent = seen.body.clone();
    sent["model"] = profile.request["model"].clone();
    assert_eq!(
        sent, profile.request,
        "{kind:?}: the documented body is forwarded"
    );
}

fn messages() -> Value {
    json!([{"role": "user", "content": "Hello"}])
}

#[tokio::test]
async fn azure_openai_v1_authenticates_with_the_api_key_header() {
    replay(&Profile {
        kind: ProviderKind::AzureOpenai,
        source: Source::Fetched("2026-10-29"),
        doc: "https://learn.microsoft.com/en-us/azure/ai-foundry/openai/api-version-lifecycle",
        base: "/openai/v1",
        path: "/chat/completions",
        auth: ("api-key", ""),
        model: "gpt-4.1-nano",
        request: json!({
            "model": "gpt-4.1-nano",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Explain what the bitter lesson is?"}
            ]
        }),
        // Azure adds the content-filter annotations beside the OpenAI fields
        response: json!({
            "id": "chatcmpl-AzureExample",
            "object": "chat.completion",
            "created": 1757000000,
            "model": "gpt-4.1-nano-2025-04-14",
            "prompt_filter_results": [{"prompt_index": 0, "content_filter_results": {
                "hate": {"filtered": false, "severity": "safe"},
                "violence": {"filtered": false, "severity": "safe"}
            }}],
            "choices": [{
                "index": 0,
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": "The bitter lesson is..."},
                "content_filter_results": {"hate": {"filtered": false, "severity": "safe"}}
            }],
            "usage": {"prompt_tokens": 24, "completion_tokens": 9, "total_tokens": 33}
        }),
    })
    .await;
}

#[tokio::test]
async fn bedrock_runtime_authenticates_with_a_bearer_api_key_under_openai_v1() {
    replay(&Profile {
        kind: ProviderKind::Bedrock,
        source: Source::Fetched("2026-10-29"),
        doc: "https://docs.aws.amazon.com/bedrock/latest/userguide/inference-chat-completions.html",
        base: "/openai/v1",
        path: "/chat/completions",
        auth: ("authorization", "Bearer "),
        model: "openai.gpt-oss-120b-1:0",
        request: json!({"model": "openai.gpt-oss-120b-1:0", "messages": messages()}),
        response: json!({
            "id": "chatcmpl-BedrockExample",
            "object": "chat.completion",
            "created": 1757000000,
            "model": "openai.gpt-oss-120b-1:0",
            "choices": [{
                "index": 0,
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": "Hello! How can I help you today?"}
            }],
            "usage": {"prompt_tokens": 71, "completion_tokens": 14, "total_tokens": 85}
        }),
    })
    .await;
}

#[tokio::test]
async fn vertex_takes_an_oauth_access_token_as_a_bearer_under_the_project_endpoint() {
    replay(&Profile {
        kind: ProviderKind::Vertex,
        source: Source::Fetched("2026-10-29"),
        doc: "https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/migrate/openai/examples",
        base: "/v1/projects/PROJECT/locations/global/endpoints/openapi",
        path: "/chat/completions",
        // the access token (`gcloud auth print-access-token`) is the key
        auth: ("authorization", "Bearer "),
        model: "google/gemini-3.8-flash",
        request: json!({"model": "google/gemini-3.8-flash", "messages": messages()}),
        response: json!({
            "choices": [{
                "finish_reason": "stop",
                "index": 0,
                "message": {"content": "Hello! How can I help you?", "role": "assistant"}
            }],
            "created": 1757000000,
            "id": "VertexExampleId",
            "model": "google/gemini-3.8-flash",
            "object": "chat.completion",
            "usage": {"completion_tokens": 8, "prompt_tokens": 2, "total_tokens": 10}
        }),
    })
    .await;
}

#[tokio::test]
async fn gigachat_takes_an_oauth_access_token_as_a_bearer() {
    replay(&Profile {
        kind: ProviderKind::Gigachat,
        // the OpenAPI file the reference pages are built from: `servers` is
        // `https://api.giga.chat/v1`, chat completions is `POST /v1/chat/completions`
        // on `https://api.giga.chat`, and every request but the token's carries the
        // access token as a bearer (#3488)
        source: Source::Fetched("2026-10-29"),
        doc: "https://developers.sber.ru/docs/files/openapi/gigachat/api.yml",
        base: "/v1",
        path: "/chat/completions",
        // the 30-minute access token from the OAuth endpoint is the key
        auth: ("authorization", "Bearer "),
        model: "GigaChat",
        request: json!({
            "model": "GigaChat",
            "messages": [
                {"role": "system", "content": "Ты — профессиональный переводчик на английский язык. Переведи точно сообщение пользователя."},
                {"role": "user", "content": "GigaChat — это сервис, который умеет взаимодействовать с пользователем в формате диалога, писать код, создавать тексты и картинки по запросу пользователя."}
            ],
            "stream": false,
            "update_interval": 0
        }),
        response: json!({
            "choices": [{
                "message": {
                    "content": "GigaChat is a service capable of interacting with the user in a dialogue format, writing code, and creating texts and images upon user's request.",
                    "role": "assistant"
                },
                "index": 0,
                "finish_reason": "stop"
            }],
            "created": 1760434636,
            "model": "GigaChat:2.0.28.2",
            "object": "chat.completion",
            "usage": {"prompt_tokens": 55, "completion_tokens": 30, "total_tokens": 85, "precached_prompt_tokens": 4}
        }),
    })
    .await;
}

#[tokio::test]
async fn gemini_openai_shim_lives_under_v1beta_openai() {
    replay(&Profile {
        kind: ProviderKind::Gemini,
        source: Source::Fetched("2026-10-29"),
        doc: "https://ai.google.dev/gemini-api/docs/openai",
        base: "/v1beta/openai",
        path: "/chat/completions",
        auth: ("authorization", "Bearer "),
        model: "gemini-3.8-flash",
        request: json!({
            "model": "gemini-3.8-flash",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Explain to me how AI works"}
            ]
        }),
        response: json!({
            "choices": [{
                "finish_reason": "stop",
                "index": 0,
                "message": {"content": "AI works by...", "role": "assistant"}
            }],
            "created": 1757000000,
            "model": "gemini-3.8-flash",
            "object": "chat.completion",
            "usage": {"completion_tokens": 12, "prompt_tokens": 14, "total_tokens": 26}
        }),
    })
    .await;
}

#[tokio::test]
async fn groq_keeps_the_openai_prefix_of_its_base() {
    replay(&Profile {
        kind: ProviderKind::Groq,
        source: Source::Fetched("2026-10-29"),
        doc: "https://console.groq.com/docs/openai",
        base: "/openai/v1",
        path: "/chat/completions",
        auth: ("authorization", "Bearer "),
        model: "llama-3.3-70b-versatile",
        request: json!({"model": "llama-3.3-70b-versatile", "messages": messages()}),
        // Groq adds its own timing block and request id
        response: json!({
            "id": "chatcmpl-GroqExample",
            "object": "chat.completion",
            "created": 1757000000,
            "model": "llama-3.3-70b-versatile",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hello! How can I assist you today?"},
                "logprobs": null,
                "finish_reason": "stop"
            }],
            "usage": {
                "queue_time": 0.05,
                "prompt_tokens": 36,
                "prompt_time": 0.002,
                "completion_tokens": 9,
                "completion_time": 0.02,
                "total_tokens": 45,
                "total_time": 0.022
            },
            "system_fingerprint": "fp_example",
            "x_groq": {"id": "req_example"}
        }),
    })
    .await;
}

#[tokio::test]
async fn mistral_base_carries_v1_and_the_gateway_prefix_is_not_doubled() {
    replay(&Profile {
        kind: ProviderKind::Mistral,
        source: Source::Fetched("2026-10-29"),
        doc: "https://docs.mistral.ai/api/",
        base: "/v1",
        path: "/chat/completions",
        auth: ("authorization", "Bearer "),
        model: "mistral-large-latest",
        request: json!({"model": "mistral-large-latest", "messages": messages()}),
        response: json!({
            "id": "cmpl-e5cc70bb28c444948073e77776eb30ef",
            "object": "chat.completion",
            "model": "mistral-large-latest",
            "usage": {"prompt_tokens": 16, "completion_tokens": 34, "total_tokens": 50},
            "created": 1702256327,
            "choices": [{
                "index": 0,
                "message": {"content": "Hello!", "tool_calls": null, "role": "assistant"},
                "finish_reason": "stop"
            }]
        }),
    })
    .await;
}

#[tokio::test]
async fn deepseek_base_has_no_version_segment() {
    replay(&Profile {
        kind: ProviderKind::Deepseek,
        source: Source::Fetched("2026-10-29"),
        doc: "https://api-docs.deepseek.com/",
        base: "",
        path: "/chat/completions",
        auth: ("authorization", "Bearer "),
        model: "deepseek-chat",
        request: json!({
            "model": "deepseek-chat",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Hello!"}
            ],
            "stream": false
        }),
        // DeepSeek reports its context-cache hits beside the OpenAI counters
        response: json!({
            "id": "930c60df-bf64-41c9-a88e-3ec75f81e00e",
            "object": "chat.completion",
            "created": 1705651092,
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hello! How can I help you today?"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 9,
                "total_tokens": 19,
                "prompt_cache_hit_tokens": 0,
                "prompt_cache_miss_tokens": 10
            },
            "system_fingerprint": "fp_example"
        }),
    })
    .await;
}
