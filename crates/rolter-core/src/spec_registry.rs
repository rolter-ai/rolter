//! The external specification source registry (#907).
//!
//! `specifications.toml` at the root of this crate records every external API,
//! protocol, semantic convention and wire format rolter implements, translates,
//! emits, parses or validates, and one entry per [`ProviderKind`]. This module
//! is its schema (every table denies unknown keys), its validator
//! ([`Registry::validate`]) and the one consumer that publishes it at run time:
//! [`annotate_openapi`], which gives both planes' `GET /openapi.json` its
//! `externalDocs` and `x-rolter-*` provenance.
//!
//! The registry is the only place a reviewed version or URL is written down.
//! Code points at a record with a `spec-ref` comment naming its id rather
//! than copying the link, and a workspace test fails when a comment names an
//! id the registry does not have. A runtime constant exists only where rolter
//! sends or checks the value on the wire — [`crate::probe::ANTHROPIC_VERSION`]
//! is one, and a test holds it to the `anthropic-messages` record — because a
//! constant that only describes a version goes stale without anything
//! noticing.
//!
//! The field reference and the rules are in
//! `docs/dev-docs/development/spec-registry.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::ProviderKind;

/// The registry file, embedded so a binary serves its claims without the tree.
pub const SOURCE: &str = include_str!("../specifications.toml");

/// Where the registry is published, for the OpenAPI documents' `externalDocs`.
pub const REGISTRY_URL: &str =
    "https://github.com/rolter-ai/rolter/blob/master/crates/rolter-core/specifications.toml";

/// The schema version this module reads.
pub const SCHEMA_VERSION: u32 = 1;

/// The parsed registry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub schema_version: u32,
    /// rolter's own endpoints, which implement no external specification
    #[serde(default)]
    pub native: Native,
    #[serde(default, rename = "spec")]
    pub specs: Vec<Spec>,
    #[serde(default, rename = "provider")]
    pub providers: Vec<Provider>,
}

/// Endpoints that are rolter's own rather than any vendor's.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Native {
    #[serde(default)]
    pub gateway_endpoints: Vec<String>,
}

/// One external specification and what rolter claims about it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Stable local id, the target of `spec-ref` comments and of
    /// `x-rolter-spec-refs`. Published: never renamed.
    pub id: String,
    pub category: Category,
    pub authority: String,
    pub title: String,
    /// The official primary source, always HTTPS.
    pub url: String,
    #[serde(default)]
    pub related: Vec<String>,
    pub contract: Contract,
    /// The revision implemented or sent on the wire; required by every
    /// versioned [`Contract`], and never "latest".
    #[serde(default)]
    pub version: Option<String>,
    /// An immutable upstream commit the version was audited at.
    #[serde(default)]
    pub pinned: Option<String>,
    /// When the claim was last checked against the source, `YYYY-MM-DD`.
    pub reviewed_at: String,
    pub conformance: Conformance,
    /// `METHOD /path` on the gateway, in OpenAPI path-template form.
    #[serde(default)]
    pub gateway_endpoints: Vec<String>,
    /// `METHOD /path` on the control plane.
    #[serde(default)]
    pub control_endpoints: Vec<String>,
    /// `components/schemas` names in the gateway's OpenAPI document.
    #[serde(default)]
    pub gateway_schemas: Vec<String>,
    /// Source files that implement the claim, relative to the repository root.
    #[serde(default)]
    pub modules: Vec<String>,
    #[serde(default)]
    pub supported: Vec<String>,
    #[serde(default)]
    pub omissions: Vec<String>,
    #[serde(default)]
    pub deviations: Vec<String>,
    /// `path/to/file.rs` or `path/to/file.rs::test_name`.
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// One [`ProviderKind`]: where its API is documented and how rolter reaches it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub kind: ProviderKind,
    pub authority: String,
    pub url: String,
    /// An example `api_base`, where one is documented.
    #[serde(default)]
    pub base: Option<String>,
    /// Ids of the [`Spec`] records this kind speaks.
    pub dialects: Vec<String>,
    pub auth: AuthScheme,
    /// The path rolter sends the kind's primary request to.
    pub primary_path: String,
    pub conformance: Conformance,
    pub reviewed_at: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub tests: Vec<String>,
}

/// What kind of contract a record describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    AiApi,
    Translation,
    WireFormat,
    Transport,
    Identity,
    Telemetry,
    ApiDescription,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::AiApi => "ai-api",
            Category::Translation => "translation",
            Category::WireFormat => "wire-format",
            Category::Transport => "transport",
            Category::Identity => "identity",
            Category::Telemetry => "telemetry",
            Category::ApiDescription => "api-description",
        }
    }
}

/// How the upstream source versions itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Contract {
    /// A dated revision such as Anthropic's `2023-06-01`.
    Dated,
    /// `MAJOR.MINOR.PATCH`.
    Semver,
    /// A major version that evolves by backward-compatible additions, such as
    /// OpenAI's REST `v1`: it has no finer revision to record.
    MajorOnly,
    /// A living document with no revision; `reviewed_at` is the claim's date.
    Living,
    /// An IETF RFC.
    Rfc,
    /// A published standard from another body (W3C, OpenID Foundation).
    Standard,
    /// A vendor surface marked preview or beta.
    VendorPreview,
}

impl Contract {
    pub fn as_str(self) -> &'static str {
        match self {
            Contract::Dated => "dated",
            Contract::Semver => "semver",
            Contract::MajorOnly => "major-only",
            Contract::Living => "living",
            Contract::Rfc => "rfc",
            Contract::Standard => "standard",
            Contract::VendorPreview => "vendor-preview",
        }
    }

    /// Whether a record of this contract must name its version.
    pub fn is_versioned(self) -> bool {
        !matches!(self, Contract::Living | Contract::VendorPreview)
    }
}

/// How much of a specification rolter honours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Conformance {
    /// The vendor's wire format, forwarded or served unchanged.
    NativePassthrough,
    /// rolter rewrites between dialects, for the subset it lists.
    TranslationSubset,
    /// Bytes are relayed; rolter neither validates nor negotiates the payload.
    TransportOnly,
    /// The vendor's shape plus rolter's own additions.
    Extension,
    /// Some of the specification, as listed.
    Partial,
    /// A provider's auth/path profile over an OpenAI-shaped transport.
    ProviderProfile,
    /// A provider kind rolter ships a profile for, but whose public API
    /// reference was not found: whether it speaks an OpenAI-compatible API at
    /// a stable base URL is not established (#3428).
    Unverified,
}

impl Conformance {
    pub fn as_str(self) -> &'static str {
        match self {
            Conformance::NativePassthrough => "native-passthrough",
            Conformance::TranslationSubset => "translation-subset",
            Conformance::TransportOnly => "transport-only",
            Conformance::Extension => "extension",
            Conformance::Partial => "partial",
            Conformance::ProviderProfile => "provider-profile",
            Conformance::Unverified => "unverified",
        }
    }
}

/// The header a provider kind's credential travels in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthScheme {
    /// `Authorization: Bearer <key>`
    Bearer,
    /// `x-api-key: <key>`
    XApiKey,
    /// `api-key: <key>`
    ApiKey,
    /// `x-goog-api-key: <key>`
    XGoogApiKey,
}

impl AuthScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthScheme::Bearer => "bearer",
            AuthScheme::XApiKey => "x-api-key",
            AuthScheme::ApiKey => "api-key",
            AuthScheme::XGoogApiKey => "x-goog-api-key",
        }
    }

    /// The lowercase header name, as [`ProviderKind::auth_header`] spells it.
    pub fn header(self) -> &'static str {
        match self {
            AuthScheme::Bearer => "authorization",
            other => other.as_str(),
        }
    }
}

/// Which plane's OpenAPI document an endpoint belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    Gateway,
    Control,
}

/// Parse a registry document.
pub fn parse(source: &str) -> Result<Registry, String> {
    toml::from_str(source).map_err(|err| err.to_string())
}

/// The embedded registry, parsed once. `None` only if the embedded file does
/// not parse, which the crate's own tests rule out; callers degrade to serving
/// no provenance rather than failing a request.
pub fn registry() -> Option<&'static Registry> {
    static REGISTRY: OnceLock<Option<Registry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| match parse(SOURCE) {
            Ok(registry) => Some(registry),
            Err(err) => {
                tracing::error!(error = %err, "the embedded specification registry does not parse");
                None
            }
        })
        .as_ref()
}

/// Whether the registry records `kind` as [`Conformance::Unverified`]: no
/// public API reference was located for it (#3428). The dashboard marks such a
/// kind from this rather than from a list of its own.
pub fn is_unverified(kind: ProviderKind) -> bool {
    registry().is_some_and(|registry| {
        registry
            .providers
            .iter()
            .any(|p| p.kind == kind && p.conformance == Conformance::Unverified)
    })
}

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE"];

/// Split `METHOD /path` into its parts, or say what is wrong with it.
pub fn parse_endpoint(endpoint: &str) -> Result<(&str, &str), String> {
    let (method, path) = endpoint
        .split_once(' ')
        .ok_or_else(|| format!("'{endpoint}' is not `METHOD /path`"))?;
    if !METHODS.contains(&method) {
        return Err(format!("'{endpoint}': method must be one of {METHODS:?}"));
    }
    if !path.starts_with('/') || path.contains(char::is_whitespace) || path.contains("{*") {
        return Err(format!(
            "'{endpoint}': the path must be an OpenAPI path template starting with '/'"
        ));
    }
    Ok((method, path))
}

impl Registry {
    /// The record with `id`.
    pub fn spec(&self, id: &str) -> Option<&Spec> {
        self.specs.iter().find(|spec| spec.id == id)
    }

    /// The records that name `method path` on `plane`, in registry order.
    pub fn specs_for<'a>(
        &'a self,
        plane: Plane,
        method: &'a str,
        path: &'a str,
    ) -> impl Iterator<Item = &'a Spec> + 'a {
        self.specs.iter().filter(move |spec| {
            let endpoints = match plane {
                Plane::Gateway => &spec.gateway_endpoints,
                Plane::Control => &spec.control_endpoints,
            };
            endpoints.iter().any(|endpoint| {
                parse_endpoint(endpoint)
                    .is_ok_and(|(m, p)| m.eq_ignore_ascii_case(method) && p == path)
            })
        })
    }

    /// Every rule a record must meet that can be checked without the tree.
    /// Empty when the registry is valid; the workspace test
    /// `crates/rolter-core/tests/spec_registry.rs` adds the checks that read
    /// files (modules, tests, `spec-ref` comments, generated docs).
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.schema_version != SCHEMA_VERSION {
            problems.push(format!(
                "schema_version is {}, this reader understands {SCHEMA_VERSION}",
                self.schema_version
            ));
        }

        let mut ids = BTreeSet::new();
        for spec in &self.specs {
            let at = format!("spec '{}'", spec.id);
            if !is_id(&spec.id) {
                problems.push(format!("{at}: an id is lowercase letters, digits and '-'"));
            }
            if !ids.insert(spec.id.as_str()) {
                problems.push(format!("{at}: the id is used twice"));
            }
            for (field, value) in [("authority", &spec.authority), ("title", &spec.title)] {
                if value.trim().is_empty() {
                    problems.push(format!("{at}: {field} is empty"));
                }
            }
            check_https(&mut problems, &at, "url", &spec.url);
            for url in &spec.related {
                check_https(&mut problems, &at, "related", url);
            }
            check_date(&mut problems, &at, &spec.reviewed_at);
            check_version(&mut problems, &at, spec);
            if matches!(
                spec.conformance,
                Conformance::ProviderProfile | Conformance::Unverified
            ) {
                problems.push(format!(
                    "{at}: {} describes a [[provider]], not a [[spec]]",
                    spec.conformance.as_str()
                ));
            }
            for endpoint in spec.gateway_endpoints.iter().chain(&spec.control_endpoints) {
                if let Err(err) = parse_endpoint(endpoint) {
                    problems.push(format!("{at}: {err}"));
                }
            }
            for (field, list) in [
                ("supported", &spec.supported),
                ("modules", &spec.modules),
                ("tests", &spec.tests),
            ] {
                if list.is_empty() {
                    problems.push(format!("{at}: {field} is empty"));
                }
            }
            for test in &spec.tests {
                check_test_ref(&mut problems, &at, test);
            }
        }

        for endpoint in &self.native.gateway_endpoints {
            match parse_endpoint(endpoint) {
                Err(err) => problems.push(format!("native: {err}")),
                Ok((method, path)) => {
                    if self
                        .specs_for(Plane::Gateway, method, path)
                        .next()
                        .is_some()
                    {
                        problems.push(format!(
                            "native: '{endpoint}' is also claimed by a [[spec]]; it is one or the other"
                        ));
                    }
                }
            }
        }

        let mut seen = BTreeMap::new();
        for provider in &self.providers {
            let at = format!("provider '{:?}'", provider.kind);
            if seen.insert(format!("{:?}", provider.kind), ()).is_some() {
                problems.push(format!("{at}: the kind has two records"));
            }
            if provider.authority.trim().is_empty() {
                problems.push(format!("{at}: authority is empty"));
            }
            check_https(&mut problems, &at, "url", &provider.url);
            check_date(&mut problems, &at, &provider.reviewed_at);
            if provider.dialects.is_empty() {
                problems.push(format!("{at}: dialects is empty"));
            }
            for dialect in &provider.dialects {
                if self.spec(dialect).is_none() {
                    problems.push(format!("{at}: dialect '{dialect}' is not a [[spec]] id"));
                }
            }
            if !provider.primary_path.starts_with('/') {
                problems.push(format!("{at}: primary_path must start with '/'"));
            }
            if provider.tests.is_empty() {
                problems.push(format!("{at}: tests is empty"));
            }
            for test in &provider.tests {
                check_test_ref(&mut problems, &at, test);
            }
        }
        for kind in ProviderKind::ALL {
            if !self.providers.iter().any(|provider| provider.kind == kind) {
                problems.push(format!("provider kind {kind:?} has no [[provider]] record"));
            }
        }
        problems
    }
}

fn is_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn check_https(problems: &mut Vec<String>, at: &str, field: &str, url: &str) {
    let ok = url
        .strip_prefix("https://")
        .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
        && !url.contains(char::is_whitespace);
    if !ok {
        problems.push(format!("{at}: {field} '{url}' is not an https URL"));
    }
}

fn check_date(problems: &mut Vec<String>, at: &str, date: &str) {
    match chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") {
        Err(_) => problems.push(format!("{at}: reviewed_at '{date}' is not YYYY-MM-DD")),
        Ok(day) => {
            // a day of slack for a reviewer ahead of UTC
            let tomorrow = chrono::Utc::now().date_naive() + chrono::Duration::days(1);
            if day > tomorrow {
                problems.push(format!("{at}: reviewed_at '{date}' is in the future"));
            }
        }
    }
}

fn check_version(problems: &mut Vec<String>, at: &str, spec: &Spec) {
    for value in spec.version.iter().chain(&spec.pinned) {
        if value.to_ascii_lowercase().contains("latest") {
            problems.push(format!(
                "{at}: '{value}' says latest; record the revision actually reviewed"
            ));
        }
    }
    if let Some(pinned) = &spec.pinned {
        if pinned.len() != 40 || !pinned.bytes().all(|b| b.is_ascii_hexdigit()) {
            problems.push(format!("{at}: pinned must be a full 40-hex commit"));
        }
    }
    let version = spec.version.as_deref().map(str::trim).unwrap_or("");
    if !spec.contract.is_versioned() {
        return;
    }
    if version.is_empty() {
        problems.push(format!(
            "{at}: a {} contract needs a version",
            spec.contract.as_str()
        ));
        return;
    }
    let shaped = match spec.contract {
        // `forwarded`: rolter relays the version the two ends negotiate
        Contract::Dated => {
            version == "forwarded" || chrono::NaiveDate::parse_from_str(version, "%Y-%m-%d").is_ok()
        }
        Contract::Semver => {
            let parts: Vec<&str> = version.split('.').collect();
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        }
        Contract::MajorOnly => version
            .strip_prefix('v')
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit())),
        _ => true,
    };
    if !shaped {
        problems.push(format!(
            "{at}: version '{version}' is not the shape a {} contract takes",
            spec.contract.as_str()
        ));
    }
}

fn check_test_ref(problems: &mut Vec<String>, at: &str, test: &str) {
    let file = test.split_once("::").map_or(test, |(file, _)| file);
    if !file.starts_with("crates/") || !file.ends_with(".rs") {
        problems.push(format!(
            "{at}: test '{test}' is not `crates/<path>.rs` or `crates/<path>.rs::<name>`"
        ));
    }
}

/// Write the registry's claims into an OpenAPI document of `plane`: an
/// `externalDocs` and the `x-rolter-spec-refs`, `x-rolter-conformance` and
/// `x-rolter-reviewed-at` extensions on every operation (and, on the gateway,
/// every schema) a record names, and a pointer to the registry at the root.
///
/// Only rolter's own document is touched; vendor request and response
/// payloads never carry a rolter field.
pub fn annotate_openapi(doc: &mut Value, plane: Plane) {
    if let Some(registry) = registry() {
        annotate_with(registry, doc, plane);
    }
}

/// [`annotate_openapi`] against a given registry.
pub fn annotate_with(registry: &Registry, doc: &mut Value, plane: Plane) {
    let Some(root) = doc.as_object_mut() else {
        return;
    };
    root.insert(
        "externalDocs".into(),
        json!({
            "description": "the external specifications rolter implements, with the revision \
                reviewed and how much of each it honours",
            "url": REGISTRY_URL,
        }),
    );
    let descriptions: Vec<&Spec> = registry
        .specs
        .iter()
        .filter(|spec| spec.category == Category::ApiDescription)
        .collect();
    insert_refs(root, &descriptions);

    if let Some(paths) = root.get_mut("paths").and_then(Value::as_object_mut) {
        for (path, item) in paths.iter_mut() {
            let Some(item) = item.as_object_mut() else {
                continue;
            };
            for (method, operation) in item.iter_mut() {
                let method = method.to_ascii_uppercase();
                if !METHODS.contains(&method.as_str()) {
                    continue;
                }
                let specs: Vec<&Spec> = registry.specs_for(plane, &method, path).collect();
                if let Some(operation) = operation.as_object_mut() {
                    provenance(operation, &specs);
                }
            }
        }
    }

    if plane == Plane::Gateway {
        let schemas = root
            .get_mut("components")
            .and_then(|components| components.get_mut("schemas"))
            .and_then(Value::as_object_mut);
        if let Some(schemas) = schemas {
            for (name, schema) in schemas.iter_mut() {
                let specs: Vec<&Spec> = registry
                    .specs
                    .iter()
                    .filter(|spec| spec.gateway_schemas.iter().any(|s| s == name))
                    .collect();
                if let Some(schema) = schema.as_object_mut() {
                    provenance(schema, &specs);
                }
            }
        }
    }
}

/// `externalDocs` from the first record that describes the API rather than its
/// framing, plus the three extensions over every record.
fn provenance(target: &mut Map<String, Value>, specs: &[&Spec]) {
    let primary = specs
        .iter()
        .find(|spec| !matches!(spec.category, Category::WireFormat | Category::Transport))
        .or_else(|| specs.first());
    let Some(primary) = primary else {
        return;
    };
    target.insert(
        "externalDocs".into(),
        json!({
            "description": format!("{}: {}", primary.authority, primary.title),
            "url": primary.url,
        }),
    );
    insert_refs(target, specs);
}

fn insert_refs(target: &mut Map<String, Value>, specs: &[&Spec]) {
    if specs.is_empty() {
        return;
    }
    let ids: Vec<Value> = specs.iter().map(|spec| json!(spec.id)).collect();
    let conformance: Map<String, Value> = specs
        .iter()
        .map(|spec| (spec.id.clone(), json!(spec.conformance.as_str())))
        .collect();
    let reviewed: Map<String, Value> = specs
        .iter()
        .map(|spec| (spec.id.clone(), json!(spec.reviewed_at)))
        .collect();
    target.insert("x-rolter-spec-refs".into(), Value::Array(ids));
    target.insert("x-rolter-conformance".into(), Value::Object(conformance));
    target.insert("x-rolter-reviewed-at".into(), Value::Object(reviewed));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded() -> Registry {
        parse(SOURCE).expect("the embedded registry parses")
    }

    #[test]
    fn the_embedded_registry_is_valid() {
        let problems = embedded().validate();
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn every_provider_kind_has_exactly_one_record() {
        let registry = embedded();
        for kind in ProviderKind::ALL {
            let count = registry
                .providers
                .iter()
                .filter(|provider| provider.kind == kind)
                .count();
            assert_eq!(count, 1, "{kind:?} has {count} [[provider]] records");
        }
    }

    // the registry's auth and path for a kind are claims the docs publish, so
    // they are held to the code that actually builds the request
    #[test]
    fn provider_records_match_how_rolter_reaches_each_kind() {
        for provider in embedded().providers {
            let (header, _) = provider.kind.auth_header("key");
            assert_eq!(
                header,
                provider.auth.header(),
                "{:?}: the registry says {}, the proxy sends {header}",
                provider.kind,
                provider.auth.as_str()
            );
            assert_eq!(
                provider.kind.primary_upstream_path(),
                provider.primary_path,
                "{:?}: primary_path",
                provider.kind
            );
        }
    }

    // #907: normal traffic and the health probes once each wrote their own
    // `anthropic-version`. Both now read one constant, the configured default
    // is that constant, and the registry records it
    #[test]
    fn the_anthropic_version_has_one_source() {
        let registry = embedded();
        let record = registry
            .spec("anthropic-messages")
            .expect("anthropic-messages is recorded");
        assert_eq!(
            record.version.as_deref(),
            Some(crate::probe::ANTHROPIC_VERSION)
        );
        assert_eq!(
            crate::CompatibilityConfig::default().anthropic_version,
            crate::probe::ANTHROPIC_VERSION
        );
        // the reference page states the default too. a database-backed
        // deployment reads the stored policy instead, whose column default a
        // migration seeds; moving the constant needs a migration of its own
        let page = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/user-docs/configuration/config-file.mdx"
        ))
        .expect("config-file.mdx is readable");
        let documented = format!(
            "<ParamField path=\"anthropic_version\" default=\"{}\"",
            crate::probe::ANTHROPIC_VERSION
        );
        assert!(
            page.contains(&documented),
            "config-file.mdx does not state the default anthropic_version as {}",
            crate::probe::ANTHROPIC_VERSION
        );
    }

    #[test]
    fn the_embedded_registry_is_what_registry_returns() {
        let registry = registry().expect("the embedded registry parses");
        assert_eq!(registry.specs.len(), embedded().specs.len());
    }

    const MINIMAL: &str = r#"
        schema_version = 1
        [[spec]]
        id = "example"
        category = "ai-api"
        authority = "Example"
        title = "Example API"
        url = "https://example.com/api"
        contract = "dated"
        version = "2024-01-01"
        reviewed_at = "2026-01-01"
        conformance = "partial"
        gateway_endpoints = ["POST /v1/example"]
        gateway_schemas = ["Example"]
        modules = ["crates/x/src/lib.rs"]
        supported = ["everything listed"]
        tests = ["crates/x/src/lib.rs::works"]
    "#;

    /// `MINIMAL` with `from` replaced, validated, ignoring the provider-kind
    /// coverage the fragment does not attempt.
    fn problems_after(from: &str, to: &str) -> Vec<String> {
        assert!(MINIMAL.contains(from), "'{from}' is not in the fixture");
        let registry = parse(&MINIMAL.replace(from, to)).expect("the fixture parses");
        registry
            .validate()
            .into_iter()
            .filter(|problem| !problem.starts_with("provider kind"))
            .collect()
    }

    #[test]
    fn the_minimal_fixture_is_valid() {
        assert!(problems_after("", "").is_empty());
    }

    #[test]
    fn a_versioned_record_may_not_say_latest() {
        let problems = problems_after("\"2024-01-01\"", "\"latest\"");
        assert!(
            problems.iter().any(|p| p.contains("says latest")),
            "{problems:?}"
        );
        let problems = problems_after(
            "contract = \"dated\"\n        version = \"2024-01-01\"",
            "contract = \"semver\"\n        version = \"latest\"",
        );
        assert!(
            problems.iter().any(|p| p.contains("says latest")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_versioned_record_needs_a_version_of_its_shape() {
        let problems = problems_after("version = \"2024-01-01\"", "");
        assert!(
            problems.iter().any(|p| p.contains("needs a version")),
            "{problems:?}"
        );
        let problems = problems_after("\"2024-01-01\"", "\"v1\"");
        assert!(
            problems.iter().any(|p| p.contains("not the shape")),
            "{problems:?}"
        );
        assert!(problems_after("\"2024-01-01\"", "\"forwarded\"").is_empty());
        let living = problems_after("contract = \"dated\"", "contract = \"living\"");
        assert!(living.is_empty(), "{living:?}");
    }

    #[test]
    fn a_primary_source_is_https() {
        let problems = problems_after("https://example.com/api", "http://example.com/api");
        assert!(
            problems.iter().any(|p| p.contains("not an https URL")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_malformed_endpoint_or_test_ref_is_named() {
        let problems = problems_after("POST /v1/example", "FETCH /v1/example");
        assert!(
            problems.iter().any(|p| p.contains("method must be")),
            "{problems:?}"
        );
        let problems = problems_after("POST /v1/example", "POST /v1/{*rest}");
        assert!(
            problems.iter().any(|p| p.contains("path template")),
            "{problems:?}"
        );
        let problems = problems_after("crates/x/src/lib.rs::works", "works");
        assert!(
            problems.iter().any(|p| p.contains("test 'works'")),
            "{problems:?}"
        );
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let err = parse(&MINIMAL.replace("title = ", "titel = \"x\"\n        title = "))
            .expect_err("deny_unknown_fields");
        assert!(err.contains("titel"), "{err}");
    }

    #[test]
    fn every_kind_without_a_record_is_named() {
        let problems = parse(MINIMAL).expect("parses").validate();
        for kind in ProviderKind::ALL {
            let name = format!("provider kind {kind:?} has no [[provider]] record");
            assert!(problems.contains(&name), "{name} not reported");
        }
    }

    #[test]
    fn annotation_carries_provenance_without_touching_payload_schemas() {
        let registry = parse(MINIMAL).expect("parses");
        let mut doc = json!({
            "openapi": "3.1.0",
            "paths": {
                "/v1/example": {"parameters": [], "post": {"responses": {}}},
                "/v1/other": {"get": {"responses": {}}}
            },
            "components": {"schemas": {"Example": {"type": "object"}, "Other": {"type": "object"}}}
        });
        annotate_with(&registry, &mut doc, Plane::Gateway);

        let op = &doc["paths"]["/v1/example"]["post"];
        assert_eq!(op["externalDocs"]["url"], "https://example.com/api");
        assert_eq!(op["x-rolter-spec-refs"], json!(["example"]));
        assert_eq!(op["x-rolter-conformance"]["example"], "partial");
        assert_eq!(op["x-rolter-reviewed-at"]["example"], "2026-01-01");
        assert!(doc["paths"]["/v1/example"]["parameters"].is_array());
        assert!(doc["paths"]["/v1/other"]["get"]["x-rolter-spec-refs"].is_null());

        let schema = &doc["components"]["schemas"]["Example"];
        assert_eq!(schema["x-rolter-spec-refs"], json!(["example"]));
        // the schema's own body is untouched: no rolter field enters a payload
        assert!(schema.get("properties").is_none());
        assert!(doc["components"]["schemas"]["Other"]["externalDocs"].is_null());
        assert_eq!(doc["externalDocs"]["url"], REGISTRY_URL);

        // the control plane never reads the gateway's schema names
        let mut control = json!({"paths": {}, "components": {"schemas": {"Example": {}}}});
        annotate_with(&registry, &mut control, Plane::Control);
        assert!(control["components"]["schemas"]["Example"]["externalDocs"].is_null());
    }

    /// The conformance fixtures, relative to the repository root (#3336).
    const PROFILE_FIXTURES: &str = "crates/rolter-gateway/tests/provider_profiles.rs";

    /// Each case of the fixtures file `source`, by test name, and whether its
    /// `source:` is `Source::Fetched` rather than `Source::Recalled`. A case is
    /// an `async fn` up to the next one; one with no `source:` reads as not
    /// fetched.
    fn fixture_sources(source: &str) -> Vec<(&str, bool)> {
        let mut cases = Vec::new();
        let mut rest = source;
        while let Some(at) = rest.find("async fn ") {
            rest = &rest[at + "async fn ".len()..];
            let name_end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            let name = &rest[..name_end];
            let body = &rest[name_end..];
            let body = &body[..body.find("async fn ").unwrap_or(body.len())];
            let fetched = body
                .find("source: Source::")
                .is_some_and(|at| body[at..].starts_with("source: Source::Fetched("));
            cases.push((name, fetched));
        }
        cases
    }

    /// The `provider-profile` records, `[[spec]]` and `[[provider]]` alike,
    /// whose cited fixtures (`tests` entries in [`PROFILE_FIXTURES`]) include
    /// no fetched one, or name a case the file does not have (#3495). A
    /// recalled fixture is a regression guard for what rolter sends, not
    /// evidence about the vendor, so it cannot carry the claim alone.
    fn profiles_without_a_fetched_fixture(
        registry: &Registry,
        fixtures: &[(&str, bool)],
    ) -> Vec<String> {
        let records = registry
            .specs
            .iter()
            .map(|spec| (spec.id.clone(), spec.conformance, &spec.tests))
            .chain(registry.providers.iter().map(|provider| {
                (
                    format!("provider {:?}", provider.kind),
                    provider.conformance,
                    &provider.tests,
                )
            }));
        let mut problems = Vec::new();
        for (record, conformance, tests) in records {
            if conformance != Conformance::ProviderProfile {
                continue;
            }
            let cited: Vec<&str> = tests
                .iter()
                .filter_map(|test| test.strip_prefix(PROFILE_FIXTURES)?.strip_prefix("::"))
                .collect();
            for case in &cited {
                if !fixtures.iter().any(|(name, _)| name == case) {
                    problems.push(format!(
                        "{record}: cites {PROFILE_FIXTURES}::{case}, which the file does not have"
                    ));
                }
            }
            let fetched = cited
                .iter()
                .any(|case| fixtures.iter().any(|(name, f)| name == case && *f));
            if !cited.is_empty() && !fetched {
                problems.push(format!(
                    "{record}: claims provider-profile but cites no Fetched fixture in \
                     {PROFILE_FIXTURES} (only {}); fetch the vendor page and mark the case \
                     Fetched, or drop the claim",
                    cited.join(", ")
                ));
            }
        }
        problems
    }

    #[test]
    fn no_provider_profile_rests_on_a_recalled_fixture_only() {
        let fixtures = std::fs::read_to_string(format!(
            "{}/../../{PROFILE_FIXTURES}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("the provider profile fixtures are readable");
        let cases = fixture_sources(&fixtures);
        assert!(
            cases.iter().any(|(_, fetched)| *fetched),
            "no Fetched case was read from {PROFILE_FIXTURES}: {cases:?}"
        );
        let problems = profiles_without_a_fetched_fixture(&embedded(), &cases);
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn a_profile_on_a_recalled_fixture_is_named() {
        let fixtures = r#"
            async fn fetched_case() {
                replay(&Profile { source: Source::Fetched("2026-10-29"), });
            }
            async fn recalled_case() {
                replay(&Profile { source: Source::Recalled, });
            }
        "#;
        let cases = fixture_sources(fixtures);
        assert_eq!(cases, [("fetched_case", true), ("recalled_case", false)]);
        let record = |conformance: &str, case: &str| {
            parse(
                &MINIMAL
                    .replace(
                        "conformance = \"partial\"",
                        &format!("conformance = \"{conformance}\""),
                    )
                    .replace(
                        "tests = [\"crates/x/src/lib.rs::works\"]",
                        &format!("tests = [\"{PROFILE_FIXTURES}::{case}\"]"),
                    ),
            )
            .expect("the fixture parses")
        };
        let problems = profiles_without_a_fetched_fixture(
            &record("provider-profile", "recalled_case"),
            &cases,
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with("example: "), "{problems:?}");
        assert!(problems[0].contains("recalled_case"), "{problems:?}");
        // a fetched case carries the claim, and a record that claims less
        // needs none
        assert!(profiles_without_a_fetched_fixture(
            &record("provider-profile", "fetched_case"),
            &cases
        )
        .is_empty());
        assert!(profiles_without_a_fetched_fixture(
            &record("transport-only", "recalled_case"),
            &cases
        )
        .is_empty());
        // a case the file does not have is named too
        let missing =
            profiles_without_a_fetched_fixture(&record("provider-profile", "gone_case"), &cases);
        assert!(missing[0].contains("does not have"), "{missing:?}");
    }
}
