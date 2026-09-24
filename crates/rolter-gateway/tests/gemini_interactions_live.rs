//! Opt-in credentialed smoke test for the `gemini_interactions` dialect (#764).
//!
//! #761 unit-tested the adapter against the interactions wire format *as
//! documented*, which is as far as static verification goes: Google does not
//! publish a full JSON schema for it, so some field names — the multimodal part
//! shape in particular — were inferred from the overview and migration pages.
//!
//! Confirming those against the real endpoint needs a live `GEMINI_API_KEY`,
//! which cannot run in the per-PR gate: `quality.yml` deliberately takes no
//! secrets (#734) so dependabot and fork PRs pass the same checks. These tests
//! are therefore `#[ignore]`d and run from the dispatch-gated
//! `gemini-interactions-smoke` workflow, matching how the OpenRouter and Ollama
//! Cloud live tests are gated.
//!
//! Run locally with:
//!
//! ```text
//! GEMINI_API_KEY=... ROLTER_GEMINI_LIVE_MODEL=gemini-3.6-flash \
//!   cargo test -p rolter-gateway --test gemini_interactions_live -- --ignored
//! ```

use std::net::SocketAddr;

use axum::http::StatusCode;
use axum::Router;
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
};
use serde_json::{json, Value};

const DEFAULT_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// Credentials and model come from the environment so the suite never carries a
/// key and never pins a model that Google may retire.
fn live_env() -> (String, String) {
    let key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY");
    let model = std::env::var("ROLTER_GEMINI_LIVE_MODEL")
        .unwrap_or_else(|_| "gemini-3.6-flash".to_string());
    (key, model)
}

fn live_gateway(key: String, model: String) -> GatewayConfig {
    let provider = ProviderConfig {
        name: "gemini-interactions".into(),
        kind: ProviderKind::GeminiInteractions,
        api_base: std::env::var("ROLTER_GEMINI_API_BASE")
            .unwrap_or_else(|_| DEFAULT_API_BASE.to_string()),
        api_key: Some(key),
        ..Default::default()
    };
    let mut config = GatewayConfig::default();
    config.routes.push(ModelRoute {
        model: "gemini-live".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: provider.name.clone(),
            model: Some(model),
            weight: 1,
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    config.providers = vec![provider];
    config
}

/// Fail with the upstream body rather than a bare status code. A malformed
/// field name comes back as a 400 whose message names the offending field —
/// that message is the entire point of running this test, so it must not be
/// swallowed by `assert_eq!(status, 200)`.
async fn expect_ok(response: reqwest::Response, what: &str) -> Value {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    assert_eq!(
        status,
        StatusCode::OK,
        "{what} failed with {status}; upstream said: {body}"
    );
    serde_json::from_str(&body).unwrap_or(Value::Null)
}

/// Record what the endpoint actually returned.
///
/// A green tick proves the request was accepted, not that the shape matched
/// what the adapter inferred — a field the adapter never reads can be renamed
/// upstream without any assertion noticing. The workflow folds these lines into
/// the job summary so a run leaves evidence behind, which is what makes the
/// next field-name question answerable without spending another billable call.
fn record_shape(what: &str, observed: &Value) {
    println!("SHAPE {what}: {observed}");
}

/// The three client dialects that reach the interactions upstream. Each has its
/// own request translator *and* its own SSE converter
/// (`InteractionsToOpenAi` / `InteractionsToAnthropic` / `InteractionsToResponses`),
/// so a shape confirmed on one says nothing about the other two.
const CLIENT_DIALECTS: &[(&str, &str)] = &[
    ("openai_chat", "/v1/chat/completions"),
    ("anthropic_messages", "/v1/messages"),
    ("openai_responses", "/v1/responses"),
];

/// A minimal single-turn request in each client dialect's own shape.
fn probe_body(dialect: &str, stream: bool) -> Value {
    let mut body = match dialect {
        "anthropic_messages" => json!({
            "model": "gemini-live",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "Say OK"}]
        }),
        "openai_responses" => json!({
            "model": "gemini-live",
            "max_output_tokens": 16,
            "input": "Say OK"
        }),
        _ => json!({
            "model": "gemini-live",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "Say OK"}]
        }),
    };
    if stream {
        body["stream"] = json!(true);
    }
    body
}

/// The documented path: a text turn through the OpenAI dialect. Everything this
/// exercises (turn mapping, `system_instruction`, `generation_config`, usage)
/// follows shapes Google documents, so a failure here is a real regression
/// rather than a field-name discovery.
#[tokio::test]
#[ignore = "requires GEMINI_API_KEY and makes a billable network request"]
async fn live_text_turn_round_trips() {
    let (key, model) = live_env();
    let gateway = serve(rolter_gateway::build_router_from_config(&live_gateway(
        key, model,
    )))
    .await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-live",
            "messages": [
                {"role": "system", "content": "Reply with exactly one word."},
                {"role": "user", "content": "Say OK"}
            ],
            "max_tokens": 8
        }))
        .send()
        .await
        .unwrap();

    let body = expect_ok(response, "text turn").await;
    assert!(
        body["choices"][0]["message"]["content"].is_string(),
        "no assistant content in {body}"
    );
    // usage mapping is documented, so this asserts rather than merely reports
    assert!(
        body["usage"]["prompt_tokens"].is_number(),
        "no usage in {body}"
    );
}

/// Streaming: confirms the SSE converter emits well-formed chunks and
/// terminates. `step.delta` variants are among the shapes #764 flags as
/// inferred, so a body that arrives but yields no content is reported
/// explicitly rather than passing on the 200 alone.
#[tokio::test]
#[ignore = "requires GEMINI_API_KEY and makes a billable network request"]
async fn live_stream_terminates_and_carries_content() {
    let (key, model) = live_env();
    let gateway = serve(rolter_gateway::build_router_from_config(&live_gateway(
        key, model,
    )))
    .await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-live",
            "messages": [{"role": "user", "content": "Count to three."}],
            "max_tokens": 32,
            "stream": true
        }))
        .send()
        .await
        .unwrap();

    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.text().await.unwrap_or_default();
    assert_eq!(status, StatusCode::OK, "stream failed: {body}");
    assert!(
        content_type.contains("text/event-stream"),
        "not an SSE response ({content_type}): {body}"
    );
    assert!(
        body.contains("data: [DONE]"),
        "stream never terminated: {body}"
    );
    // an SSE stream that terminates but carried no delta means the converter
    // did not recognize the event shape — the exact failure #764 is looking for
    assert!(
        body.contains("\"delta\""),
        "stream carried no delta chunks; the step.delta shape may differ: {body}"
    );
}

/// The multimodal path, which is the least certain part of the adapter: the
/// image part field names (`mime_type`/`data` for inline, `file_uri` for a
/// remote URL) were inferred, not read off a schema.
///
/// A 400 here is the finding, not a flake — the assertion message carries the
/// upstream complaint so the corrected field name can go straight into
/// `crates/rolter-proxy/src/translation.rs`.
#[tokio::test]
#[ignore = "requires GEMINI_API_KEY and makes a billable network request"]
async fn live_inline_image_part_is_accepted() {
    let (key, model) = live_env();
    let gateway = serve(rolter_gateway::build_router_from_config(&live_gateway(
        key, model,
    )))
    .await;

    // a 1x1 png, inline as a data URL — the adapter turns this into an inline
    // image part rather than a file reference
    const PIXEL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-live",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "What colour is this image? One word."},
                {"type": "image_url", "image_url": {"url": PIXEL}}
            ]}],
            "max_tokens": 16
        }))
        .send()
        .await
        .unwrap();

    let body = expect_ok(response, "inline image part").await;
    assert!(
        body["choices"][0]["message"]["content"].is_string(),
        "image turn returned no content in {body}"
    );
}

/// A remote-URL image part, which takes the *other* branch of
/// `interaction_image_part`: `file_uri` rather than inline `mime_type`/`data`.
/// Both field names were inferred, and the inline test exercises only one of
/// them, so a `file_uri` rename would have gone unnoticed (#764).
#[tokio::test]
#[ignore = "requires GEMINI_API_KEY and makes a billable network request"]
async fn live_remote_image_part_is_accepted() {
    let (key, model) = live_env();
    let gateway = serve(rolter_gateway::build_router_from_config(&live_gateway(
        key, model,
    )))
    .await;

    // a stable, public, small image; overridable so a run is not hostage to one
    // host staying up
    let url = std::env::var("ROLTER_GEMINI_LIVE_IMAGE_URL").unwrap_or_else(|_| {
        "https://upload.wikimedia.org/wikipedia/commons/thumb/4/47/PNG_transparency_demonstration_1.png/120px-PNG_transparency_demonstration_1.png".to_string()
    });

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-live",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "Describe this image in one word."},
                {"type": "image_url", "image_url": {"url": url}}
            ]}],
            "max_tokens": 16
        }))
        .send()
        .await
        .unwrap();

    let body = expect_ok(response, "remote image part (file_uri)").await;
    record_shape("remote_image_response", &body);
    assert!(
        body["choices"][0]["message"]["content"].is_string(),
        "remote image turn returned no content in {body}"
    );
}

/// The tool path: `function_call` on the way out and `function_result` on the
/// way back. These follow documented shapes, so a failure is a regression
/// rather than a discovery — but nothing exercised them against the real
/// endpoint, and the `call_id` correlation in particular is only meaningful
/// live.
#[tokio::test]
#[ignore = "requires GEMINI_API_KEY and makes a billable network request"]
async fn live_tool_call_round_trips() {
    let (key, model) = live_env();
    let gateway = serve(rolter_gateway::build_router_from_config(&live_gateway(
        key, model,
    )))
    .await;
    let client = reqwest::Client::new();

    let tools = json!([{
        "type": "function",
        "function": {
            "name": "get_weather",
            "description": "Get the current temperature for a city.",
            "parameters": {
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"]
            }
        }
    }]);

    let response = client
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-live",
            "messages": [{"role": "user", "content": "What is the weather in Paris? Use the tool."}],
            "tools": tools,
            "tool_choice": "required",
            "max_tokens": 64
        }))
        .send()
        .await
        .unwrap();

    let body = expect_ok(response, "tool call").await;
    record_shape("tool_call_response", &body);
    let call = &body["choices"][0]["message"]["tool_calls"][0];
    assert!(
        call["function"]["name"].is_string(),
        "no tool call came back; the function_call shape may differ: {body}"
    );
    let call_id = call["id"].as_str().unwrap_or_default().to_string();
    assert!(!call_id.is_empty(), "tool call carried no id: {body}");

    // feed the result back — this is the `function_result` direction, and a
    // mismatched `call_id` field name shows up as the model ignoring the result
    let response = client
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-live",
            "messages": [
                {"role": "user", "content": "What is the weather in Paris? Use the tool."},
                {"role": "assistant", "tool_calls": [call]},
                {"role": "tool", "tool_call_id": call_id, "name": "get_weather", "content": "31C"}
            ],
            "tools": tools,
            "max_tokens": 64
        }))
        .send()
        .await
        .unwrap();

    let body = expect_ok(response, "tool result").await;
    record_shape("tool_result_response", &body);
    assert!(
        body["choices"][0]["message"]["content"].is_string(),
        "tool result turn returned no content: {body}"
    );
}

/// Server-side threading: the interaction id comes back as the response `id`
/// and is sent again as `previous_interaction_id`. Documented, but only a live
/// second turn proves the id rolter surfaces is the one Google will accept.
#[tokio::test]
#[ignore = "requires GEMINI_API_KEY and makes a billable network request"]
async fn live_interaction_id_threads_a_second_turn() {
    let (key, model) = live_env();
    let gateway = serve(rolter_gateway::build_router_from_config(&live_gateway(
        key, model,
    )))
    .await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("http://{gateway}/v1/responses"))
        .json(&json!({
            "model": "gemini-live",
            "input": "My name is Phil. Remember it.",
            "store": true,
            "max_output_tokens": 32
        }))
        .send()
        .await
        .unwrap();
    let first = expect_ok(first, "threaded first turn").await;
    record_shape("threaded_first_turn", &first);
    let id = first["id"].as_str().unwrap_or_default().to_string();
    assert!(
        !id.is_empty(),
        "no interaction id surfaced as response id: {first}"
    );

    let second = client
        .post(format!("http://{gateway}/v1/responses"))
        .json(&json!({
            "model": "gemini-live",
            "previous_response_id": id,
            "input": "What is my name?",
            "max_output_tokens": 32
        }))
        .send()
        .await
        .unwrap();
    let second = expect_ok(second, "threaded second turn").await;
    record_shape("threaded_second_turn", &second);
}

/// Every client dialect, non-streaming. The Anthropic and Responses request
/// translators lower onto the same interactions body, but their *response*
/// translators are separate — a shape confirmed on Chat Completions says
/// nothing about what `interactions_to_anthropic` reads.
#[tokio::test]
#[ignore = "requires GEMINI_API_KEY and makes a billable network request"]
async fn live_every_client_dialect_round_trips() {
    let (key, model) = live_env();
    let gateway = serve(rolter_gateway::build_router_from_config(&live_gateway(
        key, model,
    )))
    .await;
    let client = reqwest::Client::new();

    for (dialect, path) in CLIENT_DIALECTS {
        let response = client
            .post(format!("http://{gateway}{path}"))
            .json(&probe_body(dialect, false))
            .send()
            .await
            .unwrap();
        let body = expect_ok(response, &format!("{dialect} turn")).await;
        record_shape(&format!("{dialect}_response"), &body);

        // each dialect carries assistant text in its own place; an empty one
        // means the response translator did not find what it was looking for
        let text = match *dialect {
            "anthropic_messages" => &body["content"][0]["text"],
            "openai_responses" => &body["output"][0]["content"][0]["text"],
            _ => &body["choices"][0]["message"]["content"],
        };
        assert!(
            text.is_string(),
            "{dialect} response carried no assistant text; \
             its response translator may be reading a renamed field: {body}"
        );
    }
}

/// Every client dialect, streaming. `step.delta` variants are the shapes #764
/// flags as inferred, and each dialect converts them with a different emitter,
/// so all three are exercised rather than Chat Completions alone.
#[tokio::test]
#[ignore = "requires GEMINI_API_KEY and makes a billable network request"]
async fn live_every_client_dialect_streams() {
    let (key, model) = live_env();
    let gateway = serve(rolter_gateway::build_router_from_config(&live_gateway(
        key, model,
    )))
    .await;
    let client = reqwest::Client::new();

    for (dialect, path) in CLIENT_DIALECTS {
        let response = client
            .post(format!("http://{gateway}{path}"))
            .json(&probe_body(dialect, true))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = response.text().await.unwrap_or_default();
        assert_eq!(status, StatusCode::OK, "{dialect} stream failed: {body}");
        assert!(
            content_type.contains("text/event-stream"),
            "{dialect} did not stream ({content_type}): {body}"
        );
        record_shape(&format!("{dialect}_stream"), &json!(body));

        // the terminator differs by dialect: OpenAI-shaped surfaces send
        // `[DONE]`, Anthropic sends a `message_stop` event
        let terminated = match *dialect {
            "anthropic_messages" => body.contains("message_stop"),
            _ => body.contains("data: [DONE]"),
        };
        assert!(terminated, "{dialect} stream never terminated: {body}");

        // a stream that terminates but carried no text means the converter did
        // not recognize the step.delta shape — the exact failure #764 hunts for
        let carried_text = match *dialect {
            "anthropic_messages" => body.contains("content_block_delta"),
            "openai_responses" => body.contains("response.output_text.delta"),
            _ => body.contains("\"delta\""),
        };
        assert!(
            carried_text,
            "{dialect} stream carried no text deltas; \
             the step.delta shape may differ from what the adapter infers: {body}"
        );
    }
}
