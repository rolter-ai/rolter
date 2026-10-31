//! Request correlation: an end-to-end request id and inbound distributed-trace
//! continuation (ROL-60).
//!
//! [`ensure_request_id`] is a middleware that guarantees every request carries a
//! request id — reusing the caller's when present, else minting a UUID — and
//! echoes it on the response so a client can correlate its call with the gateway
//! logs. The header it travels in is `[client] request_id_header`
//! (`x-request-id` unless configured), read from the live snapshot so a change
//! applies without a restart (#2925). Handlers take the id as a [`RequestId`]
//! rather than reading a header, so they never need to know its name.
//! [`inbound_trace_id`] pulls the trace id out of a W3C `traceparent` or a
//! B3 header so the request log adopts the caller's trace instead of starting a
//! disconnected one; the id is stored on each [`RequestLog`](crate::logging::RequestLog)
//! and surfaces in ClickHouse for cross-service correlation.
//!
//! [`GatewayMakeSpan`] takes that further (#805): it builds the request span and
//! makes the inbound context its real OpenTelemetry *parent*, and [`outbound_headers`]
//! injects the gateway's own span into the upstream call rather than copying the
//! caller's header verbatim. Together those are what put a dashboard click, the
//! gateway, and the provider request in one correctly-parented trace. All of it
//! is inert unless an OTLP endpoint is configured.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use tower_http::trace::{MakeSpan, OnResponse};
use tracing::Span;

use crate::state::AppState;

/// Pipeline-stage span, defined in `rolter-core` so the proxy's translation
/// stages use the same one. Re-exported here because that is where the gateway
/// reaches for tracing helpers.
pub(crate) use rolter_core::stage_span;

/// The header name a snapshot carries the request id in, from `[client]
/// request_id_header`.
///
/// A name that fails [`rolter_core::request_id_header_problem`] falls back to
/// `x-request-id`. Config validation and the control plane both refuse such a
/// name before it gets here, so the fallback only guards a config that skipped
/// validation; reading a credential header as the id would copy the caller's
/// key into the response and the request log.
pub fn request_id_header_name(configured: &str) -> HeaderName {
    let default = || HeaderName::from_static(rolter_core::DEFAULT_REQUEST_ID_HEADER);
    if let Some(problem) = rolter_core::request_id_header_problem(configured) {
        tracing::warn!(
            %problem,
            "client.request_id_header is unusable; using {}",
            rolter_core::DEFAULT_REQUEST_ID_HEADER
        );
        return default();
    }
    HeaderName::from_bytes(configured.as_bytes()).unwrap_or_else(|_| default())
}

/// The request id [`ensure_request_id`] settled on for this request.
///
/// It travels in the request's extensions rather than being read back from a
/// header, so every reader sees the value the response echoes even when a
/// snapshot reload renames the header while the request is in flight. Extract
/// it like any other axum extractor; it never rejects, and is empty only for a
/// request that did not pass through the middleware.
#[derive(Clone, Debug)]
pub struct RequestId(HeaderValue);

impl RequestId {
    /// The id as text, empty when the request carried none.
    pub fn as_str(&self) -> &str {
        // built from a `&str` the header map accepted, so this only fails for a
        // value that was never a request id
        self.0.to_str().unwrap_or_default()
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self(HeaderValue::from_static(""))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RequestId {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.extensions.get::<Self>().cloned().unwrap_or_default())
    }
}

/// Ensure the request has a request id in the configured header (generating
/// one when absent or blank), hand it to handlers as a [`RequestId`], and
/// mirror it onto the response under the same header.
///
/// The id is also written back onto the request's own headers, so a minted id
/// reaches an upstream that forwards the header (an allowlisted
/// `forwarded_headers` entry, an MCP server) exactly as a caller-supplied one
/// does.
pub async fn ensure_request_id(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    // one name for the whole request: a reload between the lookup and the
    // echo must not read the id from one header and return it in another
    let name = state.snapshot.load().request_id_header.clone();
    let inbound = req
        .headers()
        .get(&name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let header = match inbound {
        Some(id) => HeaderValue::from_str(id),
        None => HeaderValue::try_from(new_request_id()),
    };

    // a generated uuid is always a valid header value; a caller-supplied one that
    // isn't is dropped rather than failing the request
    let Ok(header) = header else {
        return next.run(req).await;
    };
    let id = RequestId(header.clone());
    req.headers_mut().insert(name.clone(), header.clone());
    req.extensions_mut().insert(id.clone());
    record_request_id(id.as_str());
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(name, header);
    // the trace layer's response hook logs the id and has no snapshot to learn
    // the header name from
    resp.extensions_mut().insert(id);
    resp
}

fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Builds the per-request span and adopts the caller's inbound trace context as
/// its parent (#805).
///
/// This has to be the [`TraceLayer`](tower_http::trace::TraceLayer)'s own
/// span-maker rather than a middleware layered inside it. `DefaultMakeSpan`
/// creates the request span at **DEBUG**, so under the default `RUST_LOG=info`
/// it is disabled — and setting a parent on a disabled span does nothing, which
/// left the stage spans as disconnected roots carrying a freshly-minted trace id
/// even though the caller had sent a perfectly good `traceparent`. Building the
/// span here, at `INFO`, is what makes it real enough to parent and to export.
///
/// When no OTLP pipeline is installed it falls back to the stock DEBUG span, so
/// an untraced deployment keeps paying exactly what it paid before: the span is
/// disabled by the default filter and never allocated.
#[derive(Clone, Copy, Debug, Default)]
pub struct GatewayMakeSpan;

impl<B> MakeSpan<B> for GatewayMakeSpan {
    fn make_span(&mut self, req: &axum::http::Request<B>) -> Span {
        if !rolter_core::telemetry::is_active() {
            // stock behaviour: disabled at the default filter, costs nothing
            return tracing::debug_span!(
                "request",
                method = %req.method(),
                uri = %req.uri(),
                version = ?req.version(),
                rolter.request.id = tracing::field::Empty,
            );
        }

        // `otel.name` is what a backend shows as the span name; method + route
        // reads better in a waterfall than the bare path. `otel.kind=server`
        // marks this as the inbound leg: rolter is a server in the GenAI
        // conventions' sense, and the kind was previously never set at all —
        // `otel.kind` appeared nowhere in the gateway (#808).
        //
        // the tenant fields start empty and are filled in by `record_tenant`
        // once the virtual key resolves: this span is built by the tower layer,
        // which runs before auth, so the identity is not known yet (#836)
        let span = tracing::info_span!(
            "gateway.request",
            otel.name = %format!("{} {}", req.method(), req.uri().path()),
            otel.kind = "server",
            http.request.method = %req.method(),
            url.path = %req.uri().path(),
            rolter.org.id = tracing::field::Empty,
            rolter.team.id = tracing::field::Empty,
            rolter.project.id = tracing::field::Empty,
            rolter.request.id = tracing::field::Empty,
        );

        let mut carrier = rolter_core::telemetry::TraceCarrier::new();
        for name in PROPAGATED_TRACE_HEADERS {
            if let Some(value) = req.headers().get(*name).and_then(|v| v.to_str().ok()) {
                carrier.insert(name, value);
            }
        }
        rolter_core::telemetry::adopt_inbound(&span, &carrier.finish());
        span
    }
}

/// Attribute names carrying tenant identity on exported spans (#836).
///
/// Spelled out as literals at the span macro too, since `tracing` will not take
/// a constant as a field name; a test pins the two together.
pub const ORG_ID: &str = "rolter.org.id";
pub const TEAM_ID: &str = "rolter.team.id";
pub const PROJECT_ID: &str = "rolter.project.id";
/// Attribute name carrying the settled request id on exported spans (#2964).
pub const REQUEST_ID: &str = "rolter.request.id";

/// Stamp the settled request id onto the request span.
///
/// The span is built by the tower layer, outside [`ensure_request_id`], so the
/// id (which may be minted, and whose header name comes from the live snapshot)
/// is not known at span creation; it is recorded afterwards, the same way the
/// tenant is. That keeps the layer order untouched. Recording onto a disabled
/// or field-less span is a no-op, so an untraced deployment pays nothing.
pub fn record_request_id(id: &str) {
    Span::current().record(REQUEST_ID, id);
}

/// Stamp the resolved tenant onto the request span.
///
/// ADR-0026 decided that per-tenant telemetry destinations are routed by an
/// OpenTelemetry Collector rather than fanned out by rolter, and that "rolter's
/// job is to *stamp* the attribute". This is that stamp, and without it the
/// decision is unimplementable: the collector's routing has nothing to key on,
/// because until now no exported span carried tenant identity at all. The
/// request logs in ClickHouse have always had it; the traces never did.
///
/// The names are rolter-local rather than `enduser.*` or a `tenant.id` guess.
/// The GenAI and HTTP conventions define nothing for this, and inventing a
/// convention-shaped name that a future spec might redefine is worse than an
/// obviously-local one.
///
/// Empty values are skipped rather than recorded as `""`. A config-defined key
/// has no org, and an attribute present-but-empty on some spans and absent on
/// others is harder to write a routing rule against than one that is simply
/// absent.
pub fn record_tenant(org: &str, team: &str, project: &str) {
    // cheap and correct when untraced: with no pipeline the span is disabled,
    // so `record` on it is a no-op that touches no exporter
    let span = Span::current();
    for (name, value) in [(ORG_ID, org), (TEAM_ID, team), (PROJECT_ID, project)] {
        if !value.is_empty() {
            span.record(name, value);
        }
    }
}

/// The trace id to stamp on this request's [`RequestLog`](crate::logging::RequestLog).
///
/// Prefers the id of the span actually being exported, so ClickHouse and the
/// trace backend agree by construction; falls back to parsing the inbound
/// header when no pipeline is installed, which is the pre-#805 behaviour and
/// the only path an untraced deployment takes.
pub fn request_trace_id(headers: &HeaderMap) -> String {
    rolter_core::telemetry::current_trace_id().unwrap_or_else(|| inbound_trace_id(headers))
}

/// [`TraceLayer`](tower_http::trace::TraceLayer) response hook that surfaces
/// failing requests on the terminal at the default `info` filter.
///
/// The stock `DefaultOnResponse` logs every response at `DEBUG`, so with the
/// default `RUST_LOG=info` an operator running `rolter`/`uvx rolter` never sees
/// 4xx/5xx responses (ROL-230): a 404 for an unknown model, a 401 for a bad key
/// or a 502 from a dead upstream all vanish unless `RUST_LOG` is turned up. This
/// hook picks the level from the status class instead — server errors at
/// `error`, client errors at `warn`, everything else at `debug` — so errors are
/// visible out of the box while successful traffic stays quiet. Pair it with
/// `.on_failure(())` on the layer so classified 5xx responses are not also
/// logged by the default failure hook.
#[derive(Clone, Copy, Debug, Default)]
pub struct StatusAwareOnResponse;

/// Pick the log level (and its message) for a response status class: server
/// errors are loud, client errors are warnings, success is quiet.
fn level_for_status(status: axum::http::StatusCode) -> (tracing::Level, &'static str) {
    if status.is_server_error() {
        (tracing::Level::ERROR, "request failed")
    } else if status.is_client_error() {
        (tracing::Level::WARN, "request rejected")
    } else {
        (tracing::Level::DEBUG, "request completed")
    }
}

impl<B> OnResponse<B> for StatusAwareOnResponse {
    fn on_response(self, response: &axum::http::Response<B>, latency: Duration, _span: &Span) {
        let status = response.status();
        let latency_ms = latency.as_millis();
        let request_id = response
            .extensions()
            .get::<RequestId>()
            .map(RequestId::as_str)
            .unwrap_or("");
        // the message + level are fixed per status class; the macro must still be
        // invoked per-branch because the level is a compile-time argument
        match level_for_status(status).0 {
            tracing::Level::ERROR => {
                tracing::error!(
                    status = status.as_u16(),
                    latency_ms,
                    request_id,
                    "request failed"
                )
            }
            tracing::Level::WARN => {
                tracing::warn!(
                    status = status.as_u16(),
                    latency_ms,
                    request_id,
                    "request rejected"
                )
            }
            _ => {
                tracing::debug!(
                    status = status.as_u16(),
                    latency_ms,
                    request_id,
                    "request completed"
                )
            }
        }
    }
}

// spec-ref: w3c-trace-context, b3
/// Extract an inbound trace id from a W3C `traceparent`, a B3 single header, or
/// the `x-b3-traceid` multi-header, normalized to lowercase hex. Returns an
/// empty string when no well-formed trace id is present.
pub fn inbound_trace_id(headers: &HeaderMap) -> String {
    // W3C traceparent: `version-traceid-spanid-flags`; trace id is 32 hex chars
    if let Some(tp) = headers.get("traceparent").and_then(|v| v.to_str().ok()) {
        let parts: Vec<&str> = tp.split('-').collect();
        if parts.len() >= 3 && is_hex(parts[1], 32) {
            return parts[1].to_lowercase();
        }
    }
    // B3 single header: `traceid-spanid[-sampled[-parentspanid]]` (64- or 128-bit)
    if let Some(b3) = headers.get("b3").and_then(|v| v.to_str().ok()) {
        let first = b3.split('-').next().unwrap_or("");
        if is_hex(first, 32) || is_hex(first, 16) {
            return first.to_lowercase();
        }
    }
    // B3 multi header
    if let Some(tid) = headers.get("x-b3-traceid").and_then(|v| v.to_str().ok()) {
        if is_hex(tid, 32) || is_hex(tid, 16) {
            return tid.to_lowercase();
        }
    }
    String::new()
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// standard distributed-trace headers propagated verbatim to the upstream
const PROPAGATED_TRACE_HEADERS: &[&str] = &[
    "traceparent",
    "tracestate",
    "b3",
    "x-b3-traceid",
    "x-b3-spanid",
    "x-b3-sampled",
    "x-b3-parentspanid",
    "x-b3-flags",
];

/// Collect the caller's inbound trace-context headers so the forwarder can
/// propagate them verbatim to the upstream, letting vLLM/SGLang/TGI continue the
/// same trace (ROL-61). Returns an empty vec when the caller sent none, so an
/// untraced request adds nothing to the wire.
pub fn outbound_trace_headers(headers: &HeaderMap) -> Vec<(&'static str, String)> {
    PROPAGATED_TRACE_HEADERS
        .iter()
        .filter_map(|&name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.is_empty())
                .map(|v| (name, v.to_string()))
        })
        .collect()
}

/// Trace context injected from the current span, or `None` when no OTLP
/// pipeline is installed or the current span has no valid context — in which
/// case the caller falls back to copying the inbound headers.
fn injected_trace_headers() -> Option<Vec<(String, String)>> {
    if !rolter_core::telemetry::is_active() {
        return None;
    }
    let injected = rolter_core::telemetry::inject_current();
    (!injected.is_empty()).then_some(injected)
}

/// Trace context plus the operator-configured client headers, ready to hand to
/// the forwarder (#564).
///
/// Trace headers always propagate and always win: an allowlist entry naming one
/// of them cannot replace the context the gateway resolved, so a misconfigured
/// list can never break correlation.
///
/// With an OTLP pipeline installed the trace context is *injected from the
/// current span* rather than copied from the caller (#805). That is the
/// difference between the provider call being a child of the gateway span and
/// being its sibling — copying the inbound header verbatim parented the upstream
/// leg to the caller, which made every waterfall built from the data wrong. Call
/// this from inside the span the upstream request should hang off.
///
/// With no pipeline installed it copies the caller's headers verbatim, exactly
/// as before.
pub fn outbound_headers(headers: &HeaderMap, forwarded: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = injected_trace_headers().unwrap_or_else(|| {
        outbound_trace_headers(headers)
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect()
    });
    for name in forwarded {
        if PROPAGATED_TRACE_HEADERS.contains(&name.as_str()) {
            continue;
        }
        if let Some(value) = headers
            .get(name.as_str())
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
        {
            out.push((name.clone(), value.to_string()));
        }
    }
    collect_vendor_headers(headers, &mut out);
    out
}

/// Vendor header namespaces forwarded **by prefix**, on top of the operator's
/// allowlist.
///
/// Anthropic extends its protocol through headers — a capability pairs an
/// `anthropic-beta` value with body fields, and the pair travels together — and
/// its gateway protocol reference says to pass `anthropic-*` request headers
/// through unchanged. An allowlist cannot express that: it is a closed list
/// against a vendor that ships new capabilities *as new header values*, so
/// every new beta silently breaks until an operator adds it by name. Stripping
/// the header while the body passes produces hard `400`s
/// (`Extra inputs are not permitted`) rather than a graceful downgrade (#1013).
///
/// Match is on the `name-` prefix, never a bare `name`, so this can never widen
/// to an unrelated header that merely starts with the same letters.
const VENDOR_HEADER_PREFIXES: [&str; 1] = ["anthropic-"];

/// Append every inbound header in a [`VENDOR_HEADER_PREFIXES`] namespace.
///
/// Skips names already present, so an operator who *also* listed one in
/// `forwarded_headers` gets one copy rather than a duplicate the upstream would
/// see as a comma-joined value. The proxy drops these again for a provider that
/// does not speak the dialect, so nothing reaches an upstream that has no use
/// for it.
fn collect_vendor_headers(headers: &HeaderMap, out: &mut Vec<(String, String)>) {
    for (name, value) in headers {
        let name = name.as_str();
        if !VENDOR_HEADER_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        if out.iter().any(|(existing, _)| existing == name) {
            continue;
        }
        if let Some(value) = value.to_str().ok().filter(|s| !s.is_empty()) {
            out.push((name.to_string(), value.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    /// Collects every field recorded onto a span after creation, so the tenant
    /// attributes can be asserted as *recorded* rather than merely as called.
    #[derive(Clone, Default)]
    struct Recorded(std::sync::Arc<parking_lot::Mutex<Vec<(String, String)>>>);

    impl Recorded {
        fn get(&self, key: &str) -> Option<String> {
            let seen = self.0.lock();
            seen.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        }

        fn keys(&self) -> Vec<String> {
            self.0.lock().iter().map(|(k, _)| k.clone()).collect()
        }
    }

    impl tracing::field::Visit for Recorded {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0
                .lock()
                .push((field.name().to_string(), value.to_string()));
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .lock()
                .push((field.name().to_string(), format!("{value:?}")));
        }
    }

    impl<S> tracing_subscriber::Layer<S> for Recorded
    where
        S: tracing::Subscriber,
    {
        fn on_record(
            &self,
            _span: &tracing::span::Id,
            values: &tracing::span::Record<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            values.record(&mut self.clone());
        }
    }

    /// The stamp ADR-0026 assigns to rolter: without it a collector routing
    /// tenant telemetry has nothing to key on, since no exported span carried
    /// tenant identity before #836.
    #[test]
    fn the_resolved_tenant_lands_on_the_request_span() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let recorded = Recorded::default();
        let subscriber = tracing_subscriber::registry().with(recorded.clone());
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!(
                "gateway.request",
                rolter.org.id = tracing::field::Empty,
                rolter.team.id = tracing::field::Empty,
                rolter.project.id = tracing::field::Empty,
            );
            let _entered = span.enter();
            record_tenant("org-1", "team-2", "project-3");
        });

        assert_eq!(recorded.get(ORG_ID).as_deref(), Some("org-1"));
        assert_eq!(recorded.get(TEAM_ID).as_deref(), Some("team-2"));
        assert_eq!(recorded.get(PROJECT_ID).as_deref(), Some("project-3"));
    }

    /// A config-defined key has no org. An attribute that is present-but-empty
    /// on some spans and absent on others is harder to route on than one that
    /// is simply absent, so empties are skipped.
    #[test]
    fn an_unattributed_request_records_no_tenant_attributes() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let recorded = Recorded::default();
        let subscriber = tracing_subscriber::registry().with(recorded.clone());
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!(
                "gateway.request",
                rolter.org.id = tracing::field::Empty,
                rolter.team.id = tracing::field::Empty,
                rolter.project.id = tracing::field::Empty,
            );
            let _entered = span.enter();
            record_tenant("", "", "");
        });

        assert!(
            recorded.keys().is_empty(),
            "nothing recorded, got {:?}",
            recorded.keys()
        );
    }

    /// `tracing` will not accept a constant as a span field name, so the names
    /// exist twice: here and at the macro call site in `make_span`. This pins
    /// them together — a rename that misses one fails here rather than
    /// silently producing a span the collector cannot route.
    #[test]
    fn the_tenant_attribute_names_match_the_span_declaration() {
        assert_eq!(ORG_ID, "rolter.org.id");
        assert_eq!(TEAM_ID, "rolter.team.id");
        assert_eq!(PROJECT_ID, "rolter.project.id");

        let declared = include_str!("trace.rs");
        assert_eq!(REQUEST_ID, "rolter.request.id");
        for name in [ORG_ID, TEAM_ID, PROJECT_ID, REQUEST_ID] {
            assert!(
                declared.contains(&format!("{name} = tracing::field::Empty")),
                "{name} must be declared on the gateway.request span"
            );
        }
    }

    /// The id a client quotes must be findable on the exported span, not only
    /// through the log row (#2964).
    #[test]
    fn the_request_id_lands_on_the_request_span() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let recorded = Recorded::default();
        let subscriber = tracing_subscriber::registry().with(recorded.clone());
        tracing::subscriber::with_default(subscriber, || {
            let span =
                tracing::info_span!("gateway.request", rolter.request.id = tracing::field::Empty);
            let _entered = span.enter();
            record_request_id("req-42");
        });

        assert_eq!(recorded.get(REQUEST_ID).as_deref(), Some("req-42"));
    }

    /// Drives the real router, so the layer order, `GatewayMakeSpan` and
    /// `ensure_request_id` are tested together: the span is built before the
    /// middleware runs, which is why the id has to be recorded afterwards.
    /// A current-thread runtime keeps the server task on the thread the
    /// thread-local subscriber is installed on.
    #[tokio::test]
    async fn the_request_span_carries_the_echoed_request_id() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let recorded = Recorded::default();
        let _guard =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(recorded.clone()));
        let app = crate::build_router_from_config(&rolter_core::GatewayConfig::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let url = format!("http://{addr}/v1/models");
        let client = reqwest::Client::new();

        let supplied = client
            .get(&url)
            .header("x-request-id", "caller-id-7")
            .send()
            .await
            .unwrap();
        assert_eq!(supplied.headers()["x-request-id"], "caller-id-7");
        assert_eq!(recorded.get(REQUEST_ID).as_deref(), Some("caller-id-7"));

        let minted = client.get(&url).send().await.unwrap();
        let echoed = minted.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_string();
        assert!(!echoed.is_empty());
        let seen = recorded.0.lock();
        let last = seen
            .iter()
            .rfind(|(k, _)| k == REQUEST_ID)
            .map(|(_, v)| v.as_str());
        assert_eq!(last, Some(echoed.as_str()));
    }

    /// A config that skipped validation must still never make the gateway read
    /// a credential as the request id or write the id over message framing.
    #[test]
    fn an_unusable_request_id_header_falls_back_to_x_request_id() {
        assert_eq!(
            request_id_header_name("X-Correlation-Id"),
            "x-correlation-id"
        );
        assert_eq!(request_id_header_name("x-request-id"), "x-request-id");
        for unusable in [
            "authorization",
            "Content-Length",
            "cookie",
            "not a header",
            "",
        ] {
            assert_eq!(
                request_id_header_name(unusable),
                "x-request-id",
                "{unusable:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_request_that_skipped_the_middleware_has_an_empty_request_id() {
        let (mut parts, ()) = axum::http::Request::new(()).into_parts();
        let Ok(id) = RequestId::from_request_parts(&mut parts, &()).await;
        assert_eq!(id.as_str(), "");

        parts
            .extensions
            .insert(RequestId(HeaderValue::from_static("abc-123")));
        let Ok(id) = RequestId::from_request_parts(&mut parts, &()).await;
        assert_eq!(id.as_str(), "abc-123");
    }

    #[test]
    fn parses_w3c_traceparent() {
        let h = headers(&[(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )]);
        assert_eq!(inbound_trace_id(&h), "4bf92f3577b34da6a3ce929d0e0e4736");
    }

    #[test]
    fn parses_b3_single_and_multi() {
        let single = headers(&[("b3", "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1")]);
        assert_eq!(
            inbound_trace_id(&single),
            "80f198ee56343ba864fe8b2a57d3eff7"
        );
        let multi = headers(&[("x-b3-traceid", "A3CE929D0E0E4736A3CE929D0E0E4736")]);
        assert_eq!(inbound_trace_id(&multi), "a3ce929d0e0e4736a3ce929d0e0e4736");
    }

    #[test]
    fn rejects_malformed_trace_ids() {
        assert_eq!(inbound_trace_id(&headers(&[])), "");
        // wrong length / non-hex are ignored
        assert_eq!(
            inbound_trace_id(&headers(&[("traceparent", "00-xyz-span-01")])),
            ""
        );
        assert_eq!(inbound_trace_id(&headers(&[("b3", "nothex-span")])), "");
    }

    #[test]
    fn response_level_tracks_status_class() {
        use axum::http::StatusCode;
        use tracing::Level;

        // success is quiet (debug), so it stays hidden at the default info filter
        assert_eq!(level_for_status(StatusCode::OK).0, Level::DEBUG);
        assert_eq!(level_for_status(StatusCode::NO_CONTENT).0, Level::DEBUG);
        // client errors (bad key, unknown model) surface as warnings — the ROL-230
        // case that was invisible before
        assert_eq!(level_for_status(StatusCode::NOT_FOUND).0, Level::WARN);
        assert_eq!(level_for_status(StatusCode::UNAUTHORIZED).0, Level::WARN);
        assert_eq!(
            level_for_status(StatusCode::TOO_MANY_REQUESTS).0,
            Level::WARN
        );
        // server / upstream errors are loud
        assert_eq!(level_for_status(StatusCode::BAD_GATEWAY).0, Level::ERROR);
        assert_eq!(
            level_for_status(StatusCode::INTERNAL_SERVER_ERROR).0,
            Level::ERROR
        );
    }

    #[test]
    fn traceparent_wins_over_b3() {
        let h = headers(&[
            (
                "traceparent",
                "00-11111111111111111111111111111111-2222222222222222-01",
            ),
            ("b3", "33333333333333333333333333333333-4444444444444444-1"),
        ]);
        assert_eq!(inbound_trace_id(&h), "11111111111111111111111111111111");
    }

    #[test]
    fn outbound_headers_adds_only_allowlisted_client_headers() {
        let h = headers(&[
            ("traceparent", "00-1111-2222-01"),
            ("x-tenant-id", "acme"),
            ("x-secret", "nope"),
            ("x-blank", ""),
        ]);
        let forwarded = vec!["x-tenant-id".to_string(), "x-blank".to_string()];
        let out = outbound_headers(&h, &forwarded);

        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out.contains(&("traceparent".to_string(), "00-1111-2222-01".to_string())));
        assert!(out.contains(&("x-tenant-id".to_string(), "acme".to_string())));
    }

    #[test]
    fn outbound_headers_never_lets_the_allowlist_duplicate_trace_context() {
        // listing a trace header must not emit it twice: the second copy would
        // be a caller-controlled value racing the context the gateway resolved
        let h = headers(&[("traceparent", "00-1111-2222-01")]);
        let out = outbound_headers(&h, &["traceparent".to_string()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, "00-1111-2222-01");
    }

    #[test]
    fn without_a_pipeline_outbound_headers_still_copies_the_caller_verbatim() {
        // #805 changed where trace context comes from, but only when an OTLP
        // pipeline is installed. with none — the default — the caller's headers
        // are copied exactly as before, so an untraced deployment sees no change
        assert!(!rolter_core::telemetry::is_active());
        let h = headers(&[
            (
                "traceparent",
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            ),
            ("tracestate", "rojo=00f067aa0ba902b7"),
        ]);
        let out = outbound_headers(&h, &[]);
        assert_eq!(out.len(), 2);
        assert!(out.contains(&(
            "traceparent".to_string(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string()
        )));
    }

    #[test]
    fn the_request_span_is_disabled_without_a_pipeline_and_real_with_one() {
        use tower_http::trace::MakeSpan;

        // the trap this pins: `DefaultMakeSpan` builds the request span at DEBUG,
        // which the default `info` filter disables — and a disabled span cannot
        // be parented or exported, so every stage span became a root carrying a
        // freshly-minted trace id. with no pipeline that stock behaviour is what
        // we want (it costs nothing); with one, the span must be real
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .body(())
            .unwrap();

        assert!(!rolter_core::telemetry::is_active());
        let span = GatewayMakeSpan.make_span(&req);
        assert_eq!(
            span.metadata().map(|m| *m.level()),
            Some(tracing::Level::DEBUG),
            "without a pipeline the request span stays the cheap DEBUG one"
        );
    }

    #[test]
    fn request_trace_id_falls_back_to_the_inbound_header() {
        // with no pipeline there is no span context to read, so the log keeps
        // taking the id off the wire exactly as it did before
        let h = headers(&[(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )]);
        assert_eq!(request_trace_id(&h), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(request_trace_id(&headers(&[])), "");
    }

    #[test]
    fn outbound_headers_with_an_empty_allowlist_is_just_trace_context() {
        let h = headers(&[("traceparent", "00-1111-2222-01"), ("x-tenant-id", "acme")]);
        assert_eq!(outbound_headers(&h, &[]).len(), 1);
    }

    /// The property that matters is the *open list*, not a fixed set: Anthropic
    /// ships new capabilities as new `anthropic-beta` values, so a beta nobody
    /// has heard of must still arrive intact with an empty allowlist (#1013).
    #[test]
    fn unknown_anthropic_headers_pass_through_without_an_allowlist_entry() {
        let h = headers(&[
            ("traceparent", "00-1111-2222-01"),
            (
                "anthropic-beta",
                "some-capability-nobody-has-shipped-yet-2031-01-01",
            ),
            ("anthropic-version", "2099-12-31"),
            ("anthropic-some-future-header", "whatever"),
            ("x-not-anthropic", "dropped"),
        ]);
        let out = outbound_headers(&h, &[]);

        let by_name: std::collections::HashMap<&str, &str> =
            out.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(
            by_name.get("anthropic-beta"),
            Some(&"some-capability-nobody-has-shipped-yet-2031-01-01")
        );
        assert_eq!(by_name.get("anthropic-version"), Some(&"2099-12-31"));
        assert_eq!(
            by_name.get("anthropic-some-future-header"),
            Some(&"whatever")
        );
        assert!(
            !by_name.contains_key("x-not-anthropic"),
            "the prefix must not widen into an allow-everything: {out:?}"
        );
    }

    /// A name merely *starting with* the letters is not in the namespace. The
    /// match is on `anthropic-`, so `anthropicfoo` stays out.
    #[test]
    fn the_vendor_prefix_requires_the_separator() {
        let h = headers(&[("anthropicfoo", "no"), ("anthropic-beta", "yes")]);
        let out = outbound_headers(&h, &[]);
        assert!(out.iter().any(|(k, _)| k == "anthropic-beta"));
        assert!(!out.iter().any(|(k, _)| k == "anthropicfoo"));
    }

    /// An operator who also listed the header by name gets one copy, not two —
    /// a duplicate would reach the upstream comma-joined into one value.
    #[test]
    fn an_allowlisted_anthropic_header_is_not_forwarded_twice() {
        let h = headers(&[("anthropic-beta", "context-management-2025-06-27")]);
        let out = outbound_headers(&h, &["anthropic-beta".to_string()]);
        assert_eq!(
            out.iter().filter(|(k, _)| k == "anthropic-beta").count(),
            1,
            "{out:?}"
        );
    }

    /// Empty values are dropped, matching how the allowlist path already treats
    /// them: an empty beta header states no capability.
    #[test]
    fn blank_anthropic_headers_are_dropped() {
        let h = headers(&[("anthropic-beta", "")]);
        assert!(!outbound_headers(&h, &[])
            .iter()
            .any(|(k, _)| k == "anthropic-beta"));
    }

    #[test]
    fn outbound_trace_headers_extracts_known_headers() {
        let h = headers(&[
            ("traceparent", "00-1111-2222-01"),
            ("tracestate", "rojo=00f067aa0ba902b7"),
            ("b3", "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1"),
            ("unknown-header", "some-value"),
            ("x-b3-traceid", ""),
        ]);
        let outbound = outbound_trace_headers(&h);
        assert_eq!(outbound.len(), 3);

        let outbound_map: std::collections::HashMap<&str, String> = outbound.into_iter().collect();
        assert_eq!(outbound_map.get("traceparent").unwrap(), "00-1111-2222-01");
        assert_eq!(
            outbound_map.get("tracestate").unwrap(),
            "rojo=00f067aa0ba902b7"
        );
        assert_eq!(
            outbound_map.get("b3").unwrap(),
            "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1"
        );
    }
}
