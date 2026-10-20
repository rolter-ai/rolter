//! Where to reach a provider to find out whether it is alive.
//!
//! Both planes need this and they must agree. The gateway sweeps providers for
//! the health screen; the control plane answers "Test connection" when an
//! operator is configuring one. If the two built the URL separately they would
//! drift the moment a provider kind was added, and an operator would get a green
//! test against an endpoint the health sweep never probes — the same failure
//! mode as the balancing-strategy mapping in #930, which fell behind twice.
//!
//! Every endpoint here is a **free, non-inference** call. A liveness check that
//! bills the operator is one they will turn off.

use crate::ProviderKind;

/// The anthropic messages api rejects requests without a version header, even on
/// the free `GET /v1/models` list endpoint.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Resolve the probe request for a provider: the URL and any header the upstream
/// requires.
///
/// When the operator left `health.path` at its default (`/`), probe the provider
/// kind's free liveness endpoint (`/v1/models` — a list call that burns no
/// tokens) so a healthy result means the API itself is up, not merely that the
/// host answers TCP. An explicit non-default `path` is honoured verbatim for
/// every provider.
pub fn probe_request(
    kind: ProviderKind,
    api_base: &str,
    configured_path: &str,
) -> (String, Vec<(String, String)>) {
    let base = api_base.trim_end_matches('/');
    if configured_path != "/" {
        return (format!("{base}{configured_path}"), Vec::new());
    }
    match kind {
        ProviderKind::Anthropic => (
            format!("{base}/v1/models"),
            vec![(
                "anthropic-version".to_string(),
                ANTHROPIC_VERSION.to_string(),
            )],
        ),
        ProviderKind::Tei => (format!("{base}/health"), Vec::new()),
        ProviderKind::Bedrock => (bedrock_models_url(base), Vec::new()),
        ProviderKind::Vertex => (vertex_models_url(base), Vec::new()),
        ProviderKind::Openai
        | ProviderKind::OpenaiCompatible
        | ProviderKind::Ollama
        | ProviderKind::OllamaCloud
        | ProviderKind::LlamaCpp => (format!("{base}/v1/models"), Vec::new()),
        _ => (format!("{base}/models"), Vec::new()),
    }
}

/// What a 2xx from [`probe_request`] is actually evidence of.
///
/// A status code alone is a weak verdict: a catch-all route, an SPA index, a
/// reverse proxy or an auth portal all answer 200 at an arbitrary path. When
/// the probe asks for a model catalogue, the catalogue is the evidence — the
/// status is only the envelope it arrived in (#980).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeExpectation {
    /// The probe lists models. A 2xx whose body is not a catalogue answered,
    /// but did not demonstrate that inference will work.
    Catalogue,
    /// The probe is a liveness endpoint with no catalogue to parse, so 2xx is
    /// the whole verdict — deliberately, not incidentally.
    Liveness,
}

/// What the probe for `kind` proves when it answers 2xx.
///
/// Only kinds whose probe URL is genuinely a liveness endpoint are
/// [`ProbeExpectation::Liveness`]; everything else asks for a catalogue, so a
/// new provider kind defaults to the stricter verdict rather than the weaker
/// one.
///
/// An operator who overrode `health.path` gets `Liveness`: the probe is then
/// whatever endpoint they nominated, and rolter has no basis to expect a
/// catalogue from it.
pub fn probe_expectation(kind: ProviderKind, configured_path: &str) -> ProbeExpectation {
    if configured_path != "/" {
        return ProbeExpectation::Liveness;
    }
    match kind {
        // tei serves embeddings; its probe is `/health`, which is a bare 200
        ProviderKind::Tei => ProbeExpectation::Liveness,
        _ => ProbeExpectation::Catalogue,
    }
}

/// Count the models in a probe response body, when it is a catalogue at all.
///
/// Returns `None` when the body is not a model list — an HTML page, a JSON
/// object with no model array, or anything else an endpoint that is not a
/// catalogue might answer with. That `None` is the whole point: it is what
/// separates "answered" from "answered with a model list".
///
/// The recognised shapes are the ones rolter's own probe URLs return:
/// `data` for the openai-shaped `/v1/models`, `modelSummaries` for Bedrock's
/// `/foundation-models`, and `models`/`publisherModels` for Vertex and the
/// google-shaped lists.
pub fn count_catalogue(body: &serde_json::Value) -> Option<usize> {
    const ARRAY_KEYS: [&str; 4] = ["data", "models", "modelSummaries", "publisherModels"];
    // a bare top-level array is a catalogue too: some openai-compatible servers
    // answer `[{...}]` rather than wrapping it
    if let Some(items) = body.as_array() {
        return Some(items.len());
    }
    ARRAY_KEYS
        .iter()
        .find_map(|key| Some(body.get(key)?.as_array()?.len()))
}

/// The model ids in a probe response body, when it is a catalogue at all.
///
/// Same shapes as [`count_catalogue`], read one level deeper: the gateway uses
/// these ids to list what a provider actually serves under
/// `provider-slug/model`, instead of only the upstream models a configured
/// route target happens to name (#1647).
///
/// An entry's id is the first of `id`, `model`, `modelId` or `name` it carries,
/// which covers the openai (`id`), bedrock (`modelId`) and google/ollama
/// (`name`) shapes; a bare string entry is its own id. A `name` that arrives as
/// a resource path (`publishers/google/models/gemini-2.5-pro`) is reduced to
/// its last segment, because that is the part an address can carry — a slash in
/// a model id would re-split as another `slug/model` address.
pub fn catalogue_ids(body: &serde_json::Value) -> Option<Vec<String>> {
    const ARRAY_KEYS: [&str; 4] = ["data", "models", "modelSummaries", "publisherModels"];
    const ID_KEYS: [&str; 4] = ["id", "model", "modelId", "name"];
    let items = body
        .as_array()
        .or_else(|| ARRAY_KEYS.iter().find_map(|key| body.get(key)?.as_array()))?;
    Some(
        items
            .iter()
            .filter_map(|item| {
                let raw = match item {
                    serde_json::Value::String(s) => s.as_str(),
                    other => ID_KEYS.iter().find_map(|key| other.get(key)?.as_str())?,
                };
                let id = raw.rsplit('/').next().unwrap_or(raw).trim();
                (!id.is_empty()).then(|| id.to_string())
            })
            .collect(),
    )
}

fn bedrock_models_url(api_base: &str) -> String {
    let base = api_base.trim_end_matches('/');
    if let Some(control) = base.strip_prefix("https://bedrock-runtime.") {
        let host = control.split('/').next().unwrap_or(control);
        return format!("https://bedrock.{host}/foundation-models");
    }
    format!("{}/foundation-models", base.trim_end_matches("/v1"))
}

fn vertex_models_url(api_base: &str) -> String {
    let base = api_base.trim_end_matches('/');
    if let Some(prefix) = base.strip_suffix("/endpoints/openapi") {
        return format!("{prefix}/publishers/google/models");
    }
    format!("{base}/models")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_shaped_kinds_probe_the_free_model_list() {
        let (url, hdr) = probe_request(ProviderKind::Openai, "https://api.openai.com/", "/");
        assert_eq!(url, "https://api.openai.com/v1/models");
        assert!(hdr.is_empty());

        let (url, _) = probe_request(ProviderKind::OpenaiCompatible, "http://vllm:8000", "/");
        assert_eq!(url, "http://vllm:8000/v1/models");
    }

    #[test]
    fn anthropic_carries_the_version_header_it_requires() {
        let (url, hdr) = probe_request(ProviderKind::Anthropic, "https://api.anthropic.com", "/");
        assert_eq!(url, "https://api.anthropic.com/v1/models");
        assert_eq!(
            hdr,
            vec![(
                "anthropic-version".to_string(),
                ANTHROPIC_VERSION.to_string()
            )]
        );
    }

    // tei serves embeddings, not a model catalogue, so /v1/models would 404 and
    // report a live provider as down
    #[test]
    fn tei_probes_its_health_endpoint() {
        let (url, headers) = probe_request(ProviderKind::Tei, "http://tei:80/", "/");
        assert_eq!(url, "http://tei:80/health");
        assert!(headers.is_empty());
    }

    #[test]
    fn bedrock_and_vertex_rewrite_onto_their_control_planes() {
        let (url, _) = probe_request(
            ProviderKind::Bedrock,
            "https://bedrock-runtime.us-east-1.amazonaws.com/v1",
            "/",
        );
        assert_eq!(
            url,
            "https://bedrock.us-east-1.amazonaws.com/foundation-models"
        );

        let (url, _) = probe_request(
            ProviderKind::Vertex,
            "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/l/endpoints/openapi",
            "/",
        );
        assert_eq!(
            url,
            "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/l/publishers/google/models"
        );
    }

    #[test]
    fn a_catalogue_probe_expects_a_catalogue_and_a_liveness_probe_does_not() {
        for kind in [
            ProviderKind::Openai,
            ProviderKind::Anthropic,
            ProviderKind::OpenaiCompatible,
            ProviderKind::Bedrock,
            ProviderKind::Vertex,
            ProviderKind::Cohere,
        ] {
            assert_eq!(probe_expectation(kind, "/"), ProbeExpectation::Catalogue);
        }
        assert_eq!(
            probe_expectation(ProviderKind::Tei, "/"),
            ProbeExpectation::Liveness
        );
    }

    /// An operator-nominated path is whatever they chose, so rolter has no
    /// basis to demand a catalogue from it.
    #[test]
    fn an_overridden_health_path_is_judged_on_liveness_alone() {
        assert_eq!(
            probe_expectation(ProviderKind::Openai, "/healthz"),
            ProbeExpectation::Liveness
        );
    }

    #[test]
    fn the_catalogue_shapes_every_probe_url_returns_are_counted() {
        use serde_json::json;
        assert_eq!(count_catalogue(&json!({"data": [1, 2, 3]})), Some(3));
        assert_eq!(count_catalogue(&json!({"models": [1]})), Some(1));
        assert_eq!(count_catalogue(&json!({"modelSummaries": []})), Some(0));
        assert_eq!(
            count_catalogue(&json!({"publisherModels": [1, 2]})),
            Some(2)
        );
        assert_eq!(count_catalogue(&json!([1, 2])), Some(2));
    }

    /// The case that made a doubled base look healthy in #947: a 2xx whose body
    /// is not a catalogue at all.
    #[test]
    fn a_non_catalogue_body_counts_as_no_catalogue() {
        use serde_json::json;
        assert_eq!(count_catalogue(&json!({"message": "ok"})), None);
        assert_eq!(count_catalogue(&json!({"data": "not-an-array"})), None);
        assert_eq!(count_catalogue(&json!("<!doctype html>")), None);
        assert_eq!(count_catalogue(&json!(null)), None);
    }

    #[test]
    fn catalogue_ids_reads_every_shape_the_probe_urls_return() {
        use serde_json::json;
        assert_eq!(
            catalogue_ids(&json!({"data": [{"id": "gpt-4o"}, {"id": "o3-mini"}]})),
            Some(vec!["gpt-4o".to_string(), "o3-mini".to_string()])
        );
        assert_eq!(
            catalogue_ids(&json!({"modelSummaries": [{"modelId": "anthropic.claude-3"}]})),
            Some(vec!["anthropic.claude-3".to_string()])
        );
        assert_eq!(
            catalogue_ids(&json!({"models": [{"name": "llama3:8b"}]})),
            Some(vec!["llama3:8b".to_string()])
        );
        // a bare string list, as some openai-compatible servers answer
        assert_eq!(
            catalogue_ids(&json!(["qwen3"])),
            Some(vec!["qwen3".to_string()])
        );
    }

    /// A google resource path is not addressable as-is: the slash would re-split
    /// as another `slug/model` address, so only the last segment is a model id.
    #[test]
    fn catalogue_ids_reduce_a_resource_path_to_its_last_segment() {
        use serde_json::json;
        assert_eq!(
            catalogue_ids(
                &json!({"publisherModels": [{"name": "publishers/google/models/gemini-2.5-pro"}]})
            ),
            Some(vec!["gemini-2.5-pro".to_string()])
        );
    }

    #[test]
    fn catalogue_ids_of_a_non_catalogue_body_is_none() {
        use serde_json::json;
        assert_eq!(catalogue_ids(&json!({"message": "ok"})), None);
        assert_eq!(catalogue_ids(&json!("<!doctype html>")), None);
        // a catalogue whose entries carry no usable id yields an empty list,
        // not a wrong one
        assert_eq!(
            catalogue_ids(&json!({"data": [{"object": "model"}, {"id": ""}]})),
            Some(Vec::new())
        );
    }

    #[test]
    fn an_explicit_path_wins_over_the_kind_default() {
        let (url, hdr) = probe_request(ProviderKind::Anthropic, "https://x.test", "/healthz");
        assert_eq!(url, "https://x.test/healthz");
        assert!(hdr.is_empty(), "an explicit path carries no kind headers");
    }
}
