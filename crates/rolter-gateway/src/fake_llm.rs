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

/// collect the `documents` field: an array of strings, or objects carrying a
/// `text` field (Cohere/Jina both accept the latter)
fn rerank_documents(body: &Value) -> Vec<String> {
    match body.get("documents") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Object(_) => v
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string()),
                other => other.to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// deterministic relevance score in (0, 1) derived from query+document bytes so
/// repeated calls rank identically (stable smoke tests)
fn fake_score(query: &str, doc: &str) -> f64 {
    let mut seed: u64 = 1469598103934665603; // fnv offset basis
    for b in query.bytes().chain(std::iter::once(0)).chain(doc.bytes()) {
        seed ^= b as u64;
        seed = seed.wrapping_mul(1099511628211); // fnv prime
    }
    // map to (0, 1), avoiding exact 0/1 endpoints
    ((seed >> 11) as f64 + 1.0) / ((1u64 << 53) as f64 + 2.0)
}

/// Handle a `/v1/rerank` request for `fake-llm` (Cohere/Jina-compatible).
pub fn rerank(body: &Value) -> Response {
    let query = match body.get("query").and_then(Value::as_str) {
        Some(q) => q,
        None => {
            return crate::error::ApiError::new(StatusCode::BAD_REQUEST, "missing 'query' field")
                .with_code("missing_required_parameter")
                .with_param("query")
                .into_response()
        }
    };
    let documents = rerank_documents(body);
    if documents.is_empty() {
        return crate::error::ApiError::new(
            StatusCode::BAD_REQUEST,
            "missing or empty 'documents' field",
        )
        .with_code("missing_required_parameter")
        .with_param("documents")
        .into_response();
    }
    let return_documents = body
        .get("return_documents")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // score every document, then rank by descending relevance
    let mut ranked: Vec<(usize, f64)> = documents
        .iter()
        .enumerate()
        .map(|(i, doc)| (i, fake_score(query, doc)))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));

    // honor top_n when the caller caps the result count
    if let Some(top_n) = body.get("top_n").and_then(Value::as_u64) {
        ranked.truncate(top_n as usize);
    }

    let results: Vec<Value> = ranked
        .iter()
        .map(|&(index, score)| {
            let mut entry = json!({"index": index, "relevance_score": score});
            if return_documents {
                entry["document"] = json!({"text": documents[index]});
            }
            entry
        })
        .collect();

    let prompt_tokens: u64 =
        approx_tokens(query) + documents.iter().map(|d| approx_tokens(d)).sum::<u64>();
    let payload = json!({
        "model": MODEL_NAME,
        "results": results,
        "usage": {
            "prompt_tokens": prompt_tokens,
            "total_tokens": prompt_tokens,
        },
    });
    Json(payload).into_response()
}

/// a 1x1 transparent PNG, base64-encoded — the deterministic pixel the fake
/// image model returns for every request
const FAKE_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

/// Handle a `/v1/images/generations` request for `fake-llm` (OpenAI-compatible).
pub fn images(body: &Value) -> Response {
    if body
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .is_none()
    {
        return crate::error::ApiError::new(StatusCode::BAD_REQUEST, "missing 'prompt' field")
            .with_code("missing_required_parameter")
            .with_param("prompt")
            .into_response();
    }
    // number of images to return (OpenAI defaults to 1, caps at 10)
    let n = body
        .get("n")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 10);
    // "url" (default) returns a self-contained data URI; "b64_json" the raw base64
    let as_url = body
        .get("response_format")
        .and_then(Value::as_str)
        .map(|f| f != "b64_json")
        .unwrap_or(true);

    let data: Vec<Value> = (0..n)
        .map(|_| {
            if as_url {
                json!({"url": format!("data:image/png;base64,{FAKE_PNG_B64}")})
            } else {
                json!({"b64_json": FAKE_PNG_B64})
            }
        })
        .collect();

    Json(json!({"created": unix_now(), "data": data})).into_response()
}

/// build a minimal but valid WAV container: a 44-byte PCM header describing
/// `samples` bytes of 8-bit mono 8kHz audio, followed by that many silence
/// (0x80 mid-point) samples. deterministic and playable by any decoder
fn fake_wav(samples: usize) -> Vec<u8> {
    let data_len = samples as u32;
    let mut wav = Vec::with_capacity(44 + samples);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes()); // chunk size
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // subchunk1 size (PCM)
    wav.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // channels = mono
    wav.extend_from_slice(&8000u32.to_le_bytes()); // sample rate
    wav.extend_from_slice(&8000u32.to_le_bytes()); // byte rate
    wav.extend_from_slice(&1u16.to_le_bytes()); // block align
    wav.extend_from_slice(&8u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.resize(44 + samples, 0x80); // 8-bit PCM silence (mid-point)
    wav
}

/// Handle a `/v1/audio/speech` request for `fake-llm` (OpenAI-compatible).
/// Returns a short silent WAV clip so the binary-response path is exercisable
/// without an upstream TTS provider.
pub fn speech(body: &Value) -> Response {
    let input = match body.get("input").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => s,
        _ => {
            return crate::error::ApiError::new(StatusCode::BAD_REQUEST, "missing 'input' field")
                .with_code("missing_required_parameter")
                .with_param("input")
                .into_response()
        }
    };
    // clip length scales with the input word count (bounded), so different
    // requests yield different-sized but deterministic audio
    let samples = (approx_tokens(input) as usize * 100).clamp(100, 8000);
    let wav = fake_wav(samples);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "audio/wav")
        .body(Body::from(wav))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build audio").into_response()
        })
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
    async fn rerank_ranks_and_respects_top_n() {
        let req = json!({
            "model": MODEL_NAME,
            "query": "capital of france",
            "documents": ["paris", "berlin", "rome", "madrid"],
            "top_n": 2,
            "return_documents": true,
        });
        let resp = rerank(&req);
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let results = value["results"].as_array().unwrap();
        assert_eq!(results.len(), 2); // top_n cap
                                      // sorted by descending relevance_score
        let s0 = results[0]["relevance_score"].as_f64().unwrap();
        let s1 = results[1]["relevance_score"].as_f64().unwrap();
        assert!(s0 >= s1);
        // return_documents surfaces the original text
        assert!(results[0]["document"]["text"].is_string());
        assert!(value["usage"]["prompt_tokens"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn rerank_deterministic_scores() {
        let req = json!({"model": MODEL_NAME, "query": "q", "documents": ["a", "b"]});
        let first = to_bytes(rerank(&req).into_body(), usize::MAX)
            .await
            .unwrap();
        let second = to_bytes(rerank(&req).into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn rerank_missing_query_or_documents_is_400() {
        let no_query = rerank(&json!({"model": MODEL_NAME, "documents": ["a"]}));
        assert_eq!(no_query.status(), StatusCode::BAD_REQUEST);
        let no_docs = rerank(&json!({"model": MODEL_NAME, "query": "q"}));
        assert_eq!(no_docs.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn images_default_url_and_count() {
        let resp = images(&json!({"model": MODEL_NAME, "prompt": "a cat", "n": 3}));
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let data = value["data"].as_array().unwrap();
        assert_eq!(data.len(), 3);
        assert!(data[0]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }

    #[tokio::test]
    async fn images_b64_json_format() {
        let resp =
            images(&json!({"model": MODEL_NAME, "prompt": "a cat", "response_format": "b64_json"}));
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["data"].as_array().unwrap().len(), 1);
        assert!(value["data"][0]["b64_json"].is_string());
    }

    #[tokio::test]
    async fn images_missing_prompt_is_400() {
        let resp = images(&json!({"model": MODEL_NAME}));
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn speech_returns_valid_wav() {
        let resp = speech(&json!({"model": MODEL_NAME, "input": "hello there", "voice": "alloy"}));
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "audio/wav");
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        // RIFF/WAVE magic + a data chunk consistent with the declared length
        assert_eq!(&body[0..4], b"RIFF");
        assert_eq!(&body[8..12], b"WAVE");
        assert!(body.len() > 44);
        let declared = u32::from_le_bytes([body[4], body[5], body[6], body[7]]) as usize;
        assert_eq!(declared, body.len() - 8);
    }

    #[tokio::test]
    async fn speech_deterministic() {
        let req = json!({"model": MODEL_NAME, "input": "same text"});
        let a = to_bytes(speech(&req).into_body(), usize::MAX)
            .await
            .unwrap();
        let b = to_bytes(speech(&req).into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn speech_missing_input_is_400() {
        let resp = speech(&json!({"model": MODEL_NAME}));
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
