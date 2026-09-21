//! Which requests the semantic response cache may answer, and which stored
//! responses each one may be answered with (#1476).
//!
//! A semantic hit replays a response that was produced for a *different*
//! request, so similarity of the conversation text is only half of the
//! question. Two requests can read alike and still expect incompatible
//! replies: one asked for an SSE stream and the other for a JSON body, one
//! offered a tool schema or demanded a JSON-schema answer and the other did
//! not, or they carry different system instructions. None of that is visible
//! to an embedding of the user's words.
//!
//! [`classify`] therefore splits a request into two parts:
//!
//! - the **text** that is embedded and compared by cosine similarity: the
//!   `user` and `assistant` turns of the conversation, or a completion prompt;
//! - a **partition** digest over *everything else* in the body — `stream`,
//!   `tools`, `tool_choice`, `response_format`, sampling parameters, the model,
//!   and system or developer instructions from either dialect. Candidates are
//!   only ever compared inside one partition, so a reply can only be replayed
//!   to a request that agrees with it on every one of those fields exactly.
//!
//! The partition is deliberately an allowlist of what may vary rather than a
//! list of what may not: a field the gateway has never heard of lands in the
//! partition and splits the cache, which costs hit rate but never correctness.
//!
//! A request whose meaning the text would not capture is not looked up at all:
//! images, audio, files and other non-text content parts, tool calls and tool
//! results inside the conversation, batch prompts, and every endpoint other
//! than chat completions, Anthropic messages and legacy completions. Those
//! requests still use the exact-match cache.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Version of the semantic index layout and of the partition rules above.
///
/// It is part of every semantic index key, so bumping it orphans every entry
/// written under the old rules — they are never read again and expire on their
/// TTL. Entries written before #1476 had no partition at all and could replay
/// an SSE stream to a JSON caller; this is what keeps them from doing so after
/// an upgrade.
pub(crate) const SEMANTIC_LAYOUT_VERSION: &str = "v2";

/// A request the semantic cache may answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SemanticRequest {
    /// Normalised conversation text sent to the embedding model.
    pub text: String,
    /// Hex sha-256 of the canonical form of every other request field.
    pub partition: String,
}

/// Classify a post-injection request body for the semantic cache. `None` means
/// the request must bypass semantic lookup (it may still use the exact cache).
pub(crate) fn classify(path: &str, body: &[u8]) -> Option<SemanticRequest> {
    let dialect = match path {
        "/v1/chat/completions" => Dialect::OpenAiChat,
        "/v1/messages" => Dialect::Anthropic,
        "/v1/completions" => Dialect::Completions,
        _ => return None,
    };
    let Value::Object(mut fields) = serde_json::from_slice::<Value>(body).ok()? else {
        return None;
    };

    let mut text = String::with_capacity(body.len().min(1024));
    // system and developer turns are matched exactly, never by similarity:
    // "answer in German" and "answer in Japanese" embed almost identically
    let mut instructions = Vec::new();
    match dialect {
        Dialect::OpenAiChat | Dialect::Anthropic => {
            let Value::Array(messages) = fields.remove("messages")? else {
                return None;
            };
            for (index, message) in messages.iter().enumerate() {
                let message = message.as_object()?;
                if message
                    .keys()
                    .any(|key| !matches!(key.as_str(), "role" | "content" | "name"))
                {
                    // tool_calls, function_call, tool_call_id, audio, refusal:
                    // a turn whose meaning is not all in its text
                    return None;
                }
                let role = message.get("role")?.as_str()?;
                let parts = text_parts(message.get("content")?)?;
                match (dialect, role) {
                    (_, "user" | "assistant") => {
                        append_normalized(role, &mut text);
                        if let Some(name) = message.get("name") {
                            append_normalized(name.as_str()?, &mut text);
                        }
                        for part in parts {
                            append_normalized(part, &mut text);
                        }
                    }
                    (Dialect::OpenAiChat, "system" | "developer") => {
                        instructions.push(Value::Array(vec![
                            Value::from(index),
                            Value::Object(message.clone()),
                        ]));
                    }
                    _ => return None,
                }
            }
            if dialect == Dialect::Anthropic {
                // top-level `system` stays in the partition as-is; only its
                // shape is checked, so a non-text system block bypasses
                if let Some(system) = fields.get("system") {
                    text_parts(system)?;
                }
            }
        }
        Dialect::Completions => {
            // an array prompt is a batch of completions, or token ids
            let Value::String(prompt) = fields.remove("prompt")? else {
                return None;
            };
            append_normalized(&prompt, &mut text);
        }
    }
    if text.is_empty() {
        return None;
    }

    // caller attribution does not change the reply; keeping it would give each
    // end user a private cache on a route that is meant to be shared
    fields.remove("user");
    fields.remove("metadata");
    // an omitted `stream` and `stream: false` ask for the same JSON body
    if fields.get("stream") == Some(&Value::Bool(false)) {
        fields.remove("stream");
    }
    if !instructions.is_empty() {
        // a key no request body can carry, so it cannot collide with a field
        fields.insert("\u{0}instructions".to_string(), Value::Array(instructions));
    }

    let mut hasher = Sha256::new();
    hasher.update(dialect.tag());
    hasher.update([0x1f]);
    hash_canonical(&Value::Object(fields), &mut hasher);
    Some(SemanticRequest {
        text,
        partition: rolter_auth::hex::encode(&hasher.finalize()),
    })
}

/// Whether a stored response is the shape the caller asked for. The partition
/// already separates streaming from non-streaming callers; this is the last
/// check before bytes leave, so an entry from any other writer still cannot
/// hand an SSE body to a JSON caller or the reverse.
pub(crate) fn replay_matches_mode(stream: bool, content_type: &str) -> bool {
    let is_sse = content_type
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"));
    is_sse == stream
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    OpenAiChat,
    Anthropic,
    Completions,
}

impl Dialect {
    fn tag(self) -> &'static [u8] {
        match self {
            Dialect::OpenAiChat => b"openai-chat",
            Dialect::Anthropic => b"anthropic-messages",
            Dialect::Completions => b"openai-completions",
        }
    }
}

/// The text of a message `content` (or an Anthropic `system`), or `None` when
/// any part of it is not plain text.
fn text_parts(content: &Value) -> Option<Vec<&str>> {
    match content {
        Value::String(text) => Some(vec![text.as_str()]),
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                let part = part.as_object()?;
                if part.get("type")?.as_str()? != "text"
                    || part
                        .keys()
                        .any(|key| !matches!(key.as_str(), "type" | "text" | "cache_control"))
                {
                    return None;
                }
                part.get("text")?.as_str()
            })
            .collect(),
        _ => None,
    }
}

fn append_normalized(text: &str, out: &mut String) {
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
}

/// Feed a value into `hasher` with object keys sorted, so the digest does not
/// depend on the order a client wrote its fields in, nor on whether
/// `serde_json` was built with `preserve_order`.
fn hash_canonical(value: &Value, hasher: &mut Sha256) {
    match value {
        Value::Object(map) => {
            hasher.update(b"{");
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            for key in keys {
                hash_canonical(&Value::String(key.clone()), hasher);
                hasher.update(b":");
                hash_canonical(&map[key], hasher);
                hasher.update(b",");
            }
            hasher.update(b"}");
        }
        Value::Array(items) => {
            hasher.update(b"[");
            for item in items {
                hash_canonical(item, hasher);
                hasher.update(b",");
            }
            hasher.update(b"]");
        }
        // scalars serialise unambiguously; strings are quoted and escaped
        scalar => hasher.update(scalar.to_string().as_bytes()),
    }
}

#[cfg(test)]
mod http_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn class(path: &str, body: Value) -> Option<SemanticRequest> {
        classify(path, &serde_json::to_vec(&body).unwrap())
    }

    fn chat(body: Value) -> SemanticRequest {
        class("/v1/chat/completions", body).expect("a supported chat shape")
    }

    #[test]
    fn chat_text_is_the_conversation_and_excludes_instructions() {
        let request = chat(serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": [{"type": "text", "text": "hello   world"}]},
                {"role": "assistant", "content": "hi"},
                {"role": "user", "name": "ann", "content": "again"}
            ]
        }));
        assert_eq!(request.text, "user hello world assistant hi user ann again");
    }

    #[test]
    fn stream_mode_splits_the_partition_but_false_equals_absent() {
        let base =
            serde_json::json!({"model": "m", "messages": [{"role": "user", "content": "hi"}]});
        let mut streaming = base.clone();
        streaming["stream"] = Value::Bool(true);
        let mut explicit_false = base.clone();
        explicit_false["stream"] = Value::Bool(false);
        assert_ne!(chat(base.clone()).partition, chat(streaming).partition);
        assert_eq!(chat(base).partition, chat(explicit_false).partition);
    }

    #[test]
    fn every_contract_field_splits_the_partition() {
        let base =
            serde_json::json!({"model": "m", "messages": [{"role": "user", "content": "hi"}]});
        let base_partition = chat(base.clone()).partition;
        for (field, value) in [
            (
                "tools",
                serde_json::json!([{"type": "function", "function": {"name": "send_email"}}]),
            ),
            ("tool_choice", serde_json::json!("required")),
            (
                "response_format",
                serde_json::json!({"type": "json_object"}),
            ),
            ("temperature", serde_json::json!(1.5)),
            ("max_tokens", serde_json::json!(5)),
            ("n", serde_json::json!(3)),
            ("stream_options", serde_json::json!({"include_usage": true})),
            ("model", serde_json::json!("other")),
            ("some_future_field", serde_json::json!(true)),
        ] {
            let mut body = base.clone();
            body[field] = value;
            assert_ne!(
                chat(body).partition,
                base_partition,
                "{field} must partition"
            );
        }
    }

    #[test]
    fn tool_schemas_that_differ_only_in_name_do_not_share() {
        let tools = |name: &str| {
            serde_json::json!({
                "model": "m",
                "messages": [{"role": "user", "content": "do it"}],
                "tools": [{"type": "function", "function": {"name": name, "parameters": {}}}]
            })
        };
        let read = chat(tools("read_report"));
        let send = chat(tools("send_email"));
        assert_eq!(read.text, send.text);
        assert_ne!(read.partition, send.partition);
    }

    #[test]
    fn attribution_fields_and_key_order_do_not_split() {
        let a: Value = serde_json::from_str(
            r#"{"model":"m","temperature":0,"messages":[{"role":"user","content":"hi"}],"user":"u1"}"#,
        )
        .unwrap();
        let b: Value = serde_json::from_str(
            r#"{"messages":[{"role":"user","content":"hi"}],"temperature":0,"model":"m","user":"u2","metadata":{"x":"y"}}"#,
        )
        .unwrap();
        assert_eq!(chat(a).partition, chat(b).partition);
    }

    #[test]
    fn openai_system_and_developer_instructions_are_exact_partition_keys() {
        let with = |role: &str, text: &str| {
            chat(serde_json::json!({
                "model": "m",
                "messages": [{"role": role, "content": text}, {"role": "user", "content": "hello"}]
            }))
        };
        let german = with("system", "Answer in German");
        let japanese = with("system", "Answer in Japanese");
        let developer = with("developer", "Answer in German");
        assert_eq!(german.text, japanese.text);
        assert_ne!(german.partition, japanese.partition);
        assert_ne!(german.partition, developer.partition);
    }

    #[test]
    fn anthropic_top_level_system_is_an_exact_partition_key() {
        let with = |system: Value| {
            class(
                "/v1/messages",
                serde_json::json!({
                    "model": "claude", "max_tokens": 16, "system": system,
                    "messages": [{"role": "user", "content": "hello"}]
                }),
            )
            .expect("a supported anthropic shape")
        };
        let german = with(Value::from("Answer in German"));
        let japanese = with(Value::from("Answer in Japanese"));
        let blocks = with(serde_json::json!([{"type": "text", "text": "Answer in German"}]));
        assert_eq!(german.text, "user hello");
        assert_eq!(german.text, japanese.text);
        assert_ne!(german.partition, japanese.partition);
        assert_ne!(german.partition, blocks.partition);
    }

    #[test]
    fn dialects_never_share_a_partition() {
        let body =
            serde_json::json!({"model": "m", "messages": [{"role": "user", "content": "hi"}]});
        let openai = class("/v1/chat/completions", body.clone()).unwrap();
        let anthropic = class("/v1/messages", body).unwrap();
        assert_eq!(openai.text, anthropic.text);
        assert_ne!(openai.partition, anthropic.partition);
    }

    #[test]
    fn completions_accept_one_string_prompt_only() {
        let single = class(
            "/v1/completions",
            serde_json::json!({"model": "m", "prompt": " once\tupon "}),
        )
        .unwrap();
        assert_eq!(single.text, "once upon");
        for prompt in [serde_json::json!(["a", "b"]), serde_json::json!([1, 2, 3])] {
            assert_eq!(
                class(
                    "/v1/completions",
                    serde_json::json!({"model": "m", "prompt": prompt})
                ),
                None
            );
        }
    }

    #[test]
    fn unsupported_shapes_bypass() {
        let bypassed = [
            // non-text content, both dialects
            (
                "/v1/chat/completions",
                serde_json::json!({"messages": [{"role": "user", "content": [
                {"type": "text", "text": "what is this"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}]}]}),
            ),
            (
                "/v1/chat/completions",
                serde_json::json!({"messages": [{"role": "user", "content": [
                {"type": "input_audio", "input_audio": {"data": "AAAA", "format": "wav"}}]}]}),
            ),
            (
                "/v1/messages",
                serde_json::json!({"messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}]}]}),
            ),
            (
                "/v1/messages",
                serde_json::json!({"system": [{"type": "image", "source": {}}],
                "messages": [{"role": "user", "content": "hi"}]}),
            ),
            // tool traffic inside the conversation
            (
                "/v1/chat/completions",
                serde_json::json!({"messages": [
                {"role": "user", "content": "weather?"},
                {"role": "assistant", "content": null, "tool_calls": [{"id": "1"}]},
                {"role": "tool", "tool_call_id": "1", "content": "sunny"}]}),
            ),
            (
                "/v1/messages",
                serde_json::json!({"messages": [{"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "1", "content": "sunny"}]}]}),
            ),
            // anthropic has no system role inside messages
            (
                "/v1/messages",
                serde_json::json!({"messages": [{"role": "system", "content": "x"}]}),
            ),
            // nothing to compare
            ("/v1/chat/completions", serde_json::json!({"messages": []})),
            (
                "/v1/chat/completions",
                serde_json::json!({"messages": [{"role": "system", "content": "x"}]}),
            ),
            // endpoints whose output must match the input exactly
            (
                "/v1/embeddings",
                serde_json::json!({"model": "e", "input": "hello"}),
            ),
            (
                "/v1/audio/speech",
                serde_json::json!({"model": "tts", "input": "hello"}),
            ),
            (
                "/v1/images/generations",
                serde_json::json!({"model": "img", "prompt": "a cat"}),
            ),
            (
                "/v1/responses",
                serde_json::json!({"model": "m", "input": "hello"}),
            ),
        ];
        for (path, body) in bypassed {
            assert_eq!(class(path, body.clone()), None, "{path} {body}");
        }
        assert_eq!(classify("/v1/chat/completions", b"not json"), None);
    }

    #[test]
    fn replay_mode_follows_the_content_type() {
        assert!(replay_matches_mode(true, "text/event-stream"));
        assert!(replay_matches_mode(
            true,
            "text/event-stream; charset=utf-8"
        ));
        assert!(replay_matches_mode(false, "application/json"));
        assert!(!replay_matches_mode(false, "text/event-stream"));
        assert!(!replay_matches_mode(true, "application/json"));
    }
}
