//! OpenTelemetry GenAI semantic conventions (#808).
//!
//! Before this, the gateway emitted two `gen_ai.*` attributes — one of them the
//! deprecated `gen_ai.system` — and **neither** of the two the spec marks
//! *Required*. Because both required attributes were missing or misnamed, no
//! conformant backend classified our spans as GenAI spans at all: the failure
//! was total rather than partial.
//!
//! Every attribute key lives here as a constant rather than inline in the
//! handler. The conventions are still marked *Development* upstream, and
//! `gen_ai.system` → `gen_ai.provider.name` is itself proof that they rename
//! things; when the next rename lands it should be one edit in this file.
//!
//! ## Deliberate deviations
//!
//! Three of rolter's routes have no answer in the spec. They are recorded here
//! rather than left to be rediscovered:
//!
//! - **`/v1/rerank`** — no matching value exists in the `gen_ai.operation.name`
//!   enum. Emitted as [`OP_RERANK`], a rolter-local value, so the span is still
//!   grouped and attributed rather than silently unlabelled. A local value is
//!   preferable to borrowing an unrelated one such as `embeddings`, which would
//!   put reranks into someone's embeddings dashboard.
//! - **`/v1/audio/translations`** and `/v1/audio/transcriptions` — the output
//!   modality is expressible (`text`), the input modality (audio) is not. Both
//!   are emitted as `generate_content` with `gen_ai.output.type=text`; the fact
//!   that the input was audio is simply not representable today.
//! - **`/v1/realtime`** — a websocket session does not fit the request/response
//!   span shape at all, so it gets no GenAI operation. Forcing one would
//!   describe a span that does not exist.
//!
//! ## What is deliberately *not* emitted
//!
//! Prompt and completion content. The opt-in `gen_ai.input.messages` /
//! `gen_ai.output.messages` attributes stay off, and spans carry no request or
//! response bodies. Payload capture is a separate, explicitly configured
//! feature that writes to ClickHouse, not something telemetry acquires by the
//! back door.

// several of these keys are also spelled out as literals at the `tracing` span
// macro call site, because `tracing` requires span field names to be literals
// and will not accept a constant. this module stays the canonical registry
// anyway — the conventions are still *Development* upstream and
// `gen_ai.system` -> `gen_ai.provider.name` is proof they rename things — and a
// unit test pins every spelling, so a rename that misses a call site fails a
// test rather than silently producing an unrecognised span
#![allow(dead_code)]

/// `gen_ai.operation.name` — **Required** by the spec, previously absent.
pub(crate) const OPERATION_NAME: &str = "gen_ai.operation.name";
/// `gen_ai.provider.name` — **Required** by the spec. Replaces `gen_ai.system`.
pub(crate) const PROVIDER_NAME: &str = "gen_ai.provider.name";
/// `gen_ai.system` — the superseded spelling.
///
/// Still emitted alongside [`PROVIDER_NAME`]. The rename is recent and backends
/// are mid-transition; emitting only the new key would blank out any dashboard
/// still grouping by the old one, and emitting only the old key is the bug this
/// module fixes. The cost is one duplicated low-cardinality attribute.
pub(crate) const SYSTEM: &str = "gen_ai.system";
/// `gen_ai.request.model` — conditionally required; the model as asked for.
pub(crate) const REQUEST_MODEL: &str = "gen_ai.request.model";
/// `gen_ai.response.model` — the model the provider says it actually served.
pub(crate) const RESPONSE_MODEL: &str = "gen_ai.response.model";
/// `gen_ai.response.finish_reasons` — **Recommended**; why generation stopped.
///
/// The spec types this as an array of strings, one per generation. `tracing`
/// has no array field type — its `Value` set is scalars, `&str` and `Debug` —
/// so a multi-choice response is recorded as one comma-joined string rather
/// than a list. Single-choice responses, which is nearly all traffic, are
/// unaffected either way, and a joined string still answers the question the
/// attribute exists for ("did this get truncated?").
///
/// The values stay provider-native: OpenAI's `stop`/`length`/`content_filter`,
/// Anthropic's `end_turn`/`max_tokens`/`stop_sequence`/`tool_use`. The spec's
/// own examples are provider-native, and normalising them would erase real
/// distinctions — Anthropic's `end_turn` and `stop_sequence` are both "finished
/// naturally" but only one of them means the caller's stop sequence fired.
pub(crate) const RESPONSE_FINISH_REASONS: &str = "gen_ai.response.finish_reasons";
/// `gen_ai.output.type` — output modality, for the non-text routes.
pub(crate) const OUTPUT_TYPE: &str = "gen_ai.output.type";
/// `gen_ai.usage.input_tokens` — recommended.
pub(crate) const USAGE_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
/// `gen_ai.usage.output_tokens` — recommended.
pub(crate) const USAGE_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";
/// `gen_ai.response.id` — **Recommended**; the provider's own id for the call.
///
/// This is the join key between a rolter span and the provider's record of the
/// same request. Without it a provider-side support ticket is a description of
/// a call rather than a reference to one, and the provider cannot find it.
pub(crate) const RESPONSE_ID: &str = "gen_ai.response.id";
/// `gen_ai.embeddings.dimension.count` — the width of the vectors returned.
///
/// Embeddings spans otherwise say nothing about the vectors at all: the
/// operation name and the token counts are identical whether the provider
/// returned 384 dimensions or 3072, and dimensionality is what an index has to
/// agree with.
pub(crate) const EMBEDDINGS_DIMENSION_COUNT: &str = "gen_ai.embeddings.dimension.count";
/// `gen_ai.request.encoding_formats` — how the caller asked for the vectors.
///
/// Spec'd as an array; `tracing` has no array field type, so the values are
/// joined the same way `RESPONSE_FINISH_REASONS` is (#835). Low cardinality by
/// construction — the wire values are `float` and `base64`.
pub(crate) const REQUEST_ENCODING_FORMATS: &str = "gen_ai.request.encoding_formats";
/// `error.type` — conditionally required on failure.
pub(crate) const ERROR_TYPE: &str = "error.type";

/// `gen_ai.operation.name` values from the spec enum.
pub(crate) const OP_CHAT: &str = "chat";
pub(crate) const OP_TEXT_COMPLETION: &str = "text_completion";
pub(crate) const OP_EMBEDDINGS: &str = "embeddings";
pub(crate) const OP_GENERATE_CONTENT: &str = "generate_content";
/// Not in the spec enum; see the module docs for why rerank gets a local value.
pub(crate) const OP_RERANK: &str = "rerank";

/// `gen_ai.output.type` values.
pub(crate) const OUTPUT_TEXT: &str = "text";
pub(crate) const OUTPUT_IMAGE: &str = "image";
pub(crate) const OUTPUT_SPEECH: &str = "speech";

/// How one inference route maps onto the conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Operation {
    /// `gen_ai.operation.name`
    pub(crate) name: &'static str,
    /// `gen_ai.output.type`, where the modality is expressible
    pub(crate) output_type: Option<&'static str>,
}

impl Operation {
    const fn new(name: &'static str, output_type: Option<&'static str>) -> Self {
        Self { name, output_type }
    }
}

/// Map a request path to its GenAI operation, or `None` when the route is not
/// an inference call the conventions describe.
///
/// Matching is on the path rolter routes on, so a provider-specific suffix or a
/// query string cannot change the classification.
pub(crate) fn operation_for_path(path: &str) -> Option<Operation> {
    let path = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    Some(match path {
        // the three chat dialects are one operation: they differ in wire
        // format, not in what the model is being asked to do
        "/v1/chat/completions" | "/v1/messages" | "/v1/responses" => {
            Operation::new(OP_CHAT, Some(OUTPUT_TEXT))
        }
        "/v1/completions" => Operation::new(OP_TEXT_COMPLETION, Some(OUTPUT_TEXT)),
        "/v1/embeddings" => Operation::new(OP_EMBEDDINGS, None),
        "/v1/images/generations" | "/v1/images/edits" | "/v1/images/variations" => {
            Operation::new(OP_GENERATE_CONTENT, Some(OUTPUT_IMAGE))
        }
        "/v1/audio/speech" => Operation::new(OP_GENERATE_CONTENT, Some(OUTPUT_SPEECH)),
        // audio in, text out: the output modality is expressible and the input
        // one is not (see the module docs)
        "/v1/audio/transcriptions" | "/v1/audio/translations" => {
            Operation::new(OP_GENERATE_CONTENT, Some(OUTPUT_TEXT))
        }
        "/v1/rerank" => Operation::new(OP_RERANK, None),
        _ => return None,
    })
}

/// The span name the conventions prescribe: `{operation} {model}`.
///
/// `tracing` span names are compile-time constants, so this is published as the
/// `otel.name` field, which `tracing-opentelemetry` uses to rename the exported
/// span. Without it every inference span exports as `upstream.request` and a
/// backend cannot group by operation.
pub(crate) fn span_name(operation: &Operation, model: &str) -> String {
    if model.is_empty() {
        return operation.name.to_string();
    }
    format!("{} {model}", operation.name)
}

/// The `gen_ai.request.encoding_formats` value for an embeddings request.
///
/// OpenAI's `encoding_format` is a single string; the convention's attribute is
/// a list, and other dialects may send one. Both shapes are read, and anything
/// else yields `None` rather than a stringified blob — an attribute nobody can
/// group by is worse than an absent one.
pub(crate) fn encoding_formats(request: &serde_json::Value) -> Option<String> {
    let value = request.get("encoding_format")?;
    if let Some(one) = value.as_str() {
        return (!one.is_empty()).then(|| one.to_string());
    }
    let mut iter = value
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|v| !v.is_empty());
    let mut joined = iter.next()?.to_string();
    for s in iter {
        joined.push(',');
        joined.push_str(s);
    }
    Some(joined)
}

/// The `error.type` value for a failed upstream attempt.
///
/// The spec asks for "the low-cardinality class of error", so an HTTP failure
/// is reported as its status code and a transport failure by kind. A provider's
/// free-text error message is deliberately not used: it is unbounded
/// cardinality and routinely contains request-specific detail.
pub(crate) fn error_type_for_status(status: u16) -> &'static str {
    match status {
        400 => "400",
        401 => "401",
        403 => "403",
        404 => "404",
        408 => "408",
        409 => "409",
        413 => "413",
        422 => "422",
        429 => "429",
        500 => "500",
        502 => "502",
        503 => "503",
        504 => "504",
        s if (400..500).contains(&s) => "client_error",
        s if (500..600).contains(&s) => "server_error",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tracing` requires span field names to be literals, so several of these
    /// keys are also spelled out at the macro call site in `handlers.rs`. This
    /// module stays the canonical registry — the conventions are still marked
    /// *Development* upstream and `gen_ai.system` -> `gen_ai.provider.name` is
    /// proof they rename things — so this test pins the spellings. A rename
    /// that misses a call site shows up here rather than as a silently
    /// unrecognised span.
    #[test]
    fn the_attribute_registry_matches_the_spec_spellings() {
        assert_eq!(OPERATION_NAME, "gen_ai.operation.name");
        assert_eq!(PROVIDER_NAME, "gen_ai.provider.name");
        assert_eq!(SYSTEM, "gen_ai.system");
        assert_eq!(REQUEST_MODEL, "gen_ai.request.model");
        assert_eq!(RESPONSE_MODEL, "gen_ai.response.model");
        assert_eq!(RESPONSE_FINISH_REASONS, "gen_ai.response.finish_reasons");
        assert_eq!(OUTPUT_TYPE, "gen_ai.output.type");
        assert_eq!(USAGE_INPUT_TOKENS, "gen_ai.usage.input_tokens");
        assert_eq!(USAGE_OUTPUT_TOKENS, "gen_ai.usage.output_tokens");
        assert_eq!(RESPONSE_ID, "gen_ai.response.id");
        assert_eq!(
            EMBEDDINGS_DIMENSION_COUNT,
            "gen_ai.embeddings.dimension.count"
        );
        assert_eq!(REQUEST_ENCODING_FORMATS, "gen_ai.request.encoding_formats");
        assert_eq!(ERROR_TYPE, "error.type");
    }

    #[test]
    fn a_single_encoding_format_string_is_taken_as_written() {
        let request = serde_json::json!({ "encoding_format": "base64" });
        assert_eq!(encoding_formats(&request).as_deref(), Some("base64"));
    }

    /// The attribute is spec'd as a list and `tracing` has no array field type,
    /// so a list arrives joined the same way finish reasons do.
    #[test]
    fn a_list_of_encoding_formats_is_joined() {
        let request = serde_json::json!({ "encoding_format": ["float", "base64"] });
        assert_eq!(encoding_formats(&request).as_deref(), Some("float,base64"));
    }

    /// An attribute nobody can group by is worse than an absent one, so a shape
    /// that is neither a string nor a list of strings reports nothing rather
    /// than a stringified blob.
    #[test]
    fn an_unusable_encoding_format_reports_nothing() {
        for request in [
            serde_json::json!({}),
            serde_json::json!({ "encoding_format": "" }),
            serde_json::json!({ "encoding_format": [] }),
            serde_json::json!({ "encoding_format": 7 }),
            serde_json::json!({ "encoding_format": { "kind": "float" } }),
        ] {
            assert_eq!(encoding_formats(&request), None, "{request}");
        }
    }

    /// The two the spec marks *Required*. Their absence was total failure:
    /// without them a backend does not classify the span as GenAI at all.
    #[test]
    fn the_required_attributes_are_the_new_spellings() {
        assert!(OPERATION_NAME.starts_with("gen_ai."));
        assert_ne!(PROVIDER_NAME, SYSTEM, "provider.name replaced system");
    }

    #[test]
    fn the_chat_dialects_share_one_operation() {
        // openai, anthropic and responses differ in wire format, not in what is
        // being asked of the model — a backend grouping by operation should see
        // one bucket, not three
        for path in ["/v1/chat/completions", "/v1/messages", "/v1/responses"] {
            let op = operation_for_path(path).expect(path);
            assert_eq!(op.name, OP_CHAT, "{path}");
        }
    }

    #[test]
    fn the_modality_routes_declare_their_output_type() {
        assert_eq!(
            operation_for_path("/v1/images/generations")
                .unwrap()
                .output_type,
            Some(OUTPUT_IMAGE)
        );
        assert_eq!(
            operation_for_path("/v1/audio/speech").unwrap().output_type,
            Some(OUTPUT_SPEECH)
        );
        // embeddings have no output modality in the spec's sense
        assert_eq!(
            operation_for_path("/v1/embeddings").unwrap().output_type,
            None
        );
    }

    #[test]
    fn rerank_gets_a_local_value_rather_than_a_borrowed_one() {
        // borrowing `embeddings` would drop reranks into someone's embeddings
        // dashboard; the deviation is deliberate and documented
        let op = operation_for_path("/v1/rerank").unwrap();
        assert_eq!(op.name, OP_RERANK);
    }

    #[test]
    fn non_inference_routes_get_no_operation() {
        for path in ["/healthz", "/metrics", "/v1/models", "/v1/realtime", "/"] {
            assert!(operation_for_path(path).is_none(), "{path}");
        }
    }

    #[test]
    fn a_query_string_or_trailing_slash_does_not_change_the_classification() {
        assert_eq!(
            operation_for_path("/v1/chat/completions/").map(|o| o.name),
            Some(OP_CHAT)
        );
        assert_eq!(
            operation_for_path("/v1/chat/completions?stream=true").map(|o| o.name),
            Some(OP_CHAT)
        );
    }

    #[test]
    fn the_span_name_follows_the_convention() {
        let op = operation_for_path("/v1/chat/completions").unwrap();
        assert_eq!(span_name(&op, "gpt-4o"), "chat gpt-4o");
        // a missing model must not produce a trailing space
        assert_eq!(span_name(&op, ""), "chat");
    }

    #[test]
    fn error_type_is_low_cardinality() {
        assert_eq!(error_type_for_status(429), "429");
        assert_eq!(error_type_for_status(500), "500");
        // an unlisted status collapses into its class rather than becoming a
        // new time series per code
        assert_eq!(error_type_for_status(418), "client_error");
        assert_eq!(error_type_for_status(599), "server_error");
        assert_eq!(error_type_for_status(0), "unknown");
    }
}
