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

/// Build the OpenAPI document. The version tracks the crate version so a served
/// spec always matches the running binary.
pub fn document() -> Value {
    let error_response = json!({
        "description": "OpenAI-style error",
        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}
    });

    json!({
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
                        "200": {"description": "chat completion or an SSE stream", "content": {"application/json": {"schema": {"type": "object"}}, "text/event-stream": {"schema": {"type": "string"}}}},
                        "400": error_response, "401": error_response, "429": error_response
                    }
                }
            },
            "/v1/completions": {
                "post": {
                    "summary": "OpenAI legacy text completions",
                    "operationId": "createCompletion",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "required": ["model"], "properties": {"model": {"type": "string"}, "prompt": {"type": "string"}}}}}},
                    "responses": {"200": {"description": "completion", "content": {"application/json": {"schema": {"type": "object"}}}}, "400": error_response}
                }
            },
            "/v1/responses": {
                "post": {
                    "summary": "OpenAI Responses (provider-native passthrough; streaming supported)",
                    "operationId": "createResponse",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "required": ["model"], "properties": {"model": {"type": "string"}, "input": {} , "stream": {"type": "boolean"}}}}}},
                    "responses": {"200": {"description": "response or an SSE event stream", "content": {"application/json": {"schema": {"type": "object"}}, "text/event-stream": {"schema": {"type": "string"}}}}, "400": error_response, "401": error_response, "501": error_response}
                }
            },
            "/v1/responses/{response_id}": {
                "get": {
                    "summary": "Retrieve a tenant-scoped OpenAI response",
                    "operationId": "retrieveResponse",
                    "parameters": [{"name":"response_id","in":"path","required":true,"schema":{"type":"string"}}],
                    "responses": {"200":{"description":"response","content":{"application/json":{"schema":{"type":"object"}}}},"401":error_response,"404":error_response,"501":error_response}
                },
                "delete": {
                    "summary": "Delete a tenant-scoped OpenAI response",
                    "operationId": "deleteResponse",
                    "parameters": [{"name":"response_id","in":"path","required":true,"schema":{"type":"string"}}],
                    "responses": {"200":{"description":"deletion result","content":{"application/json":{"schema":{"type":"object"}}}},"401":error_response,"404":error_response,"501":error_response}
                }
            },
            "/v1/responses/{response_id}/cancel": {
                "post": {
                    "summary": "Cancel a tenant-scoped OpenAI response",
                    "operationId": "cancelResponse",
                    "parameters": [{"name":"response_id","in":"path","required":true,"schema":{"type":"string"}}],
                    "responses": {"200":{"description":"cancelled response","content":{"application/json":{"schema":{"type":"object"}}}},"401":error_response,"404":error_response,"501":error_response}
                }
            },
            "/v1/responses/{response_id}/input_items": {
                "get": {
                    "summary": "List input items for a tenant-scoped OpenAI response",
                    "operationId": "listResponseInputItems",
                    "parameters": [{"name":"response_id","in":"path","required":true,"schema":{"type":"string"}}],
                    "responses": {"200":{"description":"input item list","content":{"application/json":{"schema":{"type":"object"}}}},"401":error_response,"404":error_response,"501":error_response}
                }
            },
            "/v1/messages": {
                "post": {
                    "summary": "Anthropic Messages (streaming supported)",
                    "operationId": "createMessage",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "required": ["model", "messages"], "properties": {"model": {"type": "string"}, "max_tokens": {"type": "integer"}, "messages": {"type": "array", "items": {"type": "object"}}}}}}},
                    "responses": {"200": {"description": "message or an SSE stream", "content": {"application/json": {"schema": {"type": "object"}}, "text/event-stream": {"schema": {"type": "string"}}}}, "400": error_response}
                }
            },
            "/v1/embeddings": {
                "post": {
                    "summary": "OpenAI embeddings",
                    "operationId": "createEmbedding",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "required": ["model", "input"], "properties": {"model": {"type": "string"}, "input": {"oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]}}}}}},
                    "responses": {"200": {"description": "embedding list", "content": {"application/json": {"schema": {"type": "object"}}}}, "400": error_response}
                }
            },
            "/v1/rerank": {
                "post": {
                    "summary": "Cohere/Jina rerank",
                    "operationId": "createRerank",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "required": ["model", "query", "documents"], "properties": {"model": {"type": "string"}, "query": {"type": "string"}, "documents": {"type": "array", "items": {"type": "string"}}, "top_n": {"type": "integer"}, "return_documents": {"type": "boolean"}}}}}},
                    "responses": {"200": {"description": "ranked results", "content": {"application/json": {"schema": {"type": "object"}}}}, "400": error_response}
                }
            },
            "/v1/images/generations": {
                "post": {
                    "summary": "OpenAI image generation",
                    "operationId": "createImage",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "required": ["model", "prompt"], "properties": {"model": {"type": "string"}, "prompt": {"type": "string"}, "n": {"type": "integer"}, "response_format": {"type": "string", "enum": ["url", "b64_json"]}}}}}},
                    "responses": {"200": {"description": "image list", "content": {"application/json": {"schema": {"type": "object"}}}}, "400": error_response}
                }
            },
            "/v1/audio/speech": {
                "post": {
                    "summary": "OpenAI text-to-speech (binary audio response)",
                    "operationId": "createSpeech",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "required": ["model", "input"], "properties": {"model": {"type": "string"}, "input": {"type": "string"}, "voice": {"type": "string"}}}}}},
                    "responses": {"200": {"description": "audio bytes", "content": {"audio/*": {"schema": {"type": "string", "format": "binary"}}}}, "400": error_response}
                }
            },
            "/v1/audio/transcriptions": {
                "post": {
                    "summary": "OpenAI speech-to-text (multipart upload)",
                    "operationId": "createTranscription",
                    "requestBody": {"required": true, "content": {"multipart/form-data": {"schema": {"$ref": "#/components/schemas/AudioUpload"}}}},
                    "responses": {"200": {"description": "transcript", "content": {"application/json": {"schema": {"type": "object"}}, "text/plain": {"schema": {"type": "string"}}}}, "400": error_response}
                }
            },
            "/v1/audio/translations": {
                "post": {
                    "summary": "OpenAI audio translation to English (multipart upload)",
                    "operationId": "createTranslation",
                    "requestBody": {"required": true, "content": {"multipart/form-data": {"schema": {"$ref": "#/components/schemas/AudioUpload"}}}},
                    "responses": {"200": {"description": "translation", "content": {"application/json": {"schema": {"type": "object"}}, "text/plain": {"schema": {"type": "string"}}}}, "400": error_response}
                }
            },
            "/v1/models": {
                "get": {
                    "summary": "List the public model names this gateway serves",
                    "operationId": "listModels",
                    "responses": {"200": {"description": "model list", "content": {"application/json": {"schema": {"type": "object"}}}}}
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
                "ChatCompletionRequest": {
                    "type": "object",
                    "required": ["model", "messages"],
                    "properties": {
                        "model": {"type": "string", "description": "a public model name served by this gateway"},
                        "messages": {"type": "array", "items": {"type": "object"}},
                        "stream": {"type": "boolean", "default": false},
                        "temperature": {"type": "number"},
                        "max_tokens": {"type": "integer"}
                    }
                },
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
    })
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
            "/v1/messages",
            "/v1/embeddings",
            "/v1/rerank",
            "/v1/images/generations",
            "/v1/audio/speech",
            "/v1/audio/transcriptions",
            "/v1/audio/translations",
            "/v1/models",
            "/healthz",
        ] {
            assert!(doc["paths"][path].is_object(), "missing path {path}");
        }
        // security schemes are declared and referenced
        assert!(doc["components"]["securitySchemes"]["bearerAuth"].is_object());
        assert!(doc["components"]["schemas"]["Error"].is_object());
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
