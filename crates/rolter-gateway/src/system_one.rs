//! System One typed decisions on the gateway (#3150).
//!
//! spec-ref: system-one
//!
//! A decision model answers typed questions (choice, score, yes/no) about a
//! `state` instead of generating text. Every server rolter fronts for it speaks
//! one endpoint, `POST /v1/systemone`, and the request travels through the same
//! proxy pipeline as `/v1/rerank`: virtual keys, budgets, rate limits, routing
//! on `model`, balancing, usage and cost. Nothing here translates the body.
//!
//! What this module adds is the boundary between decisions and everything
//! else. A decision model cannot complete, embed or call tools, and a hosted
//! chat api has no decision endpoint, so a request that crosses that line is
//! refused here with a pointer to the right endpoint rather than forwarded for
//! the upstream to answer `404`.

use std::collections::HashMap;

use axum::http::StatusCode;
use rolter_core::upstream::SYSTEM_ONE_PATH;
use rolter_core::{ModelRoute, ProviderConfig};

use crate::error::ApiError;

/// Why `route` cannot serve a request for `path`, judged by the kinds of the
/// providers its targets reach, or `None` when it can.
///
/// The verdict is about the route as a whole: decisions need at least one
/// target that serves them, and every other endpoint needs at least one target
/// that is not decision-only. A route that mixes the two is left to balance,
/// since refusing it would also refuse the requests its other targets answer.
/// A target whose provider is missing from the snapshot says nothing either
/// way; the forwarder reports it as it does today.
pub(crate) fn endpoint_refusal(
    path: &str,
    route: &ModelRoute,
    providers: &HashMap<String, ProviderConfig>,
) -> Option<ApiError> {
    let mut kinds = route
        .every_target()
        .filter_map(|target| providers.get(&target.provider))
        .map(|provider| provider.kind)
        .peekable();
    kinds.peek()?;
    let model = &route.model;
    if path == SYSTEM_ONE_PATH {
        if kinds.any(|kind| kind.serves_decisions()) {
            return None;
        }
        return Some(
            ApiError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "model '{model}' is not a decision model: {SYSTEM_ONE_PATH} is served by \
                     system_one providers"
                ),
            )
            .with_code("unsupported_endpoint")
            .with_param("model"),
        );
    }
    if kinds.all(|kind| kind.decision_only()) {
        return Some(
            ApiError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "model '{model}' is a decision model and only answers POST {SYSTEM_ONE_PATH}"
                ),
            )
            .with_code("unsupported_endpoint")
            .with_param("model"),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::ProviderKind;
    use serde_json::json;

    fn providers(kinds: &[(&str, ProviderKind)]) -> HashMap<String, ProviderConfig> {
        kinds
            .iter()
            .map(|(name, kind)| {
                (
                    name.to_string(),
                    ProviderConfig {
                        name: name.to_string(),
                        kind: *kind,
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    fn route(targets: &[&str]) -> ModelRoute {
        let targets: Vec<_> = targets.iter().map(|p| json!({ "provider": p })).collect();
        serde_json::from_value(json!({ "model": "jev", "targets": targets })).expect("a route")
    }

    fn refused(
        path: &str,
        route: &ModelRoute,
        providers: &HashMap<String, ProviderConfig>,
    ) -> bool {
        endpoint_refusal(path, route, providers).is_some()
    }

    #[test]
    fn a_decision_route_answers_only_decisions() {
        let providers = providers(&[("typesafe", ProviderKind::SystemOne)]);
        let route = route(&["typesafe"]);
        assert!(!refused(SYSTEM_ONE_PATH, &route, &providers));
        for path in [
            "/v1/chat/completions",
            "/v1/messages",
            "/v1/embeddings",
            "/v1/rerank",
        ] {
            assert!(refused(path, &route, &providers), "{path}");
        }
    }

    #[test]
    fn a_chat_api_is_not_sent_decisions() {
        let providers = providers(&[
            ("anthropic", ProviderKind::Anthropic),
            ("gemini", ProviderKind::GeminiInteractions),
        ]);
        let route = route(&["anthropic", "gemini"]);
        assert!(refused(SYSTEM_ONE_PATH, &route, &providers));
        assert!(!refused("/v1/chat/completions", &route, &providers));
    }

    /// an openai-compatible server or an ollama daemon may serve both, so
    /// neither endpoint is refused for them
    #[test]
    fn a_self_hosted_server_may_answer_either() {
        for kind in [ProviderKind::OpenaiCompatible, ProviderKind::Ollama] {
            let providers = providers(&[("local", kind)]);
            let route = route(&["local"]);
            assert!(!refused(SYSTEM_ONE_PATH, &route, &providers), "{kind:?}");
            assert!(
                !refused("/v1/chat/completions", &route, &providers),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_mixed_route_is_left_to_balance() {
        let providers = providers(&[
            ("typesafe", ProviderKind::SystemOne),
            ("openai", ProviderKind::Openai),
        ]);
        let route = route(&["typesafe", "openai"]);
        assert!(!refused(SYSTEM_ONE_PATH, &route, &providers));
        assert!(!refused("/v1/chat/completions", &route, &providers));
    }

    #[test]
    fn variant_targets_count() {
        let providers = providers(&[("typesafe", ProviderKind::SystemOne)]);
        let route: ModelRoute = serde_json::from_value(json!({
            "model": "jev",
            "variants": [{ "name": "a", "targets": [{ "provider": "typesafe" }] }],
        }))
        .expect("a route");
        assert!(refused("/v1/chat/completions", &route, &providers));
        assert!(!refused(SYSTEM_ONE_PATH, &route, &providers));
    }

    #[test]
    fn unknown_providers_say_nothing() {
        let route = route(&["gone"]);
        assert!(!refused(SYSTEM_ONE_PATH, &route, &HashMap::new()));
        assert!(!refused("/v1/chat/completions", &route, &HashMap::new()));
    }

    #[test]
    fn the_refusal_names_the_endpoint_to_call() {
        let providers = providers(&[("typesafe", ProviderKind::SystemOne)]);
        let error = endpoint_refusal("/v1/chat/completions", &route(&["typesafe"]), &providers)
            .expect("refused");
        let body = error.body();
        let message = body["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("/v1/systemone"), "{body}");
        assert_eq!(body["error"]["code"], "unsupported_endpoint");
        assert_eq!(body["error"]["param"], "model");
    }
}
