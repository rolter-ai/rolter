//! HTTP regression tests for semantic-cache compatibility (#1476).
//!
//! The gateway is served in-process against a stub upstream that answers chat,
//! Anthropic messages and embeddings, and the response cache is the in-memory
//! test backend, so these run anywhere without Redis or a paid model. The stub
//! gives *every* input the same embedding: similarity alone would match any
//! two requests, so each miss below is the partition or the bypass at work,
//! and each hit is proof the partition does not over-split.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, RouteCache,
    SemanticCacheConfig, Target,
};
use serde_json::{json, Value};

use crate::cache::{CachedResponse, ResponseCache};
use crate::state::AppState;

#[derive(Default)]
struct Calls {
    completions: AtomicUsize,
    embeddings: AtomicUsize,
}

impl Calls {
    fn completions(&self) -> usize {
        self.completions.load(Ordering::SeqCst)
    }

    fn embeddings(&self) -> usize {
        self.embeddings.load(Ordering::SeqCst)
    }
}

/// Numbers every generated answer, so a replay is recognisable by its number.
async fn openai_chat(
    State(calls): State<Arc<Calls>>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let n = calls.completions.fetch_add(1, Ordering::SeqCst) + 1;
    let answer = format!("answer-{n}");
    if body["stream"] == json!(true) {
        let sse = format!(
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{answer}\"}}}}]}}\n\ndata: [DONE]\n\n"
        );
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            sse,
        )
            .into_response()
    } else {
        Json(json!({
            "id": format!("chatcmpl-{n}"),
            "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": answer}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }))
        .into_response()
    }
}

async fn anthropic_messages(State(calls): State<Arc<Calls>>) -> Json<Value> {
    let n = calls.completions.fetch_add(1, Ordering::SeqCst) + 1;
    Json(json!({
        "id": format!("msg_{n}"),
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": format!("answer-{n}")}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1, "output_tokens": 1}
    }))
}

async fn embeddings(State(calls): State<Arc<Calls>>) -> Json<Value> {
    calls.embeddings.fetch_add(1, Ordering::SeqCst);
    Json(json!({"object": "list", "data": [{"index": 0, "embedding": [1.0, 0.0, 0.0]}]}))
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

struct Harness {
    gateway: SocketAddr,
    calls: Arc<Calls>,
    cache: ResponseCache,
    client: reqwest::Client,
}

impl Harness {
    /// A gateway with an OpenAI-compatible route `chat` and an Anthropic route
    /// `claude`, both semantically cached with embeddings from the stub.
    async fn start() -> Self {
        let calls = Arc::new(Calls::default());
        let upstream = serve(
            Router::new()
                .route("/v1/chat/completions", post(openai_chat))
                .route("/v1/messages", post(anthropic_messages))
                .route("/v1/embeddings", post(embeddings))
                .with_state(calls.clone()),
        )
        .await;

        let mut config = GatewayConfig::default();
        config.cache.enabled = true;
        for (name, kind) in [
            ("openai", ProviderKind::OpenaiCompatible),
            ("anthropic", ProviderKind::Anthropic),
        ] {
            config.providers.push(ProviderConfig {
                name: name.to_string(),
                kind,
                api_base: format!("http://{upstream}"),
                ..Default::default()
            });
        }
        for (route, provider) in [("chat", "openai"), ("claude", "anthropic")] {
            config.routes.push(ModelRoute {
                model: route.to_string(),
                strategy: BalancingStrategy::RoundRobin,
                targets: vec![Target {
                    provider: provider.to_string(),
                    model: None,
                    weight: 1,
                }],
                params: Default::default(),
                param_policy: Default::default(),
                advanced: Default::default(),
                cache: Some(RouteCache {
                    enabled: true,
                    ttl_secs: Some(600),
                    per_key: false,
                    semantic: Some(SemanticCacheConfig {
                        provider: "openai".to_string(),
                        model: "embed".to_string(),
                        threshold: 0.9,
                        max_candidates: 16,
                    }),
                }),
                variants: Default::default(),
                tenancy: None,
            });
        }

        let mut state = AppState::with_logging(&config, None);
        let cache = ResponseCache::in_memory();
        state.response_cache = cache.clone();
        let gateway = serve(crate::build_router(
            state,
            &config.server.metrics_path,
            config.server.max_body_bytes,
        ))
        .await;
        Self {
            gateway,
            calls,
            cache,
            client: reqwest::Client::new(),
        }
    }

    async fn send(&self, path: &str, body: Value) -> Reply {
        let response = self
            .client
            .post(format!("http://{}{path}", self.gateway))
            .json(&body)
            .send()
            .await
            .unwrap();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        let cache = header("x-rolter-cache");
        let content_type = header("content-type");
        let status = response.status().as_u16();
        let body = response.text().await.unwrap();
        assert_eq!(status, 200, "{path} answered {status}: {body}");
        Reply {
            cache,
            content_type,
            body,
        }
    }

    async fn chat(&self, body: Value) -> Reply {
        self.send("/v1/chat/completions", body).await
    }
}

#[derive(Debug)]
struct Reply {
    cache: String,
    content_type: String,
    body: String,
}

impl Reply {
    fn is_hit(&self) -> bool {
        self.cache == "HIT"
    }

    fn is_sse(&self) -> bool {
        self.content_type.starts_with("text/event-stream")
    }
}

fn user(text: &str) -> Value {
    json!([{"role": "user", "content": text}])
}

#[tokio::test]
async fn a_compatible_request_with_different_words_is_a_semantic_hit() {
    let h = Harness::start().await;
    let first = h
        .chat(json!({"model": "chat", "temperature": 0, "messages": user("what is the capital of France")}))
        .await;
    assert!(!first.is_hit());
    // different text, so the exact cache cannot answer: only a semantic hit can
    let second = h
        .chat(
            json!({"model": "chat", "temperature": 0, "user": "someone-else",
            "messages": user("tell me the capital of France")}),
        )
        .await;
    assert!(second.is_hit(), "{second:?}");
    assert!(second.body.contains("answer-1"));
    assert!(!second.is_sse());
    assert_eq!(h.calls.completions(), 1);
}

#[tokio::test]
async fn a_streamed_entry_never_answers_a_json_request_or_the_reverse() {
    let h = Harness::start().await;
    let streamed = h
        .chat(json!({"model": "chat", "stream": true, "messages": user("hello")}))
        .await;
    assert!(!streamed.is_hit());
    assert!(streamed.is_sse());

    // the reproduction from #1476: same messages, stream=false
    let json_reply = h
        .chat(json!({"model": "chat", "stream": false, "messages": user("hello")}))
        .await;
    assert!(!json_reply.is_hit(), "{json_reply:?}");
    assert!(!json_reply.is_sse());
    assert!(json_reply.content_type.starts_with("application/json"));
    assert!(serde_json::from_str::<Value>(&json_reply.body).is_ok());
    assert_eq!(h.calls.completions(), 2);

    // and each mode still hits its own entry
    let streamed_again = h
        .chat(json!({"model": "chat", "stream": true, "messages": user("hello there")}))
        .await;
    assert!(streamed_again.is_hit() && streamed_again.is_sse());
    assert!(streamed_again.body.contains("answer-1"));
    let json_again = h
        .chat(json!({"model": "chat", "messages": user("hello there")}))
        .await;
    assert!(json_again.is_hit() && !json_again.is_sse());
    assert!(json_again.body.contains("answer-2"));
    assert_eq!(h.calls.completions(), 2);
}

#[tokio::test]
async fn anthropic_system_instructions_are_part_of_the_match() {
    let h = Harness::start().await;
    let ask = |system: &str, text: &str| json!({"model": "claude", "max_tokens": 64, "system": system, "messages": user(text)});
    let german = h
        .send("/v1/messages", ask("Answer in German", "hello"))
        .await;
    assert!(!german.is_hit());
    let japanese = h
        .send("/v1/messages", ask("Answer in Japanese", "hello"))
        .await;
    assert!(!japanese.is_hit(), "{japanese:?}");
    assert!(japanese.body.contains("answer-2"));

    let german_again = h
        .send("/v1/messages", ask("Answer in German", "hello, friend"))
        .await;
    assert!(german_again.is_hit(), "{german_again:?}");
    assert!(german_again.body.contains("answer-1"));
    assert_eq!(h.calls.completions(), 2);
}

#[tokio::test]
async fn openai_system_instructions_are_part_of_the_match() {
    let h = Harness::start().await;
    let ask = |system: &str, text: &str| {
        json!({"model": "chat", "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": text}
        ]})
    };
    assert!(!h.chat(ask("Answer in German", "hello")).await.is_hit());
    assert!(!h.chat(ask("Answer in Japanese", "hello")).await.is_hit());
    assert!(h.chat(ask("Answer in Japanese", "hello!")).await.is_hit());
    assert_eq!(h.calls.completions(), 2);
}

#[tokio::test]
async fn different_tools_or_output_formats_never_share_an_answer() {
    let h = Harness::start().await;
    let with_tool = |name: &str| {
        json!({"model": "chat", "messages": user("handle the quarterly report"),
            "tools": [{"type": "function", "function": {"name": name, "parameters": {"type": "object"}}}],
            "tool_choice": "auto"})
    };
    assert!(!h.chat(with_tool("read_report")).await.is_hit());
    let other_tool = h.chat(with_tool("send_email")).await;
    assert!(!other_tool.is_hit(), "{other_tool:?}");

    let structured = |format: Value| json!({"model": "chat", "messages": user("handle the quarterly report"), "response_format": format});
    let schema = json!({"type": "json_schema", "json_schema": {"name": "report", "schema": {"type": "object"}}});
    assert!(!h.chat(structured(schema.clone())).await.is_hit());
    assert!(!h
        .chat(structured(json!({"type": "json_object"})))
        .await
        .is_hit());
    // no tools and no format at all is a partition of its own too
    assert!(!h
        .chat(json!({"model": "chat", "messages": user("handle the quarterly report")}))
        .await
        .is_hit());
    assert_eq!(h.calls.completions(), 5);

    // each of those still hits for a compatible request
    assert!(h
        .chat({
            let mut body = with_tool("send_email");
            body["messages"] = user("deal with the quarterly report");
            body
        })
        .await
        .is_hit());
    assert!(h
        .chat({
            let mut body = structured(schema);
            body["messages"] = user("deal with the quarterly report");
            body
        })
        .await
        .is_hit());
    assert_eq!(h.calls.completions(), 5);
}

#[tokio::test]
async fn non_text_inputs_bypass_semantic_lookup() {
    let h = Harness::start().await;
    let with_image = |url: &str| {
        json!({"model": "chat", "messages": [{"role": "user", "content": [
            {"type": "text", "text": "what is in this picture"},
            {"type": "image_url", "image_url": {"url": url}}
        ]}]})
    };
    assert!(!h
        .chat(with_image("data:image/png;base64,AAAA"))
        .await
        .is_hit());
    let other_image = h.chat(with_image("data:image/png;base64,BBBB")).await;
    assert!(!other_image.is_hit(), "{other_image:?}");

    let anthropic_image = |data: &str| {
        json!({"model": "claude", "max_tokens": 64, "messages": [{"role": "user", "content": [
            {"type": "text", "text": "what is in this picture"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}}
        ]}]})
    };
    assert!(!h
        .send("/v1/messages", anthropic_image("AAAA"))
        .await
        .is_hit());
    assert!(!h
        .send("/v1/messages", anthropic_image("BBBB"))
        .await
        .is_hit());

    assert_eq!(h.calls.completions(), 4);
    // bypassed before the embedding call, so no embedding spend either
    assert_eq!(h.calls.embeddings(), 0);

    // the exact cache still serves an identical multimodal request
    assert!(h
        .chat(with_image("data:image/png;base64,AAAA"))
        .await
        .is_hit());
    assert_eq!(h.calls.completions(), 4);
}

#[tokio::test]
async fn entries_written_under_the_old_layout_are_never_replayed() {
    let h = Harness::start().await;
    // the pre-#1476 index key: namespace, path and route, no partition
    let namespace = GatewayConfig::default().cache.namespace;
    let legacy_index = ResponseCache::make_key(
        &format!("{namespace}:semantic"),
        "/v1/chat/completions",
        "",
        b"chat",
    );
    let poisoned = CachedResponse {
        status: 200,
        content_type: "text/event-stream".to_string(),
        body: b"data: {\"choices\":[{\"delta\":{\"content\":\"stale\"}}]}\n\ndata: [DONE]\n\n"
            .to_vec(),
    };
    h.cache
        .semantic_put(
            &legacy_index,
            "legacy",
            vec![1.0, 0.0, 0.0],
            &poisoned,
            600,
            16,
        )
        .await;

    let reply = h
        .chat(json!({"model": "chat", "messages": user("hello")}))
        .await;
    assert!(!reply.is_hit(), "{reply:?}");
    assert!(!reply.body.contains("stale"));
    assert_eq!(h.calls.completions(), 1);
}
