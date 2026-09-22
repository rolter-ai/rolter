//! Renders an OpenTelemetry Collector config document from the enabled
//! connector rows (#836).
//!
//! #511 shipped the connector registry (endpoint, auth, sampling rate,
//! on/off) with no delivery path: this is that path. ADR-0026 already decided
//! where per-destination fan-out belongs — an OpenTelemetry Collector's
//! routing, not N in-process SDK exporters, each of which would be its own
//! connection/queue/retry buffer inside the request-serving process, and
//! terminating operator-supplied URLs in the collector keeps that SSRF
//! surface out of the data plane. So this endpoint is not "ship telemetry to
//! N sinks"; it is "render the collector config that does".
//!
//! An operator points their collector at
//! `GET /api/v1/connectors/collector-config` (the OTel Collector's `confmap`
//! HTTP provider accepts a `--config=http://...` source), or fetches it on a
//! schedule and reloads. rolter's own OTLP export (`OTEL_EXPORTER_OTLP_*`)
//! targets that one collector; this document is what routes what it receives
//! out to every enabled connector, each in its own pipeline so its sampling
//! rate and auth apply independently.
//!
//! `sampling_rate` is rendered as a `probabilistic_sampler` processor scoped
//! to that connector's pipeline — sampling is the collector's job now, not
//! something rolter enforces in-process. Backpressure to a slow or dead sink
//! is likewise the collector's problem: it already queues and retries per
//! exporter, which is the whole reason fan-out was moved here.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{http::header, http::StatusCode, Router};
use std::collections::HashMap;
use std::fmt::Write as _;

use rolter_core::Error;
use rolter_store::postgres::crypto::Kek;

use crate::crud::pool;
use crate::rbac::Principal;
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route("/api/v1/connectors/collector-config", get(render))
}

struct ConnectorRow {
    name: String,
    kind: String,
    endpoint: String,
    sampling_rate: f64,
    auth_secret_ref: Option<String>,
    auth_ciphertext: Option<Vec<u8>>,
    auth_nonce: Option<Vec<u8>>,
}

/// One resolved bearer token to render into an exporter's `headers`, or the
/// env-var expression that lets the collector's own environment resolve an
/// external secret-manager reference rolter never has the value for.
enum Auth {
    /// a managed secret rolter decrypted server-side
    Bearer(String),
    /// an external reference (Vault path, k8s secret name, ...); the
    /// collector process's own environment must supply the value
    EnvPlaceholder(String),
    None,
}

async fn render(principal: Principal, State(state): State<ControlState>) -> Response {
    if let Err(err) =
        crate::rbac::authorize_superadmin(&principal, superadmin_cap!("connector", Read))
    {
        return err.into_response();
    }

    let rows: Vec<ConnectorRow> = match sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            bool,
            f64,
            Option<String>,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
        ),
    >(
        "select name, kind, endpoint, enabled, sampling_rate, auth_secret_ref, \
         auth_ciphertext, auth_nonce from observability_connectors order by name",
    )
    .fetch_all(pool(&state))
    .await
    {
        Ok(raw) => raw
            .into_iter()
            .filter(|(_, _, _, enabled, ..)| *enabled)
            .map(
                |(name, kind, endpoint, _enabled, sampling_rate, auth_secret_ref, ct, nonce)| {
                    ConnectorRow {
                        name,
                        kind,
                        endpoint,
                        sampling_rate,
                        auth_secret_ref,
                        auth_ciphertext: ct,
                        auth_nonce: nonce,
                    }
                },
            )
            .collect(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to query observability connectors");
            return crate::crud::ApiError::Core(Error::Store(
                "failed to query observability connectors".to_string(),
            ))
            .into_response();
        }
    };

    let connectors = apply_kill_switch(rows, rolter_core::telemetry::export_enabled());

    let kek = Kek::from_env();
    let yaml = render_yaml(&connectors, kek.as_ref());
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/yaml")],
        yaml,
    )
        .into_response()
}

/// The kill switch (`ROLTER_TELEMETRY_ENABLED`, #812) wins over every
/// connector row: no exporters, no pipelines, regardless of what is enabled
/// in the registry. A pure function taking the flag explicitly, rather than
/// reading the env var inline, so this is testable without mutating
/// process-global state a concurrently-running test could also depend on.
fn apply_kill_switch(rows: Vec<ConnectorRow>, export_enabled: bool) -> Vec<ConnectorRow> {
    if export_enabled {
        rows
    } else {
        Vec::new()
    }
}

fn slugify(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Assign each connector a slug unique within the document. `name` is unique
/// in the table but [`slugify`] is lossy — "My Sink" and "my/sink" both reduce
/// to `my-sink` — and two identical slugs would emit duplicate YAML keys, so
/// the collector would silently keep only the last and drop a tenant's export
/// entirely. Collisions get a numeric suffix instead.
fn unique_slugs(connectors: &[ConnectorRow]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut slugs = Vec::with_capacity(connectors.len());
    for row in connectors {
        let base = slugify(&row.name);
        let count = seen.entry(base.clone()).or_insert(0);
        *count += 1;
        slugs.push(if *count == 1 {
            base
        } else {
            format!("{base}-{count}")
        });
    }
    slugs
}

/// Escape a string for a double-quoted YAML scalar. Endpoints and managed
/// secrets are operator-supplied, so a stray quote or backslash would
/// otherwise emit a document the collector cannot parse.
fn yaml_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn resolve_auth(row: &ConnectorRow, kek: Option<&Kek>) -> Auth {
    if let (Some(ct), Some(nonce)) = (&row.auth_ciphertext, &row.auth_nonce) {
        if let Some(kek) = kek {
            if let Ok(secret) = kek.decrypt(ct, nonce) {
                return Auth::Bearer(secret);
            }
        }
        // a managed secret exists but could not be decrypted (no KEK
        // configured, or it rotated): fail toward no exporter rather than
        // sending unauthenticated traffic to a sink expecting a credential
        return Auth::None;
    }
    if let Some(reference) = &row.auth_secret_ref {
        let env_name = format!(
            "ROLTER_CONNECTOR_{}_TOKEN",
            slugify(reference).to_ascii_uppercase().replace('-', "_")
        );
        return Auth::EnvPlaceholder(env_name);
    }
    Auth::None
}

/// Render the collector document. Kept separate from the handler so the shape
/// can be asserted without a live pool or a running server.
fn render_yaml(connectors: &[ConnectorRow], kek: Option<&Kek>) -> String {
    let mut out = String::new();
    let slugs = unique_slugs(connectors);
    let _ = writeln!(
        out,
        "# rendered by rolter (GET /api/v1/connectors/collector-config); do not edit by hand"
    );
    let _ = writeln!(out, "receivers:");
    let _ = writeln!(out, "  otlp:");
    let _ = writeln!(out, "    protocols:");
    let _ = writeln!(out, "      grpc:");
    let _ = writeln!(out, "        endpoint: 0.0.0.0:4317");
    let _ = writeln!(out, "      http:");
    let _ = writeln!(out, "        endpoint: 0.0.0.0:4318");
    let _ = writeln!(out);

    let _ = writeln!(out, "processors:");
    let _ = writeln!(out, "  memory_limiter:");
    let _ = writeln!(out, "    check_interval: 1s");
    let _ = writeln!(out, "    limit_mib: 256");
    let _ = writeln!(out, "  batch:");
    for (row, slug) in connectors.iter().zip(&slugs) {
        if row.sampling_rate < 1.0 {
            let _ = writeln!(out, "  probabilistic_sampler/{slug}:");
            let _ = writeln!(
                out,
                "    sampling_percentage: {:.4}",
                row.sampling_rate * 100.0
            );
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "exporters:");
    for (row, slug) in connectors.iter().zip(&slugs) {
        // kind is a check-constrained column; a value this build does not know
        // how to export is left undeployed rather than rendered as a broken
        // exporter block a collector would refuse to start on
        if row.kind != "otlp_http" {
            continue;
        }
        let _ = writeln!(out, "  otlphttp/{slug}:");
        let _ = writeln!(out, "    endpoint: {}", yaml_quote(&row.endpoint));
        match resolve_auth(row, kek) {
            Auth::Bearer(token) => {
                let _ = writeln!(out, "    headers:");
                let _ = writeln!(
                    out,
                    "      Authorization: {}",
                    yaml_quote(&format!("Bearer {token}"))
                );
            }
            Auth::EnvPlaceholder(env_name) => {
                let _ = writeln!(out, "    headers:");
                let _ = writeln!(
                    out,
                    "      Authorization: \"Bearer ${{env:{env_name}}}\"  # set {env_name} in the collector's own environment"
                );
            }
            Auth::None => {}
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "service:");
    let _ = writeln!(out, "  pipelines:");
    for signal in ["traces", "metrics", "logs"] {
        for (row, slug) in connectors.iter().zip(&slugs) {
            if row.kind != "otlp_http" {
                continue;
            }
            let _ = writeln!(out, "    {signal}/{slug}:");
            let _ = writeln!(out, "      receivers: [otlp]");
            if row.sampling_rate < 1.0 {
                let _ = writeln!(
                    out,
                    "      processors: [memory_limiter, probabilistic_sampler/{slug}, batch]"
                );
            } else {
                let _ = writeln!(out, "      processors: [memory_limiter, batch]");
            }
            let _ = writeln!(out, "      exporters: [otlphttp/{slug}]");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, sampling_rate: f64) -> ConnectorRow {
        ConnectorRow {
            name: name.to_string(),
            kind: "otlp_http".to_string(),
            endpoint: "https://collector.example.com/v1/logs".to_string(),
            sampling_rate,
            auth_secret_ref: None,
            auth_ciphertext: None,
            auth_nonce: None,
        }
    }

    #[test]
    fn no_connectors_renders_a_config_with_no_exporters_or_pipelines() {
        let yaml = render_yaml(&[], None);
        assert!(yaml.contains("receivers:"));
        assert!(yaml.contains("exporters:"));
        assert!(!yaml.contains("otlphttp/"));
        assert!(!yaml.contains("traces/"));
    }

    #[test]
    fn each_enabled_connector_gets_its_own_exporter_and_pipelines() {
        let yaml = render_yaml(&[row("SigNoz", 1.0), row("Datadog Staging", 1.0)], None);
        assert!(yaml.contains("otlphttp/signoz"));
        assert!(yaml.contains("otlphttp/datadog-staging"));
        assert!(yaml.contains("traces/datadog-staging"));
        assert!(yaml.contains("metrics/datadog-staging"));
        assert!(yaml.contains("logs/datadog-staging"));
    }

    #[test]
    fn names_that_slugify_alike_still_get_distinct_pipelines() {
        // `name` is unique in the table but slugify is lossy, so without
        // de-duplication both rows would emit the same YAML keys and the
        // collector would silently drop one connector's export entirely
        let yaml = render_yaml(&[row("My Sink", 1.0), row("my/sink", 1.0)], None);
        assert!(yaml.contains("otlphttp/my-sink:"));
        assert!(yaml.contains("otlphttp/my-sink-2:"));
        assert_eq!(yaml.matches("exporters: [otlphttp/my-sink]").count(), 3);
        assert_eq!(yaml.matches("exporters: [otlphttp/my-sink-2]").count(), 3);
    }

    #[test]
    fn an_endpoint_containing_yaml_metacharacters_stays_quoted() {
        let mut r = row("edge", 1.0);
        // a '#' would start a comment and swallow the rest of an unquoted scalar
        r.endpoint = "https://collector.example.com/v1/logs?q=a#frag".to_string();
        let yaml = render_yaml(&[r], None);
        assert!(yaml.contains("endpoint: \"https://collector.example.com/v1/logs?q=a#frag\""));
    }

    #[test]
    fn a_secret_containing_a_quote_is_escaped_rather_than_breaking_the_document() {
        assert_eq!(yaml_quote(r#"tok"en"#), r#""tok\"en""#);
        assert_eq!(yaml_quote(r"tok\en"), r#""tok\\en""#);
    }

    #[test]
    fn full_sampling_rate_adds_no_sampler_processor() {
        let yaml = render_yaml(&[row("signoz", 1.0)], None);
        assert!(!yaml.contains("probabilistic_sampler"));
        assert!(yaml.contains("processors: [memory_limiter, batch]"));
    }

    #[test]
    fn partial_sampling_rate_adds_a_scoped_sampler_processor() {
        let yaml = render_yaml(&[row("signoz", 0.25)], None);
        assert!(yaml.contains("probabilistic_sampler/signoz:"));
        assert!(yaml.contains("sampling_percentage: 25.0000"));
        assert!(yaml.contains("processors: [memory_limiter, probabilistic_sampler/signoz, batch]"));
    }

    #[test]
    fn a_kind_this_build_cannot_export_is_left_undeployed() {
        let mut r = row("datadog", 1.0);
        r.kind = "datadog".to_string();
        let yaml = render_yaml(&[r], None);
        assert!(!yaml.contains("otlphttp/datadog"));
        assert!(!yaml.contains("traces/datadog"));
    }

    #[test]
    fn an_external_secret_reference_renders_as_an_env_placeholder() {
        let mut r = row("signoz", 1.0);
        r.auth_secret_ref = Some("vault:secret/signoz-token".to_string());
        let yaml = render_yaml(&[r], None);
        assert!(yaml.contains("${env:ROLTER_CONNECTOR_"));
        assert!(yaml.contains("Bearer $"));
    }

    #[test]
    fn the_kill_switch_wins_over_every_enabled_connector() {
        let rows = vec![row("signoz", 1.0), row("datadog", 1.0)];
        assert_eq!(apply_kill_switch(rows, false).len(), 0);
    }

    #[test]
    fn export_enabled_leaves_every_row_in_place() {
        let rows = vec![row("signoz", 1.0), row("datadog", 1.0)];
        assert_eq!(apply_kill_switch(rows, true).len(), 2);
    }

    #[test]
    fn names_with_spaces_and_punctuation_slugify_to_a_valid_component_name() {
        let yaml = render_yaml(&[row("My Signoz (EU)!", 1.0)], None);
        assert!(yaml.contains("otlphttp/my-signoz--eu--"));
    }

    #[tokio::test]
    async fn render_error_redacts_store_details() {
        let err = sqlx::Error::RowNotFound;
        let api_err = crate::crud::ApiError::Core(Error::Store(
            "failed to query observability connectors".to_string(),
        ));
        let resp = api_err.into_response();
        assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body_str.contains("failed to query observability connectors"));
        assert!(!body_str.contains(&err.to_string()));
    }
}
