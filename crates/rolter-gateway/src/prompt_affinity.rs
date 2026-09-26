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
//! image part or a tool call contributes nothing.
//!
//! Prefix affinity is decided by the leading bytes, so the text kept is
//! bounded: copying no more than [`MAX_AFFINITY_BYTES`] of it keeps a long
//! prompt from costing a second copy of itself on every request, and bounds the
//! trie walk. The readers that need all of the prompt get it as two numbers
//! measured over the whole text before the cut — its length, for the latency
//! predictor's token estimate, and a digest, for `consistent_hash`, which would
//! otherwise send every request sharing a long system prompt to one target.

use serde_json::Value;
use xxhash_rust::xxh3::Xxh3Default;

/// Upper bound on the extracted text. Which replica holds a prompt's prefix is
/// decided by its leading bytes, so the tail of a long prompt changes nothing
/// about the pick, and bounding it bounds the trie walk each request pays.
pub(crate) const MAX_AFFINITY_BYTES: usize = 32 * 1024;

/// A request's prompt as the balancer sees it.
pub(crate) struct Affinity {
    /// The leading bytes of the prompt text, at most [`MAX_AFFINITY_BYTES`] and
    /// cut on a character boundary. Always a prefix of the whole text.
    pub(crate) text: String,
    /// Byte length of the whole prompt text.
    pub(crate) len: usize,
    /// xxh3 digest of the whole prompt text.
    pub(crate) digest: u64,
}

/// The affinity of a JSON request to `path`, or `None` when the body has no
/// prompt this knows how to read, so the caller keeps its own fallback.
pub(crate) fn affinity(path: &str, body: &Value) -> Option<Affinity> {
    let mut out = Builder::new();
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
    out.finish()
}

/// Accumulates the prompt text: every piece is measured and hashed, but only
/// the first [`MAX_AFFINITY_BYTES`] are copied.
struct Builder {
    text: String,
    len: usize,
    hasher: Xxh3Default,
    /// set once a piece was cut short: nothing after the cut may be appended,
    /// or the text would stop being a prefix of the whole
    full: bool,
}

impl Builder {
    fn new() -> Self {
        Self {
            text: String::new(),
            len: 0,
            hasher: Xxh3Default::new(),
            full: false,
        }
    }

    fn push(&mut self, piece: &str) {
        self.len += piece.len();
        self.hasher.update(piece.as_bytes());
        if self.full {
            return;
        }
        let room = MAX_AFFINITY_BYTES - self.text.len();
        if piece.len() <= room {
            self.text.push_str(piece);
            return;
        }
        let mut end = room;
        while !piece.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&piece[..end]);
        self.full = true;
    }

    fn finish(self) -> Option<Affinity> {
        (self.len > 0).then(|| Affinity {
            text: self.text,
            len: self.len,
            digest: self.hasher.digest(),
        })
    }
}

/// Every `{role, content}` item of `messages`, in order. Items without a role
/// (a Responses `function_call_output`, say) still contribute their text.
fn turns(out: &mut Builder, messages: &Value) {
    let Some(items) = messages.as_array() else {
        return;
    };
    for item in items {
        let role = item.get("role").and_then(Value::as_str).unwrap_or("item");
        if let Some(content) = item.get("content") {
            push_turn(out, role, content);
        }
    }
}

fn push_turn(out: &mut Builder, role: &str, content: &Value) {
    out.push(role);
    out.push(": ");
    push_text(out, content);
    out.push("\n");
}

/// The text of a content value: a string, or the text of each part of an
/// array — OpenAI `text` / `input_text` parts and Anthropic `text` blocks all
/// carry it under `text`.
fn push_text(out: &mut Builder, content: &Value) {
    match content {
        Value::String(text) => out.push(text),
        Value::Array(parts) => {
            for part in parts {
                match part {
                    Value::String(text) => out.push(text),
                    Value::Object(_) => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            out.push(text);
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
    use rolter_balancer::{predictor::prompt_tokens, ConsistentHash, LoadBalancer, RouteContext};
    use serde_json::json;
    use xxhash_rust::xxh3::xxh3_64;

    fn affinity_text(path: &str, body: &Value) -> Option<String> {
        affinity(path, body).map(|a| a.text)
    }

    /// The routing context the gateway builds from an affinity.
    fn context(a: &Affinity) -> RouteContext<'_> {
        RouteContext {
            prompt: Some(&a.text),
            prompt_len: Some(a.len),
            prompt_digest: Some(a.digest),
            ..Default::default()
        }
    }

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
        let whole = format!("user: {long}\n");
        let a = affinity(
            "/v1/chat/completions",
            &json!({"messages": [{"role": "user", "content": long}]}),
        )
        .unwrap();
        assert!(a.text.len() <= MAX_AFFINITY_BYTES);
        assert!(whole.starts_with(&a.text));
        // what the cut dropped is still measured and hashed
        assert_eq!(a.len, whole.len());
        assert_eq!(a.digest, xxh3_64(whole.as_bytes()));
    }

    #[test]
    fn nothing_is_appended_after_the_cut() {
        // the cut lands inside a two-byte character, leaving a byte of room that
        // the next turn's text would otherwise fill
        let long = format!("{}{}", "a".repeat(MAX_AFFINITY_BYTES - 7), "é".repeat(4));
        let a = affinity(
            "/v1/chat/completions",
            &json!({"messages": [
                {"role": "user", "content": long},
                {"role": "user", "content": "x"},
            ]}),
        )
        .unwrap();
        let whole = format!("user: {long}\nuser: x\n");
        assert!(whole.starts_with(&a.text), "the text left the prefix");
        assert_eq!(a.len, whole.len());
    }

    #[test]
    fn a_huge_message_is_not_copied_in_full() {
        let huge = "x".repeat(4 * 1024 * 1024);
        let a = affinity(
            "/v1/chat/completions",
            &json!({"messages": [{"role": "user", "content": huge}]}),
        )
        .unwrap();
        assert!(a.text.len() <= MAX_AFFINITY_BYTES);
        // the buffer never grew towards the message's size
        assert!(
            a.text.capacity() <= 2 * MAX_AFFINITY_BYTES,
            "{}",
            a.text.capacity()
        );
        assert_eq!(a.len, "user: \n".len() + huge.len());
    }

    #[test]
    fn the_digest_is_of_the_text_however_the_content_is_split() {
        let whole = affinity(
            "/v1/chat/completions",
            &json!({"messages": [{"role": "user", "content": "look at this"}]}),
        )
        .unwrap();
        let parts = affinity(
            "/v1/chat/completions",
            &json!({"messages": [{"role": "user", "content": [
                {"type": "text", "text": "look at "},
                {"type": "text", "text": "this"},
            ]}]}),
        )
        .unwrap();
        assert_eq!(whole.digest, parts.digest);
        assert_eq!(whole.digest, xxh3_64(b"user: look at this\n"));
    }

    /// Requests behind one 40 KiB system prompt share their whole bounded
    /// text; `consistent_hash` has to key on what follows it, or every one of
    /// them lands on the same target.
    #[test]
    fn a_shared_long_system_prompt_does_not_pin_consistent_hash() {
        let system = "you are a careful assistant. ".repeat(40 * 1024 / 29 + 1);
        assert!(system.len() > 40 * 1024);
        let affinities: Vec<Affinity> = (0..32)
            .map(|i| {
                affinity(
                    "/v1/chat/completions",
                    &json!({"messages": [
                        {"role": "system", "content": system},
                        {"role": "user", "content": format!("question {i}")},
                    ]}),
                )
                .unwrap()
            })
            .collect();
        assert!(affinities.windows(2).all(|w| w[0].text == w[1].text));
        let lb = ConsistentHash::new(4);
        let targets: std::collections::HashSet<usize> = affinities
            .iter()
            .map(|a| lb.pick(&context(a), &[]).unwrap())
            .collect();
        assert!(targets.len() > 1, "every request hashed to {targets:?}");
        // and a repeated request keeps its target
        let first = lb.pick(&context(&affinities[0]), &[]);
        assert_eq!(first, lb.pick(&context(&affinities[0]), &[]));
    }

    /// The latency predictor sizes a prompt from its whole length, so a long
    /// prompt's token estimate keeps growing past the 32 KiB cut (8192 tokens
    /// at four bytes a token).
    #[test]
    fn the_token_estimate_is_taken_before_the_cut() {
        let long = "word ".repeat(40 * 1024);
        let a = affinity(
            "/v1/chat/completions",
            &json!({"messages": [{"role": "user", "content": long}]}),
        )
        .unwrap();
        assert!(a.text.len() <= MAX_AFFINITY_BYTES);
        let tokens = prompt_tokens(&context(&a));
        assert!(tokens > 8192, "{tokens}");
        assert_eq!(tokens, a.len / 4);
    }
}
