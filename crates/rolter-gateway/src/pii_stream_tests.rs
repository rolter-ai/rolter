//! Streamed PII restoration (#911): the segmentation harness and the pipe's
//! failure, backpressure, cancellation and leak behaviour.
//!
//! The stub sanitizer speaks the NDJSON protocol of `restore_stream_url` and
//! runs the placeholder holdback the contract asks of a real one: a trie over
//! the exchange's own placeholders, as a scan over a short list here. Its
//! answer on every segmentation is compared with the buffered restore of the
//! same text, which for these placeholders is a plain replace.
//!
//! Two layers. The [`RestoringStream`](crate::pii_stream::RestoringStream) is
//! driven directly over in-memory chunks, so every byte cut can be chosen: one
//! byte per chunk covers a cut inside every `<` escape and every
//! multi-byte UTF-8 character. Then the whole gateway serves the same streams
//! live and from the in-memory response cache.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::StreamExt;
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, PiiSanitizerConfig, ProviderConfig, ProviderKind,
    RestorationPolicy, RouteCache, SanitizeDirection, StreamingResponse, Target, WebhookTenant,
};
use serde_json::{json, Value};

use crate::cache::ResponseCache;
use crate::metrics::Metrics;
use crate::pii_stream::StreamRestore;
use crate::state::AppState;

/// What the stub's mapping restores each placeholder to. The values never
/// appear in a request, so any place they turn up came through the restore.
const MAP: &[(&str, &str)] = &[
    ("<PERSON_1>", "Jan \"JK\" Kowalski"),
    ("<EMAIL_ADDRESS_12>", "jan.k@example.test"),
    ("<CITY_3>", "Łódź"),
];

/// Plain text: placeholders, a `<` that starts none, a placeholder the
/// sanitizer never issued that shares a prefix with one it did, multi-byte
/// characters, and a trailing `<` that is held until the channel closes.
const TEXT: &str =
    "Hi <PERSON_1>, a < b; mail <EMAIL_ADDRESS_12> from <CITY_3>! <PERSON_10> zażółć <";
const TEXT2: &str = "<CITY_3> then <PERSON_1><EMAIL_ADDRESS_12>.";
/// JSON source, where a restored `"` must come back escaped.
const ARGS: &str =
    r#"{"name":"<PERSON_1>","city":"<CITY_3>","note":"a < b","mail":"<EMAIL_ADDRESS_12>"}"#;

fn escaped(value: &str) -> String {
    let quoted = serde_json::to_string(value).unwrap();
    quoted[1..quoted.len() - 1].to_string()
}

/// The buffered restore of `text`.
fn restored(text: &str, json: bool) -> String {
    MAP.iter()
        .fold(text.to_string(), |text, (placeholder, value)| {
            let value = if json {
                escaped(value)
            } else {
                value.to_string()
            };
            text.replace(placeholder, &value)
        })
}

// ── the stub sanitizer ───────────────────────────────────────────────────────

#[derive(Default)]
struct Stub {
    /// stop answering once this many replies went out, until `release`
    stall_after: Option<usize>,
    release: tokio::sync::Notify,
    /// abort the reply stream once this many replies went out
    fail_after: Option<usize>,
    replies: AtomicUsize,
    streams: AtomicUsize,
    saw_end: AtomicBool,
    saw_reset: AtomicBool,
    opens: parking_lot::Mutex<Vec<Value>>,
    /// lines that arrived, the first included, counted as they are read
    /// rather than as they are answered
    received: AtomicUsize,
    /// send the response head only together with the first reply, as a
    /// sanitizer that buffers its head would
    head_with_first_reply: bool,
    /// hold back every character it is sent, as a broken shim would
    hold_all: bool,
}

/// The holdback: restore every whole placeholder in `pending`, and hold back
/// the longest suffix that could still become one.
fn holdback(pending: &mut String, json: bool) -> (String, usize) {
    let mut out = String::new();
    let mut rest = pending.as_str();
    let mut held = "";
    'scan: while !rest.is_empty() {
        for (placeholder, value) in MAP {
            if let Some(after) = rest.strip_prefix(placeholder) {
                out.push_str(&if json {
                    escaped(value)
                } else {
                    value.to_string()
                });
                rest = after;
                continue 'scan;
            }
        }
        if MAP
            .iter()
            .any(|(placeholder, _)| placeholder.len() > rest.len() && placeholder.starts_with(rest))
        {
            held = rest;
            break;
        }
        let next = rest.chars().next().unwrap();
        out.push(next);
        rest = &rest[next.len_utf8()..];
    }
    let held = held.to_string();
    let count = held.chars().count();
    *pending = held;
    (out, count)
}

async fn restore_stream(State(stub): State<Arc<Stub>>, body: Body) -> axum::response::Response {
    stub.streams.fetch_add(1, Ordering::SeqCst);
    let head_with_first_reply = stub.head_with_first_reply;
    let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<Bytes>>(64);
    let (head_tx, head_rx) = tokio::sync::oneshot::channel::<()>();
    let mut head_tx = Some(head_tx);
    // read apart from answering, so a stalled stub still counts what arrives
    let (chunks_tx, mut chunks) = tokio::sync::mpsc::unbounded_channel::<Option<Bytes>>();
    let reader = stub.clone();
    tokio::spawn(async move {
        let mut input = body.into_data_stream();
        loop {
            let next = match input.next().await {
                Some(Ok(chunk)) => {
                    let lines = chunk.iter().filter(|b| **b == b'\n').count();
                    reader.received.fetch_add(lines, Ordering::SeqCst);
                    Some(chunk)
                }
                Some(Err(_)) => None,
                None => {
                    let _ = chunks_tx.send(Some(Bytes::new()));
                    return;
                }
            };
            let failed = next.is_none();
            if chunks_tx.send(next).is_err() || failed {
                return;
            }
        }
    });
    tokio::spawn(async move {
        let mut buf = Vec::new();
        let mut opened = false;
        let mut channels: HashMap<String, (String, bool)> = HashMap::new();
        loop {
            match chunks.recv().await {
                // an empty chunk is the reader's clean end of the body
                Some(Some(chunk)) if chunk.is_empty() => {
                    if !stub.saw_end.load(Ordering::SeqCst) {
                        stub.saw_reset.store(true, Ordering::SeqCst);
                    }
                    return;
                }
                Some(Some(chunk)) => buf.extend_from_slice(&chunk),
                Some(None) => {
                    stub.saw_reset.store(true, Ordering::SeqCst);
                    return;
                }
                None => {
                    if !stub.saw_end.load(Ordering::SeqCst) {
                        stub.saw_reset.store(true, Ordering::SeqCst);
                    }
                    return;
                }
            }
            while let Some(end) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=end).collect();
                let line: Value = serde_json::from_slice(&line).unwrap();
                if !opened {
                    opened = true;
                    stub.opens.lock().push(line);
                    continue;
                }
                let seq = line["seq"].as_u64().unwrap();
                let reply = if let Some(name) = line["close"].as_str() {
                    let (pending, _) = channels.remove(name).unwrap_or_default();
                    json!({"seq": seq, "text": pending, "held": 0})
                } else if line["end"] == true {
                    stub.saw_end.store(true, Ordering::SeqCst);
                    json!({"seq": seq, "text": "", "held": 0})
                } else {
                    let name = line["channel"].as_str().unwrap().to_string();
                    let json = line["format"] == "json";
                    let entry = channels
                        .entry(name)
                        .or_insert_with(|| (String::new(), json));
                    entry.0.push_str(line["text"].as_str().unwrap());
                    let (text, held) = if stub.hold_all {
                        (String::new(), entry.0.chars().count())
                    } else {
                        holdback(&mut entry.0, entry.1)
                    };
                    json!({"seq": seq, "text": text, "held": held})
                };
                let sent = stub.replies.load(Ordering::SeqCst);
                if stub.fail_after == Some(sent) {
                    let _ = tx.send(Err(std::io::Error::other("stub failure"))).await;
                    return;
                }
                if stub.stall_after == Some(sent) {
                    stub.release.notified().await;
                }
                if let Some(head) = head_tx.take() {
                    let _ = head.send(());
                }
                let mut bytes = serde_json::to_vec(&reply).unwrap();
                bytes.push(b'\n');
                if tx.send(Ok(Bytes::from(bytes))).await.is_err() {
                    stub.saw_reset.store(true, Ordering::SeqCst);
                    return;
                }
                stub.replies.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    if head_with_first_reply {
        let _ = head_rx.await;
    }
    let mut rx = rx;
    Body::from_stream(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))).into_response()
}

/// The request-leg stub: content unchanged, and a token every time.
async fn sanitize(Json(body): Json<Value>) -> Json<Value> {
    Json(json!({
        "content": body["content"],
        "findings": [{"entity_type": "PERSON", "count": 1, "placeholders": ["<PERSON_1>"]}],
        "restoration_token": "tok-stream",
    }))
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn serve_stub(stub: Stub) -> (SocketAddr, Arc<Stub>) {
    let stub = Arc::new(stub);
    let app = Router::new()
        .route("/sanitize", post(sanitize))
        .route("/restore-stream", post(restore_stream))
        .with_state(stub.clone());
    (serve(app).await, stub)
}

fn sanitizer_config(stub: SocketAddr, timeout_ms: u64) -> PiiSanitizerConfig {
    PiiSanitizerConfig {
        enabled: true,
        url: format!("http://{stub}/sanitize"),
        restore_url: format!("http://{stub}/restore"),
        restore_stream_url: format!("http://{stub}/restore-stream"),
        direction: SanitizeDirection::Request,
        restoration: RestorationPolicy::TrustedDownstream,
        streaming: StreamingResponse::Restore,
        timeout_ms,
        ..Default::default()
    }
}

fn restorer(stub: SocketAddr, timeout_ms: u64, metrics: &Arc<Metrics>) -> StreamRestore {
    restorer_over(
        &crate::egress_client::testing::permissive(),
        stub,
        timeout_ms,
        metrics,
    )
}

/// [`restorer`] over a client the caller keeps, so a run of many streams
/// shares one connection pool rather than building a client each time.
fn restorer_over(
    egress: &crate::egress_client::EgressClient,
    stub: SocketAddr,
    timeout_ms: u64,
    metrics: &Arc<Metrics>,
) -> StreamRestore {
    StreamRestore::new(
        &sanitizer_config(stub, timeout_ms),
        egress,
        metrics,
        "tok-stream",
        "trace",
        WebhookTenant::default(),
    )
}

/// Run `chunks` through a restoring stream and collect what the caller gets.
async fn run(restore: StreamRestore, chunks: Vec<Bytes>) -> Vec<u8> {
    let inner = futures_util::stream::iter(chunks.into_iter().map(Ok::<_, reqwest::Error>));
    let mut out = Vec::new();
    let mut stream = restore.wrap(Box::pin(inner));
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk.unwrap());
    }
    out
}

// ── segmentations and dialects ───────────────────────────────────────────────

/// Every frame segmentation that matters for `text`: one frame, one character
/// per frame, one cut at every offset inside every `<...>` span, and uneven
/// runs of one to five characters.
fn segmentations(text: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = text.chars().collect();
    let piece = |range: std::ops::Range<usize>| chars[range].iter().collect::<String>();
    let mut out = vec![
        vec![text.to_string()],
        chars.iter().map(char::to_string).collect(),
    ];
    let mut start = None;
    for (at, c) in chars.iter().enumerate() {
        match c {
            '<' => start = Some(at),
            '>' => {
                if let Some(open) = start.take() {
                    for cut in open + 1..=at {
                        out.push(vec![piece(0..cut), piece(cut..chars.len())]);
                    }
                }
            }
            _ => {}
        }
    }
    let mut uneven = Vec::new();
    let (mut at, mut k) = (0, 0);
    while at < chars.len() {
        let len = (1 + (k * 7 + 3) % 5).min(chars.len() - at);
        uneven.push(piece(at..at + len));
        at += len;
        k += 1;
    }
    out.push(uneven);
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Dialect {
    Chat,
    Messages,
    Responses,
}

fn event(name: Option<&str>, data: &Value, escape_lt: bool) -> String {
    let mut data = data.to_string();
    if escape_lt {
        // `<` only ever sits inside a string in JSON, so this is the escape a
        // provider would write
        data = data.replace('<', "\\u003c");
    }
    match name {
        Some(name) => format!("event: {name}\ndata: {data}\n\n"),
        None => format!("data: {data}\n\n"),
    }
}

/// The SSE events of one stream: `a` and `b` are the segmentations of the two
/// plain-text channels, `args` of the JSON one.
fn stream_events(
    dialect: Dialect,
    a: &[String],
    b: &[String],
    args: &[String],
    escape_lt: bool,
) -> Vec<String> {
    let mut events = Vec::new();
    let longest = a.len().max(b.len()).max(args.len());
    match dialect {
        Dialect::Chat => {
            let chunk = |choices: Value| json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "model": "m", "choices": choices});
            events.push(event(
                None,
                &chunk(json!([{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}])),
                escape_lt,
            ));
            // n = 2: choice 0 streams content and a tool call, choice 1 content
            for k in 0..longest {
                if let Some(text) = a.get(k) {
                    events.push(event(None, &chunk(json!([{"index": 0, "delta": {"content": text}, "finish_reason": null}])), escape_lt));
                }
                if let Some(text) = b.get(k) {
                    events.push(event(None, &chunk(json!([{"index": 1, "delta": {"content": text}, "finish_reason": null}])), escape_lt));
                }
                if let Some(text) = args.get(k) {
                    let mut call = json!({"index": 0, "function": {"arguments": text}});
                    if k == 0 {
                        call["id"] = json!("call_1");
                        call["type"] = json!("function");
                        call["function"]["name"] = json!("lookup");
                    }
                    events.push(event(None, &chunk(json!([{"index": 0, "delta": {"tool_calls": [call]}, "finish_reason": null}])), escape_lt));
                }
            }
            events.push(event(
                None,
                &chunk(json!([{"index": 0, "delta": {}, "finish_reason": "tool_calls"}])),
                escape_lt,
            ));
            events.push(event(
                None,
                &chunk(json!([{"index": 1, "delta": {}, "finish_reason": "stop"}])),
                escape_lt,
            ));
            events.push(event(
                None,
                &json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [], "usage": {"prompt_tokens": 3, "completion_tokens": 7, "total_tokens": 10}}),
                escape_lt,
            ));
            events.push("data: [DONE]\n\n".to_string());
        }
        Dialect::Messages => {
            events.push(event(
                Some("message_start"),
                &json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "model": "m", "content": [], "usage": {"input_tokens": 3, "output_tokens": 1}}}),
                escape_lt,
            ));
            let blocks: [(&[String], Value, &str, &str); 3] = [
                (
                    b,
                    json!({"type": "thinking", "thinking": ""}),
                    "thinking_delta",
                    "thinking",
                ),
                (a, json!({"type": "text", "text": ""}), "text_delta", "text"),
                (
                    args,
                    json!({"type": "tool_use", "id": "toolu_1", "name": "lookup", "input": {}}),
                    "input_json_delta",
                    "partial_json",
                ),
            ];
            for (index, (pieces, start, kind, field)) in blocks.into_iter().enumerate() {
                events.push(event(
                    Some("content_block_start"),
                    &json!({"type": "content_block_start", "index": index, "content_block": start}),
                    escape_lt,
                ));
                for text in pieces {
                    let mut delta = json!({"type": kind});
                    delta[field] = json!(text);
                    events.push(event(
                        Some("content_block_delta"),
                        &json!({"type": "content_block_delta", "index": index, "delta": delta}),
                        escape_lt,
                    ));
                }
                events.push(event(
                    Some("content_block_stop"),
                    &json!({"type": "content_block_stop", "index": index}),
                    escape_lt,
                ));
            }
            events.push(event(Some("message_delta"), &json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 7}}), escape_lt));
            events.push(event(
                Some("message_stop"),
                &json!({"type": "message_stop"}),
                escape_lt,
            ));
        }
        Dialect::Responses => {
            let mut seq = 0;
            let mut push = |data: Value| {
                let mut data = data;
                data["sequence_number"] = json!(seq);
                seq += 1;
                let name = data["type"].as_str().unwrap().to_string();
                events.push(event(Some(&name), &data, escape_lt));
            };
            let full_a: String = a.concat();
            let full_args: String = args.concat();
            let message = |text: &str| json!({"id": "msg_1", "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text, "annotations": []}]});
            let call = |arguments: &str| json!({"id": "fc_1", "type": "function_call", "call_id": "call_1", "name": "lookup", "arguments": arguments});
            push(
                json!({"type": "response.created", "response": {"id": "resp_1", "status": "in_progress", "output": []}}),
            );
            push(
                json!({"type": "response.output_item.added", "output_index": 0, "item": message("")}),
            );
            push(
                json!({"type": "response.content_part.added", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": ""}}),
            );
            for text in a {
                push(
                    json!({"type": "response.output_text.delta", "item_id": "msg_1", "output_index": 0, "content_index": 0, "delta": text}),
                );
            }
            push(
                json!({"type": "response.output_text.done", "item_id": "msg_1", "output_index": 0, "content_index": 0, "text": full_a}),
            );
            push(
                json!({"type": "response.content_part.done", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": full_a}}),
            );
            push(
                json!({"type": "response.output_item.done", "output_index": 0, "item": message(&full_a)}),
            );
            push(
                json!({"type": "response.output_item.added", "output_index": 1, "item": call("")}),
            );
            for text in args {
                push(
                    json!({"type": "response.function_call_arguments.delta", "item_id": "fc_1", "output_index": 1, "delta": text}),
                );
            }
            push(
                json!({"type": "response.function_call_arguments.done", "item_id": "fc_1", "output_index": 1, "arguments": full_args}),
            );
            push(
                json!({"type": "response.output_item.done", "output_index": 1, "item": call(&full_args)}),
            );
            push(
                json!({"type": "response.completed", "response": {"id": "resp_1", "status": "completed", "output": [message(&full_a), call(&full_args)], "usage": {"input_tokens": 3, "output_tokens": 7, "total_tokens": 10}}}),
            );
        }
    }
    events
}

/// The data of every event in an SSE body.
fn sse_data(body: &[u8]) -> Vec<Value> {
    String::from_utf8(body.to_vec())
        .expect("the stream stays valid UTF-8")
        .split("\n\n")
        .filter_map(|event| {
            event
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .filter(|data| *data != "[DONE]")
                .map(|data| serde_json::from_str(data).expect("every frame stays JSON"))
        })
        .collect()
}

fn event_count(body: &[u8]) -> usize {
    String::from_utf8_lossy(body)
        .split("\n\n")
        .filter(|event| !event.trim().is_empty())
        .count()
}

/// The text each channel of `dialect` reassembles to on the client.
struct Reassembled {
    a: String,
    b: String,
    args: String,
}

fn reassemble(dialect: Dialect, body: &[u8]) -> Reassembled {
    let mut out = Reassembled {
        a: String::new(),
        b: String::new(),
        args: String::new(),
    };
    for data in sse_data(body) {
        match dialect {
            Dialect::Chat => {
                for choice in data["choices"].as_array().into_iter().flatten() {
                    let delta = &choice["delta"];
                    let text = delta["content"].as_str().unwrap_or_default();
                    match choice["index"].as_u64() {
                        Some(0) => out.a.push_str(text),
                        Some(1) => out.b.push_str(text),
                        _ => {}
                    }
                    for call in delta["tool_calls"].as_array().into_iter().flatten() {
                        out.args
                            .push_str(call["function"]["arguments"].as_str().unwrap_or_default());
                    }
                }
            }
            Dialect::Messages => {
                if data["type"] == "content_block_delta" {
                    let delta = &data["delta"];
                    match data["index"].as_u64() {
                        Some(0) => out.b.push_str(delta["thinking"].as_str().unwrap()),
                        Some(1) => out.a.push_str(delta["text"].as_str().unwrap()),
                        Some(2) => out.args.push_str(delta["partial_json"].as_str().unwrap()),
                        _ => {}
                    }
                }
            }
            Dialect::Responses => match data["type"].as_str() {
                Some("response.output_text.delta") => {
                    out.a.push_str(data["delta"].as_str().unwrap())
                }
                Some("response.function_call_arguments.delta") => {
                    out.args.push_str(data["delta"].as_str().unwrap())
                }
                _ => {}
            },
        }
    }
    out
}

/// The Responses API repeats each channel's whole text in its done events and
/// in the final response; every copy must say what the deltas said.
fn assert_responses_repeats(body: &[u8], text: &str, args: &str, case: &str) {
    for data in sse_data(body) {
        let repeated: Vec<(&Value, &str)> = match data["type"].as_str() {
            Some("response.output_text.done") => vec![(&data["text"], text)],
            Some("response.content_part.done") => vec![(&data["part"]["text"], text)],
            Some("response.function_call_arguments.done") => vec![(&data["arguments"], args)],
            Some("response.output_item.done") if data["output_index"] == 0 => {
                vec![(&data["item"]["content"][0]["text"], text)]
            }
            Some("response.output_item.done") => vec![(&data["item"]["arguments"], args)],
            Some("response.completed") => vec![
                (&data["response"]["output"][0]["content"][0]["text"], text),
                (&data["response"]["output"][1]["arguments"], args),
            ],
            _ => continue,
        };
        for (value, expected) in repeated {
            assert_eq!(value, expected, "{case}: {}", data["type"]);
        }
    }
}

fn assert_restored(dialect: Dialect, body: &[u8], case: &str) {
    let got = reassemble(dialect, body);
    let text = restored(TEXT, false);
    let args = restored(ARGS, true);
    assert_eq!(got.a, text, "{case}");
    if dialect != Dialect::Responses {
        assert_eq!(got.b, restored(TEXT2, false), "{case}");
    }
    assert_eq!(got.args, args, "{case}");
    // the restored arguments are still the JSON the model wrote
    let parsed: Value = serde_json::from_str(&got.args).expect("restored arguments parse");
    assert_eq!(parsed["name"], "Jan \"JK\" Kowalski", "{case}");
    if dialect == Dialect::Responses {
        assert_responses_repeats(body, &text, &args, case);
    }
}

/// Every case of the matrix: a segmentation of each channel and whether `<`
/// is escaped.
#[allow(clippy::type_complexity)]
fn cases() -> Vec<(String, Vec<String>, Vec<String>, Vec<String>, bool)> {
    let (a, b, args) = (
        segmentations(TEXT),
        segmentations(TEXT2),
        segmentations(ARGS),
    );
    let total = a.len().max(b.len()).max(args.len());
    let mut out = Vec::new();
    for escape_lt in [false, true] {
        for k in 0..total {
            out.push((
                format!("segmentation {k}, escaped < = {escape_lt}"),
                a[k % a.len()].clone(),
                b[k % b.len()].clone(),
                args[k % args.len()].clone(),
                escape_lt,
            ));
        }
    }
    out
}

#[test]
fn the_holdback_stub_matches_the_buffered_restore() {
    for pieces in segmentations(TEXT) {
        let mut pending = String::new();
        let mut out = String::new();
        for piece in &pieces {
            pending.push_str(piece);
            out.push_str(&holdback(&mut pending, false).0);
        }
        out.push_str(&pending);
        assert_eq!(out, restored(TEXT, false), "{pieces:?}");
    }
}

/// The harness: every segmentation, every byte cut, every dialect, against
/// the buffered restore. One byte per chunk cuts inside every `<` and
/// every multi-byte character.
#[tokio::test]
async fn every_segmentation_reassembles_to_the_buffered_restore() {
    let (stub, _) = serve_stub(Stub::default()).await;
    let metrics = Arc::new(Metrics::default());
    let egress = crate::egress_client::testing::permissive();
    let mut runs = 0;
    for dialect in [Dialect::Chat, Dialect::Messages, Dialect::Responses] {
        for (case, a, b, args, escape_lt) in cases() {
            let events = stream_events(dialect, &a, &b, &args, escape_lt);
            let whole: Vec<u8> = events.concat().into_bytes();
            let chunkings: [(&str, Vec<Bytes>); 3] = [
                (
                    "one chunk per frame",
                    events.iter().map(|e| Bytes::from(e.clone())).collect(),
                ),
                (
                    "one byte per chunk",
                    whole
                        .iter()
                        .map(|b| Bytes::copy_from_slice(&[*b]))
                        .collect(),
                ),
                ("one chunk", vec![Bytes::from(whole.clone())]),
            ];
            for (chunking, chunks) in chunkings {
                // a byte cut inside `\u003c` needs the escaped variant; the
                // plain one still cuts inside every multi-byte character
                let case = format!("{dialect:?}, {case}, {chunking}");
                let body = run(restorer_over(&egress, stub, 5_000, &metrics), chunks).await;
                assert_restored(dialect, &body, &case);
                // frame for frame: nothing dropped, nothing added
                assert_eq!(event_count(&body), events.len(), "{case}");
                runs += 1;
            }
        }
    }
    assert_eq!(
        metrics.pii_restorations_total.load(Ordering::SeqCst),
        runs,
        "every stream restored"
    );
    assert_eq!(metrics.pii_restore_errors_total.load(Ordering::SeqCst), 0);
}

/// A frame whose text did not change goes out byte for byte as it came.
#[tokio::test]
async fn a_frame_without_placeholders_is_forwarded_untouched() {
    let (stub, _) = serve_stub(Stub::default()).await;
    let metrics = Arc::new(Metrics::default());
    let frame = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"plain\"},\"logprobs\":null,\"finish_reason\":null}]}\n\n";
    let body = run(
        restorer(stub, 5_000, &metrics),
        vec![Bytes::from(frame), Bytes::from("data: [DONE]\n\n")],
    )
    .await;
    assert_eq!(
        String::from_utf8(body).unwrap(),
        format!("{frame}data: [DONE]\n\n")
    );
}

/// The first line names the exchange and carries the token, once.
#[tokio::test]
async fn the_pipe_opens_with_the_token_and_ends_with_end() {
    let (stub_addr, stub) = serve_stub(Stub::default()).await;
    let metrics = Arc::new(Metrics::default());
    let events = stream_events(Dialect::Chat, &[TEXT.to_string()], &[], &[], false);
    run(
        restorer(stub_addr, 5_000, &metrics),
        events.into_iter().map(Bytes::from).collect(),
    )
    .await;
    // the end line goes out as the stream finishes; give it a moment to land
    for _ in 0..100 {
        if stub.saw_end.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(stub.saw_end.load(Ordering::SeqCst));
    let opens = stub.opens.lock();
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0]["restoration_token"], "tok-stream");
    assert_eq!(opens[0]["trace_id"], "trace");
}

/// A mid-stream failure: restored text up to it, then the text the sanitizer
/// was holding, then placeholders — nothing duplicated or lost.
#[tokio::test]
async fn a_mid_stream_failure_continues_with_placeholders() {
    let pieces: Vec<String> = TEXT.chars().map(|c| c.to_string()).collect();
    let events = stream_events(Dialect::Chat, &pieces, &[], &[], false);
    // answer up to the middle of `<EMAIL_ADDRESS_12>`, then fail
    let cut = TEXT.find("<EMAIL").unwrap() + 4;
    let cut_chars = TEXT[..cut].chars().count();
    let (stub_addr, _) = serve_stub(Stub {
        fail_after: Some(cut_chars),
        ..Stub::default()
    })
    .await;
    let metrics = Arc::new(Metrics::default());
    let body = run(
        restorer(stub_addr, 5_000, &metrics),
        events.iter().cloned().map(Bytes::from).collect(),
    )
    .await;
    let got = reassemble(Dialect::Chat, &body).a;
    let expected = format!("{}{}", restored(&TEXT[..cut], false), &TEXT[cut..]);
    assert_eq!(got, expected);
    assert!(got.contains("<EMAIL_ADDRESS_12>"), "{got}");
    assert!(got.starts_with("Hi Jan \"JK\" Kowalski,"), "{got}");
    assert_eq!(event_count(&body), events.len());
    assert_eq!(metrics.pii_restore_errors_total.load(Ordering::SeqCst), 1);
    assert_eq!(metrics.pii_restorations_total.load(Ordering::SeqCst), 0);
}

/// No reply within `timeout_ms` is a failure like any other: the stream goes
/// on unrestored rather than waiting forever.
#[tokio::test]
async fn a_stalled_sanitizer_times_out_into_an_unrestored_stream() {
    let (stub_addr, stub) = serve_stub(Stub {
        stall_after: Some(3),
        ..Stub::default()
    })
    .await;
    let metrics = Arc::new(Metrics::default());
    let pieces: Vec<String> = TEXT.chars().map(|c| c.to_string()).collect();
    let events = stream_events(Dialect::Chat, &pieces, &[], &[], false);
    let body = run(
        restorer(stub_addr, 200, &metrics),
        events.iter().cloned().map(Bytes::from).collect(),
    )
    .await;
    stub.release.notify_waiters();
    // the first three characters were restored text; the rest is as sent
    assert_eq!(reassemble(Dialect::Chat, &body).a, TEXT);
    assert_eq!(metrics.pii_restore_errors_total.load(Ordering::SeqCst), 1);
}

/// A pipe that does not open serves the stream unrestored.
#[tokio::test]
async fn a_pipe_that_does_not_open_serves_the_stream_unrestored() {
    let metrics = Arc::new(Metrics::default());
    // a listener that answers 503 to the streaming endpoint
    let refusing = serve(Router::new().route(
        "/restore-stream",
        post(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
    ))
    .await;
    let events = stream_events(Dialect::Chat, &[TEXT.to_string()], &[], &[], false);
    let body = run(
        restorer(refusing, 5_000, &metrics),
        events.iter().cloned().map(Bytes::from).collect(),
    )
    .await;
    assert_eq!(reassemble(Dialect::Chat, &body).a, TEXT);
    assert_eq!(metrics.pii_restore_errors_total.load(Ordering::SeqCst), 1);

    // and one the egress policy refuses is never dialled
    let counter = crate::egress_client::testing::Counter::start().await;
    let mut config = sanitizer_config(refusing, 5_000);
    config.restore_stream_url = counter.url("/restore-stream");
    let denied =
        crate::egress_client::EgressClient::new(crate::egress_client::testing::deny_loopback());
    let restore = StreamRestore::new(
        &config,
        &denied,
        &metrics,
        "tok-stream",
        "trace",
        WebhookTenant::default(),
    );
    let body = run(restore, events.into_iter().map(Bytes::from).collect()).await;
    assert_eq!(reassemble(Dialect::Chat, &body).a, TEXT);
    assert_eq!(counter.accepted(), 0);
    assert_eq!(metrics.pii_restore_errors_total.load(Ordering::SeqCst), 2);
}

/// Backpressure: while the sanitizer stalls, the gateway stops pulling the
/// upstream once its window of unanswered deltas is full, and resumes when the
/// sanitizer does.
#[tokio::test]
async fn a_stalled_sanitizer_stops_the_gateway_reading_the_upstream() {
    let (stub_addr, stub) = serve_stub(Stub {
        stall_after: Some(4),
        ..Stub::default()
    })
    .await;
    let metrics = Arc::new(Metrics::default());
    const FRAMES: usize = 5_000;
    let pulled = Arc::new(AtomicUsize::new(0));
    let counter = pulled.clone();
    let upstream = futures_util::stream::iter(0..FRAMES).map(move |k| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok::<_, reqwest::Error>(Bytes::from(event(
            None,
            &json!({"choices": [{"index": 0, "delta": {"content": format!("w{k} ")}}]}),
            false,
        )))
    });
    let restore = restorer(stub_addr, 30_000, &metrics);
    let consumer = tokio::spawn(async move {
        let mut stream = restore.wrap(Box::pin(upstream));
        let mut frames = 0;
        while let Some(chunk) = stream.next().await {
            chunk.unwrap();
            frames += 1;
        }
        frames
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    let while_stalled = pulled.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        pulled.load(Ordering::SeqCst),
        while_stalled,
        "still pulling"
    );
    // the four answered before the stall, the window, and the frame that
    // filled it
    assert!(
        while_stalled <= 4 + crate::pii_stream::MAX_UNANSWERED_LINES + 1,
        "pulled {while_stalled} frames while the sanitizer stalled"
    );
    stub.release.notify_waiters();
    let frames = tokio::time::timeout(Duration::from_secs(30), consumer)
        .await
        .expect("the stream resumes")
        .unwrap();
    assert_eq!(frames, FRAMES);
    assert_eq!(pulled.load(Ordering::SeqCst), FRAMES);
    assert_eq!(metrics.pii_restore_errors_total.load(Ordering::SeqCst), 0);
}

/// A first token slower than `timeout_ms` (a long prefill) does not count
/// against the pipe: the deadline starts with the first delta sent, and a
/// sanitizer that sends its head with its first reply is answered in time.
#[tokio::test]
async fn a_slow_first_token_still_restores() {
    let (stub_addr, _) = serve_stub(Stub {
        head_with_first_reply: true,
        ..Stub::default()
    })
    .await;
    let metrics = Arc::new(Metrics::default());
    let events = stream_events(Dialect::Chat, &segmentations(TEXT)[1], &[], &[], false);
    let first = Bytes::from(events[0].clone());
    let rest: Vec<_> = events[1..]
        .iter()
        .map(|e| Ok::<_, reqwest::Error>(Bytes::from(e.clone())))
        .collect();
    let upstream = futures_util::stream::once(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok::<_, reqwest::Error>(first)
    })
    .chain(futures_util::stream::iter(rest));
    let mut stream = restorer(stub_addr, 200, &metrics).wrap(Box::pin(upstream));
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(reassemble(Dialect::Chat, &body).a, restored(TEXT, false));
    assert_eq!(metrics.pii_restore_errors_total.load(Ordering::SeqCst), 0);
}

/// One chunk holding a whole body — a buffered cache miss or a cached replay —
/// is held to the window like a live stream.
#[tokio::test]
async fn one_large_chunk_never_leaves_more_than_the_window_unanswered() {
    let (stub_addr, stub) = serve_stub(Stub {
        stall_after: Some(0),
        ..Stub::default()
    })
    .await;
    let metrics = Arc::new(Metrics::default());
    let pieces: Vec<String> = (0..2_000).map(|k| format!("w{k} ")).collect();
    let events = stream_events(Dialect::Chat, &pieces, &[], &[], false);
    let whole = Bytes::from(events.concat());
    let restore = restorer(stub_addr, 30_000, &metrics);
    let consumer = tokio::spawn(run(restore, vec![whole]));
    tokio::time::sleep(Duration::from_millis(400)).await;
    // the first line is the unanswered-by-design open line
    let unanswered = stub.received.load(Ordering::SeqCst) - 1;
    assert!(unanswered > 0, "nothing reached the stub");
    assert!(
        unanswered <= crate::pii_stream::MAX_UNANSWERED_LINES,
        "{unanswered} lines unanswered"
    );
    stub.release.notify_waiters();
    let body = tokio::time::timeout(Duration::from_secs(30), consumer)
        .await
        .expect("the stream resumes")
        .unwrap();
    assert_eq!(reassemble(Dialect::Chat, &body).a, pieces.concat());
}

/// A provider that sends `"finish_reason": ""` on every chunk has not finished
/// anything: a placeholder cut across those chunks is still restored.
#[tokio::test]
async fn an_empty_finish_reason_does_not_close_the_channel() {
    let (stub_addr, _) = serve_stub(Stub::default()).await;
    let metrics = Arc::new(Metrics::default());
    let chunks: Vec<Bytes> = TEXT
        .chars()
        .map(|c| {
            Bytes::from(event(
                None,
                &json!({"choices": [{"index": 0, "delta": {"content": c.to_string()}, "finish_reason": ""}]}),
                false,
            ))
        })
        .collect();
    let body = run(restorer(stub_addr, 5_000, &metrics), chunks).await;
    assert_eq!(reassemble(Dialect::Chat, &body).a, restored(TEXT, false));
}

/// A sanitizer that holds back more than any placeholder is not running a
/// placeholder holdback: past the cap the reply is out of contract, and the
/// stream goes on unrestored with nothing lost.
#[tokio::test]
async fn holding_more_than_the_cap_fails_into_an_unrestored_stream() {
    let (stub_addr, _) = serve_stub(Stub {
        hold_all: true,
        ..Stub::default()
    })
    .await;
    let metrics = Arc::new(Metrics::default());
    let text: String = "x".repeat(rolter_core::pii_sanitizer::MAX_STREAM_HELD_CHARS + 50);
    let pieces: Vec<String> = text.chars().map(|c| c.to_string()).collect();
    let events = stream_events(Dialect::Chat, &pieces, &[], &[], false);
    let body = run(
        restorer(stub_addr, 5_000, &metrics),
        events.into_iter().map(Bytes::from).collect(),
    )
    .await;
    assert_eq!(reassemble(Dialect::Chat, &body).a, text);
    assert_eq!(metrics.pii_restore_errors_total.load(Ordering::SeqCst), 1);
}

// ── through the gateway ──────────────────────────────────────────────────────

/// Which stream the upstream serves, chosen by the request's own text.
#[derive(Clone)]
struct Upstream {
    calls: Arc<AtomicUsize>,
    /// set when a never-ending stream's body is dropped
    dropped: Arc<AtomicBool>,
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn case_of(body: &Value) -> String {
    body["messages"][0]["content"]
        .as_str()
        .or_else(|| body["input"].as_str())
        .unwrap_or_default()
        .to_string()
}

fn upstream_stream(dialect: Dialect, case: &str, upstream: &Upstream) -> axum::response::Response {
    upstream.calls.fetch_add(1, Ordering::SeqCst);
    let sse = ([(axum::http::header::CONTENT_TYPE, "text/event-stream")],);
    if case == "forever" {
        let flag = DropFlag(upstream.dropped.clone());
        let frames = futures_util::stream::unfold((0u64, flag), |(k, flag)| async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let frame = event(
                None,
                &json!({"choices": [{"index": 0, "delta": {"content": "<PERSON_1> "}}]}),
                false,
            );
            Some((Ok::<_, std::io::Error>(Bytes::from(frame)), (k + 1, flag)))
        });
        return (sse, Body::from_stream(frames)).into_response();
    }
    if case == "json" {
        // an upstream that ignores `"stream": true`
        return Json(json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi <PERSON_1>"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }))
        .into_response();
    }
    let k: usize = case
        .strip_prefix("case:")
        .and_then(|k| k.parse().ok())
        .unwrap_or(0);
    let (_, a, b, args, escape_lt) = cases().swap_remove(k);
    let events = stream_events(dialect, &a, &b, &args, escape_lt);
    let frames = futures_util::stream::iter(
        events
            .into_iter()
            .map(|e| Ok::<_, std::io::Error>(Bytes::from(e))),
    );
    (sse, Body::from_stream(frames)).into_response()
}

async fn chat_upstream(
    State(up): State<Upstream>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    upstream_stream(Dialect::Chat, &case_of(&body), &up)
}

async fn messages_upstream(
    State(up): State<Upstream>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    upstream_stream(Dialect::Messages, &case_of(&body), &up)
}

async fn responses_upstream(
    State(up): State<Upstream>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    upstream_stream(Dialect::Responses, &case_of(&body), &up)
}

/// A ClickHouse stand-in keeping every insert body.
async fn serve_clickhouse() -> (SocketAddr, Arc<parking_lot::Mutex<Vec<String>>>) {
    let seen: Arc<parking_lot::Mutex<Vec<String>>> = Arc::default();
    let sink = seen.clone();
    let app = Router::new().route(
        "/",
        post(move |body: String| {
            let sink = sink.clone();
            async move { sink.lock().push(body) }
        }),
    );
    (serve(app).await, seen)
}

struct Gateway {
    addr: SocketAddr,
    stub: Arc<Stub>,
    upstream: Upstream,
    state: AppState,
    clickhouse: Arc<parking_lot::Mutex<Vec<String>>>,
    client: reqwest::Client,
}

impl Gateway {
    async fn start(cache: bool) -> Self {
        let upstream = Upstream {
            calls: Arc::default(),
            dropped: Arc::default(),
        };
        let upstream_addr = serve(
            Router::new()
                .route("/v1/chat/completions", post(chat_upstream))
                .route("/v1/messages", post(messages_upstream))
                .route("/v1/responses", post(responses_upstream))
                .with_state(upstream.clone()),
        )
        .await;
        let (stub_addr, stub) = serve_stub(Stub::default()).await;
        let (clickhouse_addr, clickhouse) = serve_clickhouse().await;
        let mut config = GatewayConfig::default();
        config.cache.enabled = cache;
        config.logging.clickhouse_url = Some(format!("http://{clickhouse_addr}"));
        config.logging.flush_ms = 20;
        config.logging.payload_capture.enabled = true;
        config.pii_sanitizer = sanitizer_config(stub_addr, 5_000);
        // `openai` so `/v1/responses` reaches the upstream as itself rather
        // than translated to chat completions
        for (name, kind) in [
            ("openai", ProviderKind::Openai),
            ("anthropic", ProviderKind::Anthropic),
        ] {
            config.providers.push(ProviderConfig {
                name: name.to_string(),
                kind,
                api_base: format!("http://{upstream_addr}"),
                ..Default::default()
            });
        }
        for (model, provider) in [("chat", "openai"), ("claude", "anthropic")] {
            config.routes.push(ModelRoute {
                model: model.to_string(),
                strategy: BalancingStrategy::RoundRobin,
                targets: vec![Target {
                    provider: provider.to_string(),
                    ..Default::default()
                }],
                params: Default::default(),
                param_policy: Default::default(),
                advanced: Default::default(),
                cache: cache.then_some(RouteCache {
                    enabled: true,
                    ttl_secs: Some(600),
                    per_key: false,
                    semantic: None,
                }),
                variants: Default::default(),
                tenancy: None,
            });
        }
        let mut state = AppState::with_logging(&config, None);
        state.response_cache = ResponseCache::in_memory();
        let addr = serve(crate::build_router(
            state.clone(),
            &config.server.metrics_path,
            config.server.max_body_bytes,
        ))
        .await;
        Self {
            addr,
            stub,
            upstream,
            state,
            clickhouse,
            client: reqwest::Client::new(),
        }
    }

    fn request(&self, dialect: Dialect, case: &str) -> reqwest::RequestBuilder {
        let (path, body) = match dialect {
            Dialect::Chat => (
                "/v1/chat/completions",
                json!({"model": "chat", "stream": true, "messages": [{"role": "user", "content": case}]}),
            ),
            Dialect::Messages => (
                "/v1/messages",
                json!({"model": "claude", "stream": true, "max_tokens": 64, "messages": [{"role": "user", "content": case}]}),
            ),
            Dialect::Responses => (
                "/v1/responses",
                json!({"model": "chat", "stream": true, "input": case}),
            ),
        };
        self.client
            .post(format!("http://{}{path}", self.addr))
            .json(&body)
    }

    async fn ask(&self, dialect: Dialect, case: &str) -> (String, Vec<u8>) {
        let response = self.request(dialect, case).send().await.unwrap();
        assert_eq!(response.status(), 200, "{dialect:?} {case}");
        let cache = response
            .headers()
            .get("x-rolter-cache")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        (cache, response.bytes().await.unwrap().to_vec())
    }
}

/// The same matrix on a live stream through the gateway, and — on the two
/// cacheable paths — on a cached SSE replay, restored with the replaying
/// request's own token. The Responses API is never cached.
#[tokio::test]
async fn the_gateway_restores_a_live_stream_and_a_cached_replay() {
    let gateway = Gateway::start(true).await;
    let total = cases().len();
    // a spread of the matrix keeps the end-to-end run short; the direct
    // harness above covers every case
    let mut picks: Vec<usize> = (0..total).step_by(7).chain([1, total - 1]).collect();
    picks.sort_unstable();
    picks.dedup();
    for dialect in [Dialect::Chat, Dialect::Messages, Dialect::Responses] {
        for &k in &picks {
            let case = format!("case:{k}");
            let (cache, body) = gateway.ask(dialect, &case).await;
            assert_ne!(cache, "HIT");
            assert_restored(dialect, &body, &format!("{dialect:?} live {case}"));
            if dialect != Dialect::Responses {
                let (cache, body) = gateway.ask(dialect, &case).await;
                assert_eq!(cache, "HIT", "{dialect:?} {case}");
                assert_restored(dialect, &body, &format!("{dialect:?} replay {case}"));
            }
        }
    }
    assert_eq!(
        gateway
            .state
            .metrics
            .pii_restore_errors_total
            .load(Ordering::SeqCst),
        0
    );
}

/// The stream's accounting still reads the provider's bytes: usage is billed
/// from them and the captured payload holds placeholders, never what the
/// restore put back.
#[tokio::test]
async fn usage_and_capture_read_the_upstream_bytes() {
    let gateway = Gateway::start(false).await;
    // case 0 streams each text in one frame, so the placeholder is whole in
    // the captured body
    let (_, body) = gateway.ask(Dialect::Chat, "case:0").await;
    assert_restored(Dialect::Chat, &body, "chat");
    let rows = wait_for_inserts(&gateway.clickhouse, 2).await;
    let joined = rows.join("\n");
    assert!(joined.contains("\"total_tokens\":10"), "{joined}");
    assert!(joined.contains("PERSON_1>"), "{joined}");
    for (_, value) in MAP {
        assert!(
            !joined.contains(value),
            "a restored value reached a log row"
        );
    }
}

async fn wait_for_inserts(seen: &parking_lot::Mutex<Vec<String>>, count: usize) -> Vec<String> {
    for _ in 0..200 {
        let rows = seen.lock().clone();
        if rows.len() >= count {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("expected {count} inserts, saw {:?}", seen.lock());
}

/// A streamed request the upstream answers with plain JSON has no stream to
/// restore: it goes out as it came, and the restore error counter says so.
#[tokio::test]
async fn a_stream_answered_without_a_stream_is_counted() {
    let gateway = Gateway::start(false).await;
    let response = gateway.request(Dialect::Chat, "json").send().await.unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "hi <PERSON_1>");
    assert_eq!(
        gateway
            .state
            .metrics
            .pii_restore_errors_total
            .load(Ordering::SeqCst),
        1
    );
}

/// A caller that hangs up mid-stream: the sanitizer sees its stream reset and
/// the upstream request is dropped.
#[tokio::test]
async fn a_client_disconnect_resets_the_pipe_and_drops_the_upstream() {
    let gateway = Gateway::start(false).await;
    let response = gateway
        .request(Dialect::Chat, "forever")
        .send()
        .await
        .unwrap();
    let mut body = response.bytes_stream();
    // a few restored frames arrive, then the caller goes away
    let mut seen = Vec::new();
    while seen.len() < 200 {
        seen.extend_from_slice(&body.next().await.unwrap().unwrap());
    }
    assert!(String::from_utf8_lossy(&seen).contains("Kowalski"));
    drop(body);
    for _ in 0..200 {
        if gateway.stub.saw_reset.load(Ordering::SeqCst)
            && gateway.upstream.dropped.load(Ordering::SeqCst)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(gateway.stub.saw_reset.load(Ordering::SeqCst), "pipe reset");
    assert!(!gateway.stub.saw_end.load(Ordering::SeqCst));
    assert!(
        gateway.upstream.dropped.load(Ordering::SeqCst),
        "upstream dropped"
    );
}

/// The leak test from the ADR: a restoring stream with `tracing` at TRACE
/// into a capturing subscriber and payload capture on. No restored value
/// appears in a log line, in the captured payload or in the metrics.
#[tokio::test]
async fn no_restored_value_reaches_a_log_line_the_capture_or_the_metrics() {
    #[derive(Clone, Default)]
    struct Capture(Arc<parking_lot::Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    // this test's runtime is single-threaded, so every task it spawns logs here
    let _guard = tracing::subscriber::set_default(subscriber);

    let gateway = Gateway::start(true).await;
    for dialect in [Dialect::Chat, Dialect::Messages, Dialect::Responses] {
        let (_, body) = gateway.ask(dialect, "case:5").await;
        assert_restored(dialect, &body, &format!("{dialect:?}"));
    }
    // and a replay, restored again from the cache
    let (cache, _) = gateway.ask(Dialect::Chat, "case:5").await;
    assert_eq!(cache, "HIT");
    let rows = wait_for_inserts(&gateway.clickhouse, 2).await;
    let metrics = gateway
        .client
        .get(format!("http://{}/metrics", gateway.addr))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    let logs = String::from_utf8_lossy(&capture.0.lock()).to_string();
    assert!(logs.lines().count() > 10, "the subscriber captured nothing");
    let captured = rows.join("\n");
    for (_, value) in MAP {
        for (place, text) in [
            ("a log line", &logs),
            ("the captured payload", &captured),
            ("the metrics", &metrics),
        ] {
            assert!(!text.contains(value), "{value:?} reached {place}");
            assert!(
                !text.contains(&escaped(value)),
                "{value:?} reached {place}, escaped"
            );
        }
    }
    assert!(!logs.contains("tok-stream"), "the token reached a log line");
}
