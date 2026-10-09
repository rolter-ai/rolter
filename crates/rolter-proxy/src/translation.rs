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
use rolter_core::{
    upstream, CompatibilityConfig, Error, ModelDefaultsConfig, ProviderKind, Result, RoleProfile,
};
use serde_json::{json, Map, Value};

/// Public wire dialect presented by a client or accepted by an upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    OpenAiChat,
    OpenAiResponses,
    AnthropicMessages,
    /// google gemini native `generateContent` / `streamGenerateContent`
    GeminiGenerate,
    /// google gemini Interactions (`POST {api_base}/interactions`), the
    /// stateful agentic surface whose thread is carried by
    /// `previous_interaction_id`
    GeminiInteractions,
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
    InteractionsToOpenAi,
    InteractionsToAnthropic,
    InteractionsToResponses,
}

struct TranslationPair {
    source: Protocol,
    target: Protocol,
    /// Fallible because a dialect with no equivalent for a content part must
    /// fail closed rather than drop it: a translator that silently shortens the
    /// body sends the provider a request the caller never made, and both ends
    /// return a normal 200 (#882, ADR-0014)
    request: fn(Value) -> Result<Value>,
    response: fn(Value) -> Value,
    stream: StreamTranslation,
}

static TRANSLATION_PAIRS: &[TranslationPair] = &[
    TranslationPair {
        source: Protocol::OpenAiChat,
        target: Protocol::AnthropicMessages,
        request: |v| Ok(openai_request(v)),
        response: anthropic_response,
        stream: StreamTranslation::AnthropicToOpenAi,
    },
    TranslationPair {
        source: Protocol::OpenAiResponses,
        target: Protocol::OpenAiChat,
        request: |v| Ok(responses_request(v)),
        response: responses_from_openai,
        stream: StreamTranslation::OpenAiToResponses,
    },
    TranslationPair {
        source: Protocol::OpenAiResponses,
        target: Protocol::AnthropicMessages,
        request: |v| Ok(responses_to_anthropic_request(v)),
        response: responses_from_anthropic,
        stream: StreamTranslation::AnthropicToResponses,
    },
    TranslationPair {
        source: Protocol::AnthropicMessages,
        target: Protocol::OpenAiChat,
        request: |v| Ok(anthropic_request(v)),
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
    TranslationPair {
        source: Protocol::OpenAiChat,
        target: Protocol::GeminiInteractions,
        request: openai_to_interactions,
        response: interactions_to_openai,
        stream: StreamTranslation::InteractionsToOpenAi,
    },
    TranslationPair {
        source: Protocol::AnthropicMessages,
        target: Protocol::GeminiInteractions,
        request: anthropic_to_interactions,
        response: interactions_to_anthropic,
        stream: StreamTranslation::InteractionsToAnthropic,
    },
    TranslationPair {
        source: Protocol::OpenAiResponses,
        target: Protocol::GeminiInteractions,
        request: responses_to_interactions,
        response: interactions_to_responses,
        stream: StreamTranslation::InteractionsToResponses,
    },
];

/// The Anthropic Messages API rejects a request without `max_tokens`, so a
/// translated request that carried neither `max_tokens` nor
/// `max_completion_tokens` gets the deployment's configured default (#546).
fn apply_max_tokens_default(value: &mut Value, upstream: Protocol, default_max_tokens: u32) {
    if upstream != Protocol::AnthropicMessages {
        return;
    }
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    if !obj.contains_key("max_tokens") {
        obj.insert("max_tokens".into(), json!(default_max_tokens));
    }
}

/// The upstream dialect's key for a completion-length cap. Gemini nests it
/// inside `generationConfig` and `Passthrough` covers non-chat endpoints
/// (embeddings, audio, images), so neither takes a default here.
fn max_tokens_key(upstream: Protocol) -> Option<&'static str> {
    match upstream {
        Protocol::OpenAiChat | Protocol::AnthropicMessages => Some("max_tokens"),
        Protocol::OpenAiResponses => Some("max_output_tokens"),
        Protocol::GeminiGenerate | Protocol::GeminiInteractions | Protocol::Passthrough => None,
    }
}

/// Fill in the deployment's inference defaults for keys the request left out
/// (#564).
///
/// Runs in the *upstream* dialect, after any translation, so the key names
/// match what the provider actually reads. Every write is gated on the key
/// being absent: an explicit client value always wins, including an explicit
/// `null`, which is a deliberate "use the provider's own default".
pub(crate) fn apply_model_defaults(
    value: &mut Value,
    upstream: Protocol,
    defaults: &ModelDefaultsConfig,
) {
    let Some(max_tokens_key) = max_tokens_key(upstream) else {
        return;
    };
    if !defaults.is_active() {
        return;
    }
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    // an empty `model` is as good as absent — some clients send one rather
    // than omitting the field
    if let Some(model) = &defaults.default_model {
        let missing = match obj.get("model") {
            None => true,
            Some(Value::String(existing)) => existing.trim().is_empty(),
            Some(_) => false,
        };
        if missing {
            obj.insert("model".into(), json!(model));
        }
    }
    if let Some(temperature) = defaults.temperature {
        obj.entry("temperature")
            .or_insert_with(|| json!(temperature));
    }
    if let Some(top_p) = defaults.top_p {
        obj.entry("top_p").or_insert_with(|| json!(top_p));
    }
    if let Some(max_tokens) = defaults.max_tokens {
        // openai chat accepts either spelling; a request that set the newer one
        // has already said what it wants
        let already_capped =
            upstream == Protocol::OpenAiChat && obj.contains_key("max_completion_tokens");
        if !already_capped {
            obj.entry(max_tokens_key)
                .or_insert_with(|| json!(max_tokens));
        }
    }
}

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
            (_, ProviderKind::GeminiInteractions) => Protocol::GeminiInteractions,
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
        // the literals live in rolter-core, which the dashboard previews them
        // from, so what the sheet shows and what is called are one string
        match self.upstream {
            Protocol::OpenAiChat => upstream::CHAT_COMPLETIONS_PATH,
            Protocol::OpenAiResponses => upstream::RESPONSES_PATH,
            Protocol::AnthropicMessages => upstream::ANTHROPIC_MESSAGES_PATH,
            // gemini builds its URL from the model + method in the forwarder;
            // the fixed path is unused for this upstream
            Protocol::GeminiGenerate => original,
            // interactions is a single model-less endpoint; the model travels
            // in the body, so the gateway route path is replaced outright
            Protocol::GeminiInteractions => upstream::GEMINI_INTERACTIONS_PATH,
            Protocol::Passthrough => original,
        }
    }

    /// Whether this plan targets the gemini native generateContent upstream,
    /// whose URL embeds the model and streaming method rather than a fixed path.
    pub fn is_gemini_generate(self) -> bool {
        self.upstream == Protocol::GeminiGenerate
    }

    /// Translate with the deployment's compatibility defaults.
    pub fn translate_request(self, body: Bytes) -> Result<Bytes> {
        self.translate_request_with(body, &CompatibilityConfig::default())
    }

    /// Translate applying `compat`, the control-plane-owned cross-dialect
    /// behavior (#546).
    pub fn translate_request_with(
        self,
        body: Bytes,
        compat: &CompatibilityConfig,
    ) -> Result<Bytes> {
        // the interactions surface only accepts chat-shaped dialects; any other
        // endpoint (embeddings, audio, …) fails closed rather than being
        // forwarded verbatim to an endpoint that cannot serve it
        if self.upstream == Protocol::GeminiInteractions
            && registered_pair(self.client, self.upstream).is_none()
        {
            return Err(Error::Config(
                "gemini_interactions_unsupported: this endpoint has no interactions equivalent; \
                 use /v1/chat/completions, /v1/responses or /v1/messages"
                    .to_string(),
            ));
        }
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
        value = (pair.request)(value)?;
        apply_max_tokens_default(&mut value, self.upstream, compat.default_max_tokens);
        normalize_openai_roles(&mut value, self.upstream, self.role_profile);
        serde_json::to_vec(&value).map(Bytes::from).map_err(|err| {
            Error::Config(format!("role_capability: failed to encode request: {err}"))
        })
    }

    /// Fill in the deployment's inference defaults on an already-translated
    /// body.
    ///
    /// Returns the body untouched — without parsing it — whenever the feature
    /// is off or the upstream dialect takes no defaults, so a deployment that
    /// never enables this pays nothing on the hot path.
    pub fn apply_model_defaults(self, body: Bytes, defaults: &ModelDefaultsConfig) -> Bytes {
        if !defaults.is_active() || max_tokens_key(self.upstream).is_none() {
            return body;
        }
        let Ok(mut value) = serde_json::from_slice::<Value>(&body) else {
            return body;
        };
        apply_model_defaults(&mut value, self.upstream, defaults);
        serde_json::to_vec(&value).map(Bytes::from).unwrap_or(body)
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
        translate_json(body, self.client, self.upstream)
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
    if let Some(values) = control.get("breakpoints").and_then(Value::as_array) {
        for v in values {
            if let Some(breakpoint) = v.as_str() {
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
        }
    } else {
        mark_last_content(&mut value, "system", &marker);
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

    // Determine capacity: optional system prompt + input elements
    let input_len = match obj.get("input") {
        Some(serde_json::Value::Array(items)) => items.len(),
        Some(_) => 1,
        None => 0,
    };
    let capacity = if obj.contains_key("instructions") {
        input_len + 1
    } else {
        input_len
    };
    let mut messages = Vec::with_capacity(capacity);

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
    let usage = TokenUsage::from_chat_body(&v);
    json!({"id":v.get("id").cloned().unwrap_or_else(|| json!("resp_rolter")),"object":"response","status":"completed","model":v.get("model").cloned().unwrap_or(Value::Null),"output":[{"id":"msg_rolter","type":"message","status":"completed","role":"assistant","content":content}],"usage":usage.to_responses()})
}

fn responses_from_anthropic(v: Value) -> Value {
    responses_from_openai(anthropic_response(v))
}

fn translate_json(body: Bytes, from: Protocol, to: Protocol) -> Bytes {
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return body;
    };
    let Some(pair) = registered_pair(from, to) else {
        return body;
    };
    let translated = (pair.response)(value);
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
                "tool_use_id":message.get("tool_call_id").cloned().unwrap_or_else(|| Value::String(String::new())),
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
                    "id":call.get("id").cloned().unwrap_or_else(|| Value::String(String::new())),
                    "name":function.get("name").cloned().unwrap_or_else(|| Value::String(String::new())),
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
        if let Some(max) = obj.remove("max_completion_tokens") {
            obj.insert("max_tokens".into(), max);
        }
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
                    "name":function.get("name").cloned().unwrap_or_else(|| Value::String(String::new())),
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
                .unwrap_or_else(|| Value::String(String::new()));
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
                Some("text") | Some("input_text") => json!({"type":"text","text":part.get("text").cloned().unwrap_or_else(|| Value::String(String::new()))}),
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
            json!({"type":"text","text":block.get("text").cloned().unwrap_or_else(|| Value::String(String::new()))})
        }
        _ => block,
    }
}

/// The prompt-cache hit of an OpenAI-shaped `usage` object, whichever way the
/// provider spells it (#2877). All of them count the cached tokens *inside*
/// the prompt total, which is how `ModelPriceConfig::cost` reads them:
///
/// - `prompt_tokens_details.cached_tokens`: Chat Completions (OpenAI, Azure
///   OpenAI, OpenRouter, Groq, xAI, DeepSeek, Kimi, Qwen, Z.ai and the many
///   providers that copy the shape)
/// - `input_tokens_details.cached_tokens`: the Responses API
/// - `prompt_cache_hit_tokens`: DeepSeek, whose `prompt_tokens` is documented
///   as hits plus `prompt_cache_miss_tokens`. It is the older spelling; the
///   details block above now carries the same number
/// - `cached_tokens` at the top of `usage`: Kimi (Moonshot), beside the details
///   block, and gateways that flatten it
/// - `precached_prompt_tokens`: GigaChat
///
/// The first spelling present wins. Anthropic's `cache_read_input_tokens` is
/// not here on purpose: it sits *beside* `input_tokens`, not inside it, so the
/// callers that read it also add it back into the prompt.
pub fn cached_prompt_tokens(usage: &Value) -> Option<u64> {
    const SPELLINGS: [&str; 5] = [
        "/prompt_tokens_details/cached_tokens",
        "/input_tokens_details/cached_tokens",
        "/prompt_cache_hit_tokens",
        "/cached_tokens",
        "/precached_prompt_tokens",
    ];
    SPELLINGS
        .iter()
        .find_map(|pointer| usage.pointer(pointer)?.as_u64())
}

/// Token counts of one usage object, held in the one shape every dialect
/// converts through.
///
/// The dialects disagree on where the prompt-cache share sits. Chat
/// Completions, the Responses API and Gemini count cached tokens *inside* the
/// prompt total (`prompt_tokens`, `input_tokens`, `promptTokenCount`) and name
/// them in a details block; Anthropic's `input_tokens` leaves cache reads and
/// writes *out* and reports them beside it. `prompt` here is always the whole
/// prompt, and the Anthropic emitter takes the cache share off again, so each
/// side reads the figures in its own convention. That matters beyond what the
/// client sees: the gateway prices a request from the body the client
/// receives, and `ModelPriceConfig::cost` expects the cached share to be part
/// of the prompt it is handed (#2863).
///
/// Gemini reports two counts that belong to neither side of the headline pair,
/// and both are billed, so both are folded in (#2875). Thinking tokens
/// (`thoughtsTokenCount`, `total_thought_tokens`) are charged at the output
/// rate, and sit beside `candidatesTokenCount` rather than inside it, so they
/// are added to `completion`. Tool-use prompt tokens (`toolUsePromptTokenCount`,
/// `total_tool_use_tokens`) are what a built-in tool such as URL context fed
/// back to the model, charged as input tokens, so they are added to `prompt`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct TokenUsage {
    /// every prompt token, cache reads and writes included
    prompt: u64,
    completion: u64,
    /// prompt tokens served from the provider's cache. `None` when the source
    /// said nothing about caching, so a translated body does not state a zero
    /// the provider never reported
    cache_read: Option<u64>,
    /// prompt tokens written to the provider's cache
    cache_write: Option<u64>,
    /// the provider's own `total_tokens` when a chat or responses body states
    /// one; the sum of prompt and completion otherwise. Gemini and Interactions
    /// never set it: their totals cover thinking and tool-use tokens, which are
    /// counted into `prompt` and `completion` here, so the sum is the total
    total: Option<u64>,
}

impl TokenUsage {
    /// An Anthropic usage object. `input_tokens` excludes the cache reads and
    /// writes, so they are added back into the prompt.
    fn from_anthropic(usage: &Value) -> Self {
        let count = |key: &str| usage.get(key).and_then(Value::as_u64);
        let cache_read = count("cache_read_input_tokens");
        let cache_write = count("cache_creation_input_tokens");
        Self {
            prompt: count("input_tokens")
                .unwrap_or(0)
                .saturating_add(cache_read.unwrap_or(0))
                .saturating_add(cache_write.unwrap_or(0)),
            completion: count("output_tokens").unwrap_or(0),
            cache_read,
            cache_write,
            total: None,
        }
    }

    /// A Chat Completions usage object, or a Responses API one: the counts are
    /// spelled `prompt_tokens` / `input_tokens` and the cache share sits in
    /// `prompt_tokens_details` / `input_tokens_details`, inside the prompt.
    /// Providers that cache without following that shape spell the share
    /// elsewhere (see [`cached_prompt_tokens`]), which is read too.
    /// `cache_write_tokens` is not part of either API; gateways that report a
    /// write count (OpenRouter) put it beside `cached_tokens`.
    fn from_openai(usage: &Value) -> Self {
        let count = |key: &str, alternative: &str| {
            usage
                .get(key)
                .or_else(|| usage.get(alternative))
                .and_then(Value::as_u64)
        };
        let detail = |field: &str| {
            ["prompt_tokens_details", "input_tokens_details"]
                .iter()
                .find_map(|block| usage.get(*block)?.get(field)?.as_u64())
        };
        Self {
            prompt: count("prompt_tokens", "input_tokens").unwrap_or(0),
            completion: count("completion_tokens", "output_tokens").unwrap_or(0),
            cache_read: cached_prompt_tokens(usage),
            cache_write: detail("cache_write_tokens"),
            total: usage.get("total_tokens").and_then(Value::as_u64),
        }
    }

    /// The usage of a whole Chat Completions body or chunk, zeros when it
    /// carries none.
    fn from_chat_body(body: &Value) -> Self {
        body.get("usage")
            .filter(|usage| usage.is_object())
            .map(Self::from_openai)
            .unwrap_or_default()
    }

    /// A Gemini `usageMetadata` block. `promptTokenCount` already includes the
    /// cached content, which `cachedContentTokenCount` counts.
    /// `candidatesTokenCount` leaves the thinking tokens out and
    /// `thoughtsTokenCount` counts them; they are billed as output, so they
    /// join the completion (#2875), and the tool-use prompt tokens join the
    /// prompt. The provider's `totalTokenCount` is not carried: it is the sum
    /// of these anyway, and a total derived from the figures shown cannot
    /// disagree with them.
    fn from_gemini(metadata: Option<&Value>) -> Self {
        let count = |key: &str| metadata?.get(key)?.as_u64();
        Self {
            prompt: count("promptTokenCount")
                .unwrap_or(0)
                .saturating_add(count("toolUsePromptTokenCount").unwrap_or(0)),
            completion: count("candidatesTokenCount")
                .unwrap_or(0)
                .saturating_add(count("thoughtsTokenCount").unwrap_or(0)),
            cache_read: count("cachedContentTokenCount"),
            cache_write: None,
            total: None,
        }
    }

    /// Fold in a later report of the same stream. Usage in a stream is
    /// cumulative, so each figure keeps its largest value, and a report that
    /// omits a figure (anthropic's `message_delta` often carries the output
    /// count alone) leaves it as it was.
    fn absorb(&mut self, later: Self) {
        let larger = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.prompt = self.prompt.max(later.prompt);
        self.completion = self.completion.max(later.completion);
        self.cache_read = larger(self.cache_read, later.cache_read);
        self.cache_write = larger(self.cache_write, later.cache_write);
        self.total = larger(self.total, later.total);
    }

    fn total(&self) -> u64 {
        self.total
            .unwrap_or_else(|| self.prompt.saturating_add(self.completion))
    }

    /// `{cached_tokens, cache_write_tokens}`, the block both OpenAI shapes
    /// nest under their own name. `None` when no cache figure was reported.
    fn cache_details(&self) -> Option<Value> {
        if self.cache_read.is_none() && self.cache_write.is_none() {
            return None;
        }
        let mut details = Map::new();
        details.insert("cached_tokens".into(), json!(self.cache_read.unwrap_or(0)));
        if let Some(written) = self.cache_write {
            details.insert("cache_write_tokens".into(), json!(written));
        }
        Some(Value::Object(details))
    }

    /// The usage object of a Chat Completions body or chunk.
    fn to_chat(self) -> Value {
        let mut usage = json!({
            "prompt_tokens": self.prompt,
            "completion_tokens": self.completion,
            "total_tokens": self.total(),
        });
        if let Some(details) = self.cache_details() {
            usage["prompt_tokens_details"] = details;
        }
        usage
    }

    /// The usage object of a Responses API body or `response.completed`.
    fn to_responses(self) -> Value {
        let mut usage = json!({
            "input_tokens": self.prompt,
            "output_tokens": self.completion,
            "total_tokens": self.total(),
        });
        if let Some(details) = self.cache_details() {
            usage["input_tokens_details"] = details;
        }
        usage
    }

    /// The usage object of an Anthropic message: `input_tokens` is what is
    /// left of the prompt once the cache reads and writes are taken off.
    fn to_anthropic(self) -> Value {
        let cached = self
            .cache_read
            .unwrap_or(0)
            .saturating_add(self.cache_write.unwrap_or(0));
        let mut usage = json!({
            "input_tokens": self.prompt.saturating_sub(cached),
            "output_tokens": self.completion,
        });
        if let Some(read) = self.cache_read {
            usage["cache_read_input_tokens"] = json!(read);
        }
        if let Some(written) = self.cache_write {
            usage["cache_creation_input_tokens"] = json!(written);
        }
        usage
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
    let usage = v
        .get("usage")
        .map(TokenUsage::from_anthropic)
        .unwrap_or_default();
    json!({
        "id":v.get("id").cloned().unwrap_or_else(|| json!("chatcmpl-rolter")),
        "object":"chat.completion",
        "created":0,
        "model":v.get("model").cloned().unwrap_or(Value::Null),
        "choices":[{"index":0,"message":message,"finish_reason":anthropic_finish(v.get("stop_reason"))}],
        "usage":usage.to_chat()
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
        "usage":TokenUsage::from_chat_body(&v).to_anthropic()
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
            .collect::<String>(),
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
fn openai_to_gemini(mut v: Value) -> Result<Value> {
    let Some(obj) = v.as_object_mut() else {
        return Ok(v);
    };
    let messages = obj
        .remove("messages")
        .and_then(|v| match v {
            Value::Array(a) => Some(a),
            _ => None,
        })
        .unwrap_or_default();
    let mut system_parts = Vec::new();
    let mut contents = Vec::with_capacity(messages.len());
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        match role {
            "system" | "developer" => {
                system_parts.extend(gemini_parts_from_content(message.get("content"))?);
            }
            "tool" => {
                contents.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": message.get("tool_call_id").cloned().unwrap_or_else(|| Value::String("tool".into())),
                            "response": {"result": content_text(message.get("content"))}
                        }
                    }]
                }));
            }
            _ => {
                let mut parts = gemini_parts_from_content(message.get("content"))?;
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
                                "name": function.get("name").cloned().unwrap_or_else(|| Value::String(String::new())),
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
    if let Some(tools) = obj.remove("tools").and_then(|t| match t {
        Value::Array(a) => Some(a),
        _ => None,
    }) {
        let declarations: Vec<Value> = tools
            .iter()
            .filter_map(|tool| tool.get("function"))
            .map(|function| {
                let mut decl = json!({
                    "name": function.get("name").cloned().unwrap_or_else(|| Value::String(String::new()))
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

    Ok(Value::Object(out))
}

fn anthropic_to_gemini(v: Value) -> Result<Value> {
    openai_to_gemini(anthropic_request(v))
}

fn responses_to_gemini(v: Value) -> Result<Value> {
    openai_to_gemini(responses_request(v))
}

/// Build Gemini `parts` from an OpenAI message `content` value (string, or an
/// array of typed parts). Data URLs become `inlineData`; other URLs become
/// `fileData`.
///
/// Fails closed on an unrecognized part type for the same reason as
/// [`interaction_parts_from_content`] (#882).
fn gemini_parts_from_content(content: Option<&Value>) -> Result<Vec<Value>> {
    match content {
        Some(Value::String(text)) => Ok(vec![json!({"text": text})]),
        Some(Value::Array(parts)) => parts
            .iter()
            .map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") | Some("input_text") => Ok(json!({
                    "text": part.get("text").cloned().unwrap_or_else(|| Value::String(String::new()))
                })),
                Some("image_url") | Some("input_image") => {
                    let url = part
                        .pointer("/image_url/url")
                        .or_else(|| part.get("image_url"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Ok(gemini_media_part(url))
                }
                other => Err(unsupported_content_part("gemini generateContent", other)),
            })
            .collect(),
        Some(Value::Null) | None => Ok(Vec::new()),
        Some(other) => Ok(vec![json!({"text": other.to_string()})]),
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
                    "name": function_call.get("name").cloned().unwrap_or_else(|| Value::String(String::new())),
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
    let usage = TokenUsage::from_gemini(v.get("usageMetadata"));
    json!({
        "id": "chatcmpl-rolter",
        "object": "chat.completion",
        "created": 0,
        "model": v.get("modelVersion").cloned().unwrap_or(Value::Null),
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": usage.to_chat()
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

/// Translate an OpenAI Chat Completions request into a Gemini Interactions
/// `interactions.create` body (#599).
///
/// Conversation turns become `input` items (`role` `user`/`model` with typed
/// content parts); system/developer messages are hoisted into
/// `system_instruction`; assistant tool calls become `function_call` items and
/// `tool` messages become `function_result` items. Sampling parameters move to
/// `generation_config` and OpenAI function tools become flat `tools` entries.
///
/// Statefulness is client-driven: `previous_response_id` (Responses) or an
/// explicit `previous_interaction_id` is forwarded as
/// `previous_interaction_id`, and the interaction id is echoed back as the
/// response `id`, so a thread is resumed without rolter keeping its own store.
fn openai_to_interactions(mut v: Value) -> Result<Value> {
    let Some(obj) = v.as_object_mut() else {
        return Ok(v);
    };
    let messages = obj
        .remove("messages")
        .and_then(|v| match v {
            Value::Array(a) => Some(a),
            _ => None,
        })
        .unwrap_or_default();
    // `system_text` alone cannot stand in for the old `Vec<String>`: a system
    // turn carrying empty content is still a turn, and `join("\n")` emitted a
    // separator for it and produced a (possibly empty) string. count the turns
    // so emptiness of the text is not mistaken for absence of the turns
    let mut system_text = String::new();
    let mut system_turns = 0usize;
    let mut input = Vec::with_capacity(messages.len());
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        match role {
            "system" | "developer" => {
                if system_turns > 0 {
                    system_text.push('\n');
                }
                system_turns += 1;
                system_text.push_str(&content_text(message.get("content")));
            }
            "tool" => {
                let mut item = json!({
                    "type": "function_result",
                    "call_id": message.get("tool_call_id").cloned().unwrap_or_else(|| Value::String("tool".into())),
                    "result": [{"type": "text", "text": content_text(message.get("content"))}]
                });
                if let Some(name) = message.get("name").filter(|v| !v.is_null()) {
                    item["name"] = name.clone();
                }
                input.push(item);
            }
            "assistant" => {
                let content = interaction_parts_from_content(message.get("content"))?;
                if !content.is_empty() {
                    input.push(json!({"role": "model", "content": content}));
                }
                for call in message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let function = call.get("function").unwrap_or(&Value::Null);
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call.get("id").cloned().unwrap_or_else(|| Value::String(String::new())),
                        "name": function.get("name").cloned().unwrap_or_else(|| Value::String(String::new())),
                        "arguments": interaction_arguments(function.get("arguments"))
                    }));
                }
            }
            _ => input.push(json!({
                "role": "user",
                "content": interaction_parts_from_content(message.get("content"))?
            })),
        }
    }

    let mut out = Map::new();
    if let Some(model) = obj.remove("model") {
        out.insert("model".into(), model);
    }
    out.insert("input".into(), Value::Array(input));
    if system_turns > 0 {
        out.insert("system_instruction".into(), json!(system_text));
    }
    if obj.remove("stream").and_then(|v| v.as_bool()) == Some(true) {
        out.insert("stream".into(), json!(true));
    }
    if let Some(previous) = obj
        .remove("previous_interaction_id")
        .or_else(|| obj.remove("previous_response_id"))
        .filter(|v| !v.is_null())
    {
        out.insert("previous_interaction_id".into(), previous);
    }
    if let Some(store) = obj.remove("store").filter(|v| !v.is_null()) {
        out.insert("store".into(), store);
    }

    let mut generation = Map::new();
    if let Some(temperature) = obj.remove("temperature") {
        generation.insert("temperature".into(), temperature);
    }
    if let Some(top_p) = obj.remove("top_p") {
        generation.insert("top_p".into(), top_p);
    }
    if let Some(max) = obj
        .remove("max_completion_tokens")
        .or_else(|| obj.remove("max_tokens"))
    {
        generation.insert("max_output_tokens".into(), max);
    }
    if let Some(stop) = obj.remove("stop") {
        generation.insert(
            "stop_sequences".into(),
            match stop {
                Value::String(s) => json!([s]),
                other => other,
            },
        );
    }
    if let Some(choice) = obj.remove("tool_choice") {
        let mode = match &choice {
            Value::String(s) if s == "required" => Some("any"),
            Value::String(s) if s == "none" => Some("none"),
            Value::String(s) if s == "auto" => Some("auto"),
            Value::Object(_) => Some("any"),
            _ => None,
        };
        if let Some(mode) = mode {
            generation.insert("tool_choice".into(), json!(mode));
        }
    }
    if !generation.is_empty() {
        out.insert("generation_config".into(), Value::Object(generation));
    }

    if let Some(tools) = obj.remove("tools").and_then(|t| match t {
        Value::Array(a) => Some(a),
        _ => None,
    }) {
        let declared: Vec<Value> = tools
            .iter()
            .map(|tool| tool.get("function").unwrap_or(tool))
            .map(|function| {
                let mut declaration = json!({
                    "type": "function",
                    "name": function.get("name").cloned().unwrap_or_else(|| Value::String(String::new()))
                });
                if let Some(description) = function.get("description").filter(|v| !v.is_null()) {
                    declaration["description"] = description.clone();
                }
                if let Some(parameters) = function.get("parameters").filter(|v| !v.is_null()) {
                    declaration["parameters"] = parameters.clone();
                }
                declaration
            })
            .collect();
        if !declared.is_empty() {
            out.insert("tools".into(), Value::Array(declared));
        }
    }

    Ok(Value::Object(out))
}

fn anthropic_to_interactions(v: Value) -> Result<Value> {
    openai_to_interactions(anthropic_request(v))
}

/// Lower a Responses request onto interactions. `responses_request` drops the
/// stateful fields on its way to Chat, so they are captured first and restored
/// as their interactions equivalents.
fn responses_to_interactions(v: Value) -> Result<Value> {
    let previous = v
        .get("previous_response_id")
        .cloned()
        .filter(|v| !v.is_null());
    let store = v.get("store").cloned().filter(|v| !v.is_null());
    let mut out = openai_to_interactions(responses_request(v))?;
    if let Some(obj) = out.as_object_mut() {
        if let Some(previous) = previous {
            obj.insert("previous_interaction_id".into(), previous);
        }
        if let Some(store) = store {
            obj.insert("store".into(), store);
        }
    }
    Ok(out)
}

/// Build interactions `content` parts from an OpenAI message `content` value.
/// Data URLs become inline image bytes; other URLs are referenced by uri.
///
/// A part type interactions has no equivalent for is an error, not a no-op:
/// dropping it would forward a request missing content the caller sent and
/// still return 200 from both rolter and the provider (#882).
fn interaction_parts_from_content(content: Option<&Value>) -> Result<Vec<Value>> {
    match content {
        Some(Value::String(text)) => Ok(vec![json!({"type": "text", "text": text})]),
        Some(Value::Array(parts)) => parts
            .iter()
            .map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") | Some("input_text") => Ok(json!({
                    "type": "text",
                    "text": part.get("text").cloned().unwrap_or_else(|| Value::String(String::new()))
                })),
                Some("image_url") | Some("input_image") => {
                    let url = part
                        .pointer("/image_url/url")
                        .or_else(|| part.get("image_url"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Ok(interaction_image_part(url))
                }
                other => Err(unsupported_content_part("interactions", other)),
            })
            .collect(),
        Some(Value::Null) | None => Ok(Vec::new()),
        Some(other) => Ok(vec![json!({"type": "text", "text": other.to_string()})]),
    }
}

/// The fail-closed error for a content part a target dialect cannot carry.
///
/// Named `unsupported_content_part` so the gateway can map it to a 400 the same
/// way it maps `role_capability`; the part type is quoted verbatim so the
/// caller can see exactly which part it has to remove.
fn unsupported_content_part(dialect: &str, part_type: Option<&str>) -> Error {
    let named = match part_type {
        Some(part_type) => format!("content part type '{part_type}'"),
        None => "a content part with no 'type' field".to_string(),
    };
    Error::Config(format!(
        "unsupported_content_part: the {dialect} upstream has no equivalent for {named}; \
         remove it or route this model to a provider that accepts it"
    ))
}

fn interaction_image_part(url: &str) -> Value {
    if let Some((media_type, data)) = data_url(url) {
        json!({"type": "image", "mime_type": media_type, "data": data})
    } else {
        json!({"type": "image", "file_uri": url})
    }
}

/// Tool-call arguments cross the wire as a JSON string in OpenAI dialects and
/// as a string in interactions too, so a structured value is re-encoded.
fn interaction_arguments(arguments: Option<&Value>) -> Value {
    match arguments {
        Some(Value::String(raw)) => json!(raw),
        Some(other) => json!(serde_json::to_string(other).unwrap_or_else(|_| "{}".into())),
        None => json!("{}"),
    }
}

/// Translate a Gemini Interactions resource into an OpenAI Chat Completion.
/// The interaction id is surfaced as the completion `id` so a client can thread
/// the next call with `previous_response_id` / `previous_interaction_id`.
fn interactions_to_openai(v: Value) -> Value {
    let mut text = String::new();
    let mut calls = Vec::new();
    for step in v
        .get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match step.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                let index = calls.len();
                calls.push(json!({
                    "id": step.get("call_id").or_else(|| step.get("id")).cloned().unwrap_or_else(|| json!(format!("call_{index}"))),
                    "type": "function",
                    "function": {
                        "name": step.get("name").cloned().unwrap_or_else(|| Value::String(String::new())),
                        "arguments": interaction_arguments(step.get("arguments"))
                    }
                }));
            }
            // thoughts are internal reasoning traces, never client output
            Some("thought") | Some("thought_summary") => {}
            _ => text.push_str(&interaction_step_text(step)),
        }
    }
    if text.is_empty() {
        if let Some(output) = v.get("output_text").and_then(Value::as_str) {
            text.push_str(output);
        }
    }
    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() { Value::Null } else { Value::String(text) }
    });
    let finish = if calls.is_empty() {
        interaction_finish(v.get("status"))
    } else {
        message["tool_calls"] = Value::Array(calls);
        json!("tool_calls")
    };
    let usage = interaction_usage(v.get("usage"));
    json!({
        "id": v.get("id").cloned().unwrap_or_else(|| json!("chatcmpl-rolter")),
        "object": "chat.completion",
        "created": 0,
        "model": v.get("model").cloned().unwrap_or(Value::Null),
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": usage.to_chat()
    })
}

fn interactions_to_anthropic(v: Value) -> Value {
    openai_response(interactions_to_openai(v))
}

fn interactions_to_responses(v: Value) -> Value {
    responses_from_openai(interactions_to_openai(v))
}

/// Flatten one interaction step's `content` (or its convenience `text` field)
/// into plain text.
fn interaction_step_text(step: &Value) -> String {
    let mut text = String::new();
    match step.get("content") {
        Some(Value::String(raw)) => text.push_str(raw),
        Some(Value::Array(parts)) => {
            for part in parts {
                if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                    text.push_str(part_text);
                }
            }
        }
        _ => {
            if let Some(raw) = step.get("text").and_then(Value::as_str) {
                text.push_str(raw);
            }
        }
    }
    text
}

/// The interactions usage block as a [`TokenUsage`], tolerating both the
/// `total_input_tokens` and short `input_tokens` spellings. The cached share
/// is `total_cached_tokens`, which sits inside the input total. Thinking tokens
/// (`total_thought_tokens`) are billed as output and are not part of
/// `total_output_tokens`, so they join the completion; tool-use tokens
/// (`total_tool_use_tokens`) are billed as input, so they join the prompt
/// (#2875). Google's own docs spell the same counts `thoughts_tokens` and
/// `tool_use_input_tokens` in places, so those are read as well.
fn interaction_usage(usage: Option<&Value>) -> TokenUsage {
    let field = |names: &[&str]| -> Option<u64> {
        let usage = usage?;
        names
            .iter()
            .find_map(|name| usage.get(*name).and_then(Value::as_u64))
    };
    let prompt = field(&["total_input_tokens", "input_tokens"]).unwrap_or(0);
    let tool_use = field(&[
        "total_tool_use_tokens",
        "tool_use_tokens",
        "tool_use_input_tokens",
    ]);
    let completion = field(&["total_output_tokens", "output_tokens"]).unwrap_or(0);
    let thoughts = field(&["total_thought_tokens", "thought_tokens", "thoughts_tokens"]);
    TokenUsage {
        prompt: prompt.saturating_add(tool_use.unwrap_or(0)),
        completion: completion.saturating_add(thoughts.unwrap_or(0)),
        cache_read: field(&["total_cached_tokens", "cached_tokens"]),
        cache_write: None,
        total: None,
    }
}

/// Read a field from a streaming interactions event, which either nests the
/// resource under `interaction` or carries the fields at the top level.
fn interaction_field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value
        .get("interaction")
        .and_then(|interaction| interaction.get(name))
        .or_else(|| value.get(name))
        .filter(|value| !value.is_null())
}

fn interaction_finish(status: Option<&Value>) -> Value {
    match status.and_then(Value::as_str) {
        Some("requires_action") => json!("tool_calls"),
        Some("incomplete") => json!("length"),
        _ => json!("stop"),
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
    /// usage of an anthropic stream so far: `message_start` carries the input
    /// side and `message_delta` the output side, and the client should see both
    /// on the closing chunk rather than whichever event came last
    usage: TokenUsage,
    gemini_finished: bool,
    interaction_finished: bool,
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
        let data = join_data_lines(&text);
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
            Some(StreamTranslation::InteractionsToOpenAi) => {
                self.interactions_to_openai_stream(event, &data, true)
            }
            Some(StreamTranslation::InteractionsToAnthropic) => {
                let chunks = self.interactions_to_openai_stream(event, &data, true);
                chunks
                    .into_iter()
                    .flat_map(|chunk| self.openai_frame_to_anthropic(&chunk))
                    .collect()
            }
            Some(StreamTranslation::InteractionsToResponses) => {
                let chunks = self.interactions_to_openai_stream(event, &data, true);
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
        let mut chunks = Vec::with_capacity(1);
        match event.or_else(|| v.get("type").and_then(Value::as_str)) {
            Some("message_start") => {
                self.state.id = v.pointer("/message/id").and_then(Value::as_str).unwrap_or("chatcmpl-rolter").to_string();
                self.state.model = v.pointer("/message/model").and_then(Value::as_str).unwrap_or_default().to_string();
                let usage = v.pointer("/message/usage").filter(|u| u.is_object()).map(TokenUsage::from_anthropic);
                if let Some(usage) = usage {
                    self.state.usage = usage;
                }
                chunks.push(openai_chunk(&self.state, json!({"role":"assistant","content":""}), Value::Null, usage.as_ref()));
            }
            Some("content_block_start") => {
                let index = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if v.pointer("/content_block/type") == Some(&json!("tool_use")) {
                    let ti = self.state.next_tool; self.state.next_tool += 1; self.state.tool_indexes.insert(index, ti);
                    chunks.push(openai_chunk(&self.state, json!({"tool_calls":[{"index":ti,"id":v.pointer("/content_block/id").cloned().unwrap_or(Value::Null),"type":"function","function":{"name":v.pointer("/content_block/name").cloned().unwrap_or(Value::Null),"arguments":""}}]}), Value::Null, None));
                }
            }
            Some("content_block_delta") => match v.pointer("/delta/type").and_then(Value::as_str) {
                Some("text_delta") => chunks.push(openai_chunk(&self.state, json!({"content":v.pointer("/delta/text").cloned().unwrap_or_else(|| Value::String(String::new()))}), Value::Null, None)),
                Some("input_json_delta") => {
                    let index = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let ti = self.state.tool_indexes.get(&index).copied().unwrap_or(0);
                    chunks.push(openai_chunk(&self.state, json!({"tool_calls":[{"index":ti,"function":{"arguments":v.pointer("/delta/partial_json").cloned().unwrap_or_else(|| Value::String(String::new()))}}]}), Value::Null, None));
                }
                _ => {}
            },
            Some("message_delta") => {
                let usage = v.get("usage").filter(|u| u.is_object()).map(|u| {
                    self.state.usage.absorb(TokenUsage::from_anthropic(u));
                    self.state.usage
                });
                chunks.push(openai_chunk(&self.state, json!({}), anthropic_finish(v.pointer("/delta/stop_reason")), usage.as_ref()));
            }
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
            // a stream's first chunk rarely carries usage; when it does, the
            // output side is still unspent
            let mut start_usage = TokenUsage::from_chat_body(&v).to_anthropic();
            start_usage["output_tokens"] = json!(0);
            out.push(sse(Some("message_start"), &json!({"type":"message_start","message":{"id":self.state.id,"type":"message","role":"assistant","model":self.state.model,"content":[],"stop_reason":Value::Null,"stop_sequence":Value::Null,"usage":start_usage}})));
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
            out.push(sse(Some("message_delta"), &json!({"type":"message_delta","delta":{"stop_reason":openai_finish(Some(reason)),"stop_sequence":Value::Null},"usage":TokenUsage::from_chat_body(&v).to_anthropic()})));
        } else if v.get("usage").is_some() {
            // OpenAI commonly sends usage in a final choices-less chunk. Keep
            // it visible to Anthropic clients and to the gateway's accounting
            // stream instead of losing it after the finish-reason event.
            out.push(sse(Some("message_delta"), &json!({"type":"message_delta","delta":{},"usage":TokenUsage::from_chat_body(&v).to_anthropic()})));
        }
        out
    }

    fn openai_to_responses_frame(&mut self, frame: &Bytes) -> Vec<Bytes> {
        let text = String::from_utf8_lossy(frame);
        let data = join_data_lines(&text);
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
        if let Some(usage) = v.get("usage").filter(|u| u.is_object()) {
            self.state.response_usage = TokenUsage::from_openai(usage).to_responses();
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
                            "name": function_call.get("name").cloned().unwrap_or_else(|| Value::String(String::new())),
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
            let usage = TokenUsage::from_gemini(v.get("usageMetadata"));
            out.push(openai_chunk(&self.state, json!({}), finish, Some(&usage)));
            if emit_done {
                out.push(Bytes::from_static(b"data: [DONE]\n\n"));
            }
        }
        out
    }

    /// convert one gemini interactions sse payload into openai
    /// chat.completion.chunk frames. the discriminator lives in the payload's
    /// `event_type` (mirrored on the `event:` line by some deployments), and
    /// `interaction.completed` carries the terminal usage block, so the finish
    /// chunk and the `[DONE]` sentinel are synthesized from it.
    fn interactions_to_openai_stream(
        &mut self,
        event: Option<&str>,
        data: &str,
        emit_done: bool,
    ) -> Vec<Bytes> {
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        let event_type = v
            .get("event_type")
            .and_then(Value::as_str)
            .or(event)
            .unwrap_or_default()
            .to_string();
        let mut out = Vec::new();
        if !self.state.started {
            self.state.started = true;
            self.state.id = interaction_field(&v, "id")
                .and_then(Value::as_str)
                .unwrap_or("chatcmpl-rolter")
                .to_string();
            self.state.model = interaction_field(&v, "model")
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
        match event_type.as_str() {
            "step.start" => {
                let step = v.get("step").unwrap_or(&v);
                if step.get("type").and_then(Value::as_str) == Some("function_call") {
                    let index = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let tool_index = self.state.next_tool;
                    self.state.next_tool += 1;
                    self.state.tool_indexes.insert(index, tool_index);
                    out.push(openai_chunk(
                        &self.state,
                        json!({"tool_calls":[{
                            "index": tool_index,
                            "id": step.get("call_id").or_else(|| step.get("id")).cloned().unwrap_or_else(|| json!(format!("call_{tool_index}"))),
                            "type": "function",
                            "function": {
                                "name": step.get("name").cloned().unwrap_or_else(|| Value::String(String::new())),
                                "arguments": ""
                            }
                        }]}),
                        Value::Null,
                        None,
                    ));
                }
            }
            "step.delta" => {
                let delta = v.get("delta").unwrap_or(&Value::Null);
                match delta.get("type").and_then(Value::as_str) {
                    Some("arguments_delta") => {
                        let index = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        let tool_index = self.state.tool_indexes.get(&index).copied().unwrap_or(0);
                        out.push(openai_chunk(
                            &self.state,
                            json!({"tool_calls":[{
                                "index": tool_index,
                                "function": {"arguments": delta.get("arguments").cloned().unwrap_or_else(|| Value::String(String::new()))}
                            }]}),
                            Value::Null,
                            None,
                        ));
                    }
                    // thought summaries are internal reasoning, never emitted
                    Some("thought_summary") => {}
                    _ => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            out.push(openai_chunk(
                                &self.state,
                                json!({"content":text}),
                                Value::Null,
                                None,
                            ));
                        }
                    }
                }
            }
            "interaction.completed" | "error" => {
                if self.state.interaction_finished {
                    return out;
                }
                self.state.interaction_finished = true;
                let finish = if self.state.next_tool > 0 {
                    json!("tool_calls")
                } else {
                    interaction_finish(interaction_field(&v, "status"))
                };
                let usage = interaction_usage(interaction_field(&v, "usage"));
                out.push(openai_chunk(&self.state, json!({}), finish, Some(&usage)));
                if emit_done {
                    out.push(Bytes::from_static(b"data: [DONE]\n\n"));
                }
            }
            _ => {}
        }
        out
    }

    /// re-parse an openai chunk frame we just produced and re-emit it as
    /// anthropic sse — used to chain gemini→openai→anthropic for native clients.
    fn openai_frame_to_anthropic(&mut self, frame: &Bytes) -> Vec<Bytes> {
        let text = String::from_utf8_lossy(frame);
        let data = join_data_lines(&text);
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

fn join_data_lines(text: &str) -> String {
    let mut lines = text
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim);
    if let Some(first) = lines.next() {
        let mut s = String::with_capacity(text.len());
        s.push_str(first);
        for line in lines {
            s.push('\n');
            s.push_str(line);
        }
        s
    } else {
        String::new()
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
    let mut out = Vec::with_capacity(512);
    if let Some(e) = event {
        out.extend_from_slice(b"event: ");
        out.extend_from_slice(e.as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(b"data: ");
    // a failed serialization can have written a partial value first, so rewind
    // to the frame boundary before the fallback — otherwise the `{}` would be
    // appended to a half-written object and the frame would not parse
    let data_start = out.len();
    if serde_json::to_writer(&mut out, value).is_err() {
        out.truncate(data_start);
        out.extend_from_slice(b"{}");
    }
    out.extend_from_slice(b"\n\n");
    Bytes::from(out)
}

fn openai_chunk(
    state: &StreamState,
    delta: Value,
    finish_reason: Value,
    usage: Option<&TokenUsage>,
) -> Bytes {
    let mut value = json!({"id":state.id,"object":"chat.completion.chunk","created":0,"model":state.model,"choices":[{"index":0,"delta":delta,"finish_reason":finish_reason}]});
    if let Some(usage) = usage {
        value["usage"] = usage.to_chat();
    }
    sse(None, &value)
}

#[cfg(test)]
mod tests {

    /// A system turn with empty content is still a turn.
    ///
    /// The `Vec<String>` this loop used to build was joined with `"\n"`, so an
    /// empty turn contributed a separator, and a request whose only system turn
    /// was empty still emitted `system_instruction: ""`. Accumulating straight
    /// into a `String` makes "the text is empty" and "there were no turns" look
    /// identical, which silently drops the field and the leading separator.
    #[test]
    fn an_empty_system_turn_is_still_a_system_turn() {
        let out = openai_to_interactions(json!({
            "model": "m",
            "messages": [{"role": "system"}, {"role": "user", "content": "hi"}]
        }))
        .unwrap();
        assert_eq!(out.get("system_instruction"), Some(&json!("")));

        let out = openai_to_interactions(json!({
            "model": "m",
            "messages": [
                {"role": "system", "content": ""},
                {"role": "system", "content": "abc"},
                {"role": "user", "content": "hi"}
            ]
        }))
        .unwrap();
        assert_eq!(out.get("system_instruction"), Some(&json!("\nabc")));

        // and no system turn at all still omits the field entirely
        let out = openai_to_interactions(json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .unwrap();
        assert!(out.get("system_instruction").is_none());
    }
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
    fn anthropic_max_tokens_default_comes_from_the_compatibility_policy() {
        // the Messages API rejects a request without max_tokens, so a translated
        // request that set neither field gets the configured default (#546)
        let body = Bytes::from_static(br#"{"messages":[{"role":"user","content":"hello"}]}"#);
        let plan = plan(Protocol::OpenAiChat, Protocol::AnthropicMessages);
        let compat = CompatibilityConfig {
            anthropic_version: "2023-06-01".to_string(),
            default_max_tokens: 4096,
        };
        let out = plan.translate_request_with(body.clone(), &compat).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["max_tokens"], 4096);

        // the default never overrides what the caller asked for
        let explicit = Bytes::from_static(
            br#"{"messages":[{"role":"user","content":"hello"}],"max_completion_tokens":32}"#,
        );
        let out = plan.translate_request_with(explicit, &compat).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["max_tokens"], 32);

        // and the compiled-in default still applies when no policy is passed
        let out = plan.translate_request(body).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["max_tokens"], 1024);
    }

    fn defaults() -> ModelDefaultsConfig {
        ModelDefaultsConfig {
            enabled: true,
            default_model: Some("gpt-4o-mini".to_string()),
            temperature: Some(0.5),
            top_p: Some(0.8),
            max_tokens: Some(256),
        }
    }

    #[test]
    fn model_defaults_fill_only_the_keys_the_caller_left_out() {
        let plan = plan(Protocol::OpenAiChat, Protocol::OpenAiChat);
        let body = Bytes::from_static(br#"{"messages":[{"role":"user","content":"hi"}]}"#);
        let out = plan.apply_model_defaults(body, &defaults());
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["model"], "gpt-4o-mini");
        assert_eq!(v["temperature"], 0.5);
        assert_eq!(v["top_p"], 0.8);
        assert_eq!(v["max_tokens"], 256);

        // an explicit value always wins, including an explicit null
        let explicit = Bytes::from_static(
            br#"{"model":"claude","temperature":0,"top_p":null,"max_tokens":9,"messages":[]}"#,
        );
        let out = plan.apply_model_defaults(explicit, &defaults());
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["model"], "claude");
        assert_eq!(v["temperature"], 0);
        assert!(v["top_p"].is_null());
        assert_eq!(v["max_tokens"], 9);
    }

    #[test]
    fn model_defaults_use_the_upstream_dialects_token_key() {
        let responses = plan(Protocol::OpenAiResponses, Protocol::OpenAiResponses);
        let out =
            responses.apply_model_defaults(Bytes::from_static(br#"{"input":"hi"}"#), &defaults());
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["max_output_tokens"], 256);
        assert!(v.get("max_tokens").is_none());

        // openai chat accepts either spelling; the newer one is already a cap
        let chat = plan(Protocol::OpenAiChat, Protocol::OpenAiChat);
        let out = chat.apply_model_defaults(
            Bytes::from_static(br#"{"max_completion_tokens":16,"messages":[]}"#),
            &defaults(),
        );
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(v.get("max_tokens").is_none());
    }

    #[test]
    fn model_defaults_leave_non_chat_and_disabled_bodies_untouched() {
        let body = Bytes::from_static(br#"{"input":"hi"}"#);

        // embeddings, audio and images resolve to Passthrough and take no
        // sampling parameters
        let passthrough = plan(Protocol::Passthrough, Protocol::Passthrough);
        assert_eq!(
            passthrough.apply_model_defaults(body.clone(), &defaults()),
            body
        );

        // gemini nests these under generationConfig, so it opts out too
        let gemini = plan(Protocol::OpenAiChat, Protocol::GeminiGenerate);
        assert_eq!(gemini.apply_model_defaults(body.clone(), &defaults()), body);

        let chat = plan(Protocol::OpenAiChat, Protocol::OpenAiChat);
        let off = ModelDefaultsConfig {
            enabled: false,
            ..defaults()
        };
        assert_eq!(chat.apply_model_defaults(body.clone(), &off), body);

        // enabled but with nothing set is also a no-op
        let empty = ModelDefaultsConfig {
            enabled: true,
            ..ModelDefaultsConfig::default()
        };
        assert_eq!(chat.apply_model_defaults(body.clone(), &empty), body);
    }

    #[test]
    fn a_blank_model_string_counts_as_absent() {
        let chat = plan(Protocol::OpenAiChat, Protocol::OpenAiChat);
        let out = chat.apply_model_defaults(
            Bytes::from_static(br#"{"model":"  ","messages":[]}"#),
            &defaults(),
        );
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["model"], "gpt-4o-mini");
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
    fn gemini_interactions_resolves_for_chat_dialects_and_targets_one_endpoint() {
        for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
            let plan = TranslationPlan::resolve(
                path,
                ProviderKind::GeminiInteractions,
                RoleProfile::Openai,
            );
            assert_eq!(plan.upstream, Protocol::GeminiInteractions);
            assert!(!plan.is_gemini_generate());
            assert_eq!(plan.upstream_path(path), "/interactions");
        }
    }

    #[test]
    fn gemini_interactions_fails_closed_for_endpoints_it_cannot_serve() {
        let passthrough = TranslationPlan::resolve(
            "/v1/embeddings",
            ProviderKind::GeminiInteractions,
            RoleProfile::Openai,
        );
        assert_eq!(passthrough.upstream, Protocol::GeminiInteractions);
        let error = passthrough
            .translate_request(Bytes::from_static(br#"{"input":"ping"}"#))
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("gemini_interactions_unsupported"));
    }

    /// A body carrying one unknown content part, in the message shape each
    /// client dialect uses.
    fn body_with_unknown_part(client: Protocol, part: Value) -> Bytes {
        let value = match client {
            Protocol::OpenAiResponses => json!({
                "model": "gemini-3.6-flash",
                "input": [{"role": "user", "content": [part]}]
            }),
            _ => json!({
                "model": "gemini-3.6-flash",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": [part]}]
            }),
        };
        Bytes::from(serde_json::to_vec(&value).unwrap())
    }

    /// #882: an unrecognized part must abort the translation, not vanish from
    /// the body. Covers every client dialect against both Gemini upstreams,
    /// because the two translators carry independent part tables.
    #[test]
    fn unknown_content_parts_fail_closed_on_every_gemini_pair() {
        let clients = [
            Protocol::OpenAiChat,
            Protocol::AnthropicMessages,
            Protocol::OpenAiResponses,
        ];
        let upstreams = [
            (Protocol::GeminiInteractions, "interactions"),
            (Protocol::GeminiGenerate, "gemini generateContent"),
        ];
        let parts = [
            json!({"type": "input_audio", "input_audio": {"data": "AAAA", "format": "wav"}}),
            json!({"type": "file", "file": {"file_id": "file_1"}}),
            json!({"type": "some_future_part", "payload": {}}),
        ];
        for client in clients {
            for (upstream, dialect) in upstreams {
                for part in &parts {
                    let error = plan(client, upstream)
                        .translate_request(body_with_unknown_part(client, part.clone()))
                        .unwrap_err()
                        .to_string();
                    assert!(
                        error.contains("unsupported_content_part"),
                        "{client:?}->{upstream:?} did not fail closed: {error}"
                    );
                    let part_type = part["type"].as_str().unwrap();
                    assert!(
                        error.contains(part_type) && error.contains(dialect),
                        "{client:?}->{upstream:?} error names neither the part nor the dialect: {error}"
                    );
                }
            }
        }
    }

    /// A part object with no `type` at all takes the same path — it is still
    /// content the caller sent that the upstream would never see.
    #[test]
    fn a_typeless_content_part_also_fails_closed() {
        let error = plan(Protocol::OpenAiChat, Protocol::GeminiInteractions)
            .translate_request(body_with_unknown_part(
                Protocol::OpenAiChat,
                json!({"text": "no type field"}),
            ))
            .unwrap_err()
            .to_string();
        assert!(error.contains("unsupported_content_part"), "{error}");
        assert!(error.contains("no 'type' field"), "{error}");
    }

    /// The fail-closed check must not cost the supported parts their pass:
    /// text and images still translate, and a plain string body is untouched.
    #[test]
    fn supported_content_parts_still_translate_after_the_fail_closed_check() {
        let body = Bytes::from(
            serde_json::to_vec(&json!({"model":"gemini-3.6-flash","messages":[{
                "role":"user",
                "content":[
                    {"type":"text","text":"describe"},
                    {"type":"image_url","image_url":{"url":"data:image/png;base64,QUJD"}},
                    {"type":"image_url","image_url":{"url":"https://example.test/a.png"}}
                ]
            }]}))
            .unwrap(),
        );
        let out = plan(Protocol::OpenAiChat, Protocol::GeminiInteractions)
            .translate_request(body.clone())
            .unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["input"][0]["content"][0]["text"], "describe");
        assert_eq!(v["input"][0]["content"][1]["mime_type"], "image/png");
        assert_eq!(v["input"][0]["content"][1]["data"], "QUJD");
        assert_eq!(
            v["input"][0]["content"][2]["file_uri"],
            "https://example.test/a.png"
        );

        let out = plan(Protocol::OpenAiChat, Protocol::GeminiGenerate)
            .translate_request(body)
            .unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["contents"][0]["parts"][0]["text"], "describe");
        assert_eq!(
            v["contents"][0]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
        assert_eq!(
            v["contents"][0]["parts"][2]["fileData"]["fileUri"],
            "https://example.test/a.png"
        );
    }

    /// Dialects that pass unknown parts through rather than dropping them keep
    /// doing so — failing closed is only correct where there is no carrier.
    #[test]
    fn openai_anthropic_translation_still_passes_unknown_parts_through() {
        let part = json!({"type": "some_future_part", "payload": {"k": 1}});
        let out = plan(Protocol::OpenAiChat, Protocol::AnthropicMessages)
            .translate_request(body_with_unknown_part(Protocol::OpenAiChat, part.clone()))
            .unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["messages"][0]["content"][0], part);
    }

    #[test]
    fn openai_request_becomes_an_interactions_create_body() {
        let body = Bytes::from(
            serde_json::to_vec(&json!({"model":"gemini-3.6-flash","messages":[
                {"role":"system","content":"be terse"},
                {"role":"user","content":"hi"},
                {"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"q\":1}"}}]},
                {"role":"tool","tool_call_id":"call_1","content":"ok"}
            ],"temperature":0.5,"max_tokens":128,"stream":true,
            "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}],
            "tool_choice":"required"}))
            .unwrap(),
        );
        let out = plan(Protocol::OpenAiChat, Protocol::GeminiInteractions)
            .translate_request(body)
            .unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["model"], "gemini-3.6-flash");
        assert_eq!(v["system_instruction"], "be terse");
        assert_eq!(v["stream"], true);
        assert_eq!(v["input"][0]["role"], "user");
        assert_eq!(v["input"][0]["content"][0]["text"], "hi");
        assert_eq!(v["input"][1]["type"], "function_call");
        assert_eq!(v["input"][1]["call_id"], "call_1");
        assert_eq!(v["input"][1]["arguments"], "{\"q\":1}");
        assert_eq!(v["input"][2]["type"], "function_result");
        assert_eq!(v["input"][2]["result"][0]["text"], "ok");
        assert_eq!(v["generation_config"]["max_output_tokens"], 128);
        assert_eq!(v["generation_config"]["temperature"], 0.5);
        assert_eq!(v["generation_config"]["tool_choice"], "any");
        assert_eq!(v["tools"][0]["type"], "function");
        assert_eq!(v["tools"][0]["name"], "lookup");
        // chat-only fields must not leak onto the interactions wire
        assert!(v.get("messages").is_none());
        assert!(v.get("max_tokens").is_none());
    }

    #[test]
    fn responses_previous_response_id_threads_the_interaction() {
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "model":"gemini-3.6-flash",
                "input":"what is my name?",
                "previous_response_id":"int_123",
                "store":true
            }))
            .unwrap(),
        );
        let out = plan(Protocol::OpenAiResponses, Protocol::GeminiInteractions)
            .translate_request(body)
            .unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["previous_interaction_id"], "int_123");
        assert_eq!(v["store"], true);
        assert_eq!(v["input"][0]["content"][0]["text"], "what is my name?");
    }

    #[test]
    fn interaction_response_becomes_openai_completion() {
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "id":"int_123",
                "model":"gemini-3.6-flash",
                "status":"completed",
                "steps":[
                    {"type":"thought","content":[{"type":"text","text":"hidden"}]},
                    {"type":"model_output","content":[{"type":"text","text":"hello"}]}
                ],
                "usage":{"total_input_tokens":7,"total_output_tokens":3,"total_tokens":10}
            }))
            .unwrap(),
        );
        let out = plan(Protocol::OpenAiChat, Protocol::GeminiInteractions)
            .translate_response(body, false);
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["id"], "int_123");
        assert_eq!(v["choices"][0]["message"]["content"], "hello");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["usage"]["prompt_tokens"], 7);
        assert_eq!(v["usage"]["total_tokens"], 10);
    }

    #[test]
    fn interaction_function_call_step_maps_to_tool_calls() {
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "id":"int_9",
                "status":"requires_action",
                "steps":[{"type":"function_call","call_id":"c1","name":"lookup","arguments":"{\"q\":1}"}]
            }))
            .unwrap(),
        );
        let out = plan(Protocol::OpenAiChat, Protocol::GeminiInteractions)
            .translate_response(body, false);
        let v: Value = serde_json::from_slice(&out).unwrap();
        let call = &v["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(call["id"], "c1");
        assert_eq!(call["function"]["name"], "lookup");
        assert_eq!(call["function"]["arguments"], "{\"q\":1}");
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn interaction_stream_becomes_openai_chunks() {
        let sse = concat!(
            "data: {\"event_type\":\"interaction.created\",\"id\":\"int_1\",\"model\":\"gemini-3.6-flash\"}\n\n",
            "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"text\",\"text\":\"Hel\"}}\n\n",
            "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"thought_summary\",\"content\":{\"type\":\"text\",\"text\":\"x\"}}}\n\n",
            "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"text\",\"text\":\"lo\"}}\n\n",
            "data: {\"event_type\":\"interaction.completed\",\"interaction\":{\"status\":\"completed\",\"usage\":{\"total_input_tokens\":4,\"total_output_tokens\":2}}}\n\n",
        );
        let out = plan(Protocol::OpenAiChat, Protocol::GeminiInteractions)
            .translate_response(Bytes::from_static(sse.as_bytes()), true);
        let text = String::from_utf8(out.to_vec()).unwrap();
        assert!(text.contains("\"id\":\"int_1\""));
        assert!(text.contains("\"content\":\"Hel\""));
        assert!(text.contains("\"content\":\"lo\""));
        assert!(!text.contains("thought_summary"));
        assert!(text.contains("\"finish_reason\":\"stop\""));
        assert!(text.contains("\"prompt_tokens\":4"));
        assert!(text.ends_with("data: [DONE]\n\n"));
    }

    #[test]
    fn interaction_stream_tool_call_deltas_become_openai_tool_calls() {
        let sse = concat!(
            "data: {\"event_type\":\"interaction.created\",\"id\":\"int_2\"}\n\n",
            "data: {\"event_type\":\"step.start\",\"index\":0,\"step\":{\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"lookup\"}}\n\n",
            "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"arguments_delta\",\"arguments\":\"{\\\"q\\\":\"}}\n\n",
            "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"arguments_delta\",\"arguments\":\"1}\"}}\n\n",
            "data: {\"event_type\":\"interaction.completed\",\"status\":\"requires_action\"}\n\n",
        );
        let out = plan(Protocol::OpenAiChat, Protocol::GeminiInteractions)
            .translate_response(Bytes::from_static(sse.as_bytes()), true);
        let text = String::from_utf8(out.to_vec()).unwrap();
        assert!(text.contains("\"name\":\"lookup\""));
        assert!(text.contains("\"id\":\"c1\""));
        assert!(text.contains("\"arguments\":\"{\\\"q\\\":\""));
        assert!(text.contains("\"finish_reason\":\"tool_calls\""));
    }

    #[test]
    fn interaction_stream_reaches_anthropic_and_responses_clients() {
        let sse = concat!(
            "data: {\"event_type\":\"interaction.created\",\"id\":\"int_3\"}\n\n",
            "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"text\",\"text\":\"hi\"}}\n\n",
            "data: {\"event_type\":\"interaction.completed\",\"status\":\"completed\"}\n\n",
        );
        let anthropic = plan(Protocol::AnthropicMessages, Protocol::GeminiInteractions)
            .translate_response(Bytes::from_static(sse.as_bytes()), true);
        let anthropic = String::from_utf8(anthropic.to_vec()).unwrap();
        assert!(anthropic.contains("event: message_start"));
        assert!(anthropic.contains("event: message_stop"));

        let responses = plan(Protocol::OpenAiResponses, Protocol::GeminiInteractions)
            .translate_response(Bytes::from_static(sse.as_bytes()), true);
        let responses = String::from_utf8(responses.to_vec()).unwrap();
        assert!(responses.contains("response.output_text.delta"));
        assert!(responses.contains("response.completed"));
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

    // ── prompt-cache usage across dialects (#2863) ──────────────────────────
    // anthropic's `input_tokens` leaves the cache reads and writes out; chat
    // completions, the responses api and gemini count them inside the prompt.
    // a translated body states both in the receiving dialect's convention

    /// every JSON `data:` payload of an SSE body, in order, with the `event:`
    /// name of its frame when it has one
    fn frames(sse: &[u8]) -> Vec<(Option<String>, Value)> {
        String::from_utf8_lossy(sse)
            .split("\n\n")
            .filter_map(|frame| {
                let event = frame
                    .lines()
                    .find_map(|line| line.strip_prefix("event:"))
                    .map(|name| name.trim().to_string());
                let data = frame.lines().find_map(|line| line.strip_prefix("data:"))?;
                Some((event, serde_json::from_str(data.trim()).ok()?))
            })
            .collect()
    }

    /// the usage object of the last frame named `event` (or of the last frame
    /// with usage when `event` is `None`)
    fn last_usage(sse: &[u8], event: Option<&str>) -> Value {
        frames(sse)
            .into_iter()
            .filter(|(name, _)| event.is_none() || name.as_deref() == event)
            .filter_map(|(_, data)| {
                data.pointer("/usage")
                    .or_else(|| data.pointer("/response/usage"))
                    .filter(|usage| usage.is_object())
                    .cloned()
            })
            .next_back()
            .unwrap_or(Value::Null)
    }

    fn translate_json_body(client: Protocol, upstream: Protocol, body: Value) -> Value {
        let out = plan(client, upstream)
            .translate_response(Bytes::from(serde_json::to_vec(&body).unwrap()), false);
        serde_json::from_slice(&out).unwrap()
    }

    fn translate_sse_body(client: Protocol, upstream: Protocol, sse: &str) -> Bytes {
        plan(client, upstream).translate_response(Bytes::copy_from_slice(sse.as_bytes()), true)
    }

    const ANTHROPIC_CACHED_USAGE: &str = r#"{"input_tokens":10,"cache_creation_input_tokens":30,"cache_read_input_tokens":80,"output_tokens":5}"#;

    fn anthropic_cached_body() -> Value {
        json!({
            "id":"msg_1","model":"claude","content":[{"type":"text","text":"hi"}],
            "stop_reason":"end_turn",
            "usage": serde_json::from_str::<Value>(ANTHROPIC_CACHED_USAGE).unwrap(),
        })
    }

    fn chat_cached_body(details: Value) -> Value {
        json!({
            "id":"chat_1","model":"gpt",
            "choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":120,"completion_tokens":5,"total_tokens":125,
                     "prompt_tokens_details":details},
        })
    }

    #[test]
    fn an_anthropic_answer_reaches_a_chat_client_with_the_cache_inside_the_prompt() {
        let v = translate_json_body(
            Protocol::OpenAiChat,
            Protocol::AnthropicMessages,
            anthropic_cached_body(),
        );
        // 10 fresh + 80 read + 30 written
        assert_eq!(v["usage"]["prompt_tokens"], 120);
        assert_eq!(v["usage"]["completion_tokens"], 5);
        assert_eq!(v["usage"]["total_tokens"], 125);
        assert_eq!(v["usage"]["prompt_tokens_details"]["cached_tokens"], 80);
        assert_eq!(
            v["usage"]["prompt_tokens_details"]["cache_write_tokens"],
            30
        );
    }

    #[test]
    fn an_anthropic_answer_reaches_a_responses_client_with_the_cache_inside_the_input() {
        let v = translate_json_body(
            Protocol::OpenAiResponses,
            Protocol::AnthropicMessages,
            anthropic_cached_body(),
        );
        assert_eq!(v["usage"]["input_tokens"], 120);
        assert_eq!(v["usage"]["output_tokens"], 5);
        assert_eq!(v["usage"]["total_tokens"], 125);
        assert_eq!(v["usage"]["input_tokens_details"]["cached_tokens"], 80);
        assert_eq!(v["usage"]["input_tokens_details"]["cache_write_tokens"], 30);
    }

    /// an upstream that reports no caching must not have a zero stated for it
    #[test]
    fn a_body_without_cache_figures_gets_none_added() {
        let anthropic = json!({"id":"m","content":[],"usage":{"input_tokens":3,"output_tokens":4}});
        let v = translate_json_body(Protocol::OpenAiChat, Protocol::AnthropicMessages, anthropic);
        assert_eq!(v["usage"]["prompt_tokens"], 3);
        assert!(v["usage"].get("prompt_tokens_details").is_none());

        let chat = json!({"id":"c","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":4}});
        let v = translate_json_body(
            Protocol::AnthropicMessages,
            Protocol::OpenAiChat,
            chat.clone(),
        );
        assert_eq!(v["usage"]["input_tokens"], 3);
        assert!(v["usage"].get("cache_read_input_tokens").is_none());
        assert!(v["usage"].get("cache_creation_input_tokens").is_none());
        let v = translate_json_body(Protocol::OpenAiResponses, Protocol::OpenAiChat, chat);
        assert!(v["usage"].get("input_tokens_details").is_none());
    }

    #[test]
    fn a_chat_answer_reaches_a_messages_client_with_the_cache_beside_the_input() {
        let v = translate_json_body(
            Protocol::AnthropicMessages,
            Protocol::OpenAiChat,
            chat_cached_body(json!({"cached_tokens":80})),
        );
        // anthropic's input_tokens leaves the 80 cached tokens out
        assert_eq!(v["usage"]["input_tokens"], 40);
        assert_eq!(v["usage"]["cache_read_input_tokens"], 80);
        assert_eq!(v["usage"]["output_tokens"], 5);
        assert!(v["usage"].get("cache_creation_input_tokens").is_none());

        let v = translate_json_body(
            Protocol::AnthropicMessages,
            Protocol::OpenAiChat,
            chat_cached_body(json!({"cached_tokens":80,"cache_write_tokens":30})),
        );
        assert_eq!(v["usage"]["input_tokens"], 10);
        assert_eq!(v["usage"]["cache_read_input_tokens"], 80);
        assert_eq!(v["usage"]["cache_creation_input_tokens"], 30);
    }

    #[test]
    fn a_chat_answer_reaches_a_responses_client_with_the_cache_inside_the_input() {
        let v = translate_json_body(
            Protocol::OpenAiResponses,
            Protocol::OpenAiChat,
            chat_cached_body(json!({"cached_tokens":80})),
        );
        assert_eq!(v["usage"]["input_tokens"], 120);
        assert_eq!(v["usage"]["input_tokens_details"]["cached_tokens"], 80);
    }

    /// a cache figure larger than the prompt it is part of is a provider bug,
    /// not a reason to wrap a counter
    #[test]
    fn a_cache_larger_than_the_prompt_does_not_underflow() {
        let v = translate_json_body(
            Protocol::AnthropicMessages,
            Protocol::OpenAiChat,
            chat_cached_body(json!({"cached_tokens":500})),
        );
        assert_eq!(v["usage"]["input_tokens"], 0);
        assert_eq!(v["usage"]["cache_read_input_tokens"], 500);
    }

    /// the cache count is not lost on the way through the chat intermediate
    #[test]
    fn anthropic_usage_survives_a_round_trip_through_chat() {
        let original: Value = serde_json::from_str(ANTHROPIC_CACHED_USAGE).unwrap();
        let chat = TokenUsage::from_anthropic(&original).to_chat();
        let back = TokenUsage::from_openai(&chat).to_anthropic();
        assert_eq!(back, original);
    }

    #[test]
    fn gemini_cached_content_is_carried_to_every_client_dialect() {
        let gemini = json!({
            "candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":80,
                             "candidatesTokenCount":5,"thoughtsTokenCount":7,
                             "totalTokenCount":112}
        });
        // gemini counts the cached content inside promptTokenCount, like chat
        let v = translate_json_body(
            Protocol::OpenAiChat,
            Protocol::GeminiGenerate,
            gemini.clone(),
        );
        assert_eq!(v["usage"]["prompt_tokens"], 100);
        assert_eq!(v["usage"]["completion_tokens"], 12);
        assert_eq!(v["usage"]["total_tokens"], 112);
        assert_eq!(v["usage"]["prompt_tokens_details"]["cached_tokens"], 80);

        let v = translate_json_body(
            Protocol::AnthropicMessages,
            Protocol::GeminiGenerate,
            gemini.clone(),
        );
        assert_eq!(v["usage"]["input_tokens"], 20);
        assert_eq!(v["usage"]["cache_read_input_tokens"], 80);

        let v = translate_json_body(Protocol::OpenAiResponses, Protocol::GeminiGenerate, gemini);
        assert_eq!(v["usage"]["input_tokens"], 100);
        assert_eq!(v["usage"]["input_tokens_details"]["cached_tokens"], 80);
    }

    #[test]
    fn interactions_cached_tokens_are_carried_to_every_client_dialect() {
        let interaction = json!({
            "id":"i1","status":"completed","model":"gemini",
            "steps":[{"type":"model_output","content":[{"type":"text","text":"hi"}]}],
            "usage":{"total_input_tokens":100,"total_cached_tokens":80,
                     "total_output_tokens":5,"total_tokens":105}
        });
        let v = translate_json_body(
            Protocol::OpenAiChat,
            Protocol::GeminiInteractions,
            interaction.clone(),
        );
        assert_eq!(v["usage"]["prompt_tokens"], 100);
        assert_eq!(v["usage"]["prompt_tokens_details"]["cached_tokens"], 80);

        let v = translate_json_body(
            Protocol::AnthropicMessages,
            Protocol::GeminiInteractions,
            interaction.clone(),
        );
        assert_eq!(v["usage"]["input_tokens"], 20);
        assert_eq!(v["usage"]["cache_read_input_tokens"], 80);

        let v = translate_json_body(
            Protocol::OpenAiResponses,
            Protocol::GeminiInteractions,
            interaction,
        );
        assert_eq!(v["usage"]["input_tokens_details"]["cached_tokens"], 80);
    }

    // anthropic reports the input side on `message_start` and, depending on
    // the API version, only the output count (or the cumulative figures) on
    // `message_delta`
    const ANTHROPIC_STREAM: &str = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude\",\"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":30,\"cache_read_input_tokens\":80,\"output_tokens\":1}}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
    );

    #[test]
    fn an_anthropic_stream_reaches_a_chat_client_with_the_cache_on_every_usage_chunk() {
        let out = translate_sse_body(
            Protocol::OpenAiChat,
            Protocol::AnthropicMessages,
            ANTHROPIC_STREAM,
        );
        let usages: Vec<Value> = frames(&out)
            .into_iter()
            .filter_map(|(_, data)| data.get("usage").cloned())
            .collect();
        assert_eq!(usages.len(), 2, "message_start and message_delta");
        // the closing chunk is what a client reads; it carries the input side
        // from message_start and the output side from message_delta
        let closing = &usages[1];
        assert_eq!(closing["prompt_tokens"], 120);
        assert_eq!(closing["completion_tokens"], 5);
        assert_eq!(closing["total_tokens"], 125);
        assert_eq!(closing["prompt_tokens_details"]["cached_tokens"], 80);
        assert_eq!(closing["prompt_tokens_details"]["cache_write_tokens"], 30);
        assert_eq!(usages[0]["prompt_tokens_details"]["cached_tokens"], 80);
    }

    /// newer api versions repeat the cumulative figures on `message_delta`
    #[test]
    fn a_cumulative_message_delta_does_not_double_the_cache() {
        let sse = ANTHROPIC_STREAM.replace(
            r#""usage":{"output_tokens":5}"#,
            &format!(r#""usage":{ANTHROPIC_CACHED_USAGE}"#),
        );
        let out = translate_sse_body(Protocol::OpenAiChat, Protocol::AnthropicMessages, &sse);
        let closing = last_usage(&out, None);
        assert_eq!(closing["prompt_tokens"], 120);
        assert_eq!(closing["prompt_tokens_details"]["cached_tokens"], 80);
    }

    #[test]
    fn an_anthropic_stream_reaches_a_responses_client_with_the_cache_in_the_completed_event() {
        let out = translate_sse_body(
            Protocol::OpenAiResponses,
            Protocol::AnthropicMessages,
            ANTHROPIC_STREAM,
        );
        let usage = last_usage(&out, Some("response.completed"));
        assert_eq!(usage["input_tokens"], 120);
        assert_eq!(usage["output_tokens"], 5);
        assert_eq!(usage["total_tokens"], 125);
        assert_eq!(usage["input_tokens_details"]["cached_tokens"], 80);
    }

    // a chat stream reports usage once, on a chunk with no choices
    const CHAT_STREAM: &str = concat!(
        "data: {\"id\":\"c1\",\"model\":\"gpt\",\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c1\",\"model\":\"gpt\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"id\":\"c1\",\"model\":\"gpt\",\"choices\":[],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":5,\"total_tokens\":125,\"prompt_tokens_details\":{\"cached_tokens\":80}}}\n\n",
        "data: [DONE]\n\n"
    );

    #[test]
    fn a_chat_stream_reaches_a_messages_client_with_the_cache_beside_the_input() {
        let out = translate_sse_body(
            Protocol::AnthropicMessages,
            Protocol::OpenAiChat,
            CHAT_STREAM,
        );
        let usage = last_usage(&out, Some("message_delta"));
        assert_eq!(usage["input_tokens"], 40);
        assert_eq!(usage["cache_read_input_tokens"], 80);
        assert_eq!(usage["output_tokens"], 5);
    }

    #[test]
    fn a_chat_stream_reaches_a_responses_client_with_the_cache_inside_the_input() {
        let out = translate_sse_body(Protocol::OpenAiResponses, Protocol::OpenAiChat, CHAT_STREAM);
        let usage = last_usage(&out, Some("response.completed"));
        assert_eq!(usage["input_tokens"], 120);
        assert_eq!(usage["total_tokens"], 125);
        assert_eq!(usage["input_tokens_details"]["cached_tokens"], 80);
    }

    #[test]
    fn a_gemini_stream_carries_cached_content_to_every_client_dialect() {
        let sse = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":100,\"cachedContentTokenCount\":80,\"candidatesTokenCount\":5}}\n\n";
        let out = translate_sse_body(Protocol::OpenAiChat, Protocol::GeminiGenerate, sse);
        let usage = last_usage(&out, None);
        assert_eq!(usage["prompt_tokens"], 100);
        assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 80);

        let out = translate_sse_body(Protocol::AnthropicMessages, Protocol::GeminiGenerate, sse);
        let usage = last_usage(&out, Some("message_delta"));
        assert_eq!(usage["input_tokens"], 20);
        assert_eq!(usage["cache_read_input_tokens"], 80);

        let out = translate_sse_body(Protocol::OpenAiResponses, Protocol::GeminiGenerate, sse);
        let usage = last_usage(&out, Some("response.completed"));
        assert_eq!(usage["input_tokens"], 100);
        assert_eq!(usage["input_tokens_details"]["cached_tokens"], 80);
    }

    #[test]
    fn an_interactions_stream_carries_cached_tokens_to_every_client_dialect() {
        let sse = "data: {\"event_type\":\"interaction.completed\",\"interaction\":{\"status\":\"completed\",\"usage\":{\"total_input_tokens\":100,\"total_cached_tokens\":80,\"total_output_tokens\":5}}}\n\n";
        let out = translate_sse_body(Protocol::OpenAiChat, Protocol::GeminiInteractions, sse);
        let usage = last_usage(&out, None);
        assert_eq!(usage["prompt_tokens"], 100);
        assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 80);

        let out = translate_sse_body(
            Protocol::AnthropicMessages,
            Protocol::GeminiInteractions,
            sse,
        );
        let usage = last_usage(&out, Some("message_delta"));
        assert_eq!(usage["input_tokens"], 20);
        assert_eq!(usage["cache_read_input_tokens"], 80);

        let out = translate_sse_body(Protocol::OpenAiResponses, Protocol::GeminiInteractions, sse);
        let usage = last_usage(&out, Some("response.completed"));
        assert_eq!(usage["input_tokens_details"]["cached_tokens"], 80);
    }

    // ── thinking and tool-use tokens (#2875) ────────────────────────────────
    // gemini bills thinking as output and tool-use prompts as input, and
    // reports each beside the headline count rather than inside it, so a usage
    // block read for `candidatesTokenCount` alone under-states the charge

    /// 100 prompt, 5 answer and 20 thinking tokens; the provider's total
    /// covers all three
    fn gemini_thinking_body() -> Value {
        json!({
            "candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":5,
                             "thoughtsTokenCount":20,"totalTokenCount":125}
        })
    }

    #[test]
    fn gemini_thinking_tokens_join_the_completion_for_every_client_dialect() {
        let v = translate_json_body(
            Protocol::OpenAiChat,
            Protocol::GeminiGenerate,
            gemini_thinking_body(),
        );
        assert_eq!(v["usage"]["prompt_tokens"], 100);
        assert_eq!(v["usage"]["completion_tokens"], 25);
        assert_eq!(v["usage"]["total_tokens"], 125);

        let v = translate_json_body(
            Protocol::AnthropicMessages,
            Protocol::GeminiGenerate,
            gemini_thinking_body(),
        );
        assert_eq!(v["usage"]["input_tokens"], 100);
        assert_eq!(v["usage"]["output_tokens"], 25);

        let v = translate_json_body(
            Protocol::OpenAiResponses,
            Protocol::GeminiGenerate,
            gemini_thinking_body(),
        );
        assert_eq!(v["usage"]["input_tokens"], 100);
        assert_eq!(v["usage"]["output_tokens"], 25);
        assert_eq!(v["usage"]["total_tokens"], 125);
    }

    /// whatever total the provider states, the one shown is the sum of the
    /// two counts beside it
    #[test]
    fn a_translated_gemini_total_is_always_prompt_plus_completion() {
        for stated in [json!(null), json!(7), json!(9000)] {
            let mut body = gemini_thinking_body();
            body["usageMetadata"]["totalTokenCount"] = stated.clone();
            let v = translate_json_body(Protocol::OpenAiChat, Protocol::GeminiGenerate, body);
            assert_eq!(v["usage"]["total_tokens"], 125, "provider total {stated}");
        }
        let v = translate_json_body(
            Protocol::OpenAiChat,
            Protocol::GeminiGenerate,
            json!({
                "candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":5}
            }),
        );
        assert_eq!(v["usage"]["total_tokens"], 105);
    }

    /// a model that does not think reports no `thoughtsTokenCount`, and its
    /// usage is what it always was
    #[test]
    fn a_gemini_answer_without_thoughts_is_unchanged() {
        let v = translate_json_body(
            Protocol::OpenAiChat,
            Protocol::GeminiGenerate,
            json!({
                "candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2,"totalTokenCount":7}
            }),
        );
        assert_eq!(v["usage"]["prompt_tokens"], 5);
        assert_eq!(v["usage"]["completion_tokens"], 2);
        assert_eq!(v["usage"]["total_tokens"], 7);
    }

    /// what a built-in tool (URL context, say) fed the model is charged as
    /// input, so it is part of the prompt, and cached content stays inside it
    #[test]
    fn gemini_tool_use_prompt_tokens_join_the_prompt() {
        let body = json!({
            "candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":60,
                             "toolUsePromptTokenCount":40,"candidatesTokenCount":5,
                             "thoughtsTokenCount":20,"totalTokenCount":165}
        });
        let v = translate_json_body(Protocol::OpenAiChat, Protocol::GeminiGenerate, body.clone());
        assert_eq!(v["usage"]["prompt_tokens"], 140);
        assert_eq!(v["usage"]["completion_tokens"], 25);
        assert_eq!(v["usage"]["total_tokens"], 165);
        assert_eq!(v["usage"]["prompt_tokens_details"]["cached_tokens"], 60);

        // Messages: the cache comes off the prompt, the rest is `input_tokens`
        let v = translate_json_body(Protocol::AnthropicMessages, Protocol::GeminiGenerate, body);
        assert_eq!(v["usage"]["input_tokens"], 80);
        assert_eq!(v["usage"]["cache_read_input_tokens"], 60);
    }

    #[test]
    fn a_gemini_stream_counts_thinking_tokens_for_every_client_dialect() {
        let sse = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":100,\"candidatesTokenCount\":5,\"thoughtsTokenCount\":20,\"totalTokenCount\":125}}\n\n";
        let out = translate_sse_body(Protocol::OpenAiChat, Protocol::GeminiGenerate, sse);
        let usage = last_usage(&out, None);
        assert_eq!(usage["prompt_tokens"], 100);
        assert_eq!(usage["completion_tokens"], 25);
        assert_eq!(usage["total_tokens"], 125);

        let out = translate_sse_body(Protocol::AnthropicMessages, Protocol::GeminiGenerate, sse);
        let usage = last_usage(&out, Some("message_delta"));
        assert_eq!(usage["input_tokens"], 100);
        assert_eq!(usage["output_tokens"], 25);

        let out = translate_sse_body(Protocol::OpenAiResponses, Protocol::GeminiGenerate, sse);
        let usage = last_usage(&out, Some("response.completed"));
        assert_eq!(usage["output_tokens"], 25);
        assert_eq!(usage["total_tokens"], 125);
    }

    fn interaction_thinking_body(usage: Value) -> Value {
        json!({
            "id":"i1","status":"completed","model":"gemini",
            "steps":[{"type":"model_output","content":[{"type":"text","text":"hi"}]}],
            "usage":usage
        })
    }

    #[test]
    fn interactions_thinking_and_tool_use_tokens_are_counted_for_every_client_dialect() {
        let body = interaction_thinking_body(json!({
            "total_input_tokens":100,"total_tool_use_tokens":40,
            "total_output_tokens":5,"total_thought_tokens":20,"total_tokens":165
        }));
        let v = translate_json_body(
            Protocol::OpenAiChat,
            Protocol::GeminiInteractions,
            body.clone(),
        );
        assert_eq!(v["usage"]["prompt_tokens"], 140);
        assert_eq!(v["usage"]["completion_tokens"], 25);
        assert_eq!(v["usage"]["total_tokens"], 165);

        let v = translate_json_body(
            Protocol::AnthropicMessages,
            Protocol::GeminiInteractions,
            body.clone(),
        );
        assert_eq!(v["usage"]["input_tokens"], 140);
        assert_eq!(v["usage"]["output_tokens"], 25);

        let v = translate_json_body(
            Protocol::OpenAiResponses,
            Protocol::GeminiInteractions,
            body,
        );
        assert_eq!(v["usage"]["input_tokens"], 140);
        assert_eq!(v["usage"]["output_tokens"], 25);
        assert_eq!(v["usage"]["total_tokens"], 165);
    }

    /// the long names are the API reference's; the short ones appear in
    /// Google's guides, and an answer in either is counted the same
    #[test]
    fn interactions_usage_is_read_under_either_spelling() {
        let long = interaction_usage(Some(&json!({
            "total_input_tokens":100,"total_tool_use_tokens":40,
            "total_output_tokens":5,"total_thought_tokens":20
        })));
        let short = interaction_usage(Some(&json!({
            "input_tokens":100,"tool_use_input_tokens":40,
            "output_tokens":5,"thoughts_tokens":20
        })));
        assert_eq!(long, short);
        assert_eq!((long.prompt, long.completion), (140, 25));
        assert_eq!(long.total(), 165);
    }

    #[test]
    fn an_interactions_stream_counts_thinking_tokens_for_every_client_dialect() {
        let sse = "data: {\"event_type\":\"interaction.completed\",\"interaction\":{\"status\":\"completed\",\"usage\":{\"total_input_tokens\":100,\"total_tool_use_tokens\":40,\"total_output_tokens\":5,\"total_thought_tokens\":20,\"total_tokens\":165}}}\n\n";
        let out = translate_sse_body(Protocol::OpenAiChat, Protocol::GeminiInteractions, sse);
        let usage = last_usage(&out, None);
        assert_eq!(usage["prompt_tokens"], 140);
        assert_eq!(usage["completion_tokens"], 25);
        assert_eq!(usage["total_tokens"], 165);

        let out = translate_sse_body(
            Protocol::AnthropicMessages,
            Protocol::GeminiInteractions,
            sse,
        );
        let usage = last_usage(&out, Some("message_delta"));
        assert_eq!(usage["input_tokens"], 140);
        assert_eq!(usage["output_tokens"], 25);

        let out = translate_sse_body(Protocol::OpenAiResponses, Protocol::GeminiInteractions, sse);
        let usage = last_usage(&out, Some("response.completed"));
        assert_eq!(usage["output_tokens"], 25);
        assert_eq!(usage["total_tokens"], 165);
    }

    // ── cache hits spelled outside the OpenAI details blocks (#2877) ────────

    /// one case per spelling: the usage object a provider sends and the
    /// provider it is documented for
    const HIT_SPELLINGS: [(&str, &str); 4] = [
        (
            "chat completions",
            r#"{"prompt_tokens":120,"completion_tokens":5,"total_tokens":125,"prompt_tokens_details":{"cached_tokens":80}}"#,
        ),
        (
            "deepseek",
            r#"{"prompt_tokens":120,"completion_tokens":5,"total_tokens":125,"prompt_cache_hit_tokens":80,"prompt_cache_miss_tokens":40}"#,
        ),
        (
            "kimi",
            r#"{"prompt_tokens":120,"completion_tokens":5,"total_tokens":125,"cached_tokens":80}"#,
        ),
        (
            "gigachat",
            r#"{"prompt_tokens":120,"completion_tokens":5,"total_tokens":125,"precached_prompt_tokens":80}"#,
        ),
    ];

    #[test]
    fn every_spelling_of_a_cache_hit_is_read() {
        for (name, usage) in HIT_SPELLINGS {
            let usage: Value = serde_json::from_str(usage).unwrap();
            assert_eq!(cached_prompt_tokens(&usage), Some(80), "{name}");
            let read = TokenUsage::from_openai(&usage);
            assert_eq!(read.cache_read, Some(80), "{name}");
            assert_eq!(read.prompt, 120, "{name}: the hit is inside the prompt");
        }
        // the Responses API names the counts differently as well
        let responses = json!({"input_tokens":120,"input_tokens_details":{"cached_tokens":80}});
        assert_eq!(cached_prompt_tokens(&responses), Some(80));
        assert_eq!(TokenUsage::from_openai(&responses).cache_read, Some(80));
    }

    #[test]
    fn a_usage_object_without_a_hit_reports_none() {
        let usage = json!({"prompt_tokens":120,"completion_tokens":5,"cached_tokens":null});
        assert_eq!(cached_prompt_tokens(&usage), None);
        assert_eq!(TokenUsage::from_openai(&usage).cache_read, None);
        // the miss count is the complement of the hit and says nothing alone
        let miss = json!({"prompt_tokens":120,"prompt_cache_miss_tokens":120});
        assert_eq!(cached_prompt_tokens(&miss), None);
    }

    #[test]
    fn a_hit_reported_twice_is_one_hit() {
        let usage = json!({
            "prompt_tokens":120,"prompt_tokens_details":{"cached_tokens":80},
            "prompt_cache_hit_tokens":80
        });
        assert_eq!(cached_prompt_tokens(&usage), Some(80));
    }

    fn chat_body_with_usage(usage: &str) -> Value {
        json!({
            "id":"chat_1","model":"deepseek-chat",
            "choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],
            "usage": serde_json::from_str::<Value>(usage).unwrap(),
        })
    }

    #[test]
    fn a_differently_spelled_hit_reaches_a_messages_client_beside_the_input() {
        for (name, usage) in HIT_SPELLINGS {
            let v = translate_json_body(
                Protocol::AnthropicMessages,
                Protocol::OpenAiChat,
                chat_body_with_usage(usage),
            );
            assert_eq!(v["usage"]["input_tokens"], 40, "{name}");
            assert_eq!(v["usage"]["cache_read_input_tokens"], 80, "{name}");
            assert_eq!(v["usage"]["output_tokens"], 5, "{name}");
        }
    }

    #[test]
    fn a_differently_spelled_hit_reaches_a_responses_client_inside_the_input() {
        for (name, usage) in HIT_SPELLINGS {
            let v = translate_json_body(
                Protocol::OpenAiResponses,
                Protocol::OpenAiChat,
                chat_body_with_usage(usage),
            );
            assert_eq!(v["usage"]["input_tokens"], 120, "{name}");
            assert_eq!(
                v["usage"]["input_tokens_details"]["cached_tokens"], 80,
                "{name}"
            );
        }
    }

    #[test]
    fn a_differently_spelled_hit_on_a_stream_reaches_every_client_dialect() {
        for (name, usage) in HIT_SPELLINGS {
            let sse = format!(
                "data: {{\"id\":\"c1\",\"model\":\"m\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"hi\"}}}}]}}\n\n\
                 data: {{\"id\":\"c1\",\"model\":\"m\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n\
                 data: {{\"id\":\"c1\",\"model\":\"m\",\"choices\":[],\"usage\":{usage}}}\n\n\
                 data: [DONE]\n\n"
            );
            let out = translate_sse_body(Protocol::AnthropicMessages, Protocol::OpenAiChat, &sse);
            let seen = last_usage(&out, Some("message_delta"));
            assert_eq!(seen["input_tokens"], 40, "{name}");
            assert_eq!(seen["cache_read_input_tokens"], 80, "{name}");

            let out = translate_sse_body(Protocol::OpenAiResponses, Protocol::OpenAiChat, &sse);
            let seen = last_usage(&out, Some("response.completed"));
            assert_eq!(seen["input_tokens"], 120, "{name}");
            assert_eq!(seen["input_tokens_details"]["cached_tokens"], 80, "{name}");
        }
    }
}
