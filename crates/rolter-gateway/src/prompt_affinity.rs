//! The text a request's prefix affinity is decided on (#1851).
//!
//! Strategies that route on a prompt prefix — `cache_aware`, the pipeline's
//! prefix scorer, `consistent_hash` without a session, the latency predictor's
//! token estimate — used to read the raw request body. Every chat request to
//! a model opens with the same JSON envelope
//! (`{"model":"…","messages":[{"role":"user","content":"…`), and that alone
//! cleared `cache_aware`'s match threshold, so unrelated requests all "shared
//! a prefix" and piled onto whichever replica had served first.
//!
//! This reads what the upstream actually caches instead: the conversation, in
//! the order the model sees it, each turn led by its role so a user turn and
//! an assistant turn with the same words stay distinct. Only text counts — an
//! image part or a tool call contributes nothing — and the result is bounded,
//! because affinity is decided by the leading bytes.

use serde_json::Value;

/// Upper bound on the extracted text. Which replica holds a prompt's prefix is
/// decided by its leading bytes, so the tail of a long prompt changes nothing
/// about the pick, and bounding it bounds the trie walk each request pays.
pub(crate) const MAX_AFFINITY_BYTES: usize = 32 * 1024;

/// The affinity text of a JSON request to `path`, or `None` when the body has
/// no prompt this knows how to read, so the caller keeps its own fallback.
pub(crate) fn affinity_text(path: &str, body: &Value) -> Option<String> {
    let mut out = String::new();
    match path {
        "/v1/chat/completions" => turns(&mut out, body.get("messages")?),
        "/v1/messages" => {
            if let Some(system) = body.get("system") {
                push_turn(&mut out, "system", system);
            }
            turns(&mut out, body.get("messages")?);
        }
        "/v1/responses" => {
            if let Some(instructions) = body.get("instructions") {
                push_turn(&mut out, "system", instructions);
            }
            match body.get("input")? {
                input @ Value::String(_) => push_turn(&mut out, "user", input),
                items => turns(&mut out, items),
            }
        }
        "/v1/completions" | "/v1/embeddings" => {
            let field = if path == "/v1/completions" {
                "prompt"
            } else {
                "input"
            };
            push_text(&mut out, body.get(field)?);
        }
        _ => return None,
    }
    if out.is_empty() {
        return None;
    }
    if out.len() > MAX_AFFINITY_BYTES {
        let mut end = MAX_AFFINITY_BYTES;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    Some(out)
}

/// Every `{role, content}` item of `messages`, in order. Items without a role
/// (a Responses `function_call_output`, say) still contribute their text.
fn turns(out: &mut String, messages: &Value) {
    let Some(items) = messages.as_array() else {
        return;
    };
    for item in items {
        if out.len() >= MAX_AFFINITY_BYTES {
            return;
        }
        let role = item.get("role").and_then(Value::as_str).unwrap_or("item");
        if let Some(content) = item.get("content") {
            push_turn(out, role, content);
        }
    }
}

fn push_turn(out: &mut String, role: &str, content: &Value) {
    out.push_str(role);
    out.push_str(": ");
    push_text(out, content);
    out.push('\n');
}

/// The text of a content value: a string, or the text of each part of an
/// array — OpenAI `text` / `input_text` parts and Anthropic `text` blocks all
/// carry it under `text`.
fn push_text(out: &mut String, content: &Value) {
    match content {
        Value::String(text) => out.push_str(text),
        Value::Array(parts) => {
            for part in parts {
                if out.len() >= MAX_AFFINITY_BYTES {
                    return;
                }
                match part {
                    Value::String(text) => out.push_str(text),
                    Value::Object(_) => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            out.push_str(text);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chat(messages: Value) -> String {
        affinity_text(
            "/v1/chat/completions",
            &json!({"model": "llama", "max_tokens": 8, "messages": messages}),
        )
        .unwrap()
    }

    #[test]
    fn the_json_envelope_is_not_part_of_the_prompt() {
        let a = chat(json!([{"role": "user", "content": "what is rust"}]));
        let b = chat(json!([{"role": "user", "content": "bake a cake"}]));
        assert_eq!(a, "user: what is rust\n");
        assert_eq!(b, "user: bake a cake\n");
        assert!(!a.contains("model") && !a.contains('{'));
    }

    #[test]
    fn a_conversation_extends_its_own_prefix() {
        let first = chat(json!([
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": "hi"},
        ]));
        let second = chat(json!([
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"},
            {"role": "user", "content": "how are you"},
        ]));
        assert!(second.starts_with(&first), "{first:?} / {second:?}");
    }

    #[test]
    fn content_parts_and_anthropic_blocks_contribute_their_text() {
        let parts = chat(json!([{"role": "user", "content": [
            {"type": "text", "text": "look at "},
            {"type": "image_url", "image_url": {"url": "data:,"}},
            {"type": "text", "text": "this"},
        ]}]));
        assert_eq!(parts, "user: look at this\n");

        let messages = affinity_text(
            "/v1/messages",
            &json!({
                "model": "claude",
                "system": [{"type": "text", "text": "be brief"}],
                "messages": [{"role": "user", "content": "hi"}],
            }),
        );
        assert_eq!(messages.as_deref(), Some("system: be brief\nuser: hi\n"));
    }

    #[test]
    fn responses_completions_and_embeddings_are_read_too() {
        let responses = affinity_text(
            "/v1/responses",
            &json!({"model": "m", "instructions": "be brief", "input": "hi"}),
        );
        assert_eq!(responses.as_deref(), Some("system: be brief\nuser: hi\n"));
        let completions = affinity_text("/v1/completions", &json!({"prompt": "once upon"}));
        assert_eq!(completions.as_deref(), Some("once upon"));
        let embeddings = affinity_text("/v1/embeddings", &json!({"input": ["a", "b"]}));
        assert_eq!(embeddings.as_deref(), Some("ab"));
    }

    #[test]
    fn an_unknown_shape_leaves_the_fallback_to_the_caller() {
        assert_eq!(
            affinity_text("/v1/images/generations", &json!({"prompt": "cat"})),
            None
        );
        assert_eq!(
            affinity_text("/v1/chat/completions", &json!({"model": "m"})),
            None
        );
        assert_eq!(
            affinity_text("/v1/chat/completions", &json!({"messages": []})),
            None
        );
    }

    #[test]
    fn a_long_prompt_is_cut_on_a_character_boundary() {
        let long = "é".repeat(MAX_AFFINITY_BYTES);
        let text = chat(json!([{"role": "user", "content": long}]));
        assert!(text.len() <= MAX_AFFINITY_BYTES);
        assert!(text.starts_with("user: é"));
    }
}
