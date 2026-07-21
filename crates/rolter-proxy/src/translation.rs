//! Wire-protocol translation between client and upstream API dialects.
//!
//! A [`TranslationPlan`] is resolved independently from provider identity. New
//! dialects add a protocol mapping and a pair implementation here; forwarding,
//! retries, caching and response accounting do not need provider-specific code.

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;
use rolter_core::{Error, ProviderKind, Result, RoleProfile};
use serde_json::{json, Map, Value};

/// Public wire dialect presented by a client or accepted by an upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    OpenAiChat,
    OpenAiResponses,
    AnthropicMessages,
    /// google gemini native `generateContent` / `streamGenerateContent`
    GeminiGenerate,
    Passthrough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamTranslation {
    OpenAiToAnthropic,
    AnthropicToOpenAi,
    OpenAiToResponses,
    AnthropicToResponses,
    GeminiToOpenAi,
    GeminiToAnthropic,
    GeminiToResponses,
}

struct TranslationPair {
    source: Protocol,
    target: Protocol,
    request: fn(Value) -> Value,
    response: fn(Value) -> Value,
    stream: StreamTranslation,
}

static TRANSLATION_PAIRS: &[TranslationPair] = &[
    TranslationPair {
        source: Protocol::OpenAiChat,
        target: Protocol::AnthropicMessages,
        request: openai_request,
        response: anthropic_response,
        stream: StreamTranslation::AnthropicToOpenAi,
    },
    TranslationPair {
        source: Protocol::OpenAiResponses,
        target: Protocol::OpenAiChat,
        request: responses_request,
        response: responses_from_openai,
        stream: StreamTranslation::OpenAiToResponses,
    },
    TranslationPair {
        source: Protocol::OpenAiResponses,
        target: Protocol::AnthropicMessages,
        request: responses_to_anthropic_request,
        response: responses_from_anthropic,
        stream: StreamTranslation::AnthropicToResponses,
    },
    TranslationPair {
        source: Protocol::AnthropicMessages,
        target: Protocol::OpenAiChat,
        request: anthropic_request,
        response: openai_response,
        stream: StreamTranslation::OpenAiToAnthropic,
    },
    TranslationPair {
        source: Protocol::OpenAiChat,
        target: Protocol::GeminiGenerate,
        request: openai_to_gemini,
        response: gemini_to_openai,
        stream: StreamTranslation::GeminiToOpenAi,
    },
    TranslationPair {
        source: Protocol::AnthropicMessages,
        target: Protocol::GeminiGenerate,
        request: anthropic_to_gemini,
        response: gemini_to_anthropic,
        stream: StreamTranslation::GeminiToAnthropic,
    },
    TranslationPair {
        source: Protocol::OpenAiResponses,
        target: Protocol::GeminiGenerate,
        request: responses_to_gemini,
        response: gemini_to_responses,
        stream: StreamTranslation::GeminiToResponses,
    },
];

fn registered_pair(source: Protocol, target: Protocol) -> Option<&'static TranslationPair> {
    TRANSLATION_PAIRS
        .iter()
        .find(|pair| pair.source == source && pair.target == target)
}

/// Immutable conversion selected for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranslationPlan {
    client: Protocol,
    upstream: Protocol,
    role_profile: RoleProfile,
}

impl TranslationPlan {
    pub const fn passthrough() -> Self {
        Self {
            client: Protocol::Passthrough,
            upstream: Protocol::Passthrough,
            role_profile: RoleProfile::Openai,
        }
    }

    /// Resolve the client dialect from the endpoint and the upstream dialect
    /// from provider capabilities.
    pub fn resolve(path: &str, provider: ProviderKind, role_profile: RoleProfile) -> Self {
        let client = match path {
            "/v1/chat/completions" => Protocol::OpenAiChat,
            "/v1/responses" => Protocol::OpenAiResponses,
            "/v1/messages" => Protocol::AnthropicMessages,
            _ => Protocol::Passthrough,
        };
        let upstream = match (client, provider) {
            // gemini native speaks its own generateContent wire format; any
            // chat-shaped client dialect is translated to it
            (Protocol::OpenAiChat, ProviderKind::GeminiNative) => Protocol::GeminiGenerate,
            (Protocol::AnthropicMessages, ProviderKind::GeminiNative) => Protocol::GeminiGenerate,
            (Protocol::OpenAiResponses, ProviderKind::GeminiNative) => Protocol::GeminiGenerate,
            (Protocol::OpenAiChat, ProviderKind::Anthropic) => Protocol::AnthropicMessages,
            (Protocol::AnthropicMessages, ProviderKind::Anthropic) => Protocol::AnthropicMessages,
            (Protocol::AnthropicMessages, _) => Protocol::OpenAiChat,
            (Protocol::OpenAiResponses, ProviderKind::Openai) => Protocol::OpenAiResponses,
            (Protocol::OpenAiResponses, ProviderKind::Anthropic) => Protocol::AnthropicMessages,
            (Protocol::OpenAiResponses, _) => Protocol::OpenAiChat,
            (other, _) => other,
        };
        Self {
            client,
            upstream,
            role_profile,
        }
    }

    pub fn is_translation(self) -> bool {
        self.client != self.upstream
    }

    pub fn upstream_path(self, original: &str) -> &str {
        match self.upstream {
            Protocol::OpenAiChat => "/v1/chat/completions",
            Protocol::OpenAiResponses => "/v1/responses",
            Protocol::AnthropicMessages => "/v1/messages",
            // gemini builds its URL from the model + method in the forwarder;
            // the fixed path is unused for this upstream
            Protocol::GeminiGenerate => original,
            Protocol::Passthrough => original,
        }
    }

    /// Whether this plan targets the gemini native generateContent upstream,
    /// whose URL embeds the model and streaming method rather than a fixed path.
    pub fn is_gemini_generate(self) -> bool {
        self.upstream == Protocol::GeminiGenerate
    }

    pub fn translate_request(self, body: Bytes) -> Result<Bytes> {
        let Ok(mut value) = serde_json::from_slice::<Value>(&body) else {
            return Ok(body);
        };
        validate_instruction_roles(&value, self.client, self.role_profile)?;
        if !self.is_translation() {
            normalize_openai_roles(&mut value, self.client, self.role_profile);
            return serde_json::to_vec(&value).map(Bytes::from).map_err(|err| {
                Error::Config(format!("role_capability: failed to encode request: {err}"))
            });
        }
        let Some(pair) = registered_pair(self.client, self.upstream) else {
            return Ok(body);
        };
        value = (pair.request)(value);
        normalize_openai_roles(&mut value, self.upstream, self.role_profile);
        serde_json::to_vec(&value).map(Bytes::from).map_err(|err| {
            Error::Config(format!("role_capability: failed to encode request: {err}"))
        })
    }

    /// Translate a complete JSON response or a fully buffered SSE response.
    pub fn translate_response(self, body: Bytes, is_sse: bool) -> Bytes {
        if !self.is_translation() {
            return body;
        }
        if is_sse {
            let mut converter = SseConverter::new(self);
            let mut out = Vec::new();
            out.extend(converter.feed(&body));
            out.extend(converter.finish());
            return Bytes::from(out.concat());
        }
        translate_json(body, self.client, self.upstream, false)
    }
}

/// Normalize Rolter's portable top-level `cache_control` into Anthropic's
/// native breakpoint markers. The portable shape is:
/// `{ "enabled": true, "ttl": "5m", "breakpoints": ["system", "tools", "messages"] }`.
/// Existing provider-native nested controls are left untouched. Providers whose
/// configured wire protocol is not Anthropic Messages reject the portable
/// control explicitly rather than accepting it and silently doing nothing.
pub fn normalize_prompt_cache_control(body: Bytes, provider: ProviderKind) -> Result<Bytes> {
    let Ok(mut value) = serde_json::from_slice::<Value>(&body) else {
        return Ok(body);
    };
    let Some(control) = value
        .as_object_mut()
        .and_then(|object| object.remove("cache_control"))
    else {
        return Ok(body);
    };
    if provider != ProviderKind::Anthropic {
        return Err(Error::Config(format!(
            "prompt_cache_unsupported: provider kind '{provider:?}' does not use the Anthropic Messages protocol"
        )));
    }
    let enabled = control
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !enabled {
        return serde_json::to_vec(&value).map(Bytes::from).map_err(|err| {
            Error::Config(format!("prompt_cache: failed to encode request: {err}"))
        });
    }
    let mut marker = json!({"type": "ephemeral"});
    if let Some(ttl) = control.get("ttl").and_then(Value::as_str) {
        if !matches!(ttl, "5m" | "1h") {
            return Err(Error::Config(
                "prompt_cache: ttl must be '5m' or '1h'".to_string(),
            ));
        }
        marker["ttl"] = Value::String(ttl.to_string());
    }
    let breakpoints = control
        .get("breakpoints")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_else(|| vec!["system"]);
    for breakpoint in breakpoints {
        match breakpoint {
            "system" => mark_last_content(&mut value, "system", &marker),
            "tools" => mark_last_item(&mut value, "tools", &marker),
            "messages" => mark_last_message_content(&mut value, &marker),
            other => {
                return Err(Error::Config(format!(
                    "prompt_cache: unsupported breakpoint '{other}' (use system|tools|messages)"
                )));
            }
        }
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|err| Error::Config(format!("prompt_cache: failed to encode request: {err}")))
}

fn mark_last_item(value: &mut Value, field: &str, marker: &Value) {
    let Some(item) = value
        .get_mut(field)
        .and_then(Value::as_array_mut)
        .and_then(|items| items.last_mut())
    else {
        return;
    };
    if item.get("cache_control").is_none() {
        item["cache_control"] = marker.clone();
    }
}

fn mark_last_content(value: &mut Value, field: &str, marker: &Value) {
    let Some(content) = value.get_mut(field) else {
        return;
    };
    match content {
        Value::Array(items) => {
            if let Some(item) = items.last_mut() {
                if item.get("cache_control").is_none() {
                    item["cache_control"] = marker.clone();
                }
            }
        }
        Value::String(text) => {
            *content = json!([{"type": "text", "text": text, "cache_control": marker}]);
        }
        _ => {}
    }
}

fn mark_last_message_content(value: &mut Value, marker: &Value) {
    let Some(message) = value
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .and_then(|messages| messages.last_mut())
    else {
        return;
    };
    match message.get_mut("content") {
        Some(Value::Array(parts)) => {
            if let Some(part) = parts.last_mut() {
                if part.get("cache_control").is_none() {
                    part["cache_control"] = marker.clone();
                }
            }
        }
        Some(Value::String(text)) => {
            let text = text.clone();
            message["content"] = json!([{"type": "text", "text": text, "cache_control": marker}]);
        }
        _ => {}
    }
}

fn validate_instruction_roles(
    value: &Value,
    protocol: Protocol,
    profile: RoleProfile,
) -> Result<()> {
    if profile == RoleProfile::Openai || protocol == Protocol::AnthropicMessages {
        return Ok(());
    }
    let messages = match protocol {
        Protocol::OpenAiChat => value.get("messages").and_then(Value::as_array),
        Protocol::OpenAiResponses => value.get("input").and_then(Value::as_array),
        _ => None,
    };
    let mut saw_turn = false;
    for message in messages.into_iter().flatten() {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        if matches!(role, "system" | "developer") {
            if saw_turn {
                return Err(Error::Config(format!(
                    "role_capability: {role} is only supported before the first conversational turn for the configured {:?} profile",
                    profile
                )));
            }
        } else {
            saw_turn = true;
        }
    }
    Ok(())
}

fn normalize_openai_roles(value: &mut Value, protocol: Protocol, profile: RoleProfile) {
    if profile != RoleProfile::SystemOnly {
        return;
    }
    let Some(messages) = (protocol == Protocol::OpenAiChat)
        .then(|| value.get_mut("messages"))
        .flatten()
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for message in messages {
        if message.get("role") == Some(&json!("developer")) {
            message["role"] = json!("system");
        }
    }
}

/// Lower a Responses request to Chat Completions without dropping text, image,
/// file, function-tool, or tool-result content that has a Chat equivalent.
fn responses_request(mut v: Value) -> Value {
    let Some(obj) = v.as_object_mut() else {
        return v;
    };
    let mut messages = Vec::new();
    if let Some(instructions) = obj.remove("instructions") {
        messages.push(json!({"role":"system","content":instructions}));
    }
    match obj.remove("input") {
        Some(Value::String(text)) => messages.push(json!({"role":"user","content":text})),
        Some(Value::Array(items)) => {
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call_output") => messages.push(json!({"role":"tool","tool_call_id":item["call_id"],"content":item.get("output").cloned().unwrap_or(Value::Null)})),
                    _ => {
                        let role = item
                            .get("role")
                            .and_then(Value::as_str)
                            .unwrap_or("user")
                            .to_string();
                        let content = item.get("content").cloned().unwrap_or(item);
                        messages.push(json!({"role":role,"content":responses_content_to_chat(content)}));
                    }
                }
            }
        }
        Some(input) => messages.push(json!({"role":"user","content":input})),
        None => {}
    }
    obj.insert("messages".into(), Value::Array(messages));
    if let Some(max) = obj.remove("max_output_tokens") {
        obj.insert("max_completion_tokens".into(), max);
    }
    if let Some(tools) = obj.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            if tool.get("type") == Some(&json!("function")) && tool.get("function").is_none() {
                let function = json!({"name":tool["name"],"description":tool["description"],"parameters":tool["parameters"]});
                *tool = json!({"type":"function","function":function});
            }
        }
    }
    remove_keys(
        obj,
        &[
            "background",
            "store",
            "previous_response_id",
            "reasoning",
            "text",
        ],
    );
    v
}

fn responses_content_to_chat(content: Value) -> Value {
    match content {
        Value::Array(parts) => Value::Array(
            parts
                .into_iter()
                .map(|part| match part.get("type").and_then(Value::as_str) {
                    Some("input_text") => json!({"type":"text","text":part["text"]}),
                    Some("input_image") => {
                        let mut image_url = Map::new();
                        image_url.insert(
                            "url".into(),
                            part.get("image_url").cloned().unwrap_or(Value::Null),
                        );
                        if let Some(detail) = part.get("detail") {
                            image_url.insert("detail".into(), detail.clone());
                        }
                        json!({"type":"image_url","image_url":image_url})
                    }
                    Some("input_file") => json!({"type":"input_file","input_file":part}),
                    _ => part,
                })
                .collect(),
        ),
        other => other,
    }
}

fn responses_to_anthropic_request(v: Value) -> Value {
    openai_request(responses_request(v))
}

fn responses_from_openai(v: Value) -> Value {
    let choice = v.pointer("/choices/0").unwrap_or(&Value::Null);
    let message = choice.get("message").unwrap_or(&Value::Null);
    let mut content = Vec::new();
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        content.push(json!({"type":"output_text","text":text}));
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            content.push(json!({"type":"function_call","id":call["id"],"call_id":call["id"],"name":call["function"]["name"],"arguments":call["function"]["arguments"]}));
        }
    }
    let input = v
        .pointer("/usage/prompt_tokens")
        .cloned()
        .unwrap_or(json!(0));
    let output = v
        .pointer("/usage/completion_tokens")
        .cloned()
        .unwrap_or(json!(0));
    json!({"id":v.get("id").cloned().unwrap_or_else(|| json!("resp_rolter")),"object":"response","status":"completed","model":v.get("model").cloned().unwrap_or(Value::Null),"output":[{"id":"msg_rolter","type":"message","status":"completed","role":"assistant","content":content}],"usage":{"input_tokens":input,"output_tokens":output,"total_tokens":input.as_u64().unwrap_or(0) + output.as_u64().unwrap_or(0)}})
}

fn responses_from_anthropic(v: Value) -> Value {
    responses_from_openai(anthropic_response(v))
}

fn translate_json(body: Bytes, from: Protocol, to: Protocol, request: bool) -> Bytes {
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return body;
    };
    let Some(pair) = registered_pair(from, to) else {
        return body;
    };
    let translated = if request {
        (pair.request)(value)
    } else {
        (pair.response)(value)
    };
    serde_json::to_vec(&translated)
        .map(Bytes::from)
        .unwrap_or(body)
}

fn openai_request(mut v: Value) -> Value {
    let Some(obj) = v.as_object_mut() else {
        return v;
    };
    let messages = obj
        .remove("messages")
        .and_then(|v| match v {
            Value::Array(a) => Some(a),
            _ => None,
        })
        .unwrap_or_default();
    let mut system = Vec::with_capacity(1);
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        if role == "system" || role == "developer" {
            system.extend(openai_content(message.get("content"), true));
            continue;
        }
        if role == "tool" {
            out.push(json!({"role":"user","content":[{
                "type":"tool_result",
                "tool_use_id":message.get("tool_call_id").cloned().unwrap_or(Value::String(String::new())),
                "content":content_text(message.get("content"))
            }]}));
            continue;
        }
        let mut content = openai_content(message.get("content"), false);
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let function = call.get("function").unwrap_or(&Value::Null);
                let input = function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_else(|| json!({}));
                content.push(json!({
                    "type":"tool_use",
                    "id":call.get("id").cloned().unwrap_or(Value::String(String::new())),
                    "name":function.get("name").cloned().unwrap_or(Value::String(String::new())),
                    "input":input
                }));
            }
        }
        out.push(
            json!({"role": if role == "assistant" {"assistant"} else {"user"}, "content":content}),
        );
    }
    obj.insert("messages".into(), Value::Array(out));
    if !system.is_empty() {
        obj.insert("system".into(), Value::Array(system));
    }
    if !obj.contains_key("max_tokens") {
        let max = obj.remove("max_completion_tokens").unwrap_or(json!(1024));
        obj.insert("max_tokens".into(), max);
    }
    obj.remove("max_completion_tokens");
    if let Some(stop) = obj.remove("stop") {
        obj.insert(
            "stop_sequences".into(),
            match stop {
                Value::String(s) => json!([s]),
                v => v,
            },
        );
    }
    if let Some(tools) = obj.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            if let Some(function) = tool.get("function").cloned() {
                let mut translated = json!({
                    "name":function.get("name").cloned().unwrap_or(Value::String(String::new())),
                    "input_schema":function.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object"}))
                });
                if let Some(description) = function.get("description").filter(|v| !v.is_null()) {
                    translated["description"] = description.clone();
                }
                *tool = translated;
            }
        }
    }
    if let Some(choice) = obj.get_mut("tool_choice") {
        *choice = match choice.take() {
            Value::String(s) if s == "required" => json!({"type":"any"}),
            Value::String(s) => json!({"type":s}),
            Value::Object(m) => m
                .get("function")
                .and_then(|f| f.get("name"))
                .map(|n| json!({"type":"tool","name":n}))
                .unwrap_or(Value::Object(m)),
            other => other,
        };
    }
    remove_keys(
        obj,
        &[
            "n",
            "presence_penalty",
            "frequency_penalty",
            "logprobs",
            "stream_options",
        ],
    );
    v
}

fn anthropic_request(mut v: Value) -> Value {
    let Some(obj) = v.as_object_mut() else {
        return v;
    };
    let source_messages = obj
        .remove("messages")
        .and_then(|v| match v {
            Value::Array(a) => Some(a),
            _ => None,
        })
        .unwrap_or_default();
    let mut messages = Vec::with_capacity(source_messages.len() + 1);
    if let Some(system) = obj.remove("system") {
        messages.push(json!({"role":"system","content":anthropic_content(Some(&system))}));
    }
    for message in source_messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let mut regular = Vec::new();
        let mut tool_calls = Vec::new();
        let mut tool_results = Vec::new();
        for block in anthropic_content(message.get("content")) {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => tool_calls.push(json!({"id":block["id"],"type":"function","function":{"name":block["name"],"arguments":serde_json::to_string(&block["input"]).unwrap_or_else(|_| "{}".into())}})),
                Some("tool_result") => tool_results.push(json!({"role":"tool","tool_call_id":block["tool_use_id"],"content":content_text(block.get("content"))})),
                _ => regular.push(anthropic_block_to_openai(block)),
            }
        }
        if !regular.is_empty() || !tool_calls.is_empty() {
            let content = if regular.len() == 1 && regular[0].get("type") == Some(&json!("text")) {
                regular[0]["text"].clone()
            } else if regular.is_empty() {
                Value::Null
            } else {
                Value::Array(regular)
            };
            let mut translated = json!({"role":role,"content":content});
            if !tool_calls.is_empty() {
                translated["tool_calls"] = Value::Array(tool_calls);
            }
            messages.push(translated);
        }
        messages.extend(tool_results);
    }
    obj.insert("messages".into(), Value::Array(messages));
    if let Some(stop) = obj.remove("stop_sequences") {
        obj.insert("stop".into(), stop);
    }
    if let Some(tools) = obj.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            let name = tool
                .get("name")
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let parameters = tool
                .get("input_schema")
                .cloned()
                .unwrap_or_else(|| json!({"type":"object"}));
            let mut translated =
                json!({"type":"function","function":{"name":name,"parameters":parameters}});
            if let Some(description) = tool.get("description").filter(|v| !v.is_null()) {
                translated["function"]["description"] = description.clone();
            }
            *tool = translated;
        }
    }
    if let Some(choice) = obj.get_mut("tool_choice") {
        *choice = match choice.take() {
            Value::Object(m) if m.get("type") == Some(&json!("any")) => json!("required"),
            Value::Object(m) if m.get("type") == Some(&json!("tool")) => {
                json!({"type":"function","function":{"name":m.get("name").cloned().unwrap_or(Value::Null)}})
            }
            Value::Object(m) => m.get("type").cloned().unwrap_or(Value::Object(m)),
            other => other,
        };
    }
    v
}

fn openai_content(content: Option<&Value>, system: bool) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![json!({"type":"text","text":text})],
        Some(Value::Array(parts)) => parts.iter().map(|part| {
            match part.get("type").and_then(Value::as_str) {
                Some("text") | Some("input_text") => json!({"type":"text","text":part.get("text").cloned().unwrap_or(Value::String(String::new()))}),
                Some("image_url") | Some("image_file") | Some("input_image") => openai_image(part),
                Some("input_file") | Some("file") => openai_document(part),
                _ => part.clone(),
            }
        }).collect(),
        Some(other) if system => vec![json!({"type":"text","text":other.to_string()})],
        Some(other) => vec![other.clone()],
        None => Vec::new(),
    }
}

fn anthropic_content(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![json!({"type":"text","text":text})],
        Some(Value::Array(parts)) => parts.clone(),
        Some(other) => vec![other.clone()],
        None => Vec::new(),
    }
}

fn openai_image(part: &Value) -> Value {
    if let Some(file_id) = part.pointer("/image_file/file_id") {
        return json!({"type":"image","source":{"type":"file","file_id":file_id}});
    }
    let image = part.get("image_url").unwrap_or(&Value::Null);
    let url = image
        .get("url")
        .or_else(|| part.get("image_url"))
        .or_else(|| image.as_str().map(|_| image))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some((media_type, data)) = data_url(url) {
        json!({"type":"image","source":{"type":"base64","media_type":media_type,"data":data}})
    } else {
        json!({"type":"image","source":{"type":"url","url":url}})
    }
}

fn openai_document(part: &Value) -> Value {
    let file = part
        .get("input_file")
        .or_else(|| part.get("file"))
        .unwrap_or(part);
    if let Some(data) = file
        .get("file_data")
        .or_else(|| file.get("file_url"))
        .and_then(Value::as_str)
    {
        if let Some((media_type, payload)) = data_url(data) {
            let mut document = json!({"type":"document","source":{"type":"base64","media_type":media_type,"data":payload}});
            if let Some(filename) = file.get("filename").filter(|v| !v.is_null()) {
                document["title"] = filename.clone();
            }
            return document;
        }
        let mut document = json!({"type":"document","source":{"type":"url","url":data}});
        if let Some(filename) = file.get("filename").filter(|v| !v.is_null()) {
            document["title"] = filename.clone();
        }
        return document;
    }
    json!({"type":"document","source":{"type":"file","file_id":file.get("file_id").cloned().unwrap_or(Value::Null)}})
}

fn anthropic_block_to_openai(block: Value) -> Value {
    match block.get("type").and_then(Value::as_str) {
        Some("image") => {
            let source = &block["source"];
            if source["type"] == "file" {
                return json!({"type":"image_file","image_file":{"file_id":source["file_id"]}});
            }
            let url = if source["type"] == "base64" {
                format!(
                    "data:{};base64,{}",
                    source["media_type"]
                        .as_str()
                        .unwrap_or("application/octet-stream"),
                    source["data"].as_str().unwrap_or_default()
                )
            } else {
                source["url"].as_str().unwrap_or_default().to_string()
            };
            json!({"type":"image_url","image_url":{"url":url}})
        }
        Some("document") => {
            let source = &block["source"];
            if source["type"] == "file" {
                return json!({"type":"input_file","input_file":{"file_id":source["file_id"]}});
            }
            let file_data = if source["type"] == "base64" {
                format!(
                    "data:{};base64,{}",
                    source["media_type"]
                        .as_str()
                        .unwrap_or("application/octet-stream"),
                    source["data"].as_str().unwrap_or_default()
                )
            } else {
                source["url"].as_str().unwrap_or_default().to_string()
            };
            let mut file = json!({"type":"input_file","input_file":{"file_data":file_data}});
            if let Some(title) = block.get("title").filter(|v| !v.is_null()) {
                file["input_file"]["filename"] = title.clone();
            }
            file
        }
        Some("text") => {
            json!({"type":"text","text":block.get("text").cloned().unwrap_or(Value::String(String::new()))})
        }
        _ => block,
    }
}

fn anthropic_response(v: Value) -> Value {
    let mut text = String::new();
    let mut calls = Vec::new();
    for block in v
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => text.push_str(block.get("text").and_then(Value::as_str).unwrap_or_default()),
            Some("tool_use") => calls.push(json!({"id":block["id"],"type":"function","function":{"name":block["name"],"arguments":serde_json::to_string(&block["input"]).unwrap_or_else(|_| "{}".into())}})),
            _ => {}
        }
    }
    let mut message = json!({"role":"assistant","content": if text.is_empty() {Value::Null} else {Value::String(text)}});
    if !calls.is_empty() {
        message["tool_calls"] = Value::Array(calls);
    }
    let input = v
        .pointer("/usage/input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = v
        .pointer("/usage/output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({
        "id":v.get("id").cloned().unwrap_or_else(|| json!("chatcmpl-rolter")),
        "object":"chat.completion",
        "created":0,
        "model":v.get("model").cloned().unwrap_or(Value::Null),
        "choices":[{"index":0,"message":message,"finish_reason":anthropic_finish(v.get("stop_reason"))}],
        "usage":{"prompt_tokens":input,"completion_tokens":output,"total_tokens":input + output}
    })
}

fn openai_response(v: Value) -> Value {
    let choice = v.pointer("/choices/0").unwrap_or(&Value::Null);
    let message = choice.get("message").unwrap_or(&Value::Null);
    let mut content = openai_content(message.get("content"), false);
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let function = &call["function"];
            let input = function["arguments"]
                .as_str()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));
            content.push(
                json!({"type":"tool_use","id":call["id"],"name":function["name"],"input":input}),
            );
        }
    }
    json!({
        "id":v.get("id").cloned().unwrap_or_else(|| json!("msg_rolter")),
        "type":"message","role":"assistant",
        "model":v.get("model").cloned().unwrap_or(Value::Null),
        "content":content,
        "stop_reason":openai_finish(choice.get("finish_reason")),"stop_sequence":Value::Null,
        "usage":{"input_tokens":v.pointer("/usage/prompt_tokens").cloned().unwrap_or(json!(0)),"output_tokens":v.pointer("/usage/completion_tokens").cloned().unwrap_or(json!(0))}
    })
}

fn anthropic_finish(v: Option<&Value>) -> Value {
    match v.and_then(Value::as_str) {
        Some("max_tokens") => json!("length"),
        Some("tool_use") => json!("tool_calls"),
        Some("end_turn" | "stop_sequence") => json!("stop"),
        _ => Value::Null,
    }
}

fn openai_finish(v: Option<&Value>) -> Value {
    match v.and_then(Value::as_str) {
        Some("length") => json!("max_tokens"),
        Some("tool_calls" | "function_call") => json!("tool_use"),
        Some("stop") => json!("end_turn"),
        _ => Value::Null,
    }
}

fn content_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

fn data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    Some((meta.strip_suffix(";base64").unwrap_or(meta), data))
}

fn remove_keys(obj: &mut Map<String, Value>, keys: &[&str]) {
    for key in keys {
        obj.remove(*key);
    }
}

// --- google gemini native generateContent translation ----------------------

/// Translate an OpenAI Chat Completions request into a Gemini `generateContent`
/// body. System/developer messages become `systemInstruction`; user/assistant
/// turns become `contents` with roles `user`/`model`; tool messages become
/// `functionResponse` parts; OpenAI functions become `tools.functionDeclarations`;
/// sampling params become `generationConfig`. The `model` and `stream` fields are
/// dropped here — the forwarder carries the model and streaming method in the URL.
fn openai_to_gemini(mut v: Value) -> Value {
    let Some(obj) = v.as_object_mut() else {
        return v;
    };
    let messages = obj
        .remove("messages")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let mut system_parts = Vec::new();
    let mut contents = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        match role {
            "system" | "developer" => {
                system_parts.extend(gemini_parts_from_content(message.get("content")));
            }
            "tool" => {
                contents.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": message.get("tool_call_id").cloned().unwrap_or(Value::String("tool".into())),
                            "response": {"result": content_text(message.get("content"))}
                        }
                    }]
                }));
            }
            _ => {
                let mut parts = gemini_parts_from_content(message.get("content"));
                if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        let function = call.get("function").unwrap_or(&Value::Null);
                        let args = function
                            .get("arguments")
                            .and_then(Value::as_str)
                            .and_then(|s| serde_json::from_str::<Value>(s).ok())
                            .unwrap_or_else(|| json!({}));
                        parts.push(json!({
                            "functionCall": {
                                "name": function.get("name").cloned().unwrap_or(Value::String(String::new())),
                                "args": args
                            }
                        }));
                    }
                }
                let gemini_role = if role == "assistant" { "model" } else { "user" };
                contents.push(json!({"role": gemini_role, "parts": parts}));
            }
        }
    }

    let mut out = Map::new();
    out.insert("contents".into(), Value::Array(contents));
    if !system_parts.is_empty() {
        out.insert("systemInstruction".into(), json!({"parts": system_parts}));
    }

    // generationConfig from OpenAI sampling params
    let mut gen = Map::new();
    if let Some(t) = obj.remove("temperature") {
        gen.insert("temperature".into(), t);
    }
    if let Some(p) = obj.remove("top_p") {
        gen.insert("topP".into(), p);
    }
    let max = obj
        .remove("max_completion_tokens")
        .or_else(|| obj.remove("max_tokens"));
    if let Some(max) = max {
        gen.insert("maxOutputTokens".into(), max);
    }
    if let Some(stop) = obj.remove("stop") {
        gen.insert(
            "stopSequences".into(),
            match stop {
                Value::String(s) => json!([s]),
                other => other,
            },
        );
    }
    if let Some(n) = obj.remove("n") {
        gen.insert("candidateCount".into(), n);
    }
    if !gen.is_empty() {
        out.insert("generationConfig".into(), Value::Object(gen));
    }

    // tools: OpenAI function tools -> a single functionDeclarations group
    if let Some(tools) = obj.remove("tools").and_then(|t| t.as_array().cloned()) {
        let declarations: Vec<Value> = tools
            .iter()
            .filter_map(|tool| tool.get("function"))
            .map(|function| {
                let mut decl = json!({
                    "name": function.get("name").cloned().unwrap_or(Value::String(String::new()))
                });
                if let Some(description) = function.get("description").filter(|v| !v.is_null()) {
                    decl["description"] = description.clone();
                }
                if let Some(parameters) = function.get("parameters").filter(|v| !v.is_null()) {
                    decl["parameters"] = parameters.clone();
                }
                decl
            })
            .collect();
        if !declarations.is_empty() {
            out.insert(
                "tools".into(),
                json!([{"functionDeclarations": declarations}]),
            );
        }
    }
    if let Some(choice) = obj.remove("tool_choice") {
        let mode = match &choice {
            Value::String(s) if s == "required" => Some("ANY"),
            Value::String(s) if s == "none" => Some("NONE"),
            Value::String(s) if s == "auto" => Some("AUTO"),
            Value::Object(_) => Some("ANY"),
            _ => None,
        };
        if let Some(mode) = mode {
            out.insert(
                "toolConfig".into(),
                json!({"functionCallingConfig": {"mode": mode}}),
            );
        }
    }

    Value::Object(out)
}

fn anthropic_to_gemini(v: Value) -> Value {
    openai_to_gemini(anthropic_request(v))
}

fn responses_to_gemini(v: Value) -> Value {
    openai_to_gemini(responses_request(v))
}

/// Build Gemini `parts` from an OpenAI message `content` value (string, or an
/// array of typed parts). Data URLs become `inlineData`; other URLs become
/// `fileData`.
fn gemini_parts_from_content(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![json!({"text": text})],
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") | Some("input_text") => Some(json!({
                    "text": part.get("text").cloned().unwrap_or(Value::String(String::new()))
                })),
                Some("image_url") | Some("input_image") => {
                    let url = part
                        .pointer("/image_url/url")
                        .or_else(|| part.get("image_url"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Some(gemini_media_part(url))
                }
                _ => None,
            })
            .collect(),
        Some(other) => vec![json!({"text": other.to_string()})],
        None => Vec::new(),
    }
}

fn gemini_media_part(url: &str) -> Value {
    if let Some((media_type, data)) = data_url(url) {
        json!({"inlineData": {"mimeType": media_type, "data": data}})
    } else {
        json!({"fileData": {"fileUri": url}})
    }
}

/// Translate a Gemini `generateContent` response into an OpenAI Chat Completion.
fn gemini_to_openai(v: Value) -> Value {
    let candidate = v.pointer("/candidates/0").unwrap_or(&Value::Null);
    let mut text = String::new();
    let mut calls = Vec::new();
    for part in candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(t) = part.get("text").and_then(Value::as_str) {
            text.push_str(t);
        } else if let Some(function_call) = part.get("functionCall") {
            let args = function_call
                .get("args")
                .cloned()
                .unwrap_or_else(|| json!({}));
            calls.push(json!({
                "id": format!("call_{}", calls.len()),
                "type": "function",
                "function": {
                    "name": function_call.get("name").cloned().unwrap_or(Value::String(String::new())),
                    "arguments": serde_json::to_string(&args).unwrap_or_else(|_| "{}".into())
                }
            }));
        }
    }
    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() { Value::Null } else { Value::String(text) }
    });
    let finish = if calls.is_empty() {
        gemini_finish(candidate.get("finishReason"))
    } else {
        message["tool_calls"] = Value::Array(calls);
        json!("tool_calls")
    };
    let prompt = v
        .pointer("/usageMetadata/promptTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completion = v
        .pointer("/usageMetadata/candidatesTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = v
        .pointer("/usageMetadata/totalTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(prompt + completion);
    json!({
        "id": "chatcmpl-rolter",
        "object": "chat.completion",
        "created": 0,
        "model": v.get("modelVersion").cloned().unwrap_or(Value::Null),
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": {"prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": total}
    })
}

fn gemini_to_anthropic(v: Value) -> Value {
    openai_response(gemini_to_openai(v))
}

fn gemini_to_responses(v: Value) -> Value {
    responses_from_openai(gemini_to_openai(v))
}

fn gemini_finish(v: Option<&Value>) -> Value {
    match v.and_then(Value::as_str) {
        Some("MAX_TOKENS") => json!("length"),
        Some("SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT") => {
            json!("content_filter")
        }
        Some(_) => json!("stop"),
        None => Value::Null,
    }
}

struct SseConverter {
    plan: TranslationPlan,
    pending: Vec<u8>,
    state: StreamState,
}

#[derive(Default)]
struct StreamState {
    id: String,
    model: String,
    open_text: bool,
    tool_indexes: HashMap<usize, usize>,
    next_tool: usize,
    started: bool,
    message_start_sent: bool,
    stopped: bool,
    response_started: bool,
    response_completed: bool,
    response_usage: Value,
    gemini_finished: bool,
}

impl SseConverter {
    fn new(plan: TranslationPlan) -> Self {
        Self {
            plan,
            pending: Vec::new(),
            state: StreamState::default(),
        }
    }

    fn feed(&mut self, chunk: &[u8]) -> Vec<Bytes> {
        self.pending.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some(end) = find_frame(&self.pending) {
            let raw: Vec<u8> = self.pending.drain(..end).collect();
            drain_separator(&mut self.pending);
            frames.extend(self.convert_frame(&raw));
        }
        frames
    }

    fn finish(&mut self) -> Vec<Bytes> {
        let tail = std::mem::take(&mut self.pending);
        if tail.is_empty() {
            Vec::new()
        } else {
            self.convert_frame(&tail)
        }
    }

    fn convert_frame(&mut self, raw: &[u8]) -> Vec<Bytes> {
        let text = String::from_utf8_lossy(raw);
        let event = text
            .lines()
            .find_map(|l| l.strip_prefix("event:"))
            .map(str::trim);
        let data = text
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            return vec![Bytes::from(format!("{text}\n\n"))];
        }
        match registered_pair(self.plan.client, self.plan.upstream).map(|pair| pair.stream) {
            Some(StreamTranslation::AnthropicToOpenAi) => self.anthropic_to_openai(event, &data),
            Some(StreamTranslation::OpenAiToAnthropic) => self.openai_to_anthropic(&data),
            Some(StreamTranslation::OpenAiToResponses) => self.openai_to_responses(&data),
            Some(StreamTranslation::AnthropicToResponses) => {
                let chunks = self.anthropic_to_openai(event, &data);
                chunks
                    .into_iter()
                    .flat_map(|chunk| self.openai_to_responses_frame(&chunk))
                    .collect()
            }
            Some(StreamTranslation::GeminiToOpenAi) => self.gemini_to_openai_stream(&data, true),
            Some(StreamTranslation::GeminiToAnthropic) => {
                let chunks = self.gemini_to_openai_stream(&data, true);
                chunks
                    .into_iter()
                    .flat_map(|chunk| self.openai_frame_to_anthropic(&chunk))
                    .collect()
            }
            Some(StreamTranslation::GeminiToResponses) => {
                let chunks = self.gemini_to_openai_stream(&data, true);
                chunks
                    .into_iter()
                    .flat_map(|chunk| self.openai_to_responses_frame(&chunk))
                    .collect()
            }
            None => vec![Bytes::from(format!("{text}\n\n"))],
        }
    }

    fn anthropic_to_openai(&mut self, event: Option<&str>, data: &str) -> Vec<Bytes> {
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        let mut chunks = Vec::new();
        match event.or_else(|| v.get("type").and_then(Value::as_str)) {
            Some("message_start") => {
                self.state.id = v.pointer("/message/id").and_then(Value::as_str).unwrap_or("chatcmpl-rolter").to_string();
                self.state.model = v.pointer("/message/model").and_then(Value::as_str).unwrap_or_default().to_string();
                chunks.push(openai_chunk(&self.state, json!({"role":"assistant","content":""}), Value::Null, v.pointer("/message/usage")));
            }
            Some("content_block_start") => {
                let index = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if v.pointer("/content_block/type") == Some(&json!("tool_use")) {
                    let ti = self.state.next_tool; self.state.next_tool += 1; self.state.tool_indexes.insert(index, ti);
                    chunks.push(openai_chunk(&self.state, json!({"tool_calls":[{"index":ti,"id":v.pointer("/content_block/id").cloned().unwrap_or(Value::Null),"type":"function","function":{"name":v.pointer("/content_block/name").cloned().unwrap_or(Value::Null),"arguments":""}}]}), Value::Null, None));
                }
            }
            Some("content_block_delta") => match v.pointer("/delta/type").and_then(Value::as_str) {
                Some("text_delta") => chunks.push(openai_chunk(&self.state, json!({"content":v.pointer("/delta/text").cloned().unwrap_or(Value::String(String::new()))}), Value::Null, None)),
                Some("input_json_delta") => {
                    let index = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let ti = self.state.tool_indexes.get(&index).copied().unwrap_or(0);
                    chunks.push(openai_chunk(&self.state, json!({"tool_calls":[{"index":ti,"function":{"arguments":v.pointer("/delta/partial_json").cloned().unwrap_or(Value::String(String::new()))}}]}), Value::Null, None));
                }
                _ => {}
            },
            Some("message_delta") => chunks.push(openai_chunk(&self.state, json!({}), anthropic_finish(v.pointer("/delta/stop_reason")), v.get("usage"))),
            Some("message_stop") => chunks.push(Bytes::from_static(b"data: [DONE]\n\n")),
            Some("error") => chunks.push(sse(None, &v)),
            _ => {}
        }
        chunks
    }

    fn openai_to_anthropic(&mut self, data: &str) -> Vec<Bytes> {
        if data == "[DONE]" {
            if self.state.stopped {
                return Vec::new();
            }
            self.state.stopped = true;
            return vec![sse(Some("message_stop"), &json!({"type":"message_stop"}))];
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        if !self.state.started {
            self.state.started = true;
            self.state.id = v
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("msg_rolter")
                .to_string();
            self.state.model = v
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
        let mut out = Vec::new();
        let delta = v.pointer("/choices/0/delta").unwrap_or(&Value::Null);
        if !self.state.message_start_sent {
            self.state.message_start_sent = true;
            out.push(sse(Some("message_start"), &json!({"type":"message_start","message":{"id":self.state.id,"type":"message","role":"assistant","model":self.state.model,"content":[],"stop_reason":Value::Null,"stop_sequence":Value::Null,"usage":{"input_tokens":v.pointer("/usage/prompt_tokens").cloned().unwrap_or(json!(0)),"output_tokens":0}}})));
        }
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            if !self.state.open_text {
                self.state.open_text = true;
                out.push(sse(Some("content_block_start"), &json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})));
            }
            out.push(sse(Some("content_block_delta"), &json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}})));
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let block_index = index + usize::from(self.state.open_text);
                let is_new = !self.state.tool_indexes.contains_key(&index);
                self.state.tool_indexes.insert(index, block_index);
                if is_new {
                    out.push(sse(Some("content_block_start"), &json!({"type":"content_block_start","index":block_index,"content_block":{"type":"tool_use","id":call.get("id").cloned().unwrap_or(Value::Null),"name":call.pointer("/function/name").cloned().unwrap_or(Value::Null),"input":{}}})));
                }
                if let Some(args) = call.pointer("/function/arguments").and_then(Value::as_str) {
                    out.push(sse(Some("content_block_delta"), &json!({"type":"content_block_delta","index":block_index,"delta":{"type":"input_json_delta","partial_json":args}})));
                }
            }
        }
        if let Some(reason) = v
            .pointer("/choices/0/finish_reason")
            .filter(|v| !v.is_null())
        {
            if self.state.open_text {
                out.push(sse(
                    Some("content_block_stop"),
                    &json!({"type":"content_block_stop","index":0}),
                ));
            }
            for block_index in self.state.tool_indexes.values() {
                out.push(sse(
                    Some("content_block_stop"),
                    &json!({"type":"content_block_stop","index":block_index}),
                ));
            }
            out.push(sse(Some("message_delta"), &json!({"type":"message_delta","delta":{"stop_reason":openai_finish(Some(reason)),"stop_sequence":Value::Null},"usage":{"input_tokens":v.pointer("/usage/prompt_tokens").cloned().unwrap_or(json!(0)),"output_tokens":v.pointer("/usage/completion_tokens").cloned().unwrap_or(json!(0))}})));
        } else if v.get("usage").is_some() {
            // OpenAI commonly sends usage in a final choices-less chunk. Keep
            // it visible to Anthropic clients and to the gateway's accounting
            // stream instead of losing it after the finish-reason event.
            out.push(sse(Some("message_delta"), &json!({"type":"message_delta","delta":{},"usage":{"input_tokens":v.pointer("/usage/prompt_tokens").cloned().unwrap_or(json!(0)),"output_tokens":v.pointer("/usage/completion_tokens").cloned().unwrap_or(json!(0))}})));
        }
        out
    }

    fn openai_to_responses_frame(&mut self, frame: &Bytes) -> Vec<Bytes> {
        let text = String::from_utf8_lossy(frame);
        let data = text
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n");
        self.openai_to_responses(&data)
    }

    fn openai_to_responses(&mut self, data: &str) -> Vec<Bytes> {
        if data == "[DONE]" {
            return self.complete_responses_stream();
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        if !self.state.response_started {
            self.state.response_started = true;
            self.state.id = v
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("resp_rolter")
                .to_string();
            self.state.model = v
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
        if let Some(usage) = v.get("usage") {
            self.state.response_usage = json!({
                "input_tokens": usage.get("prompt_tokens").cloned().unwrap_or_else(|| usage.get("input_tokens").cloned().unwrap_or(json!(0))),
                "output_tokens": usage.get("completion_tokens").cloned().unwrap_or_else(|| usage.get("output_tokens").cloned().unwrap_or(json!(0))),
                "total_tokens": usage.get("total_tokens").cloned().unwrap_or(json!(0)),
            });
        }
        let mut out = Vec::new();
        if !self.state.started {
            self.state.started = true;
            out.push(sse(Some("response.created"), &json!({"type":"response.created","response":{"id":self.state.id,"object":"response","status":"in_progress","model":self.state.model}})));
            out.push(sse(Some("response.output_item.added"), &json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_rolter","type":"message","status":"in_progress","role":"assistant","content":[]}})));
        }
        let delta = v.pointer("/choices/0/delta").unwrap_or(&Value::Null);
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            if !self.state.open_text {
                self.state.open_text = true;
                out.push(sse(Some("response.content_part.added"), &json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":""}})));
            }
            out.push(sse(Some("response.output_text.delta"), &json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":text})));
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let call_index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let output_index = call_index + usize::from(self.state.open_text);
                let is_new = !self.state.tool_indexes.contains_key(&call_index);
                self.state.tool_indexes.insert(call_index, output_index);
                if is_new {
                    out.push(sse(Some("response.output_item.added"), &json!({
                        "type":"response.output_item.added", "output_index":output_index,
                        "item":{"id":call.get("id").cloned().unwrap_or(Value::Null),"type":"function_call","status":"in_progress","call_id":call.get("id").cloned().unwrap_or(Value::Null),"name":call.pointer("/function/name").cloned().unwrap_or(Value::Null),"arguments":""}
                    })));
                }
                if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str)
                {
                    out.push(sse(Some("response.function_call_arguments.delta"), &json!({
                        "type":"response.function_call_arguments.delta", "output_index":output_index,
                        "item_id":call.get("id").cloned().unwrap_or(Value::Null), "delta":arguments
                    })));
                }
            }
        }
        out
    }

    fn complete_responses_stream(&mut self) -> Vec<Bytes> {
        if self.state.response_completed {
            return Vec::new();
        }
        self.state.response_completed = true;
        let mut out = Vec::new();
        if self.state.open_text {
            out.push(sse(Some("response.output_text.done"), &json!({"type":"response.output_text.done","output_index":0,"content_index":0,"text":""})));
            out.push(sse(Some("response.content_part.done"), &json!({"type":"response.content_part.done","output_index":0,"content_index":0,"part":{"type":"output_text","text":""}})));
        }
        for output_index in self.state.tool_indexes.values() {
            out.push(sse(Some("response.function_call_arguments.done"), &json!({
                "type":"response.function_call_arguments.done", "output_index":output_index, "arguments":""
            })));
            out.push(sse(Some("response.output_item.done"), &json!({
                "type":"response.output_item.done", "output_index":output_index,
                "item":{"id":Value::Null,"type":"function_call","status":"completed","arguments":""}
            })));
        }
        out.push(sse(Some("response.output_item.done"), &json!({"type":"response.output_item.done","output_index":0,"item":{"id":"msg_rolter","type":"message","status":"completed","role":"assistant","content":[]}})));
        out.push(sse(Some("response.completed"), &json!({"type":"response.completed","response":{"id":self.state.id,"object":"response","status":"completed","model":self.state.model,"usage":self.state.response_usage}})));
        out
    }

    /// convert one gemini `streamGenerateContent` SSE data payload (a full
    /// GenerateContentResponse) into openai chat.completion.chunk frames.
    /// gemini has no `[DONE]` sentinel — the last chunk carries `finishReason`,
    /// so we synthesize the finish chunk (and optional `[DONE]`) from it.
    fn gemini_to_openai_stream(&mut self, data: &str, emit_done: bool) -> Vec<Bytes> {
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if !self.state.started {
            self.state.started = true;
            self.state.id = "chatcmpl-rolter".to_string();
            self.state.model = v
                .get("modelVersion")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            out.push(openai_chunk(
                &self.state,
                json!({"role":"assistant","content":""}),
                Value::Null,
                None,
            ));
        }
        let candidate = v.pointer("/candidates/0").cloned().unwrap_or(Value::Null);
        for part in candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(t) = part.get("text").and_then(Value::as_str) {
                out.push(openai_chunk(
                    &self.state,
                    json!({"content":t}),
                    Value::Null,
                    None,
                ));
            } else if let Some(function_call) = part.get("functionCall") {
                let ti = self.state.next_tool;
                self.state.next_tool += 1;
                let args = function_call
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                out.push(openai_chunk(
                    &self.state,
                    json!({"tool_calls":[{
                        "index": ti,
                        "id": format!("call_{ti}"),
                        "type": "function",
                        "function": {
                            "name": function_call.get("name").cloned().unwrap_or(Value::String(String::new())),
                            "arguments": serde_json::to_string(&args).unwrap_or_else(|_| "{}".into())
                        }
                    }]}),
                    Value::Null,
                    None,
                ));
            }
        }
        if let Some(reason) = candidate.get("finishReason").filter(|r| !r.is_null()) {
            if self.state.gemini_finished {
                return out;
            }
            self.state.gemini_finished = true;
            let finish = if self.state.next_tool > 0 {
                json!("tool_calls")
            } else {
                gemini_finish(Some(reason))
            };
            let usage = json!({
                "input_tokens": v.pointer("/usageMetadata/promptTokenCount").cloned().unwrap_or(json!(0)),
                "output_tokens": v.pointer("/usageMetadata/candidatesTokenCount").cloned().unwrap_or(json!(0)),
            });
            out.push(openai_chunk(&self.state, json!({}), finish, Some(&usage)));
            if emit_done {
                out.push(Bytes::from_static(b"data: [DONE]\n\n"));
            }
        }
        out
    }

    /// re-parse an openai chunk frame we just produced and re-emit it as
    /// anthropic sse — used to chain gemini→openai→anthropic for native clients.
    fn openai_frame_to_anthropic(&mut self, frame: &Bytes) -> Vec<Bytes> {
        let text = String::from_utf8_lossy(frame);
        let data = text
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n");
        self.openai_to_anthropic(&data)
    }
}

/// Incremental SSE response translator. It tolerates arbitrary HTTP chunk
/// boundaries and emits complete translated events as soon as they arrive.
pub struct TranslatedStream {
    inner: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    converter: SseConverter,
    ready: VecDeque<Bytes>,
    done: bool,
}

impl TranslatedStream {
    pub fn new(
        inner: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
        plan: TranslationPlan,
    ) -> Self {
        Self {
            inner,
            converter: SseConverter::new(plan),
            ready: VecDeque::new(),
            done: false,
        }
    }
}

impl Stream for TranslatedStream {
    type Item = reqwest::Result<Bytes>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(item) = self.ready.pop_front() {
                return Poll::Ready(Some(Ok(item)));
            }
            if self.done {
                return Poll::Ready(None);
            }
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let frames = self.converter.feed(&chunk);
                    self.ready.extend(frames);
                }
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(err))),
                Poll::Ready(None) => {
                    self.done = true;
                    let frames = self.converter.finish();
                    self.ready.extend(frames);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn find_frame(buf: &[u8]) -> Option<usize> {
    buf.windows(2)
        .position(|w| w == b"\n\n")
        .or_else(|| buf.windows(4).position(|w| w == b"\r\n\r\n"))
}

fn drain_separator(buf: &mut Vec<u8>) {
    if buf.starts_with(b"\r\n\r\n") {
        buf.drain(..4);
    } else if buf.starts_with(b"\n\n") {
        buf.drain(..2);
    }
}

fn sse(event: Option<&str>, value: &Value) -> Bytes {
    let data = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    Bytes::from(match event {
        Some(event) => format!("event: {event}\ndata: {data}\n\n"),
        None => format!("data: {data}\n\n"),
    })
}

fn openai_chunk(
    state: &StreamState,
    delta: Value,
    finish_reason: Value,
    usage: Option<&Value>,
) -> Bytes {
    let mut value = json!({"id":state.id,"object":"chat.completion.chunk","created":0,"model":state.model,"choices":[{"index":0,"delta":delta,"finish_reason":finish_reason}]});
    if let Some(u) = usage {
        let prompt = u
            .get("input_tokens")
            .or_else(|| u.get("prompt_tokens"))
            .cloned()
            .unwrap_or(json!(0));
        let completion = u
            .get("output_tokens")
            .or_else(|| u.get("completion_tokens"))
            .cloned()
            .unwrap_or(json!(0));
        let total = prompt.as_u64().unwrap_or(0) + completion.as_u64().unwrap_or(0);
        value["usage"] =
            json!({"prompt_tokens":prompt,"completion_tokens":completion,"total_tokens":total});
    }
    sse(None, &value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(client: Protocol, upstream: Protocol) -> TranslationPlan {
        TranslationPlan {
            client,
            upstream,
            role_profile: RoleProfile::Openai,
        }
    }

    #[test]
    fn openai_multimodal_and_tools_become_anthropic_blocks() {
        let body = Bytes::from(serde_json::to_vec(&json!({"model":"claude","messages":[
            {"role":"system","content":"be concise"},
            {"role":"user","content":[{"type":"text","text":"look"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}},{"type":"input_file","input_file":{"filename":"a.pdf","file_data":"data:application/pdf;base64,BB=="}}]},
            {"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"q\":1}"}}]},
            {"role":"tool","tool_call_id":"call_1","content":"ok"}
        ],"tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}]})).unwrap());
        let out = plan(Protocol::OpenAiChat, Protocol::AnthropicMessages)
            .translate_request(body)
            .unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["system"][0]["text"], "be concise");
        assert_eq!(v["messages"][0]["content"][1]["source"]["type"], "base64");
        assert_eq!(v["messages"][0]["content"][2]["type"], "document");
        assert_eq!(v["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(v["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(v["tools"][0]["input_schema"]["type"], "object");
    }

    #[test]
    fn system_only_profile_lowers_developer_without_reordering() {
        let body = Bytes::from_static(br#"{"messages":[{"role":"developer","content":"first"},{"role":"system","content":"second"},{"role":"user","content":"hello"}]}"#);
        let out = TranslationPlan {
            client: Protocol::OpenAiChat,
            upstream: Protocol::OpenAiChat,
            role_profile: RoleProfile::SystemOnly,
        }
        .translate_request(body)
        .unwrap();
        let value: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["messages"][0]["role"], "system");
        assert_eq!(value["messages"][0]["content"], "first");
        assert_eq!(value["messages"][1]["content"], "second");
    }

    #[test]
    fn anthropic_profile_preserves_leading_instruction_block_order() {
        let body = Bytes::from_static(br#"{"messages":[{"role":"developer","content":"first"},{"role":"system","content":"second"},{"role":"user","content":"hello"}]}"#);
        let out = TranslationPlan {
            client: Protocol::OpenAiChat,
            upstream: Protocol::AnthropicMessages,
            role_profile: RoleProfile::Anthropic,
        }
        .translate_request(body)
        .unwrap();
        let value: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["system"][0]["text"], "first");
        assert_eq!(value["system"][1]["text"], "second");
    }

    #[test]
    fn system_only_profile_rejects_mid_conversation_instruction() {
        let body = Bytes::from_static(br#"{"messages":[{"role":"user","content":"hello"},{"role":"developer","content":"override"}]}"#);
        let err = TranslationPlan {
            client: Protocol::OpenAiChat,
            upstream: Protocol::OpenAiChat,
            role_profile: RoleProfile::SystemOnly,
        }
        .translate_request(body)
        .unwrap_err();
        assert!(err.to_string().contains("role_capability"));
    }

    #[test]
    fn anthropic_response_becomes_openai_response() {
        let body = Bytes::from_static(br#"{"id":"msg_1","model":"claude","content":[{"type":"text","text":"hi"},{"type":"tool_use","id":"t1","name":"ping","input":{"x":1}}],"stop_reason":"tool_use","usage":{"input_tokens":3,"output_tokens":4}}"#);
        let out =
            plan(Protocol::OpenAiChat, Protocol::AnthropicMessages).translate_response(body, false);
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "hi");
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "ping"
        );
        assert_eq!(v["usage"]["total_tokens"], 7);
    }

    #[test]
    fn responses_request_becomes_chat_with_tools_and_multimodal_input() {
        let body = Bytes::from(serde_json::to_vec(&json!({
            "model":"route",
            "instructions":"be concise",
            "input":[{"role":"user","content":[{"type":"input_text","text":"look"},{"type":"input_image","image_url":"https://example.com/a.png"}]}],
            "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}],
            "max_output_tokens":32,
            "reasoning":{"effort":"high"}
        })).unwrap());
        let out = plan(Protocol::OpenAiResponses, Protocol::OpenAiChat)
            .translate_request(body)
            .unwrap();
        let value: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["messages"][0]["role"], "system");
        assert_eq!(value["messages"][1]["content"][0]["type"], "text");
        assert_eq!(value["messages"][1]["content"][1]["type"], "image_url");
        assert_eq!(value["tools"][0]["function"]["name"], "lookup");
        assert_eq!(value["max_completion_tokens"], 32);
        assert!(value.get("reasoning").is_none());
    }

    #[test]
    fn chat_response_becomes_responses_object() {
        let body = Bytes::from_static(br#"{"id":"chat_1","model":"gpt","choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}}"#);
        let out =
            plan(Protocol::OpenAiResponses, Protocol::OpenAiChat).translate_response(body, false);
        let value: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["object"], "response");
        assert_eq!(value["output"][0]["content"][0]["text"], "hi");
        assert_eq!(value["usage"]["input_tokens"], 3);
    }

    #[test]
    fn chat_sse_becomes_responses_sse() {
        let plan = plan(Protocol::OpenAiResponses, Protocol::OpenAiChat);
        let input = Bytes::from_static(b"data: {\"id\":\"chat_1\",\"model\":\"gpt\",\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\ndata: [DONE]\n\n");
        let text = String::from_utf8(plan.translate_response(input, true).to_vec()).unwrap();
        assert!(text.contains("event: response.created"));
        assert!(text.contains("event: response.output_text.delta"));
        assert!(text.contains("event: response.completed"));
    }

    #[test]
    fn chat_tool_sse_becomes_responses_function_call_events() {
        let plan = plan(Protocol::OpenAiResponses, Protocol::OpenAiChat);
        let input = Bytes::from_static(b"data: {\"id\":\"chat_1\",\"model\":\"gpt\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":1}\"}}]},\"finish_reason\":null}]}\n\ndata: [DONE]\n\n");
        let text = String::from_utf8(plan.translate_response(input, true).to_vec()).unwrap();
        assert!(text.contains("event: response.function_call_arguments.delta"));
        assert!(text.contains("event: response.function_call_arguments.done"));
    }

    #[test]
    fn anthropic_sse_translates_across_chunk_boundaries() {
        let p = plan(Protocol::OpenAiChat, Protocol::AnthropicMessages);
        let input = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude\",\"usage\":{\"input_tokens\":2}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        );
        let mut c = SseConverter::new(p);
        let split = input.len() / 2;
        let mut out = c.feed(&input.as_bytes()[..split]);
        out.extend(c.feed(&input.as_bytes()[split..]));
        out.extend(c.finish());
        let text = String::from_utf8(out.concat()).unwrap();
        assert!(text.contains("chat.completion.chunk"));
        assert!(text.contains("\"content\":\"hi\""));
        assert!(text.ends_with("data: [DONE]\n\n"));
    }

    #[test]
    fn portable_prompt_cache_marks_anthropic_breakpoints() {
        let body = Bytes::from_static(
            br#"{"system":"rules","messages":[{"role":"user","content":"hello"}],"tools":[{"name":"lookup"}],"cache_control":{"enabled":true,"ttl":"5m","breakpoints":["system","tools","messages"]}}"#,
        );
        let value: Value = serde_json::from_slice(
            &normalize_prompt_cache_control(body, ProviderKind::Anthropic).unwrap(),
        )
        .unwrap();
        assert_eq!(value["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(value["tools"][0]["cache_control"]["ttl"], "5m");
        assert_eq!(
            value["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn portable_prompt_cache_rejects_openai_compatible_providers() {
        let body = Bytes::from_static(br#"{"cache_control":{"enabled":true}}"#);
        let err = normalize_prompt_cache_control(body, ProviderKind::Bedrock).unwrap_err();
        assert!(err.to_string().contains("prompt_cache_unsupported"));
    }

    #[test]
    fn gemini_native_resolves_for_openai_client() {
        let plan = TranslationPlan::resolve(
            "/v1/chat/completions",
            ProviderKind::GeminiNative,
            RoleProfile::Openai,
        );
        assert_eq!(plan.upstream, Protocol::GeminiGenerate);
        assert!(plan.is_gemini_generate());
    }

    #[test]
    fn openai_request_becomes_gemini_generate_content() {
        let body = Bytes::from(serde_json::to_vec(&json!({"model":"gemini-2.5-flash","messages":[
            {"role":"system","content":"be terse"},
            {"role":"user","content":"hi"},
            {"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"q\":1}"}}]},
            {"role":"tool","tool_call_id":"call_1","content":"ok"}
        ],"temperature":0.5,"max_tokens":128,"tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}],"tool_choice":"required"})).unwrap());
        let out = plan(Protocol::OpenAiChat, Protocol::GeminiGenerate)
            .translate_request(body)
            .unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["systemInstruction"]["parts"][0]["text"], "be terse");
        assert_eq!(v["contents"][0]["role"], "user");
        assert_eq!(v["contents"][0]["parts"][0]["text"], "hi");
        assert_eq!(v["contents"][1]["role"], "model");
        assert_eq!(
            v["contents"][1]["parts"][0]["functionCall"]["name"],
            "lookup"
        );
        assert_eq!(
            v["contents"][2]["parts"][0]["functionResponse"]["name"],
            "call_1"
        );
        assert_eq!(v["generationConfig"]["temperature"], 0.5);
        assert_eq!(v["generationConfig"]["maxOutputTokens"], 128);
        assert_eq!(v["tools"][0]["functionDeclarations"][0]["name"], "lookup");
        assert_eq!(v["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
        // gemini native carries no top-level model/stream fields
        assert!(v.get("model").is_none());
    }

    #[test]
    fn gemini_response_becomes_openai_completion() {
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "candidates":[{"content":{"parts":[{"text":"hello world"}]},"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2,"totalTokenCount":7},
                "modelVersion":"gemini-2.5-flash"
            }))
            .unwrap(),
        );
        let out =
            plan(Protocol::OpenAiChat, Protocol::GeminiGenerate).translate_response(body, false);
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "hello world");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["usage"]["prompt_tokens"], 5);
        assert_eq!(v["usage"]["completion_tokens"], 2);
        assert_eq!(v["model"], "gemini-2.5-flash");
    }

    #[test]
    fn gemini_function_call_response_maps_to_tool_calls() {
        let body = Bytes::from(serde_json::to_vec(&json!({
            "candidates":[{"content":{"parts":[{"functionCall":{"name":"lookup","args":{"q":1}}}]},"finishReason":"STOP"}]
        })).unwrap());
        let out =
            plan(Protocol::OpenAiChat, Protocol::GeminiGenerate).translate_response(body, false);
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "lookup"
        );
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":1}"
        );
    }

    #[test]
    fn gemini_stream_becomes_openai_chunks() {
        let mut converter = SseConverter::new(plan(Protocol::OpenAiChat, Protocol::GeminiGenerate));
        let mut out = Vec::new();
        out.extend(converter.feed(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hello \"}]}}],\"modelVersion\":\"gemini-2.5-flash\"}\n\n",
        ));
        out.extend(converter.feed(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"world\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":1}}\n\n",
        ));
        out.extend(converter.finish());
        let text = out
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect::<String>();
        assert!(text.contains("\"role\":\"assistant\""));
        assert!(text.contains("\"content\":\"hello \""));
        assert!(text.contains("\"content\":\"world\""));
        assert!(text.contains("\"finish_reason\":\"stop\""));
        assert!(text.contains("data: [DONE]"));
    }

    #[test]
    fn gemini_stream_to_anthropic_emits_message_stop() {
        let mut converter = SseConverter::new(plan(Protocol::OpenAiChat, Protocol::GeminiGenerate));
        // client protocol drives the stream selection; use anthropic client
        let mut converter = {
            converter.plan.client = Protocol::AnthropicMessages;
            converter
        };
        let mut out = Vec::new();
        out.extend(converter.feed(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]},\"finishReason\":\"STOP\"}],\"modelVersion\":\"gemini-2.5-flash\"}\n\n",
        ));
        out.extend(converter.finish());
        let text = out
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect::<String>();
        assert!(text.contains("message_start"));
        assert!(text.contains("content_block_delta"));
        assert!(text.contains("message_stop"));
    }
}
