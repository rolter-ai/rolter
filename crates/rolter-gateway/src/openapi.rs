//! A served OpenAPI 3.1 description of the gateway's public request surface.
//!
//! Hand-authored (rather than macro-derived) so it stays dependency-free and
//! documents the wire contract rolter presents to clients — the OpenAI/
//! Anthropic-compatible endpoints — not rolter's internal Rust types. Served as
//! JSON at `GET /openapi.json`, and rendered interactively by Scalar at
//! `GET /docs` (assets embedded in the binary — works air-gapped).

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

// spec-ref: openapi
/// Build the OpenAPI document. The version tracks the crate version so a served
/// spec always matches the running binary.
pub fn document() -> Value {
    let error_response = json!({
        "description": "OpenAI-style error",
        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}
    });

    let mut doc = json!({
        "openapi": "3.1.0",
        "info": {
            "title": "rolter gateway",
            "description": "OpenAI/Anthropic-compatible AI gateway and load balancer. \
                Requests are authenticated with a virtual key and routed to an upstream \
                provider selected by the route's balancing strategy.",
            "version": env!("CARGO_PKG_VERSION"),
            "license": {"name": "Apache-2.0"}
        },
        "servers": [{"url": "/", "description": "this gateway"}],
        "security": [{"bearerAuth": []}, {"apiKeyAuth": []}],
        "paths": {
            "/v1/chat/completions": {
                "post": {
                    "summary": "OpenAI chat completions (streaming via \"stream\": true)",
                    "operationId": "createChatCompletion",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ChatCompletionRequest"}}}},
                    "responses": {
                        "200": {"description": "chat completion, or a stream of `chat.completion.chunk` frames ending in `data: [DONE]`", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ChatCompletionResponse"}}, "text/event-stream": {"schema": {"$ref": "#/components/schemas/ChatCompletionChunk"}}}},
                        "400": error_response, "401": error_response, "429": error_response
                    }
                }
            },
            "/v1/completions": {
                "post": {
                    "summary": "OpenAI legacy text completions",
                    "operationId": "createCompletion",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CompletionRequest"}}}},
                    "responses": {"200": {"description": "completion", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CompletionResponse"}}}}, "400": error_response}
                }
            },
            "/v1/responses": {
                "post": {
                    "summary": "OpenAI Responses (provider-native passthrough; streaming supported)",
                    "operationId": "createResponse",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ResponsesRequest"}}}},
                    "responses": {"200": {"description": "response, or a stream of `response.*` events", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ResponseObject"}}, "text/event-stream": {"schema": {"$ref": "#/components/schemas/ResponseStreamEvent"}}}}, "400": error_response, "401": error_response, "501": error_response}
                }
            },
            "/v1/responses/{response_id}": {
                "get": {
                    "summary": "Retrieve a tenant-scoped OpenAI response",
                    "operationId": "retrieveResponse",
                    "parameters": [{"name":"response_id","in":"path","required":true,"schema":{"type":"string"}}],
                    "responses": {"200":{"description":"response","content":{"application/json":{"schema":{"$ref": "#/components/schemas/ResponseObject"}}}},"401":error_response,"404":error_response,"501":error_response}
                },
                "delete": {
                    "summary": "Delete a tenant-scoped OpenAI response",
                    "operationId": "deleteResponse",
                    "parameters": [{"name":"response_id","in":"path","required":true,"schema":{"type":"string"}}],
                    "responses": {"200":{"description":"deletion result","content":{"application/json":{"schema":{"$ref": "#/components/schemas/ResponseDeletion"}}}},"401":error_response,"404":error_response,"501":error_response}
                }
            },
            "/v1/responses/{response_id}/cancel": {
                "post": {
                    "summary": "Cancel a tenant-scoped OpenAI response",
                    "operationId": "cancelResponse",
                    "parameters": [{"name":"response_id","in":"path","required":true,"schema":{"type":"string"}}],
                    "responses": {"200":{"description":"cancelled response","content":{"application/json":{"schema":{"$ref": "#/components/schemas/ResponseObject"}}}},"401":error_response,"404":error_response,"501":error_response}
                }
            },
            "/v1/responses/{response_id}/input_items": {
                "get": {
                    "summary": "List input items for a tenant-scoped OpenAI response",
                    "operationId": "listResponseInputItems",
                    "parameters": [{"name":"response_id","in":"path","required":true,"schema":{"type":"string"}}],
                    "responses": {"200":{"description":"input item list","content":{"application/json":{"schema":{"$ref": "#/components/schemas/ResponseInputItems"}}}},"401":error_response,"404":error_response,"501":error_response}
                }
            },
            "/v1/messages": {
                "post": {
                    "summary": "Anthropic Messages (streaming supported)",
                    "operationId": "createMessage",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/MessagesRequest"}}}},
                    "responses": {"200": {"description": "message, or a stream of `message_*` and `content_block_*` events", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/MessageResponse"}}, "text/event-stream": {"schema": {"$ref": "#/components/schemas/MessageStreamEvent"}}}}, "400": error_response}
                }
            },
            "/v1/embeddings": {
                "post": {
                    "summary": "OpenAI embeddings",
                    "operationId": "createEmbedding",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/EmbeddingRequest"}}}},
                    "responses": {"200": {"description": "embedding list", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/EmbeddingResponse"}}}}, "400": error_response}
                }
            },
            "/v1/rerank": {
                "post": {
                    "summary": "Cohere/Jina rerank",
                    "operationId": "createRerank",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/RerankRequest"}}}},
                    "responses": {"200": {"description": "ranked results", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/RerankResponse"}}}}, "400": error_response}
                }
            },
            "/v1/systemone": {
                "post": {
                    "summary": "System One typed decisions (choice, score and yes/no questions)",
                    "operationId": "createDecision",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/SystemOneRequest"}}}},
                    "responses": {"200": {"description": "one typed answer per question id, plus usage", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/SystemOneResponse"}}}}, "400": error_response}
                }
            },
            "/v1/images/generations": {
                "post": {
                    "summary": "OpenAI image generation",
                    "operationId": "createImage",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ImageGenerationRequest"}}}},
                    "responses": {"200": {"description": "image list", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ImageResponse"}}}}, "400": error_response}
                }
            },
            "/v1/audio/speech": {
                "post": {
                    "summary": "OpenAI text-to-speech (binary audio response)",
                    "operationId": "createSpeech",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/SpeechRequest"}}}},
                    "responses": {"200": {"description": "audio bytes", "content": {"audio/*": {"schema": {"type": "string", "format": "binary"}}}}, "400": error_response}
                }
            },
            "/v1/audio/transcriptions": {
                "post": {
                    "summary": "OpenAI speech-to-text (multipart upload)",
                    "operationId": "createTranscription",
                    "requestBody": {"required": true, "content": {"multipart/form-data": {"schema": {"$ref": "#/components/schemas/AudioUpload"}}}},
                    "responses": {"200": {"description": "transcript", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/TranscriptionResponse"}}, "text/plain": {"schema": {"type": "string"}}}}, "400": error_response}
                }
            },
            "/v1/audio/translations": {
                "post": {
                    "summary": "OpenAI audio translation to English (multipart upload)",
                    "operationId": "createTranslation",
                    "requestBody": {"required": true, "content": {"multipart/form-data": {"schema": {"$ref": "#/components/schemas/AudioUpload"}}}},
                    "responses": {"200": {"description": "translation", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/TranscriptionResponse"}}, "text/plain": {"schema": {"type": "string"}}}}, "400": error_response}
                }
            },
            "/v1/models": {
                "get": {
                    "summary": "List the public model names this gateway serves",
                    "operationId": "listModels",
                    "responses": {"200": {"description": "model list", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ModelList"}}}}}
                }
            },
            "/v1/realtime": {
                "get": {
                    "summary": "OpenAI Realtime WebSocket session",
                    "operationId": "realtimeSession",
                    "parameters": [
                        {"name": "model", "in": "query", "required": true, "schema": {"type": "string"}, "description": "a public model name served by this gateway"}
                    ],
                    "responses": {
                        "101": {"description": "WebSocket upgrade"},
                        "400": error_response, "401": error_response, "402": error_response,
                        "403": error_response, "404": error_response, "429": error_response,
                        "502": error_response, "503": error_response
                    }
                }
            },
            "/mcp/{server}": {
                "parameters": [
                    {"name":"server","in":"path","required":true,"schema":{"type":"string"},"description":"organization-scoped MCP server slug"}
                ],
                "get": {
                    "summary": "Open an authenticated MCP event stream",
                    "operationId": "mcpRootGet",
                    "responses": {"200":{"description":"downstream MCP response or SSE stream"},"401":error_response,"403":error_response,"404":error_response,"502":error_response}
                },
                "post": {
                    "summary": "Send an authenticated MCP message",
                    "operationId": "mcpRootPost",
                    "requestBody": {"content":{"application/json":{"schema":{"type":"object"}}}},
                    "responses": {"200":{"description":"downstream MCP response or SSE stream"},"401":error_response,"403":error_response,"404":error_response,"502":error_response}
                },
                "delete": {
                    "summary": "Close a downstream MCP session or resource",
                    "operationId": "mcpRootDelete",
                    "responses": {"200":{"description":"downstream MCP response"},"401":error_response,"403":error_response,"404":error_response,"502":error_response}
                }
            },
            "/mcp/{server}/{path}": {
                "parameters": [
                    {"name":"server","in":"path","required":true,"schema":{"type":"string"},"description":"organization-scoped MCP server slug"},
                    {"name":"path","in":"path","required":true,"schema":{"type":"string"},"description":"optional downstream MCP path; omit the segment to call the registered base URL"}
                ],
                "get": {
                    "summary": "Open an authenticated MCP event stream",
                    "operationId": "mcpGet",
                    "responses": {"200":{"description":"downstream MCP response or SSE stream"},"401":error_response,"403":error_response,"404":error_response,"502":error_response}
                },
                "post": {
                    "summary": "Send an authenticated MCP message",
                    "operationId": "mcpPost",
                    "requestBody": {"content":{"application/json":{"schema":{"type":"object"}}}},
                    "responses": {"200":{"description":"downstream MCP response or SSE stream"},"401":error_response,"403":error_response,"404":error_response,"502":error_response}
                },
                "delete": {
                    "summary": "Close a downstream MCP session or resource",
                    "operationId": "mcpDelete",
                    "responses": {"200":{"description":"downstream MCP response"},"401":error_response,"403":error_response,"404":error_response,"502":error_response}
                }
            },
            "/healthz": {
                "get": {"summary": "Liveness probe", "operationId": "healthz", "security": [], "responses": {"200": {"description": "ok"}}}
            }
        },
        "components": {
            "securitySchemes": {
                "bearerAuth": {"type": "http", "scheme": "bearer", "description": "virtual key as `Authorization: Bearer <key>`"},
                "apiKeyAuth": {"type": "apiKey", "in": "header", "name": "x-api-key", "description": "virtual key as `x-api-key: <key>`"}
            },
            "schemas": {
                "AudioUpload": {
                    "type": "object",
                    "required": ["model", "file"],
                    "properties": {
                        "model": {"type": "string"},
                        "file": {"type": "string", "format": "binary"},
                        "response_format": {"type": "string", "enum": ["json", "text", "verbose_json", "srt", "vtt"]}
                    }
                },
                "Error": {
                    "type": "object",
                    "properties": {
                        "error": {
                            "type": "object",
                            "properties": {
                                "message": {"type": "string"},
                                "type": {"type": "string"},
                                "code": {"type": "string"},
                                "param": {"type": "string"}
                            }
                        }
                    }
                }
            }
        }
    });
    // request and response schemas, built apart for the same reason
    if let Some(schemas) = doc["components"]["schemas"].as_object_mut() {
        schemas.extend(crate::openapi_schemas::all());
    }
    // routed and answered 501 (`unsupported_response_lifecycle`); built apart
    // because the literal above sits at `json!`'s recursion limit
    for (path, method, id, summary) in [
        (
            "/v1/responses/{response_id}/compact",
            "post",
            "compactResponse",
            "Compact a response (routed, not implemented: 501 after authentication)",
        ),
        (
            "/v1/responses/{response_id}/input_tokens",
            "get",
            "getResponseInputTokens",
            "Count a response's input tokens (routed, not implemented: 501 after authentication)",
        ),
    ] {
        doc["paths"][path] = json!({
            method: {
                "summary": summary,
                "operationId": id,
                "parameters": [{"name": "response_id", "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": {"401": error_response, "501": error_response}
            }
        });
    }
    // provenance comes from the specification registry rather than being
    // written here, so a revision or url has one source (#907)
    rolter_core::spec_registry::annotate_openapi(
        &mut doc,
        rolter_core::spec_registry::Plane::Gateway,
    );
    doc
}

/// `GET /openapi.json` — serve the OpenAPI document.
pub async fn openapi_json() -> Response {
    Json(document()).into_response()
}

/// `GET /docs` — interactive Scalar API reference rendering [`document`].
///
/// Fully self-contained for air-gapped deployments: the Scalar JS bundle is
/// embedded in the binary (served from [`docs_bundle`], never a CDN) and
/// `withDefaultFonts: false` keeps the page from reaching for
/// `fonts.scalar.com`; no proxy is configured, so the reference makes no
/// external requests at all.
pub async fn docs() -> Response {
    let config = json!({
        "url": "/openapi.json",
        "withDefaultFonts": false,
    });
    let html = scalar_api_reference::scalar_html(&config, Some(DOCS_BUNDLE_PATH));
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

/// Where the embedded Scalar bundle is mounted; referenced by the `/docs` page.
pub const DOCS_BUNDLE_PATH: &str = "/docs/scalar.js";

/// `GET /docs/scalar.js` — the Scalar JS bundle, embedded at compile time.
pub async fn docs_bundle() -> Response {
    match scalar_api_reference::get_asset_with_mime("scalar.js") {
        Some((mime, bytes)) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /` — a small service-info landing so the root is never a bare 404.
/// JSON (not HTML) keeps it machine-friendly for probes and humans alike.
pub async fn root() -> Response {
    Json(json!({
        "service": "rolter-gateway",
        "version": env!("CARGO_PKG_VERSION"),
        "docs": "/docs",
        "openapi": "/openapi.json",
        "health": "/healthz",
        "models": "/v1/models",
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_is_well_formed() {
        let doc = document();
        assert_eq!(doc["openapi"], "3.1.0");
        assert_eq!(doc["info"]["version"], env!("CARGO_PKG_VERSION"));
        // every shipped endpoint is described
        for path in [
            "/v1/chat/completions",
            "/v1/completions",
            "/v1/responses",
            "/v1/responses/{response_id}/compact",
            "/v1/responses/{response_id}/input_tokens",
            "/v1/messages",
            "/v1/embeddings",
            "/v1/rerank",
            "/v1/systemone",
            "/v1/images/generations",
            "/v1/audio/speech",
            "/v1/audio/transcriptions",
            "/v1/audio/translations",
            "/v1/models",
            "/v1/realtime",
            "/mcp/{server}",
            "/mcp/{server}/{path}",
            "/healthz",
        ] {
            assert!(doc["paths"][path].is_object(), "missing path {path}");
        }
        // security schemes are declared and referenced
        assert!(doc["components"]["securitySchemes"]["bearerAuth"].is_object());
        assert!(doc["components"]["schemas"]["Error"].is_object());
    }

    /// `METHOD /path` for every route `build_router` registers, read from its
    /// source: axum keeps no list a test could ask for. Paths are written in
    /// OpenAPI form (`{*path}` becomes `{path}`), and the three routes whose
    /// path is a constant are resolved by name.
    fn routed_endpoints() -> std::collections::BTreeSet<String> {
        let source = include_str!("lib.rs");
        let after = source
            .split_once("pub fn build_router(")
            .expect("build_router is in lib.rs")
            .1;
        // only the function's own body: a router built later in the file (a
        // test's, say) must not read as gateway routes. the path templates are
        // balanced braces, so counting them finds the body's closing one
        let open = after.find('{').expect("build_router has a body");
        let mut depth = 0;
        let close = after[open..]
            .char_indices()
            .find_map(|(i, c)| {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(open + i)
            })
            .expect("build_router's body closes");
        let body = &after[open..close];
        let mut endpoints = std::collections::BTreeSet::new();
        let mut rest = body;
        while let Some(at) = rest.find(".route(") {
            rest = &rest[at + ".route(".len()..];
            // the call's arguments, up to its matching parenthesis
            let mut depth = 1;
            let end = rest
                .char_indices()
                .find_map(|(i, c)| {
                    match c {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    (depth == 0).then_some(i)
                })
                .expect("a balanced .route( call");
            let call = &rest[..end];
            let (path, handlers) = call.split_once(',').expect("route(path, handler)");
            let path = match path.trim() {
                "metrics_path" => "/metrics".to_string(),
                "openapi::DOCS_BUNDLE_PATH" => DOCS_BUNDLE_PATH.to_string(),
                "rolter_core::upstream::SYSTEM_ONE_PATH" => {
                    rolter_core::upstream::SYSTEM_ONE_PATH.to_string()
                }
                literal if literal.starts_with('"') => literal.trim_matches('"').replace("{*", "{"),
                other => panic!("route path `{other}` is not resolved by this test"),
            };
            let handlers: String = handlers.split_whitespace().collect();
            for method in ["get", "post", "put", "patch", "delete"] {
                let called = handlers.starts_with(&format!("{method}("))
                    || handlers.contains(&format!(".{method}("));
                if called {
                    endpoints.insert(format!("{} {path}", method.to_ascii_uppercase()));
                }
            }
            rest = &rest[end..];
        }
        endpoints
    }

    // #907: every gateway route is either covered by a specification record
    // or listed as rolter's own, and no record claims a route that is gone
    #[test]
    fn every_route_is_in_the_specification_registry() {
        use rolter_core::spec_registry::{parse_endpoint, registry};
        let registry = registry().expect("the registry parses");
        let routed = routed_endpoints();
        assert!(routed.contains("POST /v1/chat/completions"), "{routed:?}");
        assert!(routed.contains("DELETE /mcp/{server}/{path}"), "{routed:?}");

        let mut claimed = std::collections::BTreeSet::new();
        for endpoint in registry
            .specs
            .iter()
            .flat_map(|spec| spec.gateway_endpoints.iter())
            .chain(&registry.native.gateway_endpoints)
        {
            let (method, path) = parse_endpoint(endpoint).expect("validated");
            claimed.insert(format!("{method} {path}"));
        }
        let uncovered: Vec<&String> = routed.difference(&claimed).collect();
        assert!(
            uncovered.is_empty(),
            "routes with no record in crates/rolter-core/specifications.toml: {uncovered:?}"
        );
        let stale: Vec<&String> = claimed.difference(&routed).collect();
        assert!(
            stale.is_empty(),
            "records claim routes that do not exist: {stale:?}"
        );
    }

    #[test]
    fn every_documented_operation_carries_its_provenance() {
        let registry = rolter_core::spec_registry::registry().expect("the registry parses");
        let doc = document();
        assert_eq!(
            doc["externalDocs"]["url"],
            rolter_core::spec_registry::REGISTRY_URL
        );
        assert_eq!(doc["x-rolter-spec-refs"], json!(["openapi"]));
        let paths = doc["paths"].as_object().expect("paths");
        for (path, item) in paths {
            for (method, operation) in item.as_object().expect("path item") {
                if method == "parameters" {
                    continue;
                }
                let endpoint = format!("{} {path}", method.to_ascii_uppercase());
                if registry.native.gateway_endpoints.contains(&endpoint) {
                    assert!(operation.get("x-rolter-spec-refs").is_none(), "{endpoint}");
                    continue;
                }
                let refs = operation["x-rolter-spec-refs"]
                    .as_array()
                    .unwrap_or_else(|| panic!("{endpoint} has no x-rolter-spec-refs"));
                for id in refs {
                    let id = id.as_str().expect("an id");
                    let spec = registry.spec(id).expect("a recorded id");
                    assert_eq!(
                        operation["x-rolter-conformance"][id],
                        spec.conformance.as_str()
                    );
                    assert_eq!(operation["x-rolter-reviewed-at"][id], spec.reviewed_at);
                }
                let url = operation["externalDocs"]["url"]
                    .as_str()
                    .expect("externalDocs");
                assert!(url.starts_with("https://"), "{endpoint}: {url}");
            }
        }
        let chat = &doc["paths"]["/v1/chat/completions"]["post"];
        assert_eq!(
            chat["x-rolter-spec-refs"],
            json!(["openai-chat-completions", "sse"])
        );
        assert_eq!(
            chat["externalDocs"]["url"],
            registry
                .spec("openai-chat-completions")
                .expect("recorded")
                .url
        );
        assert_eq!(
            doc["components"]["schemas"]["Error"]["x-rolter-spec-refs"],
            json!(["openai-errors"])
        );
    }

    // #3335: a request body and a 200 body of the model endpoints name a
    // schema, every schema is open (bodies are forwarded) and carries the
    // provenance of the record that owns it
    #[test]
    fn model_endpoint_bodies_reference_open_schemas_with_provenance() {
        let doc = document();
        let schemas = doc["components"]["schemas"].as_object().expect("schemas");
        let mut referenced = std::collections::BTreeSet::new();
        for (path, item) in doc["paths"].as_object().expect("paths") {
            if path.starts_with("/mcp/") || path == "/healthz" || path == "/v1/realtime" {
                continue;
            }
            for (method, operation) in item.as_object().expect("path item") {
                if method == "parameters" {
                    continue;
                }
                let request = operation["requestBody"]["content"].as_object();
                let ok = operation["responses"]["200"]["content"].as_object();
                for content in request.into_iter().chain(ok) {
                    for (media, body) in content {
                        if media.starts_with("audio/") || media == "text/plain" {
                            continue;
                        }
                        let target = body["schema"]["$ref"].as_str().unwrap_or_else(|| {
                            panic!("{method} {path} {media} has no named schema")
                        });
                        let name = target.trim_start_matches("#/components/schemas/");
                        assert!(schemas.contains_key(name), "{target} is not defined");
                        referenced.insert(name.to_string());
                    }
                }
            }
        }
        for name in &referenced {
            let schema = &schemas[name];
            if name == "AudioUpload" {
                continue;
            }
            assert_eq!(
                schema["additionalProperties"], true,
                "{name} must stay open"
            );
            assert!(
                schema["x-rolter-spec-refs"].is_array(),
                "{name} is not listed under a record's gateway_schemas"
            );
        }
        // no schema is defined that nothing uses, apart from the ones those
        // reference
        for name in schemas.keys() {
            assert!(
                referenced.contains(name) || ["Error", "Usage"].contains(&name.as_str()),
                "{name} is unreferenced"
            );
        }
    }

    // #3335: the stream schemas name the event families rolter emits or
    // translates, and no event it never produces
    #[test]
    fn stream_schemas_name_only_the_event_families_rolter_produces() {
        let doc = document();
        let schemas = &doc["components"]["schemas"];
        assert_eq!(
            schemas["ChatCompletionChunk"]["properties"]["object"]["const"],
            "chat.completion.chunk"
        );
        let anthropic: Vec<&str> = schemas["MessageStreamEvent"]["properties"]["type"]["enum"]
            .as_array()
            .expect("enum")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(
            anthropic,
            [
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop",
                "error",
                "ping"
            ]
        );
        let responses = schemas["ResponseStreamEvent"]["description"]
            .as_str()
            .expect("description");
        for event in [
            "response.created",
            "response.output_text.delta",
            "response.function_call_arguments.done",
            "response.completed",
        ] {
            assert!(responses.contains(event), "{event}");
        }
        assert!(!responses.contains("response.reasoning"), "{responses}");
    }

    #[test]
    fn realtime_documents_the_upgrade_and_admission_failures() {
        let doc = document();
        let operation = &doc["paths"]["/v1/realtime"]["get"];
        assert_eq!(operation["parameters"][0]["name"], "model");
        assert_eq!(operation["parameters"][0]["in"], "query");
        assert_eq!(operation["parameters"][0]["required"], true);
        for status in [
            "101", "400", "401", "402", "403", "404", "429", "502", "503",
        ] {
            assert!(
                operation["responses"][status].is_object(),
                "missing {status}"
            );
        }
    }

    #[test]
    fn docs_page_is_self_contained() {
        let config = json!({"url": "/openapi.json", "withDefaultFonts": false});
        let html = scalar_api_reference::scalar_html(&config, Some(DOCS_BUNDLE_PATH));
        // the bundle is loaded from this gateway, never a cdn (air-gapped)
        assert!(html.contains(DOCS_BUNDLE_PATH));
        assert!(!html.contains("cdn.jsdelivr.net"));
        assert!(html.contains("withDefaultFonts"));
    }

    #[test]
    fn scalar_bundle_is_embedded() {
        let (mime, bytes) = scalar_api_reference::get_asset_with_mime("scalar.js")
            .expect("scalar.js embedded in the binary");
        assert_eq!(mime, "application/javascript");
        assert!(!bytes.is_empty());
    }
}
