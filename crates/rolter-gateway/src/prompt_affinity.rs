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
//! an assistant turn with the same words stay distinct. Only text goes into the
//! prefix the trie matches on.
//!
//! Prefix affinity is decided by the leading bytes, so the text kept is
//! bounded: copying no more than [`MAX_AFFINITY_BYTES`] of it keeps a long
//! prompt from costing a second copy of itself on every request, and bounds the
//! trie walk. The readers that need all of the prompt get it as two numbers
//! measured over the whole prompt before the cut — its length, for the latency
//! predictor's token estimate, and a digest, for `consistent_hash`, which would
//! otherwise send every request sharing a long system prompt to one target.
//!
//! Those two numbers also cover what the text leaves out. Tool calls and tool
//! results (Anthropic `tool_use` / `tool_result`, OpenAI `tool_calls`,
//! Responses `function_call` / `function_call_output`) are prefilled like any
//! other prompt tokens and are often most of an agent's prompt, so they count
//! towards the length and the digest. Images, audio and files count towards
//! the digest only: their encoded size says little about their token cost, but
//! two requests that differ only in an image are still two different prompts
//! and must not hash to one target. Both are fed in without being copied.

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
    /// Byte length of the whole prompt: its text plus its tool calls and tool
    /// results.
    pub(crate) len: usize,
    /// xxh3 digest of the whole prompt: its text, its tool calls and results,
    /// and the content of every image, audio or file part, in order.
    pub(crate) digest: u64,
}

/// The routing context a JSON request is balanced on: the prompt's bounded
/// text with the length and digest of the whole of it, or, for a body this
/// module cannot read, the raw body.
///
/// Every request path builds its context here, so a reader that needs the
/// whole prompt (the latency predictor, `consistent_hash`) cannot be handed
/// only its leading bytes by accident.
pub(crate) fn route_context<'a>(
    affinity: Option<&'a Affinity>,
    raw_body: &'a [u8],
    session_key: Option<&'a str>,
    token_ids: Option<&'a [u32]>,
    adapter: Option<&'a str>,
) -> rolter_balancer::RouteContext<'a> {
    rolter_balancer::RouteContext {
        session_key,
        prompt: affinity
            .map(|a| a.text.as_str())
            .or_else(|| std::str::from_utf8(raw_body).ok()),
        prompt_len: affinity.map(|a| a.len),
        prompt_digest: affinity.map(|a| a.digest),
        token_ids,
        adapter,
    }
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

/// Accumulates the prompt: every text piece is measured and hashed, but only
/// the first [`MAX_AFFINITY_BYTES`] are copied. Parts that are not text are
/// hashed, and measured when they are tool traffic, without being copied.
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

    /// Feed `value` into the digest, and into the length when `measured`,
    /// without copying it into the text. A string goes in as its bytes; any
    /// other value as its JSON, written straight into the hasher.
    fn absorb(&mut self, value: &Value, measured: bool) {
        if let Value::String(text) = value {
            self.hasher.update(text.as_bytes());
            if measured {
                self.len += text.len();
            }
            return;
        }
        let mut sink = Absorb {
            builder: self,
            measured,
        };
        // writing a Value into a sink that never fails cannot fail: its keys
        // are strings
        let _ = serde_json::to_writer(&mut sink, value);
    }

    fn finish(self) -> Option<Affinity> {
        (self.len > 0).then(|| Affinity {
            text: self.text,
            len: self.len,
            digest: self.hasher.digest(),
        })
    }
}

/// The serializer's output for [`Builder::absorb`]: fed to the digest as it is
/// written, so a large tool result or image is never held a second time.
struct Absorb<'a> {
    builder: &'a mut Builder,
    measured: bool,
}

impl std::io::Write for Absorb<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.builder.hasher.update(bytes);
        if self.measured {
            self.builder.len += bytes.len();
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Every item of `messages`, in order: a `{role, content}` turn contributes
/// its text, and the tool traffic around it (an assistant's `tool_calls`, a
/// Responses `function_call` or `function_call_output`) its size and identity.
fn turns(out: &mut Builder, messages: &Value) {
    let Some(items) = messages.as_array() else {
        return;
    };
    for item in items {
        let role = item.get("role").and_then(Value::as_str).unwrap_or("item");
        match item.get("content") {
            Some(content) => push_turn(out, role, content),
            // a Responses item with no content: `function_call_output` carries
            // `output`, `function_call` carries `arguments`, anything else
            // (an item reference, a reasoning item) is identified whole
            None => match (item.get("output"), item.get("arguments")) {
                (Some(output), _) => out.absorb(output, true),
                (None, Some(arguments)) => out.absorb(arguments, true),
                (None, None) => out.absorb(item, false),
            },
        }
        if let Some(calls) = item.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                match call.pointer("/function/arguments") {
                    Some(arguments) => out.absorb(arguments, true),
                    None => out.absorb(call, true),
                }
            }
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
/// carry it under `text`. A part with no text is left out of the text but not
/// out of the prompt: see [`push_part`].
fn push_text(out: &mut Builder, content: &Value) {
    match content {
        Value::String(text) => out.push(text),
        Value::Array(parts) => {
            for part in parts {
                match part {
                    Value::String(text) => out.push(text),
                    Value::Object(_) => match part.get("text").and_then(Value::as_str) {
                        Some(text) => out.push(text),
                        None => push_part(out, part),
                    },
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// A content part that is not text. Tool traffic is prefilled like text, so it
/// is measured: an Anthropic `tool_result` by its content, a `tool_use` by its
/// input. Anything else (an image, audio, a file) is identified by its content
/// but not measured.
fn push_part(out: &mut Builder, part: &Value) {
    match part.get("type").and_then(Value::as_str) {
        Some("tool_result") => match part.get("content") {
            // a tool result's blocks are text or images, read like a turn's
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    match block.get("text") {
                        Some(text) => out.absorb(text, true),
                        None => out.absorb(block, false),
                    }
                }
            }
            Some(content) => out.absorb(content, true),
            None => out.absorb(part, false),
        },
        Some("tool_use") => out.absorb(part.get("input").unwrap_or(part), true),
        _ => out.absorb(part, false),
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

    /// The routing context the proxy builds from an affinity, through the
    /// same function it calls.
    fn context(a: &Affinity) -> RouteContext<'_> {
        route_context(Some(a), b"{}", None, None, None)
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

    /// A vision batch shares its instruction and differs only in the image, so
    /// the text is identical; the image still has to reach the digest, or
    /// `consistent_hash` sends the whole batch to one target.
    #[test]
    fn requests_that_differ_only_in_an_image_spread_under_consistent_hash() {
        let affinities: Vec<Affinity> = (0..32)
            .map(|i| {
                affinity(
                    "/v1/chat/completions",
                    &json!({"messages": [{"role": "user", "content": [
                        {"type": "text", "text": "Describe this image"},
                        {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{i:08}")}},
                    ]}]}),
                )
                .unwrap()
            })
            .collect();
        // the image is not text and not measured: an encoded image's size says
        // little about its token cost
        assert!(affinities
            .iter()
            .all(|a| a.text == "user: Describe this image\n" && a.len == a.text.len()));
        let digests: std::collections::HashSet<u64> = affinities.iter().map(|a| a.digest).collect();
        assert_eq!(digests.len(), affinities.len());
        let lb = ConsistentHash::new(4);
        let targets: std::collections::HashSet<usize> = affinities
            .iter()
            .map(|a| lb.pick(&context(a), &[]).unwrap())
            .collect();
        assert!(targets.len() > 1, "every image hashed to {targets:?}");

        // the same holds for an Anthropic image block
        let anthropic = |data: &str| {
            affinity(
                "/v1/messages",
                &json!({"messages": [{"role": "user", "content": [
                    {"type": "text", "text": "Describe this image"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}},
                ]}]}),
            )
            .unwrap()
        };
        assert_ne!(anthropic("AAAA").digest, anthropic("BBBB").digest);
    }

    /// An agent's prompt is mostly tool results. They are prefilled like any
    /// other token, so they count towards the size the predictor estimates
    /// from, and two turns that differ only in a tool result are two prompts.
    #[test]
    fn anthropic_tool_results_and_calls_count_towards_size_and_identity() {
        let result = "r".repeat(200 * 1024);
        let body = |content: Value| {
            json!({"messages": [
                {"role": "user", "content": "list the files"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "t1", "name": "ls", "input": {"path": "/srv"}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": content},
                ]},
            ]})
        };
        let as_string = affinity("/v1/messages", &body(json!(result))).unwrap();
        assert!(as_string.len >= result.len(), "{}", as_string.len);
        let tokens = prompt_tokens(&context(&as_string));
        assert!(tokens >= result.len() / 4, "{tokens}");
        // the text the trie matches on stays text
        assert!(!as_string.text.contains("rrrrrrrr"));

        // the block form of a tool result is measured the same way
        let as_blocks = affinity(
            "/v1/messages",
            &body(json!([{"type": "text", "text": result}])),
        )
        .unwrap();
        assert_eq!(as_blocks.len, as_string.len);

        let other = affinity("/v1/messages", &body(json!("a different listing"))).unwrap();
        assert_eq!(other.text, as_string.text);
        assert_ne!(other.digest, as_string.digest);
    }

    #[test]
    fn openai_and_responses_tool_traffic_counts_towards_size_and_identity() {
        let arguments = format!("{{\"q\":\"{}\"}}", "a".repeat(10 * 1024));
        let output = "o".repeat(50 * 1024);
        let chat = affinity(
            "/v1/chat/completions",
            &json!({"messages": [
                {"role": "user", "content": "search"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "find", "arguments": arguments}},
                ]},
                {"role": "tool", "tool_call_id": "c1", "content": output},
            ]}),
        )
        .unwrap();
        assert!(chat.len >= arguments.len() + output.len(), "{}", chat.len);

        let responses = |output: &str| {
            affinity(
                "/v1/responses",
                &json!({"input": [
                    {"role": "user", "content": "search"},
                    {"type": "function_call", "call_id": "c1", "name": "find", "arguments": arguments},
                    {"type": "function_call_output", "call_id": "c1", "output": output},
                ]}),
            )
            .unwrap()
        };
        let a = responses(&output);
        assert!(a.len >= arguments.len() + output.len(), "{}", a.len);
        let b = responses("nothing found");
        assert_eq!(a.text, b.text);
        assert_ne!(a.digest, b.digest);
    }

    /// The proxy builds its context through `route_context`, so what reaches
    /// the balancer is exactly what these tests see.
    #[test]
    fn the_route_context_carries_the_whole_prompt_or_the_raw_body() {
        let a = affinity(
            "/v1/chat/completions",
            &json!({"messages": [{"role": "user", "content": "hi"}]}),
        )
        .unwrap();
        let ctx = route_context(Some(&a), b"{\"raw\":1}", Some("s"), None, None);
        assert_eq!(ctx.prompt, Some("user: hi\n"));
        assert_eq!(ctx.prompt_len, Some(a.len));
        assert_eq!(ctx.prompt_digest, Some(a.digest));
        assert_eq!(ctx.session_key, Some("s"));

        // a body this module cannot read is balanced on as a whole
        let ctx = route_context(None, b"{\"raw\":1}", None, None, Some("lora"));
        assert_eq!(ctx.prompt, Some("{\"raw\":1}"));
        assert_eq!(ctx.prompt_len, None);
        assert_eq!(ctx.prompt_digest, None);
        assert_eq!(ctx.adapter, Some("lora"));
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
