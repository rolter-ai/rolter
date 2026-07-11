//! The built-in `fake-llm` model: a deterministic, network-free responder
//! exposed over both the OpenAI and Anthropic surfaces so rolter can start
//! and serve traffic with zero upstream providers configured (smoke-test /
//! local dev / CI without secrets).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use futures_util::stream;
use serde_json::{json, Value};

pub const MODEL_NAME: &str = "fake-llm";

const LOREM: &[&str] = &[
    "Lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet,",
    "consectetur",
    "adipiscing",
    "elit.",
    "Sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua.",
];

fn lorem_text() -> String {
    LOREM.join(" ")
}

fn next_id(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{ts:x}{n:x}")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Rough token estimate: whitespace-delimited word count. Approximate but
/// present and internally consistent, matching the spec's requirement.
fn approx_tokens(text: &str) -> u64 {
    text.split_whitespace().count().max(1) as u64
}

fn is_streaming(body: &Value) -> bool {
    body.get("stream").and_then(Value::as_bool).unwrap_or(false)
}

fn sse_response(chunks: Vec<String>) -> Response {
    let body_stream = stream::iter(
        chunks
            .into_iter()
            .map(|chunk| Ok::<_, std::convert::Infallible>(Bytes::from(chunk))),
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build stream").into_response()
        })
}

/// Best-effort prompt size: word count over every message content string,
/// including Anthropic-style content-block arrays.
fn extract_prompt_len(body: &Value) -> u64 {
    let mut total = 0usize;
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for m in messages {
            match m.get("content") {
                Some(Value::String(content)) => total += content.split_whitespace().count(),
                Some(Value::Array(blocks)) => {
                    for block in blocks {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            total += text.split_whitespace().count();
                        }
                    }
                }
                _ => {}
            }
        }
    }
    total.max(1) as u64
}

/// Handle a `/v1/chat/completions` request for `fake-llm` (OpenAI-compatible).
pub fn chat_completions(body: &Value) -> Response {
    let prompt_tokens = extract_prompt_len(body);
    let text = lorem_text();
    let completion_tokens = approx_tokens(&text);

    if !is_streaming(body) {
        let payload = json!({
            "id": next_id("chatcmpl-fake"),
            "object": "chat.completion",
            "created": unix_now(),
            "model": MODEL_NAME,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop",
            }],
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": prompt_tokens + completion_tokens,
            },
        });
        return Json(payload).into_response();
    }

    let id = next_id("chatcmpl-fake");
    let created = unix_now();
    let mut chunks = Vec::new();

    let role_chunk = json!({
        "id": id, "object": "chat.completion.chunk", "created": created, "model": MODEL_NAME,
        "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": Value::Null}],
    });
    chunks.push(format!("data: {role_chunk}\n\n"));

    for word in LOREM {
        let delta = json!({
            "id": id, "object": "chat.completion.chunk", "created": created, "model": MODEL_NAME,
            "choices": [{"index": 0, "delta": {"content": format!("{word} ")}, "finish_reason": Value::Null}],
        });
        chunks.push(format!("data: {delta}\n\n"));
    }

    let stop_chunk = json!({
        "id": id, "object": "chat.completion.chunk", "created": created, "model": MODEL_NAME,
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
    });
    chunks.push(format!("data: {stop_chunk}\n\n"));
    chunks.push("data: [DONE]\n\n".to_string());

    sse_response(chunks)
}

/// Handle a `/v1/messages` request for `fake-llm` (Anthropic-compatible).
pub fn messages(body: &Value) -> Response {
    let text = lorem_text();
    let output_tokens = approx_tokens(&text);
    let input_tokens = extract_prompt_len(body);

    if !is_streaming(body) {
        let payload = json!({
            "id": next_id("msg-fake"),
            "type": "message",
            "role": "assistant",
            "model": MODEL_NAME,
            "content": [{"type": "text", "text": text}],
            "stop_reason": "end_turn",
            "stop_sequence": Value::Null,
            "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens},
        });
        return Json(payload).into_response();
    }

    let id = next_id("msg-fake");
    let mut chunks = Vec::new();

    let message_start = json!({
        "type": "message_start",
        "message": {
            "id": id, "type": "message", "role": "assistant", "model": MODEL_NAME,
            "content": [], "stop_reason": Value::Null, "stop_sequence": Value::Null,
            "usage": {"input_tokens": input_tokens, "output_tokens": 0},
        },
    });
    chunks.push(sse_event("message_start", &message_start));

    let content_block_start = json!({
        "type": "content_block_start",
        "index": 0,
        "content_block": {"type": "text", "text": ""},
    });
    chunks.push(sse_event("content_block_start", &content_block_start));

    for word in LOREM {
        let delta = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": format!("{word} ")},
        });
        chunks.push(sse_event("content_block_delta", &delta));
    }

    chunks.push(sse_event(
        "content_block_stop",
        &json!({"type": "content_block_stop", "index": 0}),
    ));
    chunks.push(sse_event(
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": Value::Null},
            "usage": {"output_tokens": output_tokens},
        }),
    ));
    chunks.push(sse_event("message_stop", &json!({"type": "message_stop"})));

    sse_response(chunks)
}

/// dimensionality of the deterministic fake embedding vectors
const EMBED_DIM: usize = 8;

/// collect the `input` field into the list of strings to embed. openai accepts
/// a bare string or an array of strings (token-id arrays are not supported by
/// the fake model and fall back to their debug form)
fn embedding_inputs(body: &Value) -> Vec<String> {
    match body.get("input") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// deterministic unit-norm-ish vector derived from the input bytes so repeated
/// calls with the same text return identical embeddings (stable smoke tests)
fn fake_vector(text: &str) -> Vec<f64> {
    let mut seed: u64 = 1469598103934665603; // fnv offset basis
    for b in text.bytes() {
        seed ^= b as u64;
        seed = seed.wrapping_mul(1099511628211); // fnv prime
    }
    (0..EMBED_DIM)
        .map(|i| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            // map to a stable [-1, 1) value; index perturbs the stream per dim
            let v = ((seed >> 11) ^ (i as u64)) as f64 / (1u64 << 53) as f64;
            (v * 2.0) - 1.0
        })
        .collect()
}

/// Handle a `/v1/embeddings` request for `fake-llm` (OpenAI-compatible).
pub fn embeddings(body: &Value) -> Response {
    let inputs = embedding_inputs(body);
    if inputs.is_empty() {
        return crate::error::ApiError::new(
            StatusCode::BAD_REQUEST,
            "missing or empty 'input' field",
        )
        .with_code("missing_required_parameter")
        .with_param("input")
        .into_response();
    }

    let prompt_tokens: u64 = inputs.iter().map(|s| approx_tokens(s)).sum();
    let data: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(index, text)| {
            json!({
                "object": "embedding",
                "index": index,
                "embedding": fake_vector(text),
            })
        })
        .collect();

    let payload = json!({
        "object": "list",
        "data": data,
        "model": MODEL_NAME,
        "usage": {
            "prompt_tokens": prompt_tokens,
            "total_tokens": prompt_tokens,
        },
    });
    Json(payload).into_response()
}

fn sse_event(event: &str, data: &Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn openai_non_streaming_shape() {
        let resp = chat_completions(&json!({"model": MODEL_NAME, "messages": []}));
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["object"], "chat.completion");
        assert_eq!(value["model"], MODEL_NAME);
        assert_eq!(value["choices"][0]["message"]["role"], "assistant");
        assert_eq!(value["choices"][0]["finish_reason"], "stop");
        assert!(value["usage"]["total_tokens"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn openai_streaming_ends_with_done() {
        let resp = chat_completions(&json!({"model": MODEL_NAME, "stream": true, "messages": []}));
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.trim_end().ends_with("data: [DONE]"));
        assert!(text.contains("\"role\":\"assistant\""));
        assert!(text.contains("\"finish_reason\":\"stop\""));
    }

    #[tokio::test]
    async fn anthropic_non_streaming_shape() {
        let resp = messages(&json!({"model": MODEL_NAME, "messages": []}));
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["type"], "message");
        assert_eq!(value["role"], "assistant");
        assert_eq!(value["stop_reason"], "end_turn");
        assert!(value["usage"]["output_tokens"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn anthropic_streaming_has_full_event_sequence() {
        let resp = messages(&json!({"model": MODEL_NAME, "stream": true, "messages": []}));
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        for event in [
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ] {
            assert!(text.contains(&format!("event: {event}")), "missing {event}");
        }
    }

    #[tokio::test]
    async fn embeddings_string_input_shape() {
        let resp = embeddings(&json!({"model": MODEL_NAME, "input": "hello world"}));
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["object"], "list");
        assert_eq!(value["model"], MODEL_NAME);
        assert_eq!(value["data"].as_array().unwrap().len(), 1);
        assert_eq!(value["data"][0]["object"], "embedding");
        assert_eq!(value["data"][0]["index"], 0);
        assert_eq!(
            value["data"][0]["embedding"].as_array().unwrap().len(),
            EMBED_DIM
        );
        assert!(value["usage"]["prompt_tokens"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn embeddings_array_input_and_determinism() {
        let req = json!({"model": MODEL_NAME, "input": ["alpha", "beta", "alpha"]});
        let resp = embeddings(&req);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let data = value["data"].as_array().unwrap();
        assert_eq!(data.len(), 3);
        // same input text yields the same vector (deterministic)
        assert_eq!(data[0]["embedding"], data[2]["embedding"]);
        assert_ne!(data[0]["embedding"], data[1]["embedding"]);
    }

    #[tokio::test]
    async fn embeddings_missing_input_is_400() {
        let resp = embeddings(&json!({"model": MODEL_NAME}));
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn prompt_len_counts_anthropic_content_blocks() {
        let body = json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "one two three"}]},
            {"role": "user", "content": "four five"},
        ]});
        assert_eq!(extract_prompt_len(&body), 5);
    }
}
