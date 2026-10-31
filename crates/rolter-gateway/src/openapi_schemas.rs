//! The `components/schemas` of the gateway's OpenAPI document (#3335).
//!
//! Each schema lists the fields rolter reads or guarantees on a request or a
//! 200 response, and nothing else: bodies are forwarded, so every object stays
//! open (`additionalProperties: true`) and a field that is not named here is
//! passed through, not refused. They are not the vendor's full contract; the
//! registry record that owns a schema (`gateway_schemas` in
//! `crates/rolter-core/specifications.toml`) links the vendor's own reference.
//!
//! Built apart from `document()` because the literal there already sits at
//! `json!`'s recursion limit.

use serde_json::{json, Map, Value};

/// An open object schema: `required` fields first, then the named properties.
fn object(description: &str, required: &[&str], properties: Value) -> Value {
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("description".into(), json!(description));
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }
    schema.insert("properties".into(), properties);
    schema.insert("additionalProperties".into(), json!(true));
    Value::Object(schema)
}

fn model_field() -> Value {
    json!({"type": "string", "description": "a public model name served by this gateway; rolter routes on it and may rewrite it for the upstream"})
}

fn reference(name: &str) -> Value {
    json!({"$ref": format!("#/components/schemas/{name}")})
}

/// Every schema the paths reference, keyed by name.
pub fn all() -> Map<String, Value> {
    let mut schemas = Map::new();
    for (name, schema) in requests()
        .into_iter()
        .chain(responses())
        .chain(streaming_events())
    {
        schemas.insert(name.to_string(), schema);
    }
    schemas
}

fn requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "ChatCompletionRequest",
            object(
                "OpenAI Chat Completions request. rolter reads `model`, `stream` and `messages`; the rest is forwarded, or translated when the route's target speaks Anthropic Messages or Gemini.",
                &["model", "messages"],
                json!({
                    "model": model_field(),
                    "messages": {"type": "array", "items": {"type": "object", "additionalProperties": true}},
                    "stream": {"type": "boolean", "default": false, "description": "answer with `chat.completion.chunk` server-sent events"},
                    "temperature": {"type": "number"},
                    "max_tokens": {"type": "integer"}
                }),
            ),
        ),
        (
            "CompletionRequest",
            object(
                "OpenAI legacy completions request; forwarded to an OpenAI-shaped upstream only. rolter reads `model`.",
                &["model"],
                json!({"model": model_field(), "prompt": {"description": "a string or an array, forwarded as sent"}}),
            ),
        ),
        (
            "ResponsesRequest",
            object(
                "OpenAI Responses create request. rolter reads `model`, `stream`, `input` and `previous_response_id`; it is forwarded to an OpenAI upstream, or translated to Chat Completions, Anthropic Messages or Gemini.",
                &["model"],
                json!({
                    "model": model_field(),
                    "input": {"description": "a string or an array of input items"},
                    "stream": {"type": "boolean", "default": false, "description": "answer with `response.*` server-sent events"},
                    "previous_response_id": {"type": "string"}
                }),
            ),
        ),
        (
            "MessagesRequest",
            object(
                "Anthropic Messages request. rolter reads `model`, `stream`, `messages` and `max_tokens`; the rest is forwarded, or translated when the route's target speaks OpenAI Chat Completions or Gemini.",
                &["model", "messages"],
                json!({
                    "model": model_field(),
                    "max_tokens": {"type": "integer"},
                    "messages": {"type": "array", "items": {"type": "object", "additionalProperties": true}},
                    "stream": {"type": "boolean", "default": false, "description": "answer with `message_*` and `content_block_*` server-sent events"}
                }),
            ),
        ),
        (
            "EmbeddingRequest",
            object(
                "OpenAI embeddings request. rolter reads `model` and `encoding_format`.",
                &["model", "input"],
                json!({
                    "model": model_field(),
                    "input": {"oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]},
                    "encoding_format": {"type": "string", "enum": ["float", "base64"]}
                }),
            ),
        ),
        (
            "RerankRequest",
            object(
                "Cohere v1 / Jina rerank request. rolter reads `model` only and forwards the body unchanged.",
                &["model", "query", "documents"],
                json!({
                    "model": model_field(),
                    "query": {"type": "string"},
                    "documents": {"type": "array", "items": {"type": "string"}},
                    "top_n": {"type": "integer"},
                    "return_documents": {"type": "boolean"}
                }),
            ),
        ),
        (
            "ImageGenerationRequest",
            object(
                "OpenAI image generation request. rolter reads `model`; the body is forwarded unchanged.",
                &["model", "prompt"],
                json!({
                    "model": model_field(),
                    "prompt": {"type": "string"},
                    "n": {"type": "integer"},
                    "response_format": {"type": "string", "enum": ["url", "b64_json"]}
                }),
            ),
        ),
        (
            "SpeechRequest",
            object(
                "OpenAI text-to-speech request. rolter reads `model`; the body is forwarded unchanged.",
                &["model", "input"],
                json!({"model": model_field(), "input": {"type": "string"}, "voice": {"type": "string"}}),
            ),
        ),
        (
            "SystemOneRequest",
            object(
                "System One decision request. rolter reads `model`; the body is forwarded unchanged.",
                &["model", "state", "questions"],
                json!({
                    "model": model_field(),
                    "state": {"description": "the content the questions are about: a string, an object or an array"},
                    "questions": {
                        "type": "object",
                        "description": "questions keyed by ids the caller chooses; each has a `type` (`choice`, `score`, `noul`, or another the model supports), `instructions` and usually `criteria`",
                        "additionalProperties": object(
                            "one question",
                            &["type"],
                            json!({"type": {"type": "string"}, "instructions": {}, "criteria": {}}),
                        )
                    }
                }),
            ),
        ),
    ]
}

fn usage_schema(description: &str) -> Value {
    object(
        description,
        &[],
        json!({
            "prompt_tokens": {"type": "integer"},
            "completion_tokens": {"type": "integer"},
            "total_tokens": {"type": "integer"},
            "input_tokens": {"type": "integer"},
            "output_tokens": {"type": "integer"}
        }),
    )
}

fn responses() -> Vec<(&'static str, Value)> {
    vec![
        (
            "Usage",
            usage_schema(
                "Token counts as the upstream reports them: `prompt_tokens` / `completion_tokens` (OpenAI) or `input_tokens` / `output_tokens` (Anthropic, Responses, System One). rolter reads them for request logs, budgets and cost; detail objects beside them (`prompt_tokens_details`, `cache_read_input_tokens`, …) are read when present.",
            ),
        ),
        (
            "ChatCompletionResponse",
            object(
                "Chat completion. `choices[].finish_reason` and `usage` are what rolter reads; a response translated from Anthropic or Gemini carries the same shape.",
                &["choices"],
                json!({
                    "id": {"type": "string"},
                    "object": {"type": "string", "const": "chat.completion"},
                    "model": {"type": "string"},
                    "choices": {"type": "array", "items": object(
                        "one choice",
                        &[],
                        json!({"index": {"type": "integer"}, "message": {"type": "object", "additionalProperties": true}, "finish_reason": {"type": ["string", "null"]}}),
                    )},
                    "usage": reference("Usage")
                }),
            ),
        ),
        (
            "CompletionResponse",
            object(
                "Legacy completion, as the upstream answered it.",
                &[],
                json!({"id": {"type": "string"}, "object": {"type": "string", "const": "text_completion"}, "choices": {"type": "array", "items": {"type": "object", "additionalProperties": true}}, "usage": reference("Usage")}),
            ),
        ),
        (
            "ResponseObject",
            object(
                "Responses API object. rolter reads `id` (it keys the tenant-scoped lifecycle methods), `status` and `usage`.",
                &[],
                json!({
                    "id": {"type": "string"},
                    "object": {"type": "string", "const": "response"},
                    "status": {"type": "string", "description": "`completed`, `incomplete`, `failed`, `cancelled`, …"},
                    "output": {"type": "array", "items": {"type": "object", "additionalProperties": true}},
                    "usage": reference("Usage")
                }),
            ),
        ),
        (
            "ResponseDeletion",
            object(
                "Result of deleting a response, as the owning provider answered it.",
                &[],
                json!({"id": {"type": "string"}, "object": {"type": "string"}, "deleted": {"type": "boolean"}}),
            ),
        ),
        (
            "ResponseInputItems",
            object(
                "List of a response's input items, as the owning provider answered it.",
                &[],
                json!({"object": {"type": "string", "const": "list"}, "data": {"type": "array", "items": {"type": "object", "additionalProperties": true}}}),
            ),
        ),
        (
            "MessageResponse",
            object(
                "Anthropic message. `stop_reason` and `usage` are what rolter reads; a response translated from OpenAI or Gemini carries the same shape.",
                &[],
                json!({
                    "id": {"type": "string"},
                    "type": {"type": "string", "const": "message"},
                    "role": {"type": "string"},
                    "model": {"type": "string"},
                    "content": {"type": "array", "items": {"type": "object", "additionalProperties": true}},
                    "stop_reason": {"type": ["string", "null"]},
                    "usage": reference("Usage")
                }),
            ),
        ),
        (
            "EmbeddingResponse",
            object(
                "Embedding list, as the upstream answered it. rolter reads `usage`.",
                &[],
                json!({
                    "object": {"type": "string", "const": "list"},
                    "data": {"type": "array", "items": {"type": "object", "additionalProperties": true}},
                    "model": {"type": "string"},
                    "usage": reference("Usage")
                }),
            ),
        ),
        (
            "RerankResponse",
            object(
                "Ranked results in the Cohere v1 / Jina shape, as the upstream answered them.",
                &[],
                json!({"results": {"type": "array", "items": {"type": "object", "additionalProperties": true}}}),
            ),
        ),
        (
            "ImageResponse",
            object(
                "Generated images, as the upstream answered them.",
                &[],
                json!({"created": {"type": "integer"}, "data": {"type": "array", "items": {"type": "object", "additionalProperties": true}}}),
            ),
        ),
        (
            "TranscriptionResponse",
            object(
                "Transcript or translation as JSON; `text/plain` is returned for the `text`, `srt` and `vtt` formats. rolter reads `usage` when present.",
                &[],
                json!({"text": {"type": "string"}, "usage": reference("Usage")}),
            ),
        ),
        (
            "SystemOneResponse",
            object(
                "One typed answer per question id, plus usage; the shape of an answer depends on the question's `type`. rolter reads `model` and `usage`.",
                &[],
                json!({"model": {"type": "string"}, "answers": {"type": "object", "additionalProperties": true}, "usage": reference("Usage")}),
            ),
        ),
        (
            "ModelList",
            object(
                "The OpenAI list shape, answered by the gateway itself: the public model names the caller's key may use.",
                &["object", "data"],
                json!({
                    "object": {"type": "string", "const": "list"},
                    "data": {"type": "array", "items": object(
                        "one model",
                        &["id", "object"],
                        json!({
                            "id": {"type": "string", "description": "a route name or a `provider-slug/model` id"},
                            "object": {"type": "string", "const": "model"},
                            "owned_by": {"type": "string"},
                            "rolter_capabilities": object(
                                "which endpoints the resolved address serves",
                                &[],
                                json!({"decisions": {"type": "boolean"}, "chat": {"type": "boolean"}}),
                            )
                        }),
                    )}
                }),
            ),
        ),
    ]
}

/// Server-sent events. Only the families rolter forwards or produces by
/// translation are described; an event an upstream sends beyond these is
/// forwarded as-is on a native route.
fn streaming_events() -> Vec<(&'static str, Value)> {
    vec![
        (
            "ChatCompletionChunk",
            object(
                "One `data:` frame of a chat completion stream, `chat.completion.chunk`. The stream ends with `data: [DONE]`. The final chunk carries `usage` when the upstream reports it.",
                &[],
                json!({
                    "id": {"type": "string"},
                    "object": {"type": "string", "const": "chat.completion.chunk"},
                    "model": {"type": "string"},
                    "choices": {"type": "array", "items": object(
                        "one choice delta",
                        &[],
                        json!({"index": {"type": "integer"}, "delta": {"type": "object", "additionalProperties": true}, "finish_reason": {"type": ["string", "null"]}}),
                    )},
                    "usage": reference("Usage")
                }),
            ),
        ),
        (
            "MessageStreamEvent",
            object(
                "One Anthropic stream event, named by its `type`: `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta` and `message_stop`, plus `error`. A stream translated from OpenAI or Gemini produces these, without `ping`; a native Anthropic route also forwards the upstream's `ping` keep-alives as sent.",
                &["type"],
                json!({
                    "type": {"type": "string", "enum": ["message_start", "content_block_start", "content_block_delta", "content_block_stop", "message_delta", "message_stop", "error", "ping"]},
                    "index": {"type": "integer", "description": "the content block, on the `content_block_*` events"},
                    "message": {"type": "object", "additionalProperties": true, "description": "on `message_start`"},
                    "delta": {"type": "object", "additionalProperties": true, "description": "on `content_block_delta` and `message_delta`; the latter carries `stop_reason`"},
                    "usage": reference("Usage")
                }),
            ),
        ),
        (
            "ResponseStreamEvent",
            object(
                "One Responses stream event, named by its `type`: `response.created`, `response.output_item.added`, `response.content_part.added`, `response.output_text.delta`, `response.output_text.done`, `response.content_part.done`, `response.function_call_arguments.delta`, `response.function_call_arguments.done`, `response.output_item.done` and `response.completed`. A native route forwards the upstream's other `response.*` events unchanged.",
                &["type"],
                json!({
                    "type": {"type": "string"},
                    "response": {"description": "the response object, on `response.created` and `response.completed`", "$ref": "#/components/schemas/ResponseObject"},
                    "delta": {"type": "string", "description": "on the `*.delta` events"}
                }),
            ),
        ),
    ]
}
