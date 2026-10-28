//! What a request asked a model to do, beyond its token counts (#3317).
//!
//! The savings advisor (#1581, `rolter-control/src/savings.rs`) lists requests
//! that could have gone to a cheaper model in the same route. Whether one could
//! is first a question of shape: a request that calls tools, sends an image or
//! audio, carries a conversation or asks for a structured-output schema needs
//! a model that does those, whatever its token counts say. The request log
//! recorded none of it, so the advisor could only say so in its limits.
//!
//! [`RequestShape::of`] reads the flags off the request body the gateway has
//! already parsed, for the OpenAI (Chat Completions, Responses) and Anthropic
//! (Messages) dialects. It walks the message list once and allocates nothing.
//! The flags travel on the request-log row as their own columns
//! (`clickhouse/031_request_shape.sql`), each left out of a row while zero, so
//! a ClickHouse without the file keeps accepting rows and a plain one-turn
//! text request is byte for byte what it was.

use serde::Serialize;
use serde_json::Value;

/// The shape flags of one request. Every field is `0` (or `false`) for a
/// request the gateway did not read as a model call, such as an embedding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RequestShape {
    /// `1` when the request offered the model tools (functions, or any other
    /// tool kind) to call
    #[serde(skip_serializing_if = "is_zero")]
    pub has_tools: u8,
    /// `1` when an input message carried an image
    #[serde(skip_serializing_if = "is_zero")]
    pub has_image_input: u8,
    /// `1` when an input message carried audio
    #[serde(skip_serializing_if = "is_zero")]
    pub has_audio_input: u8,
    /// `1` when the request asked for output that follows a JSON schema
    #[serde(skip_serializing_if = "is_zero")]
    pub has_response_schema: u8,
    /// how many caller turns the request carried: the user messages of a chat
    /// or messages request, the user items of a responses request (a bare
    /// string input is one). Saturates at `u16::MAX`
    #[serde(skip_serializing_if = "is_zero_u16")]
    pub turns: u16,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

fn is_zero_u16(v: &u16) -> bool {
    *v == 0
}

impl RequestShape {
    /// The shape of the request `body` sent to the gateway's `path`. A path
    /// that is not a chat-shaped model call reads as no shape at all.
    pub fn of(path: &str, body: &Value) -> Self {
        let mut shape = Self::default();
        match path {
            "/v1/chat/completions" | "/v1/messages" => {
                if let Some(messages) = body.get("messages").and_then(Value::as_array) {
                    for message in messages {
                        shape.read_message(message);
                    }
                }
            }
            "/v1/responses" => match body.get("input") {
                Some(Value::String(_)) => shape.turns = 1,
                Some(Value::Array(items)) => {
                    for item in items {
                        shape.read_message(item);
                    }
                }
                _ => {}
            },
            _ => return shape,
        }
        shape.has_tools = u8::from(["tools", "functions"].iter().any(|key| {
            body.get(key)
                .and_then(Value::as_array)
                .is_some_and(|t| !t.is_empty())
        }));
        shape.has_response_schema = u8::from(asks_for_a_schema(body));
        shape
    }

    /// Fold one message (or responses input item) into the flags.
    fn read_message(&mut self, message: &Value) {
        // a responses input item with no role is a user message when it has
        // content, and a tool output or reasoning item otherwise
        let role = message.get("role").and_then(Value::as_str);
        let is_user = match role {
            Some(role) => role == "user",
            None => message.get("type").and_then(Value::as_str) == Some("message"),
        };
        if is_user {
            self.turns = self.turns.saturating_add(1);
        }
        if let Some(parts) = message.get("content").and_then(Value::as_array) {
            for part in parts {
                match part.get("type").and_then(Value::as_str) {
                    // openai chat, openai responses, anthropic
                    Some("image_url" | "input_image" | "image") => self.has_image_input = 1,
                    Some("input_audio" | "audio") => self.has_audio_input = 1,
                    _ => {}
                }
            }
        }
    }
}

/// Whether the request asked for schema-shaped output: Chat Completions'
/// `response_format: {type: json_schema}`, Responses' `text.format` of the
/// same type, or Anthropic's `output_format: {type: json_schema}`. A bare
/// `json_object` mode carries no schema and is not counted.
fn asks_for_a_schema(body: &Value) -> bool {
    let schema_type = |format: Option<&Value>| {
        format
            .and_then(|format| format.get("type"))
            .and_then(Value::as_str)
            == Some("json_schema")
    };
    schema_type(body.get("response_format"))
        || schema_type(body.get("text").and_then(|text| text.get("format")))
        || schema_type(body.get("output_format"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_plain_one_turn_chat_has_one_turn_and_nothing_else() {
        let shape = RequestShape::of(
            "/v1/chat/completions",
            &json!({"model": "m", "messages": [
                {"role": "system", "content": "be brief"},
                {"role": "user", "content": "hi"}
            ]}),
        );
        assert_eq!(
            shape,
            RequestShape {
                turns: 1,
                ..Default::default()
            }
        );
        // and serializes to exactly that, so the row is what it was
        assert_eq!(serde_json::to_value(shape).unwrap(), json!({"turns": 1}));
    }

    #[test]
    fn openai_chat_tools_images_audio_and_schema_are_read() {
        let shape = RequestShape::of(
            "/v1/chat/completions",
            &json!({
                "model": "m",
                "messages": [
                    {"role": "user", "content": [
                        {"type": "text", "text": "what is this"},
                        {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA"}}
                    ]},
                    {"role": "assistant", "content": "a cat"},
                    {"role": "user", "content": [
                        {"type": "input_audio", "input_audio": {"data": "AA", "format": "wav"}}
                    ]}
                ],
                "tools": [{"type": "function", "function": {"name": "f"}}],
                "response_format": {"type": "json_schema", "json_schema": {"name": "x"}}
            }),
        );
        assert_eq!(
            shape,
            RequestShape {
                has_tools: 1,
                has_image_input: 1,
                has_audio_input: 1,
                has_response_schema: 1,
                turns: 2,
            }
        );
    }

    #[test]
    fn anthropic_messages_are_read_the_same_way() {
        let shape = RequestShape::of(
            "/v1/messages",
            &json!({
                "model": "m", "max_tokens": 8,
                "messages": [
                    {"role": "user", "content": [
                        {"type": "image", "source": {"type": "base64", "data": "AA"}}
                    ]},
                    {"role": "assistant", "content": [{"type": "tool_use", "id": "t", "name": "f", "input": {}}]},
                    {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t"}]}
                ],
                "tools": [{"name": "f", "input_schema": {"type": "object"}}],
                "output_format": {"type": "json_schema", "schema": {}}
            }),
        );
        assert_eq!(
            shape,
            RequestShape {
                has_tools: 1,
                has_image_input: 1,
                has_audio_input: 0,
                has_response_schema: 1,
                turns: 2,
            }
        );
    }

    #[test]
    fn responses_input_is_read_as_a_string_or_items() {
        assert_eq!(
            RequestShape::of("/v1/responses", &json!({"input": "hi"})).turns,
            1
        );
        let shape = RequestShape::of(
            "/v1/responses",
            &json!({
                "input": [
                    {"role": "user", "content": [{"type": "input_image", "image_url": "https://x/y.png"}]},
                    {"type": "function_call_output", "call_id": "c", "output": "{}"},
                    {"type": "message", "content": [{"type": "input_text", "text": "and?"}]}
                ],
                "text": {"format": {"type": "json_schema", "name": "x", "schema": {}}}
            }),
        );
        assert_eq!(shape.turns, 2);
        assert_eq!(shape.has_image_input, 1);
        assert_eq!(shape.has_response_schema, 1);
        assert_eq!(shape.has_tools, 0);
    }

    #[test]
    fn a_json_object_mode_or_an_empty_tool_list_is_not_counted() {
        let shape = RequestShape::of(
            "/v1/chat/completions",
            &json!({"messages": [], "tools": [], "response_format": {"type": "json_object"}}),
        );
        assert_eq!(shape, RequestShape::default());
    }

    #[test]
    fn other_endpoints_have_no_shape() {
        for path in ["/v1/embeddings", "/v1/rerank", "/v1/systemone"] {
            assert_eq!(
                RequestShape::of(path, &json!({"input": "hi", "tools": [{}]})),
                RequestShape::default(),
                "{path}"
            );
        }
    }
}
