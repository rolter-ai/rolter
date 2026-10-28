//! The control plane's half of the `/internal/snapshot` format negotiation
//! (#1512, ADR-0038): what it serves, the `406` bodies it refuses with, the
//! once-per-node refusal log, and the deprecation headers of a superseded
//! format. The shared types and the down-step chain are
//! [`rolter_core::snapshot_format`].

use std::collections::HashSet;
use std::sync::LazyLock;

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rolter_core::snapshot_format::{
    describe, DownStep, FormatRange, Inexpressible, Unsupported, DOWN_STEPS,
};
use serde_json::json;

use crate::deprecation::{self, Deprecation};

/// The `error.code` of a statement with no format in common.
pub(crate) const UNSUPPORTED: &str = "snapshot_format_unsupported";
/// The `error.code` of a config the negotiated format cannot express.
pub(crate) const INEXPRESSIBLE: &str = "snapshot_format_inexpressible";

/// What a control plane serves: the range, the chain that reaches its older
/// end, and the registry its deprecation headers come from. Production uses
/// [`SnapshotFormats::PRODUCTION`]; tests pass their own, which is how a
/// second format is exercised while this release ships only one.
#[derive(Clone, Copy)]
pub(crate) struct SnapshotFormats {
    pub served: FormatRange,
    pub chain: &'static [DownStep],
    pub deprecations: &'static [Deprecation],
}

impl SnapshotFormats {
    pub const PRODUCTION: SnapshotFormats = SnapshotFormats {
        served: FormatRange::served(),
        chain: DOWN_STEPS,
        deprecations: deprecation::DEPRECATIONS,
    };
}

fn ranges(served: FormatRange, requested: FormatRange) -> serde_json::Value {
    json!({
        "served": {"min": served.min, "max": served.max},
        "requested": {"min": requested.min, "max": requested.max},
    })
}

/// `406 snapshot_format_unsupported`. The explanation is repeated in
/// `problems`, which every gateway built since #2947 prints, so one that
/// predates negotiation still logs why it was refused.
pub(crate) fn unsupported(refusal: &Unsupported) -> Response {
    let mut error = json!({
        "code": UNSUPPORTED,
        "message": refusal.to_string(),
        "problems": [refusal.action()],
    });
    merge(&mut error, ranges(refusal.served, refusal.requested));
    (StatusCode::NOT_ACCEPTABLE, Json(json!({ "error": error }))).into_response()
}

/// `406 snapshot_format_inexpressible`: the formats overlap, but the config
/// holds a deployment-wide setting the negotiated one cannot carry.
pub(crate) fn inexpressible(
    refusal: &Inexpressible,
    served: FormatRange,
    requested: FormatRange,
) -> Response {
    let mut error = json!({
        "code": INEXPRESSIBLE,
        "message": refusal.to_string(),
        "problems": [
            refusal.reason.clone(),
            format!(
                "upgrade the gateway to one that reads snapshot {}, or stop using the setting",
                describe(FormatRange { min: refusal.format + 1, max: served.max })
            ),
        ],
    });
    merge(&mut error, ranges(served, requested));
    (StatusCode::NOT_ACCEPTABLE, Json(json!({ "error": error }))).into_response()
}

/// `400 invalid_query` for a `formats` value that does not parse.
pub(crate) fn invalid(message: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": {"code": "invalid_query", "message": message}})),
    )
        .into_response()
}

fn merge(into: &mut serde_json::Value, from: serde_json::Value) {
    if let (Some(into), serde_json::Value::Object(from)) = (into.as_object_mut(), from) {
        into.extend(from);
    }
}

/// Most `(node, range, code)` refusals remembered before the set is cleared.
/// A fleet refusing more distinct nodes than this logs some twice, which is
/// a better failure than unbounded memory on a public endpoint.
const LOGGED_LIMIT: usize = 4096;

/// A refusal as the log remembers it: the node id, its stated range, the code.
type LoggedRefusal = (String, FormatRange, &'static str);

static LOGGED: LazyLock<parking_lot::Mutex<HashSet<LoggedRefusal>>> =
    LazyLock::new(Default::default);

/// Log a refusal at `warn` the first time this process refuses a node with a
/// given range and code, rather than on every poll. Returns whether it logged.
pub(crate) fn log_refusal(
    headers: &HeaderMap,
    code: &'static str,
    served: FormatRange,
    requested: FormatRange,
    detail: &str,
) -> bool {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty() && v.len() <= 128 && !v.chars().any(char::is_control))
            .unwrap_or("")
            .to_string()
    };
    let node = header("x-rolter-node-id");
    let build = header("x-rolter-node-build");
    {
        let mut logged = LOGGED.lock();
        if logged.len() >= LOGGED_LIMIT {
            logged.clear();
        }
        if !logged.insert((node.clone(), requested, code)) {
            return false;
        }
    }
    tracing::warn!(
        node = %if node.is_empty() { "unidentified" } else { node.as_str() },
        build = %build,
        served = %served,
        requested = %requested,
        code,
        "refusing a snapshot poll: {detail}"
    );
    true
}

/// Add `Deprecation` and `Sunset` when `format` is deprecated in `list`.
pub(crate) fn with_deprecation(
    mut response: Response,
    list: &[Deprecation],
    format: u32,
) -> Response {
    if let Some(d) = deprecation::for_snapshot_format(list, format) {
        for (name, value) in [
            ("deprecation", d.deprecation_header()),
            ("sunset", d.sunset_header()),
        ] {
            if let Some(value) = value.and_then(|v| HeaderValue::from_str(&v).ok()) {
                response
                    .headers_mut()
                    .insert(HeaderName::from_static(name), value);
            }
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Query, State};
    use axum::routing::get;
    use axum::Router;
    use rolter_core::snapshot_format::SNAPSHOT_FORMAT;
    use rolter_core::{GatewayConfig, ModelRoute, ProviderConfig, ProviderKind};
    use serde_json::Value;

    #[test]
    fn a_refusal_is_logged_once_per_node_range_and_code() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-rolter-node-id",
            HeaderValue::from_static("negotiation-log-test-node"),
        );
        let served = FormatRange::single(2);
        let one = FormatRange::single(1);
        assert!(log_refusal(&headers, UNSUPPORTED, served, one, "x"));
        assert!(!log_refusal(&headers, UNSUPPORTED, served, one, "x"));
        // a different code or range is a different refusal
        assert!(log_refusal(&headers, INEXPRESSIBLE, served, one, "x"));
        assert!(log_refusal(
            &headers,
            UNSUPPORTED,
            served,
            FormatRange::single(3),
            "x"
        ));
    }

    // test-only chains for a control plane that serves a second format, which
    // this release does not ship. In each, format 2 is the fiction and format 1
    // is what every gateway of this release reads

    fn keep(_: &mut GatewayConfig) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn untouched(_: &mut Value) {}

    /// format 2 "added" `breaker`, so the step to 1 removes it
    fn drop_breaker(config: &mut Value) {
        if let Some(config) = config.as_object_mut() {
            config.remove("breaker");
        }
    }

    /// format 2 "added" the anthropic kind, so the step to 1 omits providers of
    /// it and the sanitizer cascades the omission into their routes
    fn omit_anthropic(config: &mut GatewayConfig) -> Result<Vec<String>, String> {
        let mut problems = Vec::new();
        config.providers.retain(|p| {
            let keep = p.kind != ProviderKind::Anthropic;
            if !keep {
                problems.push(format!(
                    "provider '{}' omitted: snapshot format 1 cannot carry its kind",
                    p.name
                ));
            }
            keep
        });
        Ok(problems)
    }

    /// format 2 "added" a deployment-wide setting format 1 has no spelling for
    fn refuse(_: &mut GatewayConfig) -> Result<Vec<String>, String> {
        Err("format 1 has no spelling for the deployment-wide setting in use".to_string())
    }

    const ADDS_A_FIELD: &[DownStep] = &[DownStep {
        from: 2,
        to: 1,
        summary: "format 1 has no breaker",
        omit: keep,
        rewrite: drop_breaker,
    }];

    const OMITS: &[DownStep] = &[DownStep {
        from: 2,
        to: 1,
        summary: "format 1 has no anthropic providers",
        omit: omit_anthropic,
        rewrite: untouched,
    }];

    const REFUSES: &[DownStep] = &[DownStep {
        from: 2,
        to: 1,
        summary: "format 1 cannot carry the setting",
        omit: refuse,
        rewrite: untouched,
    }];

    const FORMAT_ONE_DEPRECATED: &[Deprecation] = &[Deprecation {
        target: deprecation::Target::SnapshotFormat(1),
        since: "1.2.0",
        announced: "2027-01-15",
        sunset: "2027-04-20",
        replacement: "Upgrade the gateways to a release that reads format 2.",
        security_fix: false,
    }];

    const fn serving(min: u32, max: u32, chain: &'static [DownStep]) -> SnapshotFormats {
        SnapshotFormats {
            served: FormatRange { min, max },
            chain,
            deprecations: &[],
        }
    }

    async fn serve_formats(config: GatewayConfig, formats: SnapshotFormats) -> String {
        let state = crate::ControlState {
            store: std::sync::Arc::new(rolter_store::InMemoryConfigStore::new(config)),
            ..crate::tests::state_with_token(None)
        };
        let app = Router::new()
            .route(
                "/internal/snapshot",
                get(
                    move |State(state): State<crate::ControlState>,
                          headers: HeaderMap,
                          Query(query): Query<crate::SnapshotQuery>| {
                        crate::get_snapshot_with(state, headers, query, formats)
                    },
                ),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/internal/snapshot")
    }

    async fn poll(url: &str, query: &str) -> (u16, HeaderMap, Value) {
        let response = reqwest::get(format!("{url}{query}")).await.unwrap();
        let status = response.status().as_u16();
        let mut headers = HeaderMap::new();
        for (name, value) in response.headers() {
            if let Ok(value) = HeaderValue::from_bytes(value.as_bytes()) {
                if let Ok(name) = HeaderName::from_bytes(name.as_str().as_bytes()) {
                    headers.insert(name, value);
                }
            }
        }
        let bytes = response.bytes().await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, headers, body)
    }

    /// The reply decoder of every gateway built before negotiation, frozen
    /// here as it was: `format` is not a field it knows.
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct PreNegotiationSnapshotResponse {
        version: u64,
        config: GatewayConfig,
        #[serde(default)]
        problems: Vec<String>,
    }

    /// Newer control plane, older gateway: a poll with no `formats` is served
    /// format 1, says so, and still decodes through the old type.
    #[tokio::test]
    async fn a_silent_gateway_is_served_format_one_and_decodes_it() {
        let url = serve_formats(GatewayConfig::default(), SnapshotFormats::PRODUCTION).await;
        let (status, _, body) = poll(&url, "").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["format"], 1, "{body}");
        let decoded: PreNegotiationSnapshotResponse =
            serde_json::from_value(body.clone()).expect("the pre-negotiation decoder reads it");
        assert_eq!(decoded.version as i64, body["version"].as_i64().unwrap());

        // and a gateway of this release states its format and is served it
        let (status, _, body) = poll(&url, &format!("?formats={SNAPSHOT_FORMAT}")).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["format"], SNAPSHOT_FORMAT);
    }

    #[tokio::test]
    async fn a_statement_that_does_not_parse_is_invalid_query() {
        let url = serve_formats(GatewayConfig::default(), SnapshotFormats::PRODUCTION).await;
        for bad in ["two", "2-1", "0", ""] {
            let (status, _, body) = poll(&url, &format!("?formats={bad}")).await;
            assert_eq!(status, 400, "{bad:?}: {body}");
            assert_eq!(body["error"]["code"], "invalid_query", "{bad:?}");
            assert!(
                body["error"]["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("inclusive range")),
                "{body}"
            );
        }
    }

    /// Serving `1..=2` with a step in which format 2 added a field: a gateway
    /// that reads 1, stated or silent, gets no field, and one that reads 2
    /// gets it.
    #[tokio::test]
    async fn an_older_format_is_served_through_the_down_step() {
        let url = serve_formats(GatewayConfig::default(), serving(1, 2, ADDS_A_FIELD)).await;
        for query in ["?formats=1", ""] {
            let (status, _, body) = poll(&url, query).await;
            assert_eq!(status, 200, "{query}: {body}");
            assert_eq!(body["format"], 1, "{query}");
            assert!(body["config"].get("breaker").is_none(), "{query}: {body}");
            assert!(body["config"].get("server").is_some(), "{query}: {body}");
        }
        for query in ["?formats=2", "?formats=1-2", "?formats=2-9"] {
            let (status, _, body) = poll(&url, query).await;
            assert_eq!(status, 200, "{query}: {body}");
            assert_eq!(body["format"], 2, "{query}");
            assert!(body["config"].get("breaker").is_some(), "{query}: {body}");
        }
    }

    /// What an older format cannot express is omitted before the sanitizer,
    /// and both the omission and its cascade ride out in `problems`.
    #[tokio::test]
    async fn an_omitting_step_names_what_it_left_out() {
        let config = GatewayConfig {
            providers: vec![
                ProviderConfig {
                    name: "openai".to_string(),
                    kind: ProviderKind::Openai,
                    api_base: "https://api.openai.com/v1".to_string(),
                    ..Default::default()
                },
                ProviderConfig {
                    name: "claude".to_string(),
                    kind: ProviderKind::Anthropic,
                    api_base: "https://api.anthropic.com".to_string(),
                    ..Default::default()
                },
            ],
            routes: vec![
                serde_json::from_value::<ModelRoute>(serde_json::json!({
                    "model": "gpt-4o", "targets": [{"provider": "openai"}]
                }))
                .unwrap(),
                serde_json::from_value::<ModelRoute>(serde_json::json!({
                    "model": "sonnet", "targets": [{"provider": "claude"}]
                }))
                .unwrap(),
            ],
            ..Default::default()
        };
        let url = serve_formats(config, serving(1, 2, OMITS)).await;

        let (status, _, body) = poll(&url, "?formats=1").await;
        assert_eq!(status, 200, "{body}");
        let providers = body["config"]["providers"].as_array().unwrap();
        assert_eq!(providers.len(), 1, "{body}");
        let routes = body["config"]["routes"].as_array().unwrap();
        assert!(
            routes.iter().all(|r| r["model"] != "sonnet"),
            "the cascade left a route to the omitted provider: {body}"
        );
        let problems = body["problems"].to_string();
        assert!(problems.contains("'claude' omitted"), "{problems}");
        assert!(problems.contains("sonnet"), "{problems}");

        // a gateway that reads format 2 is served everything
        let (_, _, body) = poll(&url, "?formats=2").await;
        assert_eq!(body["config"]["providers"].as_array().unwrap().len(), 2);
        assert!(body.get("problems").is_none(), "{body}");
    }

    /// Serving `2..=2`: a gateway that reads 1, stated or silent, is refused,
    /// and the refusal wins over the `304` a current `version` would get.
    #[tokio::test]
    async fn a_format_that_is_not_served_is_refused_before_the_304() {
        let url = serve_formats(GatewayConfig::default(), serving(2, 2, &[])).await;
        for query in ["?formats=1", "", "?version=999&formats=1", "?version=999"] {
            let (status, _, body) = poll(&url, query).await;
            assert_eq!(status, 406, "{query}: {body}");
            let error = &body["error"];
            assert_eq!(error["code"], UNSUPPORTED, "{query}");
            assert_eq!(
                error["message"],
                "the control plane serves snapshot format 2; the gateway reads 1"
            );
            assert_eq!(error["served"], serde_json::json!({"min": 2, "max": 2}));
            assert_eq!(error["requested"], serde_json::json!({"min": 1, "max": 1}));
            assert_eq!(
                error["problems"],
                serde_json::json!(["upgrade the gateway, or roll the control plane back"])
            );
        }
        // the format it does serve still gets its 304
        let (status, _, _) = poll(&url, "?version=999&formats=2").await;
        assert_eq!(status, 304);
    }

    /// Newer gateway, older control plane of this release: serving `1..=1`, a
    /// gateway that reads only 2 is refused, and told to move the other side.
    #[tokio::test]
    async fn a_gateway_newer_than_the_control_plane_is_refused() {
        let url = serve_formats(GatewayConfig::default(), SnapshotFormats::PRODUCTION).await;
        let next = SNAPSHOT_FORMAT + 1;
        let (status, _, body) = poll(&url, &format!("?formats={next}")).await;
        assert_eq!(status, 406, "{body}");
        assert_eq!(body["error"]["code"], UNSUPPORTED);
        assert_eq!(
            body["error"]["problems"],
            serde_json::json!(["upgrade the control plane first, or roll the gateway back"])
        );
        assert_eq!(body["error"]["requested"]["min"], next);
    }

    /// Serving `1..=2`, a config holding a setting format 1 cannot carry: the
    /// gateway that reads 1 is refused as inexpressible, the one that reads 2
    /// is served.
    #[tokio::test]
    async fn a_config_the_older_format_cannot_carry_is_inexpressible() {
        let url = serve_formats(GatewayConfig::default(), serving(1, 2, REFUSES)).await;
        let (status, _, body) = poll(&url, "?formats=1").await;
        assert_eq!(status, 406, "{body}");
        let error = &body["error"];
        assert_eq!(error["code"], INEXPRESSIBLE);
        assert!(
            error["message"]
                .as_str()
                .is_some_and(|m| m.contains("cannot be expressed in snapshot format 1")),
            "{body}"
        );
        assert!(error["problems"][0]
            .as_str()
            .is_some_and(|p| p.contains("deployment-wide")));
        assert_eq!(error["served"], serde_json::json!({"min": 1, "max": 2}));

        let (status, _, _) = poll(&url, "?formats=2").await;
        assert_eq!(status, 200);
    }

    /// A deprecated format is served with `Deprecation` and `Sunset`, on the
    /// `304` too; the format that supersedes it carries neither.
    #[tokio::test]
    async fn a_deprecated_format_carries_the_headers_and_only_it() {
        let formats = SnapshotFormats {
            deprecations: FORMAT_ONE_DEPRECATED,
            ..serving(1, 2, ADDS_A_FIELD)
        };
        let url = serve_formats(GatewayConfig::default(), formats).await;
        for query in ["?formats=1", "", "?formats=1&version=999"] {
            let (status, headers, _) = poll(&url, query).await;
            assert!(status == 200 || status == 304, "{query}: {status}");
            assert_eq!(headers["deprecation"], "@1799971200", "{query}");
            assert_eq!(
                headers["sunset"], "Tue, 20 Apr 2027 00:00:00 GMT",
                "{query}"
            );
        }
        let (status, headers, _) = poll(&url, "?formats=2").await;
        assert_eq!(status, 200);
        assert!(headers.get("deprecation").is_none());
        assert!(headers.get("sunset").is_none());
    }
}
