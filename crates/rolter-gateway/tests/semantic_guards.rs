//! Scoped semantic guard policies end to end (#459).
//!
//! A stand-in guardrail provider judges by the text it is shown: `attack`
//! is blocked, `secret` is rewritten to `[redacted]`, and `violent` scores the
//! `violence` category at 0.95. The upstream answers with the user's own
//! prompt, so the response stage sees the same words, on the OpenAI chat and
//! the Anthropic messages dialects, streamed and not.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::Arc;

use axum::body::Body;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, FailureMode, GatewayConfig, GuardrailPolicy, GuardrailWebhookConfig,
    ModelRoute, PolicyScope, ProviderConfig, ProviderKind, StreamingStrategy, Target,
    ViolationAction, VirtualKeyRecord, WebhookStage,
};
use serde_json::{json, Value};

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// What the upstream received, and how often it was called.
#[derive(Clone, Default)]
struct Upstream {
    bodies: Arc<Mutex<Vec<Value>>>,
    calls: Arc<AtomicUsize>,
}

/// The last user text of an OpenAI or Anthropic request.
fn prompt_of(body: &Value) -> String {
    let message = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The value of one text field of a multipart body.
fn form_field(body: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\"\r\n\r\n");
    body.split_once(&marker)
        .and_then(|(_, rest)| rest.split_once("\r\n"))
        .map(|(value, _)| value.to_string())
        .unwrap_or_default()
}

fn sse(events: Vec<String>) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(events.concat()))
        .unwrap()
}

impl Upstream {
    async fn serve(&self) -> SocketAddr {
        let chat = self.clone();
        let messages = self.clone();
        serve(
            Router::new()
                .route(
                    "/v1/chat/completions",
                    post(move |Json(body): Json<Value>| {
                        let upstream = chat.clone();
                        async move {
                            upstream.calls.fetch_add(1, SeqCst);
                            upstream.bodies.lock().push(body.clone());
                            let echo = prompt_of(&body);
                            if body["stream"] == true {
                                return sse(vec![
                                    format!(
                                        "data: {}\n\n",
                                        json!({"id":"c","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":echo}}]})
                                    ),
                                    format!(
                                        "data: {}\n\n",
                                        json!({"id":"c","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})
                                    ),
                                    "data: [DONE]\n\n".to_string(),
                                ]);
                            }
                            Json(json!({
                                "id": "c", "object": "chat.completion",
                                "choices": [{"index": 0, "message": {"role": "assistant", "content": echo}, "finish_reason": "stop"}],
                                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                            }))
                            .into_response()
                        }
                    }),
                )
                .route(
                    "/v1/messages",
                    post(move |Json(body): Json<Value>| {
                        let upstream = messages.clone();
                        async move {
                            upstream.calls.fetch_add(1, SeqCst);
                            upstream.bodies.lock().push(body.clone());
                            let echo = prompt_of(&body);
                            if body["stream"] == true {
                                let event = |name: &str, data: Value| {
                                    format!("event: {name}\ndata: {data}\n\n")
                                };
                                return sse(vec![
                                    event("message_start", json!({"type":"message_start","message":{"id":"m","type":"message","role":"assistant","content":[],"model":"claude","usage":{"input_tokens":1,"output_tokens":0}}})),
                                    event("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
                                    event("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":echo}})),
                                    event("content_block_stop", json!({"type":"content_block_stop","index":0})),
                                    event("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}})),
                                    event("message_stop", json!({"type":"message_stop"})),
                                ]);
                            }
                            Json(json!({
                                "id": "m", "type": "message", "role": "assistant", "model": "claude",
                                "content": [{"type": "text", "text": echo}], "stop_reason": "end_turn",
                                "usage": {"input_tokens": 1, "output_tokens": 1}
                            }))
                            .into_response()
                        }
                    }),
                ),
        )
        .await
    }

    /// An upstream for `/v1/audio/transcriptions` and `/v1/audio/translations`
    /// that "transcribes" the request's `prompt` field, as plain text when the
    /// caller asked for `response_format=text` and as `verbose_json` otherwise.
    async fn serve_audio(&self) -> SocketAddr {
        let transcribe = |upstream: Upstream| {
            post(move |body: String| {
                let upstream = upstream.clone();
                async move {
                    upstream.calls.fetch_add(1, SeqCst);
                    let transcript = form_field(&body, "prompt");
                    if form_field(&body, "response_format") == "text" {
                        return ([(header::CONTENT_TYPE, "text/plain")], transcript)
                            .into_response();
                    }
                    Json(json!({
                        "task": "transcribe", "duration": 60.0, "text": transcript,
                        "segments": [{"id": 0, "start": 0.0, "end": 60.0, "text": transcript}]
                    }))
                    .into_response()
                }
            })
        };
        serve(
            Router::new()
                .route("/v1/audio/transcriptions", transcribe(self.clone()))
                .route("/v1/audio/translations", transcribe(self.clone())),
        )
        .await
    }

    fn last_prompt(&self) -> String {
        self.bodies.lock().last().map(prompt_of).unwrap_or_default()
    }
}

/// The stand-in provider. Answers by the text it is shown and keeps every
/// envelope it was sent.
async fn provider(envelopes: Arc<Mutex<Vec<Value>>>) -> String {
    let app = Router::new().route(
        "/check",
        post(move |Json(envelope): Json<Value>| {
            let envelopes = envelopes.clone();
            async move {
                envelopes.lock().push(envelope.clone());
                let text = envelope["content"].to_string();
                let answer = if text.contains("attack") {
                    json!({"action": "block", "reason": "prompt injection"})
                } else if text.contains("secret") && envelope["direction"] == "request" {
                    let rewritten: Value = serde_json::from_str(
                        &envelope["content"]
                            .to_string()
                            .replace("secret", "[redacted]"),
                    )
                    .unwrap();
                    json!({"action": "transform", "content": rewritten})
                } else if text.contains("violent") {
                    json!({"categories": {"violence": 0.95}})
                } else {
                    json!({"action": "allow", "categories": {"violence": 0.01}})
                };
                Json(answer)
            }
        }),
    );
    format!("http://{}/check", serve(app).await)
}

fn guard(
    name: &str,
    url: &str,
    stage: WebhookStage,
    failure_mode: FailureMode,
) -> GuardrailWebhookConfig {
    GuardrailWebhookConfig {
        enabled: true,
        name: name.to_string(),
        url: url.to_string(),
        stage,
        timeout_ms: 1_000,
        failure_mode,
        ..GuardrailWebhookConfig::default()
    }
}

fn policy(
    name: &str,
    scope: PolicyScope,
    scope_id: Option<&str>,
    providers: &[&str],
) -> GuardrailPolicy {
    GuardrailPolicy {
        name: name.to_string(),
        enabled: true,
        scope,
        scope_id: scope_id.map(str::to_string),
        providers: providers.iter().map(|p| p.to_string()).collect(),
        thresholds: [("violence".to_string(), 0.9)].into_iter().collect(),
        on_violation: ViolationAction::Block,
        streaming: StreamingStrategy::Reject,
    }
}

fn config(upstream: SocketAddr, kind: ProviderKind) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "up".into(),
        kind,
        api_base: format!("http://{upstream}"),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: "test-model".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "up".into(),
            model: Some("test-model".into()),
            weight: 1,
            takes_over: Vec::new(),
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    config
}

/// One dialect: the path a client calls and the provider kind behind it.
#[derive(Clone, Copy, Debug)]
enum Dialect {
    OpenAi,
    Anthropic,
}

impl Dialect {
    fn path(self) -> &'static str {
        match self {
            Self::OpenAi => "/v1/chat/completions",
            Self::Anthropic => "/v1/messages",
        }
    }

    fn kind(self) -> ProviderKind {
        match self {
            Self::OpenAi => ProviderKind::OpenaiCompatible,
            Self::Anthropic => ProviderKind::Anthropic,
        }
    }

    fn body(self, prompt: &str, stream: bool) -> Value {
        match self {
            Self::OpenAi => json!({
                "model": "test-model", "stream": stream,
                "messages": [{"role": "user", "content": prompt}]
            }),
            Self::Anthropic => json!({
                "model": "test-model", "stream": stream, "max_tokens": 16,
                "messages": [{"role": "user", "content": prompt}]
            }),
        }
    }
}

struct Run {
    status: u16,
    body: String,
}

impl Run {
    fn code(&self) -> Value {
        serde_json::from_str::<Value>(&self.body).unwrap_or_default()["error"]["code"].clone()
    }
}

async fn send(
    config: &GatewayConfig,
    dialect: Dialect,
    prompt: &str,
    stream: bool,
    key: Option<&str>,
) -> Run {
    let state = rolter_gateway::AppState::with_logging(config, None);
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    let mut request = reqwest::Client::new()
        .post(format!("http://{gw}{}", dialect.path()))
        .json(&dialect.body(prompt, stream));
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.unwrap();
    Run {
        status: response.status().as_u16(),
        body: response.text().await.unwrap(),
    }
}

#[tokio::test]
async fn request_stage_blocks_redacts_and_passes_on_both_dialects() {
    for dialect in [Dialect::OpenAi, Dialect::Anthropic] {
        let upstream = Upstream::default();
        let up = upstream.serve().await;
        let envelopes = Arc::default();
        let url = provider(Arc::clone(&envelopes)).await;
        let mut config = config(up, dialect.kind());
        config.semantic_guards.providers = vec![guard(
            "inj",
            &url,
            WebhookStage::PreCall,
            FailureMode::FailClosed,
        )];
        config.semantic_guards.policies =
            vec![policy("all", PolicyScope::Deployment, None, &["inj"])];

        let blocked = send(&config, dialect, "an attack", false, None).await;
        assert_eq!(blocked.status, 400, "{dialect:?} {}", blocked.body);
        assert_eq!(blocked.code(), "guardrail_blocked", "{dialect:?}");
        assert_eq!(
            upstream.calls.load(SeqCst),
            0,
            "a blocked request never goes upstream"
        );

        let redacted = send(&config, dialect, "my secret plan", false, None).await;
        assert_eq!(redacted.status, 200, "{dialect:?} {}", redacted.body);
        assert_eq!(upstream.last_prompt(), "my [redacted] plan", "{dialect:?}");

        let passed = send(&config, dialect, "hello there", false, None).await;
        assert_eq!(passed.status, 200, "{dialect:?} {}", passed.body);
        assert_eq!(upstream.last_prompt(), "hello there", "{dialect:?}");

        let seen = envelopes.lock().clone();
        assert!(
            seen.iter()
                .all(|e| e["policy"] == "all" && e["direction"] == "request"),
            "{seen:?}"
        );
    }
}

#[tokio::test]
async fn response_stage_withholds_a_violation_and_delivers_the_rest() {
    for dialect in [Dialect::OpenAi, Dialect::Anthropic] {
        let upstream = Upstream::default();
        let up = upstream.serve().await;
        let url = provider(Arc::default()).await;
        let mut config = config(up, dialect.kind());
        config.semantic_guards.providers = vec![guard(
            "tox",
            &url,
            WebhookStage::PostCall,
            FailureMode::FailOpen,
        )];
        config.semantic_guards.policies =
            vec![policy("all", PolicyScope::Deployment, None, &["tox"])];

        let withheld = send(&config, dialect, "something violent", false, None).await;
        assert_eq!(withheld.status, 403, "{dialect:?} {}", withheld.body);
        assert_eq!(withheld.code(), "guardrail_blocked");
        assert!(withheld.body.contains("'violence'"), "{}", withheld.body);
        assert!(
            !withheld.body.contains("something violent"),
            "the answer must not leak"
        );

        let delivered = send(&config, dialect, "a calm answer", false, None).await;
        assert_eq!(delivered.status, 200, "{dialect:?} {}", delivered.body);
        assert!(
            delivered.body.contains("a calm answer"),
            "{}",
            delivered.body
        );
    }
}

#[tokio::test]
async fn a_stream_is_refused_before_the_upstream_call_by_default() {
    for dialect in [Dialect::OpenAi, Dialect::Anthropic] {
        let upstream = Upstream::default();
        let up = upstream.serve().await;
        let url = provider(Arc::default()).await;
        let mut config = config(up, dialect.kind());
        config.semantic_guards.providers = vec![guard(
            "tox",
            &url,
            WebhookStage::PostCall,
            FailureMode::FailOpen,
        )];
        config.semantic_guards.policies =
            vec![policy("all", PolicyScope::Deployment, None, &["tox"])];

        let refused = send(&config, dialect, "a calm answer", true, None).await;
        assert_eq!(refused.status, 400, "{dialect:?} {}", refused.body);
        assert_eq!(refused.code(), "guardrail_streaming_unsupported");
        assert_eq!(upstream.calls.load(SeqCst), 0, "{dialect:?}");
    }
}

#[tokio::test]
async fn a_buffered_stream_is_judged_whole_then_replayed_or_withheld() {
    for dialect in [Dialect::OpenAi, Dialect::Anthropic] {
        let upstream = Upstream::default();
        let up = upstream.serve().await;
        let envelopes = Arc::default();
        let url = provider(Arc::clone(&envelopes)).await;
        let mut config = config(up, dialect.kind());
        config.semantic_guards.providers = vec![guard(
            "tox",
            &url,
            WebhookStage::PostCall,
            FailureMode::FailOpen,
        )];
        let mut buffering = policy("all", PolicyScope::Deployment, None, &["tox"]);
        buffering.streaming = StreamingStrategy::Buffer;
        config.semantic_guards.policies = vec![buffering];

        let replayed = send(&config, dialect, "a calm answer", true, None).await;
        assert_eq!(replayed.status, 200, "{dialect:?} {}", replayed.body);
        assert!(
            replayed.body.contains("data:"),
            "still an event stream: {}",
            replayed.body
        );
        assert!(replayed.body.contains("a calm answer"), "{}", replayed.body);
        let shown = envelopes.lock().last().cloned().unwrap();
        assert_eq!(shown["streamed"], true, "{shown}");
        assert_eq!(shown["content"]["text"], "a calm answer", "{shown}");

        let withheld = send(&config, dialect, "something violent", true, None).await;
        assert_eq!(withheld.status, 403, "{dialect:?} {}", withheld.body);
        assert_eq!(withheld.code(), "guardrail_blocked");
        assert!(
            !withheld.body.contains("something violent"),
            "{}",
            withheld.body
        );
    }
}

#[tokio::test]
async fn a_streaming_scope_without_post_call_providers_still_streams() {
    let upstream = Upstream::default();
    let up = upstream.serve().await;
    let url = provider(Arc::default()).await;
    let mut config = config(up, ProviderKind::OpenaiCompatible);
    config.semantic_guards.providers = vec![guard(
        "inj",
        &url,
        WebhookStage::PreCall,
        FailureMode::FailOpen,
    )];
    config.semantic_guards.policies = vec![policy("all", PolicyScope::Deployment, None, &["inj"])];
    let streamed = send(&config, Dialect::OpenAi, "hello", true, None).await;
    assert_eq!(streamed.status, 200, "{}", streamed.body);
    assert!(streamed.body.contains("data:"), "{}", streamed.body);
}

#[tokio::test]
async fn a_fail_closed_provider_that_is_down_refuses_and_a_fail_open_one_does_not() {
    let upstream = Upstream::default();
    let up = upstream.serve().await;
    for (mode, status) in [(FailureMode::FailClosed, 400), (FailureMode::FailOpen, 200)] {
        let mut config = config(up, ProviderKind::OpenaiCompatible);
        // TEST-NET-1: nothing answers there
        config.semantic_guards.providers = vec![GuardrailWebhookConfig {
            timeout_ms: 150,
            ..guard(
                "down",
                "http://192.0.2.1:1/check",
                WebhookStage::PreCall,
                mode,
            )
        }];
        let run = send(&config, Dialect::OpenAi, "hello", false, None).await;
        assert_eq!(run.status, status, "{mode:?} {}", run.body);
    }
}

#[tokio::test]
async fn the_most_specific_policy_replaces_the_broader_one() {
    const KEY: &str = "sk-semantic-guards";
    let upstream = Upstream::default();
    let up = upstream.serve().await;
    let url = provider(Arc::default()).await;
    let mut config = config(up, ProviderKind::OpenaiCompatible);
    config.db_virtual_keys.push(VirtualKeyRecord {
        access_policy: None,
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), KEY),
        id: "key-exempt".into(),
        org_id: "org-a".into(),
        team_id: String::new(),
        project_id: String::new(),
        user_id: String::new(),
        models: vec![],
        providers: vec![],
        disabled: false,
        expires_at: None,
        cache: None,
        business_unit_id: String::new(),
        customer_id: String::new(),
        previous: None,
    });
    config.semantic_guards.providers = vec![guard(
        "inj",
        &url,
        WebhookStage::PreCall,
        FailureMode::FailClosed,
    )];
    config.semantic_guards.policies = vec![
        policy("org-a", PolicyScope::Org, Some("org-a"), &["inj"]),
        // the key's own policy lists no provider, and replaces the org's
        policy("exempt", PolicyScope::Key, Some("key-exempt"), &[]),
    ];
    let exempt = send(&config, Dialect::OpenAi, "an attack", false, Some(KEY)).await;
    assert_eq!(exempt.status, 200, "{}", exempt.body);

    config.semantic_guards.policies.pop();
    let org = send(&config, Dialect::OpenAi, "an attack", false, Some(KEY)).await;
    assert_eq!(org.status, 400, "{}", org.body);
}

/// A transcription of `spoken` to `path`, as `response_format` (#3331).
async fn transcribe(config: &GatewayConfig, path: &str, spoken: &str, format: &str) -> Run {
    let state = rolter_gateway::AppState::with_logging(config, None);
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    let boundary = "GUARDBOUND";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ntest-model\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\n{spoken}\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\n{format}\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\nRIFFxxxxWAVE\r\n--{b}--\r\n",
        b = boundary
    );
    let response = reqwest::Client::new()
        .post(format!("http://{gw}{path}"))
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    Run {
        status: response.status().as_u16(),
        body: response.text().await.unwrap(),
    }
}

#[tokio::test]
async fn a_transcript_is_judged_by_the_response_stage_in_every_format() {
    let upstream = Upstream::default();
    let up = upstream.serve_audio().await;
    let envelopes = Arc::default();
    let url = provider(Arc::clone(&envelopes)).await;
    let mut config = config(up, ProviderKind::OpenaiCompatible);
    config.semantic_guards.providers = vec![guard(
        "tox",
        &url,
        WebhookStage::PostCall,
        FailureMode::FailOpen,
    )];
    config.semantic_guards.policies = vec![policy("all", PolicyScope::Deployment, None, &["tox"])];

    for path in ["/v1/audio/transcriptions", "/v1/audio/translations"] {
        for format in ["verbose_json", "text"] {
            let withheld = transcribe(&config, path, "something violent", format).await;
            assert_eq!(withheld.status, 403, "{path} {format} {}", withheld.body);
            assert_eq!(withheld.code(), "guardrail_blocked");
            assert!(
                !withheld.body.contains("something violent"),
                "the transcript must not leak: {}",
                withheld.body
            );

            let delivered = transcribe(&config, path, "a calm answer", format).await;
            assert_eq!(delivered.status, 200, "{path} {format} {}", delivered.body);
            if format == "text" {
                // still the plain text the caller asked for
                assert_eq!(delivered.body, "a calm answer");
                let shown = envelopes.lock().last().cloned().unwrap();
                assert_eq!(shown["content"], json!({"text": "a calm answer"}));
            } else {
                let body: Value = serde_json::from_str(&delivered.body).unwrap();
                assert_eq!(body["text"], "a calm answer");
                assert_eq!(body["duration"], 60.0);
            }
        }
    }
    let seen = envelopes.lock().clone();
    assert!(
        seen.iter()
            .all(|e| e["policy"] == "all" && e["direction"] == "response"),
        "only the response stage judges a transcript: {seen:?}"
    );
}

#[tokio::test]
async fn the_built_in_output_rules_mask_and_withhold_a_transcript() {
    use rolter_core::{BuiltinRule, GuardAction, GuardStage, GuardrailRule, GuardrailsConfig};
    let upstream = Upstream::default();
    let up = upstream.serve_audio().await;
    let rule = |name: &str, builtin, action| GuardrailRule {
        name: name.to_string(),
        builtin: Some(builtin),
        pattern: None,
        stage: GuardStage::PostCall,
        action,
        replacement: None,
        include_system: false,
    };
    let mut config = config(up, ProviderKind::OpenaiCompatible);
    config.guardrails = GuardrailsConfig {
        enabled: true,
        rules: vec![
            rule("email", BuiltinRule::Email, GuardAction::Redact),
            rule("card", BuiltinRule::PaymentCard, GuardAction::Block),
        ],
        ..Default::default()
    };
    let path = "/v1/audio/transcriptions";

    let masked = transcribe(&config, path, "write to ops@corp.com", "text").await;
    assert_eq!(masked.status, 200, "{}", masked.body);
    assert_eq!(masked.body, "write to [REDACTED:EMAIL]");

    let masked = transcribe(&config, path, "write to ops@corp.com", "verbose_json").await;
    assert_eq!(masked.status, 200, "{}", masked.body);
    assert!(!masked.body.contains("ops@corp.com"), "{}", masked.body);

    let withheld = transcribe(&config, path, "card 4111 1111 1111 1111", "text").await;
    assert_eq!(withheld.status, 403, "{}", withheld.body);
    assert_eq!(withheld.code(), "guardrail_blocked");
    assert!(!withheld.body.contains("4111"), "{}", withheld.body);
}
