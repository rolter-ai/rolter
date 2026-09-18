//! Runtime configuration handed to the dashboard SPA.
//!
//! The dashboard is built ahead of time and served as static assets, so
//! anything that varies per deployment cannot be baked in at build time. The
//! control plane instead injects a `window.__ROLTER_CONFIG__` block into
//! `index.html`, which is where `ui/src/lib/telemetry.ts` reads its OTLP
//! endpoint from (#805). Without that block the dashboard's tracing is inert:
//! it loads no SDK, exports nothing and changes no headers.
//!
//! The block is rendered once at startup rather than per request. It is a few
//! hundred bytes and cannot change without a restart, so re-reading the file on
//! every dashboard load would buy nothing.

use serde::Serialize;

/// Mirrors `RolterRuntimeConfig` in `ui/src/lib/telemetry.ts`.
///
/// Field names are camelCase because TypeScript reads them straight off
/// `window.__ROLTER_CONFIG__`; nothing deserializes this back into Rust, so the
/// wire names are the contract and must track that interface by hand.
#[derive(Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiRuntimeConfig {
    /// OTLP/HTTP traces endpoint the browser exporter POSTs to, e.g.
    /// `http://localhost:4318/v1/traces`. When absent the dashboard starts no
    /// tracing at all, which is the default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_endpoint: Option<String>,
    /// reported as `service.name`; the dashboard defaults it to `rolter-ui`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_service_name: Option<String>,
    /// reported as `service.version`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// true when the control plane is serving with no admin token, so every
    /// request is treated as superadmin (#970). The dashboard renders a
    /// persistent banner while this holds — an operator otherwise cannot tell a
    /// wide-open control plane from a properly gated one by looking at it.
    ///
    /// Omitted when false so the common case injects nothing extra; the
    /// dashboard reads a missing field as "gated".
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub open_mode: bool,
    /// base URL of the user documentation site the dashboard deep-links into,
    /// e.g. `https://docs.example.com` (#1164). Absent — the default — means
    /// this deployment has no documentation host, and the dashboard suppresses
    /// every documentation link rather than rendering one that cannot resolve.
    /// An air-gapped operator mirroring the site internally points this at the
    /// mirror; nothing else in the dashboard reaches the documentation host
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_base_url: Option<String>,
}

impl UiRuntimeConfig {
    /// Whether anything at all is configured. Used only to decide what to log
    /// at startup — an empty block is still injected, so the dashboard can tell
    /// "served by a control plane that sets no config" from "opened as a file".
    pub(crate) fn is_configured(&self) -> bool {
        self.otel_endpoint.is_some()
    }
}

/// Render the injected `<script>` element.
///
/// `serde_json` emits `<` literally, so a configured value containing
/// `</script>` would close the element early and turn config into markup.
/// Escaping `<` to `\u003c` is the standard guard: it stays valid JSON, parses
/// back to the identical string in JavaScript, and cannot terminate the tag.
fn script_tag(config: &UiRuntimeConfig) -> String {
    // serializing a struct of Option<String> cannot fail, but the hot path
    // should not unwrap: an empty block degrades to "tracing off", not a panic
    let json = serde_json::to_string(config).unwrap_or_else(|_| "{}".to_string());
    let escaped = json.replace('<', "\\u003c");
    format!("<script>window.__ROLTER_CONFIG__ = {escaped};</script>")
}

/// Inject the config block into `html`, immediately before `</head>`.
///
/// The block has to run before the app reads it. Vite emits the app as
/// `<script type="module">` *inside* `<head>`, so the injected element lands
/// after it in source order — that is still correct, because a module script is
/// deferred by default and does not execute until parsing finishes, while this
/// classic inline script runs the moment it is parsed. Ordering therefore holds
/// wherever in `<head>` the block goes, and `</head>` is the stable anchor.
///
/// If the marker is somehow absent the block is prepended instead — still ahead
/// of the app, just less tidy.
pub(crate) fn inject(html: &str, config: &UiRuntimeConfig) -> String {
    let tag = script_tag(config);
    match html.find("</head>") {
        Some(at) => {
            let mut out = String::with_capacity(html.len() + tag.len());
            out.push_str(&html[..at]);
            out.push_str(&tag);
            out.push_str(&html[at..]);
            out
        }
        None => format!("{tag}{html}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> UiRuntimeConfig {
        UiRuntimeConfig {
            otel_endpoint: Some("http://localhost:4318/v1/traces".to_string()),
            otel_service_name: Some("rolter-ui".to_string()),
            version: Some("1.4.2".to_string()),
            open_mode: false,
            docs_base_url: None,
        }
    }

    #[test]
    fn injects_before_the_head_close() {
        let html = "<!doctype html><html><head><title>rolter</title></head><body></body></html>";
        let out = inject(html, &config());
        let script = out.find("__ROLTER_CONFIG__").expect("block injected");
        let head_close = out.find("</head>").expect("head preserved");
        assert!(script < head_close, "config must precede </head>: {out}");
        assert!(out.contains("<body></body>"), "body untouched: {out}");
    }

    #[test]
    fn emits_the_camel_case_names_the_dashboard_reads() {
        let out = inject("<head></head>", &config());
        assert!(out.contains("\"otelEndpoint\""), "{out}");
        assert!(out.contains("\"otelServiceName\""), "{out}");
        assert!(out.contains("\"version\""), "{out}");
        // snake_case would silently read as undefined in the dashboard
        assert!(!out.contains("otel_endpoint"), "{out}");
    }

    #[test]
    fn unset_fields_are_omitted_entirely() {
        let out = inject("<head></head>", &UiRuntimeConfig::default());
        assert!(out.contains("window.__ROLTER_CONFIG__ = {};"), "{out}");
    }

    #[test]
    fn a_value_cannot_close_the_script_element() {
        let hostile = UiRuntimeConfig {
            otel_endpoint: Some("</script><script>alert(1)</script>".to_string()),
            ..Default::default()
        };
        let out = inject("<head></head>", &hostile);
        // exactly the tag we opened and closed ourselves, and no injected one
        assert_eq!(out.matches("</script>").count(), 1, "{out}");
        assert!(!out.contains("alert(1)</script>"), "{out}");
        assert!(
            out.contains("\\u003c/script>"),
            "escaped form present: {out}"
        );
    }

    #[test]
    fn prepends_when_there_is_no_head() {
        let out = inject("<body>x</body>", &config());
        assert!(out.starts_with("<script>window.__ROLTER_CONFIG__"), "{out}");
        assert!(out.ends_with("<body>x</body>"), "{out}");
    }

    #[test]
    fn open_mode_reaches_the_dashboard_only_when_it_is_on() {
        // gated is the common case and injects nothing, which the dashboard
        // reads as "no banner"
        let out = inject("<head></head>", &config());
        assert!(!out.contains("openMode"), "{out}");

        let open = UiRuntimeConfig {
            open_mode: true,
            ..Default::default()
        };
        let out = inject("<head></head>", &open);
        assert!(out.contains("\"openMode\":true"), "{out}");
    }

    #[test]
    fn the_docs_base_url_reaches_the_dashboard_only_when_it_is_set() {
        // unset is the default and the air-gapped case: the dashboard reads a
        // missing field as "no documentation host" and renders no link
        let out = inject("<head></head>", &config());
        assert!(!out.contains("docsBaseUrl"), "{out}");

        let documented = UiRuntimeConfig {
            docs_base_url: Some("https://docs.example.com".to_string()),
            ..Default::default()
        };
        let out = inject("<head></head>", &documented);
        assert!(
            out.contains("\"docsBaseUrl\":\"https://docs.example.com\""),
            "{out}"
        );
        // snake_case would read as undefined in the dashboard
        assert!(!out.contains("docs_base_url"), "{out}");
    }

    #[test]
    fn is_configured_tracks_the_endpoint() {
        assert!(config().is_configured());
        assert!(!UiRuntimeConfig::default().is_configured());
    }
}
