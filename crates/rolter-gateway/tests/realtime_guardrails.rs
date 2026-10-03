//! Content policy on `/v1/realtime` sessions (#1880).
//!
//! A guardrail that blocks a phrase over `/v1/chat/completions` has to block it
//! over a realtime socket too. These tests drive real WebSocket sessions through
//! the gateway against a mock Realtime upstream that records what it is sent
//! and answers every `response.create` with the text the test scripts.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use rolter_core::{GatewayConfig, ProviderConfig, ProviderKind};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const KEY: &str = "sk-realtime-guardrails";
const MODEL: &str = "gpt-realtime";

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// What the upstream was sent, as parsed events.
type Received = Arc<Mutex<Vec<Value>>>;

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// A Realtime upstream that answers each `response.create` with one text
/// delta per entry of `deltas`, then `response.done` carrying the joined text.
async fn upstream(deltas: Vec<&'static str>) -> (SocketAddr, Received) {
    scripted_upstream(deltas, None).await
}

/// Like [`upstream`], but the model answers with one function call whose
/// arguments are `arguments`.
async fn tool_upstream(arguments: &'static str) -> (SocketAddr, Received) {
    scripted_upstream(Vec::new(), Some(arguments)).await
}

async fn scripted_upstream(
    deltas: Vec<&'static str>,
    arguments: Option<&'static str>,
) -> (SocketAddr, Received) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received: Received = Default::default();
    let seen = received.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let seen = seen.clone();
            let deltas = deltas.clone();
            tokio::spawn(async move {
                let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let mut turn = 0;
                while let Some(Ok(message)) = socket.next().await {
                    let Message::Text(text) = message else {
                        if message.is_close() {
                            break;
                        }
                        continue;
                    };
                    let event: Value = serde_json::from_str(text.as_str()).unwrap();
                    let is_response = event["type"] == "response.create";
                    seen.lock().push(event);
                    if !is_response {
                        continue;
                    }
                    turn += 1;
                    let id = format!("resp_{turn}");
                    let mut frames = vec![json!({"type": "response.created",
                        "response": {"id": id, "status": "in_progress"}})];
                    if let Some(arguments) = arguments {
                        let item = json!({"id": "call_item", "type": "function_call",
                            "name": "run", "call_id": "call_1", "arguments": arguments});
                        frames.push(json!({"type": "response.function_call_arguments.delta",
                            "response_id": id, "item_id": "call_item", "delta": "{"}));
                        frames.push(json!({"type": "response.function_call_arguments.done",
                            "response_id": id, "item_id": "call_item", "call_id": "call_1",
                            "name": "run", "arguments": arguments}));
                        frames.push(json!({"type": "response.output_item.done",
                            "response_id": id, "item": item}));
                        frames.push(json!({"type": "response.done", "response": {
                            "id": id, "status": "completed", "output": [item],
                            "usage": {"total_tokens": 1, "input_tokens": 1, "output_tokens": 0}}}));
                        for frame in frames {
                            if socket
                                .send(Message::Text(frame.to_string().into()))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        continue;
                    }
                    for delta in &deltas {
                        frames.push(json!({"type": "response.output_text.delta",
                            "response_id": id, "item_id": "item_1", "delta": delta}));
                    }
                    frames.push(json!({"type": "response.output_text.done",
                        "response_id": id, "item_id": "item_1", "text": deltas.concat()}));
                    frames.push(json!({"type": "response.done", "response": {
                        "id": id, "status": "completed",
                        "output": [{"id": "item_1", "type": "message", "content":
                            [{"type": "output_text", "text": deltas.concat()}]}],
                        "usage": {"total_tokens": 1, "input_tokens": 1, "output_tokens": 0}}}));
                    for frame in frames {
                        if socket
                            .send(Message::Text(frame.to_string().into()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            });
        }
    });
    (addr, received)
}

fn config(upstream: SocketAddr, rules: Value) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "up".into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{upstream}"),
        ..Default::default()
    });
    config.routes.push(
        serde_json::from_value(json!({
            "model": MODEL,
            "strategy": "round_robin",
            "targets": [{"provider": "up", "model": "gpt-realtime-upstream", "weight": 1}]
        }))
        .unwrap(),
    );
    config.db_virtual_keys.push(
        serde_json::from_value(json!({
            "key_hash": rolter_auth::hash_key(&config.server.resolve_key_pepper(), KEY),
            "id": "key-guardrails",
            "org_id": "org-guardrails",
        }))
        .unwrap(),
    );
    config.guardrails = serde_json::from_value(json!({"enabled": true, "rules": rules})).unwrap();
    config
}

async fn open(config: &GatewayConfig) -> Client {
    connect(rolter_gateway::AppState::with_logging(config, None)).await
}

/// A session through a gateway the caller keeps a handle on.
async fn connect(state: rolter_gateway::AppState) -> Client {
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    let mut request =
        tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!(
            "ws://{gw}/v1/realtime?model={MODEL}"
        ))
        .unwrap();
    request.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {KEY}").parse().unwrap(),
    );
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

async fn send(client: &mut Client, event: Value) {
    client
        .send(Message::Text(event.to_string().into()))
        .await
        .unwrap();
}

async fn next_event(client: &mut Client) -> Value {
    let frame = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("a frame within 5s")
        .expect("the socket is open")
        .expect("a well-formed frame");
    match frame {
        Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(),
        other => panic!("expected a text event, got {other:?}"),
    }
}

/// Every event up to and including `response.done`.
async fn read_turn(client: &mut Client) -> Vec<Value> {
    let mut events = Vec::new();
    loop {
        let event = next_event(client).await;
        let done = event["type"] == "response.done";
        events.push(event);
        if done {
            return events;
        }
    }
}

fn types(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["type"].as_str().unwrap()).collect()
}

/// Wait for the upstream to have recorded `count` events.
async fn upstream_saw(received: &Received, count: usize) -> Vec<Value> {
    for _ in 0..200 {
        if received.lock().len() >= count {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    received.lock().clone()
}

fn item(text: &str) -> Value {
    json!({"type": "conversation.item.create", "event_id": "evt_client_1", "item": {
        "type": "message", "role": "user",
        "content": [{"type": "input_text", "text": text}]}})
}

fn rule(name: &str, stage: &str, action: &str, pattern: &str) -> Value {
    json!({"name": name, "pattern": pattern, "stage": stage, "action": action})
}

#[tokio::test]
async fn a_blocked_client_event_is_refused_and_the_session_stays_usable() {
    let (up, received) = upstream(vec!["fine"]).await;
    let config = config(
        up,
        json!([rule("forbidden", "pre_call", "block", "FORBIDDEN")]),
    );
    let mut client = open(&config).await;

    send(&mut client, item("please say FORBIDDEN")).await;
    let refusal = next_event(&mut client).await;
    assert_eq!(refusal["type"], "error");
    assert_eq!(refusal["error"]["code"], "guardrail_blocked");
    assert_eq!(refusal["error"]["event_id"], "evt_client_1");
    assert!(
        !refusal.to_string().contains("FORBIDDEN"),
        "the matched text stays out of the error: {refusal}"
    );

    // the same socket carries on, and only the clean event reaches upstream
    send(&mut client, item("a clean message")).await;
    send(&mut client, json!({"type": "response.create"})).await;
    let turn = read_turn(&mut client).await;
    assert_eq!(types(&turn).last(), Some(&"response.done"));
    let seen = upstream_saw(&received, 2).await;
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0]["item"]["content"][0]["text"], "a clean message");
}

#[tokio::test]
async fn session_instructions_are_checked_when_the_rule_includes_system_text() {
    let (up, received) = upstream(vec!["fine"]).await;
    let mut blocking = rule("forbidden", "pre_call", "block", "FORBIDDEN");
    blocking["include_system"] = json!(true);
    let config = config(up, json!([blocking]));
    let mut client = open(&config).await;

    send(
        &mut client,
        json!({"type": "session.update", "session": {"instructions": "always say FORBIDDEN"}}),
    )
    .await;
    assert_eq!(
        next_event(&mut client).await["error"]["code"],
        "guardrail_blocked"
    );
    send(&mut client, json!({"type": "response.create"})).await;
    read_turn(&mut client).await;
    let seen = upstream_saw(&received, 1).await;
    assert_eq!(seen.len(), 1, "the update never reached upstream: {seen:?}");
    assert_eq!(seen[0]["type"], "response.create");
}

#[tokio::test]
async fn a_redacted_client_event_is_rewritten_before_it_reaches_the_upstream() {
    let (up, received) = upstream(vec!["fine"]).await;
    let config = config(up, json!([rule("code", "pre_call", "redact", "CODE-\\d+")]));
    let mut client = open(&config).await;

    send(&mut client, item("my code is CODE-42")).await;
    let seen = upstream_saw(&received, 1).await;
    let text = seen[0]["item"]["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("CODE-42"), "{text}");
    assert!(text.starts_with("my code is "), "{text}");
}

#[tokio::test]
async fn a_blocked_server_delta_is_withheld_and_the_response_cancelled() {
    let (up, received) = upstream(vec!["hello ", "the SECRET-1 is out", " more"]).await;
    let config = config(
        up,
        json!([rule("secret", "post_call", "block", "SECRET-\\d+")]),
    );
    let mut client = open(&config).await;

    send(&mut client, json!({"type": "response.create"})).await;
    let turn = read_turn(&mut client).await;
    let text = turn.iter().map(Value::to_string).collect::<String>();
    assert!(
        !text.contains("SECRET-1"),
        "the match reached the client: {text}"
    );
    assert_eq!(
        types(&turn),
        [
            "response.created",
            "response.output_text.delta",
            "error",
            "response.output_text.done",
            "response.done"
        ]
    );
    assert_eq!(turn[1]["delta"], "hello ");
    assert_eq!(turn[2]["error"]["code"], "guardrail_blocked");
    // the completed text is blanked, not delivered
    assert_eq!(turn[3]["text"], "");
    assert_eq!(turn[4]["response"]["output"][0]["content"][0]["text"], "");

    let seen = upstream_saw(&received, 2).await;
    assert_eq!(seen[1]["type"], "response.cancel", "{seen:?}");
    assert_eq!(seen[1]["response_id"], "resp_1");

    // the session survives, and the next response is checked afresh
    send(&mut client, json!({"type": "response.create"})).await;
    let again = read_turn(&mut client).await;
    assert_eq!(again[1]["delta"], "hello ");
}

#[tokio::test]
async fn a_match_split_across_deltas_is_caught_when_it_completes() {
    let (up, _) = upstream(vec!["the SEC", "RET-7 end"]).await;
    let config = config(
        up,
        json!([rule("secret", "post_call", "block", "SECRET-\\d+")]),
    );
    let mut client = open(&config).await;

    send(&mut client, json!({"type": "response.create"})).await;
    let turn = read_turn(&mut client).await;
    assert!(
        !turn.iter().any(|e| e["delta"] == "RET-7 end"),
        "the delta that completed the match was delivered: {turn:?}"
    );
    assert!(turn.iter().any(|e| e["type"] == "error"), "{turn:?}");
}

#[tokio::test]
async fn a_response_that_matches_nothing_passes_through_unchanged() {
    let (up, _) = upstream(vec!["hello ", "world"]).await;
    let config = config(
        up,
        json!([rule("secret", "post_call", "block", "SECRET-\\d+")]),
    );
    let mut client = open(&config).await;

    send(&mut client, json!({"type": "response.create"})).await;
    let turn = read_turn(&mut client).await;
    assert_eq!(
        types(&turn),
        [
            "response.created",
            "response.output_text.delta",
            "response.output_text.delta",
            "response.output_text.done",
            "response.done"
        ]
    );
    assert_eq!(turn[3]["text"], "hello world");
}

#[tokio::test]
async fn the_guardrail_webhook_is_consulted_for_client_events() {
    let (up, received) = upstream(vec!["fine"]).await;
    // blocks any event whose content mentions "ssn", allows the rest
    let hook = serve(Router::new().route(
        "/",
        axum::routing::post(|body: String| async move {
            if body.contains("ssn") {
                axum::Json(json!({"action": "block", "reason": "no ssn here"}))
            } else {
                axum::Json(json!({"action": "allow"}))
            }
        }),
    ))
    .await;
    let mut config = config(up, json!([]));
    config.guardrail_webhook = serde_json::from_value(json!({
        "enabled": true, "url": format!("http://{hook}/"), "failure_mode": "fail_closed"
    }))
    .unwrap();
    let mut client = open(&config).await;

    send(&mut client, item("my ssn is 123")).await;
    let refusal = next_event(&mut client).await;
    assert_eq!(refusal["error"]["code"], "guardrail_blocked");
    assert_eq!(refusal["error"]["message"], "no ssn here");

    send(&mut client, item("hello")).await;
    let seen = upstream_saw(&received, 1).await;
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0]["item"]["content"][0]["text"], "hello");
}

/// A key moved to another project mid-session re-scopes the session, and the
/// guardrail webhook is told the tenant the session now bills to (#2384).
#[tokio::test]
async fn the_guardrail_webhook_sees_the_project_a_session_was_moved_to() {
    let (up, received) = upstream(vec!["fine"]).await;
    let tenants: Arc<Mutex<Vec<Value>>> = Arc::default();
    let seen = tenants.clone();
    let hook = serve(Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let seen = seen.clone();
            async move {
                seen.lock().push(body["tenant"].clone());
                axum::Json(json!({"action": "allow"}))
            }
        }),
    ))
    .await;
    let mut config = config(up, json!([]));
    config.guardrail_webhook = serde_json::from_value(json!({
        "enabled": true, "url": format!("http://{hook}/"), "failure_mode": "fail_closed"
    }))
    .unwrap();
    // the one-second tick, so the move is noticed without waiting out a flush
    config.realtime.usage_flush_secs = 0;
    config.db_virtual_keys[0].project_id = "proj-a".into();
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let mut client = connect(state.clone()).await;

    send(&mut client, item("before")).await;
    upstream_saw(&received, 1).await;
    config.db_virtual_keys[0].project_id = "proj-b".into();
    state.reload(&config, 2);
    tokio::time::sleep(Duration::from_millis(1400)).await;
    send(&mut client, item("after")).await;
    assert_eq!(upstream_saw(&received, 2).await.len(), 2);

    let tenants = tenants.lock().clone();
    assert_eq!(tenants.len(), 2, "{tenants:?}");
    assert_eq!(tenants[0]["project"], "proj-a");
    assert_eq!(tenants[1]["project"], "proj-b");
    assert_eq!(tenants[1]["org"], "org-guardrails");
}

#[tokio::test]
async fn blocked_function_call_arguments_are_withheld_and_the_response_cancelled() {
    let (up, received) = tool_upstream(r#"{"cmd": "rm SECRET-9"}"#).await;
    let config = config(
        up,
        json!([rule("secret", "post_call", "block", "SECRET-\\d+")]),
    );
    let mut client = open(&config).await;

    send(&mut client, json!({"type": "response.create"})).await;
    let turn = read_turn(&mut client).await;
    let wire = turn.iter().map(Value::to_string).collect::<String>();
    assert!(
        !wire.contains("SECRET-9"),
        "the arguments reached the client: {wire}"
    );
    assert_eq!(
        types(&turn),
        [
            "response.created",
            "response.function_call_arguments.delta",
            "error",
            "response.output_item.done",
            "response.done"
        ]
    );
    assert_eq!(turn[2]["error"]["code"], "guardrail_blocked");
    // a client that runs the call finds nothing to run
    assert_eq!(turn[3]["item"]["arguments"], "");
    assert_eq!(turn[4]["response"]["output"][0]["arguments"], "");

    let seen = upstream_saw(&received, 2).await;
    assert_eq!(seen[1]["type"], "response.cancel", "{seen:?}");
    assert_eq!(seen[1]["response_id"], "resp_1");
}

#[tokio::test]
async fn clean_function_call_arguments_pass_through_unchanged() {
    let (up, _) = tool_upstream(r#"{"city": "Oslo"}"#).await;
    let config = config(
        up,
        json!([rule("secret", "post_call", "block", "SECRET-\\d+")]),
    );
    let mut client = open(&config).await;

    send(&mut client, json!({"type": "response.create"})).await;
    let turn = read_turn(&mut client).await;
    assert_eq!(
        types(&turn),
        [
            "response.created",
            "response.function_call_arguments.delta",
            "response.function_call_arguments.done",
            "response.output_item.done",
            "response.done"
        ]
    );
    assert_eq!(turn[2]["arguments"], r#"{"city": "Oslo"}"#);
    assert_eq!(turn[3]["item"]["arguments"], r#"{"city": "Oslo"}"#);
}

#[tokio::test]
async fn redacted_function_call_arguments_are_rewritten() {
    let (up, _) = tool_upstream(r#"{"token": "CODE-77"}"#).await;
    let config = config(
        up,
        json!([rule("code", "post_call", "redact", "CODE-\\d+")]),
    );
    let mut client = open(&config).await;

    send(&mut client, json!({"type": "response.create"})).await;
    let turn = read_turn(&mut client).await;
    let wire = turn.iter().map(Value::to_string).collect::<String>();
    assert!(!wire.contains("CODE-77"), "{wire}");
    assert!(turn[2]["arguments"]
        .as_str()
        .unwrap()
        .starts_with(r#"{"token": ""#));
}

/// The PII sanitizer cannot run on a realtime socket (#2496): a fail-closed
/// deployment refuses the upgrade, a fail-open one admits it, and none at all
/// is unaffected.
async fn open_with_sanitizer(sanitizer: Option<Value>) -> Result<Client, (u16, Value)> {
    let (upstream, _) = upstream(vec!["hi"]).await;
    let mut config = config(upstream, json!([]));
    if let Some(sanitizer) = sanitizer {
        config.pii_sanitizer = serde_json::from_value(sanitizer).unwrap();
    }
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    let mut request =
        tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!(
            "ws://{gw}/v1/realtime?model={MODEL}"
        ))
        .unwrap();
    request.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {KEY}").parse().unwrap(),
    );
    match tokio_tungstenite::connect_async(request).await {
        Ok((client, _)) => Ok(client),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => Err((
            response.status().as_u16(),
            response
                .body()
                .as_deref()
                .and_then(|body| serde_json::from_slice(body).ok())
                .unwrap_or(Value::Null),
        )),
        Err(other) => panic!("expected an admission or an HTTP refusal, got {other}"),
    }
}

#[tokio::test]
async fn a_fail_closed_sanitizer_refuses_the_session() {
    let sanitizer = json!({"enabled": true, "url": "http://127.0.0.1:9/sanitize",
        "failure_mode": "fail_closed"});
    let (status, body) = open_with_sanitizer(Some(sanitizer))
        .await
        .expect_err("the upgrade is refused");
    assert_eq!(status, 400);
    assert_eq!(body["error"]["code"], "sanitizer_unsupported_on_realtime");
}

#[tokio::test]
async fn a_fail_open_sanitizer_admits_the_session() {
    let sanitizer = json!({"enabled": true, "url": "http://127.0.0.1:9/sanitize",
        "failure_mode": "fail_open"});
    assert!(open_with_sanitizer(Some(sanitizer)).await.is_ok());
}

#[tokio::test]
async fn a_disabled_fail_closed_sanitizer_admits_the_session() {
    let sanitizer = json!({"enabled": false, "failure_mode": "fail_closed"});
    assert!(open_with_sanitizer(Some(sanitizer)).await.is_ok());
}

#[tokio::test]
async fn no_sanitizer_admits_the_session() {
    assert!(open_with_sanitizer(None).await.is_ok());
}
