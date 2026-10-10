//! What a route's `advanced` block changes about one upstream call (#2924).
//!
//! The gateway builds a [`RouteOverrides`] once per route, when it builds a
//! snapshot, and hands a reference to the forwarder with every call. The
//! request path therefore never parses a header name or value, and a route
//! that sets nothing carries an empty value that costs one branch.

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::RequestBuilder;
use rolter_core::{is_reserved_route_header, AdvancedModelConfig};

/// A route's static upstream headers, parsed.
///
/// An entry the wire cannot carry, or whose name is reserved (a credential or
/// a framing header, see [`rolter_core::RESERVED_ROUTE_HEADERS`]), is left out
/// here as well as on write and in the snapshot. Skipping it is the last line:
/// a single bad entry would otherwise be a request-builder error on every call
/// the route makes.
#[derive(Debug, Clone, Default)]
pub struct RouteHeaders {
    entries: Vec<RouteHeader>,
}

#[derive(Debug, Clone)]
struct RouteHeader {
    name: HeaderName,
    value: HeaderValue,
    /// a caller's own header of this name may not replace it
    locked: bool,
}

impl RouteHeaders {
    /// Parse the headers of `advanced`, in name order so the result does not
    /// depend on hash-map iteration.
    pub fn compile(advanced: &AdvancedModelConfig) -> Self {
        let mut names: Vec<&String> = advanced.headers.keys().collect();
        names.sort();
        let entries = names
            .into_iter()
            .filter(|name| !is_reserved_route_header(name))
            .filter_map(|name| {
                let value = &advanced.headers[name];
                let header_name = HeaderName::from_bytes(name.as_bytes()).ok()?;
                let mut header_value = HeaderValue::from_str(value).ok()?;
                // a route header may carry a tenant id or a gateway token; it
                // should not show up in a debug print of the request
                header_value.set_sensitive(true);
                Some(RouteHeader {
                    locked: advanced
                        .locked_headers
                        .iter()
                        .any(|locked| locked.eq_ignore_ascii_case(name)),
                    name: header_name,
                    value: header_value,
                })
            })
            .collect();
        Self { entries }
    }

    /// Whether the route sets no header at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Put the route's headers on `request`.
    ///
    /// They replace a header of the same name set before them (the
    /// deployment-wide injected headers, a forwarded caller header), with one
    /// exception: a header the caller sent and the route did not lock is the
    /// caller's to choose, so the route's value is its default and stays out.
    /// `caller` is the headers the gateway is forwarding from the caller, which
    /// is only what the deployment's `forwarded_headers` allow and the trace
    /// context; a caller cannot send an arbitrary header upstream.
    pub fn apply(&self, request: RequestBuilder, caller: &[(&str, &str)]) -> RequestBuilder {
        if self.entries.is_empty() {
            return request;
        }
        let mut headers = HeaderMap::with_capacity(self.entries.len());
        for entry in &self.entries {
            let caller_sent = caller
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(entry.name.as_str()));
            if caller_sent && !entry.locked {
                continue;
            }
            headers.insert(entry.name.clone(), entry.value.clone());
        }
        // `headers` replaces rather than appends, which is what makes a locked
        // header beat the caller's copy of it
        request.headers(headers)
    }
}

/// Per-route settings for one upstream call.
///
/// `Default` is a route that sets nothing: the forwarder behaves exactly as it
/// does for a call that never heard of routes.
#[derive(Debug, Clone, Default)]
pub struct RouteOverrides {
    /// bound on the wait for response headers, replacing the deployment's
    /// `[timeouts].request_secs` for this call
    pub request_timeout: Option<Duration>,
    pub headers: Arc<RouteHeaders>,
}

impl RouteOverrides {
    /// The overrides `advanced` asks for.
    pub fn from_advanced(advanced: &AdvancedModelConfig) -> Self {
        Self {
            request_timeout: advanced
                .limits
                .timeout_secs
                .filter(|secs| *secs > 0)
                .map(|secs| Duration::from_secs(u64::from(secs))),
            headers: Arc::new(RouteHeaders::compile(advanced)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advanced(headers: &[(&str, &str)], locked: &[&str]) -> AdvancedModelConfig {
        AdvancedModelConfig {
            headers: headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            locked_headers: locked.iter().map(|name| name.to_string()).collect(),
            ..Default::default()
        }
    }

    fn sent(headers: &RouteHeaders, caller: &[(&str, &str)]) -> HeaderMap {
        let client = reqwest::Client::new();
        let request = client
            .get("http://127.0.0.1:1/")
            .header("x-injected", "from-policy")
            .header("x-tenant", "from-caller");
        headers
            .apply(request, caller)
            .build()
            .unwrap()
            .headers()
            .clone()
    }

    #[test]
    fn a_reserved_or_unsendable_header_is_left_out() {
        let headers = RouteHeaders::compile(&advanced(
            &[
                ("x-model-region", "eu"),
                ("Authorization", "Bearer other"),
                ("x-api-key", "other"),
                ("host", "elsewhere"),
                ("bad name", "v"),
                ("x-newline", "a\nb"),
            ],
            &[],
        ));
        let map = sent(&headers, &[]);
        assert_eq!(map["x-model-region"], "eu");
        for refused in [
            "authorization",
            "x-api-key",
            "host",
            "bad name",
            "x-newline",
        ] {
            assert!(map.get(refused).is_none(), "{refused} must not be sent");
        }
    }

    #[test]
    fn a_route_header_replaces_the_deployment_wide_one() {
        let headers = RouteHeaders::compile(&advanced(&[("x-injected", "from-route")], &[]));
        let map = sent(&headers, &[]);
        let values: Vec<_> = map.get_all("x-injected").iter().collect();
        assert_eq!(values, ["from-route"], "one value, the route's");
    }

    #[test]
    fn a_caller_header_beats_an_unlocked_route_header() {
        let headers = RouteHeaders::compile(&advanced(&[("x-tenant", "from-route")], &[]));
        let map = sent(&headers, &[("x-tenant", "from-caller")]);
        assert_eq!(map["x-tenant"], "from-caller");
    }

    #[test]
    fn a_locked_route_header_beats_the_callers() {
        let headers =
            RouteHeaders::compile(&advanced(&[("X-Tenant", "from-route")], &["x-tenant"]));
        let map = sent(&headers, &[("x-tenant", "from-caller")]);
        let values: Vec<_> = map.get_all("x-tenant").iter().collect();
        assert_eq!(values, ["from-route"], "the caller's copy is replaced");
    }

    #[test]
    fn a_route_without_headers_touches_nothing() {
        let headers = RouteHeaders::compile(&AdvancedModelConfig::default());
        assert!(headers.is_empty());
        let map = sent(&headers, &[]);
        assert_eq!(map["x-injected"], "from-policy");
        assert_eq!(map["x-tenant"], "from-caller");
    }

    #[test]
    fn the_timeout_is_whole_seconds_and_zero_means_unset() {
        let with = |timeout_secs| {
            let mut advanced = AdvancedModelConfig::default();
            advanced.limits.timeout_secs = timeout_secs;
            RouteOverrides::from_advanced(&advanced).request_timeout
        };
        assert_eq!(with(None), None);
        assert_eq!(
            with(Some(0)),
            None,
            "a stored zero must not mean no timeout"
        );
        assert_eq!(with(Some(7)), Some(Duration::from_secs(7)));
    }
}
