//! `GET /api/v1/config/export` — the live configuration rendered back out as an
//! importable `rolter.toml` (#1082).
//!
//! `rolter-seed --import` treats a TOML file as desired state and converges the
//! database onto it, but there was no way to go the other direction: an
//! operator who built up providers, groups, routes and prices through the
//! dashboard had that state only in Postgres, with no GitOps path, no cheap
//! environment promotion, and no config diff.
//!
//! [`render`] closes the loop. It emits exactly the sections
//! [`crate::seed::seed`] consumes, so the document it produces is *importable*
//! rather than merely descriptive — anything the importer would silently drop
//! is deliberately not written, and the header comment says so in the file
//! instead of leaving the operator to find out on a promotion.
//!
//! Four properties the tests pin down:
//!
//! - **No secret leaves.** A provider credential is emitted as an `api_key_env`
//!   variable *name* and never as a value. `ConfigStore::load` hands back a
//!   decrypted `api_key` (the gateway needs it), so the renderer has to strip
//!   it — and does, unconditionally: no field of the emitted provider is ever
//!   sourced from [`rolter_core::ProviderConfig::api_key`]. A provider with no
//!   `api_key_env` carries a visible comment where its credential would be.
//! - **Identity is a slug, never a database id.** Everything is keyed the way
//!   the importer keys it: providers and groups by slug/name, routes by public
//!   model name, prices by model.
//! - **Determinism.** Every collection is sorted (by slug, then model) and every
//!   map is emitted in sorted key order, so a diff between two exports shows
//!   only real changes.
//! - **The file is current, not merely loadable.** It carries a
//!   `schema_version` stamp and ADR-0022's tiered `[[providers.readonly]]` /
//!   `[[providers.default]]` spelling, so `rolter check --migrations` reports it
//!   as up to date (#1513). The stamp and the shape are pinned together on
//!   purpose: the migration chain filters on `m.from >= from`, so a file stamped
//!   `2` that still carried the deprecated flat arrays would skip the v1 -> v2
//!   step forever. Emitting both tiers is also what stops the export losing
//!   `providers.default` entries, which the flat array cannot express at all.

use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelPriceConfig, ModelRoute, OverrideMode, ProviderConfig,
    ProviderGroupConfig,
};
use serde::Serialize;

#[cfg(feature = "postgres")]
use axum::extract::State;
#[cfg(feature = "postgres")]
use axum::response::{IntoResponse, Response};
#[cfg(feature = "postgres")]
use axum::routing::get;
#[cfg(feature = "postgres")]
use axum::Router;

#[cfg(feature = "postgres")]
use crate::crud::ApiResult;
#[cfg(feature = "postgres")]
use crate::rbac::{authorize_superadmin, Principal};
#[cfg(feature = "postgres")]
use crate::rbac_matrix::superadmin_cap;
#[cfg(feature = "postgres")]
use crate::ControlState;

/// The banner every export opens with. Fixed text, no timestamp: two exports of
/// an unchanged deployment must be byte-identical or a diff is worthless.
const HEADER: &str = "\
# rolter configuration export
#
# This is the deployment's live configuration in the shape `rolter-seed
# --import` accepts, so it can be committed to version control and replayed
# against another deployment:
#
#     rolter config export --output rolter.toml
#     rolter-seed --database-url \"$ROLTER_DATABASE_URL\" --import rolter.toml
#
# The import is a desired-state upsert keyed by slug, so re-importing an edited
# copy of this file applies the edits rather than duplicating rows.
#
# Round-trips: providers, provider groups and their members, routes with their
# targets, parameter defaults and override policy, model prices, published
# prompt templates, and the payload-capture policy.
#
# Deliberately not exported, because the importer does not consume them and a
# file that carried them would promise more than a re-import delivers: virtual
# keys, budgets and rate limits (both are keyed by database id rather than by
# slug), MCP servers and OAuth sessions, guardrails, users, teams, projects and
# memberships, SSO and SCIM configuration, and every operational table —
# request logs, sessions, audit records and usage.
#
# No credential is exported. A provider key is written only as the *name* of
# the environment variable it is read from; a key sealed in the control-plane
# store is marked with a comment where it would have been and has to be
# re-sealed (or pointed at an env var) on the receiving deployment.
";

/// Render `config` as a `rolter.toml` document `rolter-seed --import` accepts.
///
/// Pure and infallible: a value that cannot be represented in TOML (a JSON
/// `null` in a route's parameter defaults, say) is skipped with a comment
/// rather than failing the whole export, because withholding an operator's
/// entire configuration over one unrepresentable default helps nobody.
pub fn render(config: &GatewayConfig) -> String {
    let mut out = String::from(HEADER);
    // the stamp goes before any table header, and is sourced from the constant
    // rather than written as a literal so it cannot drift from the chain. it is
    // only honest alongside the tiered provider sections below: a file stamped
    // `2` that still carried `[[providers]]` arrays would skip the v1 -> v2 step
    // forever, because the chain filters on `m.from >= from` (#1513)
    out.push_str(&format!(
        "\nschema_version = {}\n",
        rolter_core::config_migrate::CURRENT_SCHEMA_VERSION
    ));
    render_providers(&mut out, config);
    render_provider_groups(&mut out, config);
    render_routes(&mut out, config);
    render_model_prices(&mut out, config);
    render_prompt_templates(&mut out, config);
    render_payload_capture(&mut out, config);
    out
}

/// Effective slug for a provider: the stored one, else derived from the name —
/// the same rule the importer applies when it creates a row.
fn provider_slug(provider: &ProviderConfig) -> String {
    provider
        .slug
        .clone()
        .unwrap_or_else(|| rolter_core::slug::slugify(&provider.name))
}

fn group_slug(group: &ProviderGroupConfig) -> String {
    group
        .slug
        .clone()
        .unwrap_or_else(|| rolter_core::slug::slugify(&group.name))
}

fn render_providers(out: &mut String, config: &GatewayConfig) {
    // ADR-0022's tiered spelling, not the deprecated flat `[[providers]]` array.
    // `[[providers.readonly]]` is the array-of-tables form of the same document
    // `split_section` reads as `{ readonly = [...], default = [...] }`, so it
    // keeps one key per line — an inline-table array could not carry the
    // "credential not exported" comment at all.
    //
    // the two tiers are emitted in full, readonly first: the flat array can only
    // express the readonly tier, so exporting it lost every `providers.default`
    // entry outright rather than merely spelling them the old way (#1513)
    render_provider_tier(out, "readonly", &config.providers);
    render_provider_tier(out, "default", &config.provider_defaults);
}

fn render_provider_tier(out: &mut String, tier: &str, entries: &[ProviderConfig]) {
    let mut providers: Vec<&ProviderConfig> = entries.iter().collect();
    providers.sort_by_key(|p| provider_slug(p));
    for provider in providers {
        out.push_str(&format!("\n[[providers.{tier}]]\n"));
        key(out, "name", &provider.name);
        key(out, "slug", &provider_slug(provider));
        key(out, "kind", &provider.kind);
        key(out, "api_base", &provider.api_base);
        // the only credential shape that ever leaves: the name of an
        // environment variable, never a value. neither `api_key` nor an
        // `api_keys` entry's inline `key` is reachable from here, so a
        // decrypted key cannot reach the document by accident. the accessor is
        // what makes a provider written in the plural `api_keys` spelling
        // export its variable name instead of the omission comment (#1514)
        match provider.api_key_env_name() {
            Some(env) => key(out, "api_key_env", env),
            None => out.push_str(
                "# api_key: not exported; any credential for this provider stays sealed in \
                 the control-plane store\n",
            ),
        }
        if let Some(proxy) = &provider.egress_proxy {
            key(out, "egress_proxy", proxy);
        }
        if !provider.egress_proxies.is_empty() {
            key(out, "egress_proxies", &provider.egress_proxies);
        }
    }
}

fn render_provider_groups(out: &mut String, config: &GatewayConfig) {
    render_provider_group_tier(out, "readonly", &config.provider_groups);
    render_provider_group_tier(out, "default", &config.provider_group_defaults);
}

fn render_provider_group_tier(out: &mut String, tier: &str, entries: &[ProviderGroupConfig]) {
    let mut groups: Vec<&ProviderGroupConfig> = entries.iter().collect();
    groups.sort_by_key(|g| group_slug(g));
    for group in groups {
        out.push_str(&format!("\n[[provider_groups.{tier}]]\n"));
        key(out, "name", &group.name);
        key(out, "slug", &group_slug(group));
        key(out, "strategy", strategy_name(group.strategy));
        // members attach to the most recent `[[provider_groups.<tier>]]`, so
        // this must stay inside the loop and after the group's own scalar keys
        for member in &group.members {
            out.push_str(&format!("\n[[provider_groups.{tier}.members]]\n"));
            key(out, "provider", &member.provider);
            if let Some(model) = &member.model {
                key(out, "model", model);
            }
            key(out, "weight", &member.weight);
        }
    }
}

fn render_routes(out: &mut String, config: &GatewayConfig) {
    let mut routes: Vec<&ModelRoute> = config.routes.iter().collect();
    routes.sort_by(|a, b| a.model.cmp(&b.model));
    for route in routes {
        out.push_str("\n[[routes]]\n");
        key(out, "model", &route.model);
        key(out, "strategy", strategy_name(route.strategy));
        if !route.params.is_empty() {
            out.push_str("\n[routes.params]\n");
            let mut names: Vec<&String> = route.params.keys().collect();
            names.sort();
            for name in names {
                match route.params.get(name) {
                    // a json null has no TOML spelling; naming it in a comment
                    // beats dropping it silently
                    Some(value) if !value.is_null() => key(out, name, value),
                    _ => {
                        out.push_str(&format!("# {name}: not representable in toml, omitted\n"));
                    }
                }
            }
        }
        let policy = &route.param_policy;
        if policy.mode != OverrideMode::Allow || !policy.allow.is_empty() || !policy.deny.is_empty()
        {
            out.push_str("\n[routes.param_policy]\n");
            key(
                out,
                "mode",
                match policy.mode {
                    OverrideMode::Allow => "allow",
                    OverrideMode::Deny => "deny",
                },
            );
            key(out, "allow", &policy.allow);
            key(out, "deny", &policy.deny);
        }
        for target in &route.targets {
            out.push_str("\n[[routes.targets]]\n");
            key(out, "provider", &target.provider);
            if let Some(model) = &target.model {
                key(out, "model", model);
            }
            key(out, "weight", &target.weight);
        }
    }
}

fn render_model_prices(out: &mut String, config: &GatewayConfig) {
    let mut prices: Vec<&ModelPriceConfig> = config.model_prices.iter().collect();
    prices.sort_by(|a, b| a.model.cmp(&b.model));
    for price in prices {
        out.push_str("\n[[model_prices]]\n");
        key(out, "model", &price.model);
        key(out, "input_per_mtok", &price.input_per_mtok);
        key(out, "output_per_mtok", &price.output_per_mtok);
        if let Some(cached) = price.cached_input_per_mtok {
            key(out, "cached_input_per_mtok", &cached);
        }
        key(out, "currency", &price.currency);
    }
}

fn render_prompt_templates(out: &mut String, config: &GatewayConfig) {
    let templates = &config.prompt_templates.templates;
    if templates.is_empty() {
        return;
    }
    out.push_str("\n[prompt_templates]\n");
    key(out, "enabled", &config.prompt_templates.enabled);

    let mut templates: Vec<_> = templates.iter().collect();
    templates.sort_by(|a, b| template_slug(&a.id).cmp(template_slug(&b.id)));
    for template in templates {
        out.push_str("\n[[prompt_templates.templates]]\n");
        key(out, "id", template_slug(&template.id));
        key(out, "version", &template.version);
        // scopes are database ids, so only the route-model bindings survive an
        // export; an org-wide template exports as "all routes", which is what
        // the importer makes of an empty `routes` list anyway
        let mut models: Vec<&String> = template
            .scopes
            .iter()
            .filter_map(|scope| match scope {
                rolter_core::prompt_templates::PromptTemplateActivationScope::Route {
                    model,
                    ..
                } => Some(model),
                _ => None,
            })
            .chain(template.routes.iter())
            .collect();
        models.sort();
        models.dedup();
        if !models.is_empty() {
            key(out, "routes", &models);
        }
        for variable in &template.variables {
            out.push_str("\n[[prompt_templates.templates.variables]]\n");
            key(out, "name", &variable.name);
            key(out, "required", &variable.required);
            if let Some(default) = &variable.default {
                key(out, "default", default);
            }
        }
        for decorator in &template.decorators {
            out.push_str("\n[[prompt_templates.templates.decorators]]\n");
            key(out, "role", &decorator.role);
            key(out, "position", &decorator.position);
            key(out, "content", &decorator.content);
        }
    }
}

/// A stored template's id is `"{org_id}:{slug}"`; only the slug half is
/// portable, and it is what the importer keys the row by.
fn template_slug(id: &str) -> &str {
    id.rsplit_once(':').map_or(id, |(_, slug)| slug)
}

fn render_payload_capture(out: &mut String, config: &GatewayConfig) {
    let capture = &config.logging.payload_capture;
    out.push_str("\n[logging.payload_capture]\n");
    key(out, "enabled", &capture.enabled);
    key(out, "max_bytes", &capture.max_bytes);
    key(out, "redact_fields", &capture.redact_fields);
    key(out, "models", &capture.models);
    out.push_str("# virtual key ids are this deployment's own; drop them when promoting\n");
    key(out, "virtual_key_ids", &capture.virtual_key_ids);
}

fn strategy_name(strategy: BalancingStrategy) -> &'static str {
    match strategy {
        BalancingStrategy::RoundRobin => "round_robin",
        BalancingStrategy::Random => "random",
        BalancingStrategy::PowerOfTwo => "power_of_two",
        BalancingStrategy::ConsistentHash => "consistent_hash",
        BalancingStrategy::CacheAware => "cache_aware",
        BalancingStrategy::Weighted => "weighted",
        BalancingStrategy::Pipeline => "pipeline",
        BalancingStrategy::Cheapest => "cheapest",
        BalancingStrategy::Fastest => "fastest",
        BalancingStrategy::PreciseCacheAware => "precise_cache_aware",
        BalancingStrategy::LmcacheAware => "lmcache_aware",
        BalancingStrategy::Adaptive => "adaptive",
        BalancingStrategy::LoraAware => "lora_aware",
        BalancingStrategy::PredictedLatency => "predicted_latency",
    }
}

/// Append `key = <value>` using TOML's own value serializer, so escaping,
/// float spelling and array syntax are the format's rules rather than ours.
/// A value TOML cannot express is skipped with a comment naming it — the
/// renderer has no error channel and a partial export beats none.
fn key<T: Serialize + ?Sized>(out: &mut String, name: &str, value: &T) {
    let mut rendered = String::new();
    match value.serialize(toml::ser::ValueSerializer::new(&mut rendered)) {
        Ok(_) => {
            out.push_str(name);
            out.push_str(" = ");
            out.push_str(&rendered);
            out.push('\n');
        }
        Err(_) => out.push_str(&format!("# {name}: not representable in toml, omitted\n")),
    }
}

#[cfg(feature = "postgres")]
pub(crate) fn router() -> Router<ControlState> {
    Router::new().route("/api/v1/config/export", get(export_config))
}

/// The whole deployment's configuration as an importable `rolter.toml`.
///
/// Superadmin-only and whole-deployment: the document spans every org's
/// providers and groups, so there is no tenancy scope narrow enough to
/// authorize it. Served as `application/toml` with a download filename so a
/// browser saves it rather than rendering it.
#[cfg(feature = "postgres")]
async fn export_config(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Response> {
    authorize_superadmin(&principal, superadmin_cap!("config_export", Read))?;
    let config = state.store.load().await?;
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/toml; charset=utf-8",
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"rolter.toml\"",
            ),
        ],
        render(&config),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::{ParamPolicy, ProviderKind, Target};

    /// Everything the export claims to round-trip, in one file.
    ///
    /// Shared by the round-trip test, which imports it into a real database,
    /// and by the unknown-key lint below, which parses it directly — one
    /// fixture, so a section added to the export is covered by both rather than
    /// drifting into being covered by one.
    const FIXTURE: &str = r#"
[[providers]]
name = "openai-primary"
slug = "openai-primary"
kind = "openai"
api_base = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"

[[providers]]
name = "vllm-1"
slug = "vllm-1"
kind = "openai_compatible"
api_base = "http://vllm-1:8000"

[[provider_groups]]
name = "cluster"
slug = "cluster"
strategy = "cache_aware"
[[provider_groups.members]]
provider = "vllm-1"
model = "meta-llama/Llama-3.1-8B-Instruct"
weight = 2

[[routes]]
model = "gpt-4o"
strategy = "round_robin"
[routes.params]
temperature = 0.0
max_tokens = 1024
[routes.param_policy]
mode = "deny"
allow = ["max_tokens"]
[[routes.targets]]
provider = "openai-primary"
model = "gpt-4o"
weight = 1

[[routes]]
model = "llama"
strategy = "cache_aware"
[[routes.targets]]
provider = "vllm-1"
model = "meta-llama/Llama-3.1-8B-Instruct"
weight = 3

[[model_prices]]
model = "gpt-4o"
input_per_mtok = 2.5
output_per_mtok = 10.0
currency = "USD"

[prompt_templates]
enabled = true
[[prompt_templates.templates]]
id = "support-preamble"
version = 1
routes = ["gpt-4o"]
[[prompt_templates.templates.variables]]
name = "persona"
default = "a helpful support assistant"
[[prompt_templates.templates.decorators]]
role = "system"
position = "prepend"
content = "You are {{ persona }}."

[logging.payload_capture]
enabled = true
max_bytes = 32768
redact_fields = ["authorization"]
models = ["gpt-4o"]
"#;

    /// `ModelRoute` carries no `Default`, so fixtures spell it out once here
    /// rather than in every test.
    fn route(model: &str) -> ModelRoute {
        ModelRoute {
            model: model.to_string(),
            strategy: BalancingStrategy::RoundRobin,
            targets: Vec::new(),
            params: std::collections::HashMap::new(),
            param_policy: ParamPolicy::default(),
            advanced: Default::default(),
            variants: Vec::new(),
            cache: None,
        }
    }

    fn provider(name: &str, api_key_env: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            slug: Some(name.to_string()),
            kind: ProviderKind::Openai,
            api_base: "https://api.openai.com".to_string(),
            api_key_env: api_key_env.map(str::to_string),
            ..Default::default()
        }
    }

    /// The one property that must never regress: `ConfigStore::load` hands the
    /// renderer a decrypted key because the gateway needs one, and the export
    /// must not carry it. Asserted against the bytes, not by eye.
    #[test]
    fn a_decrypted_provider_key_never_reaches_the_document() {
        let mut sealed = provider("openai", None);
        sealed.api_key = Some("sk-live-do-not-export".to_string());
        let mut via_env = provider("anthropic", Some("ANTHROPIC_API_KEY"));
        via_env.api_key = Some("sk-ant-also-secret".to_string());
        let config = GatewayConfig {
            providers: vec![sealed, via_env],
            ..Default::default()
        };

        let rendered = render(&config);
        assert!(
            !rendered.contains("sk-live-do-not-export"),
            "a sealed provider key leaked into the export:\n{rendered}"
        );
        assert!(
            !rendered.contains("sk-ant-also-secret"),
            "a provider key leaked into the export:\n{rendered}"
        );
        assert!(
            !rendered.contains("api_key ="),
            "the export wrote an api_key field at all:\n{rendered}"
        );
        // and the omission is visible in the file rather than silent
        assert!(rendered.contains("# api_key: not exported"));
        assert!(rendered.contains("api_key_env = \"ANTHROPIC_API_KEY\""));
    }

    /// A provider written in the plural `api_keys` spelling exported the
    /// "credential not exported" comment instead of its variable name, so an
    /// export/import round trip dropped the key (#1514).
    #[test]
    fn a_plural_form_credential_exports_its_variable_name() {
        let mut plural = provider("openai", None);
        plural.api_keys = vec![
            rolter_core::ApiKeyConfig {
                env: Some("OPENAI_KEY_A".to_string()),
                weight: 3,
                ..Default::default()
            },
            rolter_core::ApiKeyConfig {
                env: Some("OPENAI_KEY_B".to_string()),
                weight: 1,
                ..Default::default()
            },
        ];
        let config = GatewayConfig {
            providers: vec![plural],
            ..Default::default()
        };

        let rendered = render(&config);
        assert!(
            rendered.contains("api_key_env = \"OPENAI_KEY_A\""),
            "the plural credential did not reach the export:\n{rendered}"
        );
        assert!(
            !rendered.contains("# api_key: not exported"),
            "a provider with a credential was marked as having none:\n{rendered}"
        );
        // the importer holds one variable name per provider, so the second
        // entry is genuinely not representable and must not be half-emitted
        assert!(
            !rendered.contains("OPENAI_KEY_B"),
            "the export wrote a key the importer cannot read back:\n{rendered}"
        );
    }

    /// The no-secret guarantee has to hold for the plural spelling too: an
    /// inline `api_keys` entry is as much a secret as `api_key` is.
    #[test]
    fn an_inline_plural_key_never_reaches_the_document() {
        let mut inline = provider("openai", None);
        inline.api_keys = vec![rolter_core::ApiKeyConfig {
            key: Some("sk-plural-do-not-export".to_string()),
            weight: 1,
            ..Default::default()
        }];
        let config = GatewayConfig {
            providers: vec![inline],
            ..Default::default()
        };

        let rendered = render(&config);
        assert!(
            !rendered.contains("sk-plural-do-not-export"),
            "an inline plural key leaked into the export:\n{rendered}"
        );
        assert!(
            rendered.contains("# api_key: not exported"),
            "the omission is not visible in the file:\n{rendered}"
        );
    }

    /// Ordering is by slug and model, never by insertion, so a diff between two
    /// exports of the same deployment is empty.
    #[test]
    fn output_is_ordered_and_reparses() {
        let config = GatewayConfig {
            providers: vec![provider("zzz", None), provider("aaa", Some("AAA_KEY"))],
            routes: vec![
                ModelRoute {
                    targets: vec![Target {
                        provider: "zzz".to_string(),
                        model: Some("upstream".to_string()),
                        weight: 3,
                    }],
                    ..route("zeta")
                },
                route("alpha"),
            ],
            ..Default::default()
        };

        let rendered = render(&config);
        let first_provider = rendered.find("name = \"aaa\"").expect("aaa is exported");
        let second_provider = rendered.find("name = \"zzz\"").expect("zzz is exported");
        assert!(first_provider < second_provider, "providers are not sorted");
        let alpha = rendered
            .find("model = \"alpha\"")
            .expect("alpha is exported");
        let zeta = rendered.find("model = \"zeta\"").expect("zeta is exported");
        assert!(alpha < zeta, "routes are not sorted");

        // and the document is a config, not just text that looks like one
        let parsed = GatewayConfig::from_toml_str(&rendered).expect("the export must re-parse");
        assert_eq!(parsed.providers.len(), 2);
        assert_eq!(parsed.routes.len(), 2);
        assert_eq!(parsed.routes[0].targets.len(), 0);
        assert_eq!(parsed.routes[1].targets[0].weight, 3);
    }

    /// Parameter defaults survive with their override policy, and a JSON null —
    /// which TOML cannot spell — is named in a comment instead of vanishing.
    #[test]
    fn route_params_round_trip_and_nulls_are_named() {
        let mut params = std::collections::HashMap::new();
        params.insert("temperature".to_string(), serde_json::json!(0.0));
        params.insert("max_tokens".to_string(), serde_json::json!(1024));
        params.insert("stop".to_string(), serde_json::Value::Null);
        let config = GatewayConfig {
            routes: vec![ModelRoute {
                params,
                param_policy: ParamPolicy {
                    mode: OverrideMode::Deny,
                    allow: vec!["max_tokens".to_string()],
                    deny: Vec::new(),
                },
                ..route("gpt-4o")
            }],
            ..Default::default()
        };

        let rendered = render(&config);
        assert!(rendered.contains("temperature = 0.0"));
        assert!(rendered.contains("max_tokens = 1024"));
        assert!(rendered.contains("# stop: not representable in toml, omitted"));
        assert!(rendered.contains("mode = \"deny\""));

        let parsed = GatewayConfig::from_toml_str(&rendered).expect("the export must re-parse");
        assert_eq!(parsed.routes[0].param_policy.mode, OverrideMode::Deny);
        assert_eq!(parsed.routes[0].params.len(), 2);
    }

    /// Two renders of the same configuration are byte-identical — no timestamp,
    /// no map iteration order, nothing that makes a diff lie.
    #[test]
    fn rendering_twice_is_byte_identical() {
        let mut params = std::collections::HashMap::new();
        for name in ["a", "b", "c", "d", "e", "f", "g", "h"] {
            params.insert(name.to_string(), serde_json::json!(name));
        }
        let config = GatewayConfig {
            providers: vec![provider("openai", Some("OPENAI_API_KEY"))],
            routes: vec![ModelRoute {
                params,
                ..route("gpt-4o")
            }],
            ..Default::default()
        };
        assert_eq!(render(&config), render(&config));
    }

    /// [`FIXTURE`] as a `GatewayConfig`, plus the fields the renderer only
    /// writes conditionally, so one document exercises every key the exporter
    /// can emit.
    fn representative_config() -> GatewayConfig {
        let mut config = GatewayConfig::from_toml_str(FIXTURE).expect("the fixture must parse");
        let vllm = config
            .providers
            .iter_mut()
            .find(|p| p.name == "vllm-1")
            .expect("the fixture has a vllm provider");
        vllm.egress_proxy = Some("http://egress:3128".to_string());
        vllm.egress_proxies = vec![
            "http://egress-a:3128".to_string(),
            "http://egress-b:3128".to_string(),
        ];
        let price = config
            .model_prices
            .first_mut()
            .expect("the fixture has a price");
        price.cached_input_per_mtok = Some(rust_decimal::Decimal::new(125, 2));
        // exported behind a comment telling the operator to drop it, but
        // exported all the same, so the key has to be one rolter reads
        config.logging.payload_capture.virtual_key_ids =
            vec!["7a1f0f7e-0000-0000-0000-000000000000".to_string()];
        config
    }

    /// The export must be a file rolter itself does not complain about.
    ///
    /// Since #1438 every config load path lints the document for keys no config
    /// struct claims and warns about each one at startup. An exporter whose
    /// field vocabulary drifts from the config model would therefore greet an
    /// operator with "unrecognised config key" on a file rolter wrote — the
    /// tool contradicting itself on the first promotion (#1439). Re-parsing,
    /// which the tests above already check, does not catch this: an unknown key
    /// parses fine and is silently ignored, which is the whole reason the lint
    /// exists.
    #[test]
    fn the_exported_config_carries_no_unrecognised_keys() {
        let rendered = render(&representative_config());
        let findings =
            rolter_core::config_lint::unknown_keys(&rendered).expect("the export must be toml");
        assert!(
            findings.is_empty(),
            "rolter config export emitted keys rolter does not read: {:?}\n{rendered}",
            findings.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }

    /// The property the whole issue is about: `rolter config export` must write
    /// a file its own pre-flight reports as current.
    ///
    /// Before #1513 it emitted the deprecated `[[providers]]` arrays and no
    /// stamp, so every exported file was schema version 1 with a pending
    /// migration — the tool that generates configs generating ones rolter tells
    /// you to migrate.
    #[test]
    fn an_exported_config_has_no_pending_migrations() {
        let rendered = render(&representative_config());
        let doc: toml::Table = rendered.parse().expect("the export must be toml");
        assert_eq!(
            rolter_core::config_migrate::schema_version_of(&doc),
            rolter_core::config_migrate::CURRENT_SCHEMA_VERSION,
        );
        let mut doc = doc;
        let report = rolter_core::config_migrate::migrate(&mut doc);
        assert!(
            report.is_empty(),
            "rolter config export wrote a file with pending migrations: {:?}\n{rendered}",
            report.changes().collect::<Vec<_>>()
        );
        assert!(
            !report.ahead,
            "the export must not stamp a version this build cannot read"
        );
    }

    /// The stamp is only honest because the shape moved with it.
    ///
    /// The chain filters on `m.from >= from`, so a file stamped `2` that still
    /// carried `[[providers]]` arrays would skip the v1 -> v2 step forever. It
    /// would load correctly today — `split_section` accepts both spellings — and
    /// then misbehave under the first v2 -> v3 migration written against the
    /// tiered shape. Pin the two together so neither can move alone.
    #[test]
    fn the_export_uses_the_tiered_provider_spelling() {
        let rendered = render(&representative_config());
        assert!(rendered.contains("[[providers.readonly]]"), "{rendered}");
        assert!(
            !rendered.contains("\n[[providers]]"),
            "the deprecated flat array must not be emitted: {rendered}"
        );
        assert!(
            !rendered.contains("\n[[provider_groups]]"),
            "the deprecated flat array must not be emitted: {rendered}"
        );
    }

    /// The `default` tier survives an export/import round trip.
    ///
    /// The flat array could not express it at all — it is the readonly tier by
    /// definition — so exporting a deployment that had `providers.default`
    /// entries silently dropped them. That is a data loss on promotion, not a
    /// spelling preference, and it is the other half of why the shape had to
    /// move.
    #[test]
    fn the_default_tier_round_trips() {
        let mut config = representative_config();
        config.provider_defaults = vec![provider("seeded-openai", Some("SEEDED_KEY"))];
        config.provider_group_defaults = vec![ProviderGroupConfig {
            name: "seeded-pool".to_string(),
            slug: Some("seeded-pool".to_string()),
            ..config
                .provider_groups
                .first()
                .cloned()
                .expect("the fixture has a group")
        }];

        let rendered = render(&config);
        let parsed = GatewayConfig::from_toml_str(&rendered).expect("the export must re-parse");

        assert_eq!(parsed.provider_defaults.len(), 1);
        assert_eq!(parsed.provider_defaults[0].name, "seeded-openai");
        assert_eq!(parsed.provider_group_defaults.len(), 1);
        assert_eq!(parsed.provider_group_defaults[0].name, "seeded-pool");
        // and the readonly tier is not disturbed by the default tier beside it
        assert_eq!(parsed.providers.len(), config.providers.len());
        assert_eq!(parsed.provider_groups.len(), config.provider_groups.len());
        // members hang off the right tier's most recent entry
        assert_eq!(
            parsed.provider_group_defaults[0].members.len(),
            config.provider_group_defaults[0].members.len()
        );
    }

    /// The same guarantee for the other end of the range: a deployment with
    /// nothing configured still exports `[logging.payload_capture]` and the
    /// header, and neither may carry a key the loader ignores.
    #[test]
    fn an_empty_deployment_exports_no_unrecognised_keys() {
        let rendered = render(&GatewayConfig::default());
        let findings =
            rolter_core::config_lint::unknown_keys(&rendered).expect("the export must be toml");
        assert!(
            findings.is_empty(),
            "an empty export emitted keys rolter does not read: {:?}\n{rendered}",
            findings.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_stored_template_id_exports_as_its_slug() {
        assert_eq!(
            template_slug("0f4a2b7c-0000-0000-0000-000000000000:support"),
            "support"
        );
        assert_eq!(template_slug("support"), "support");
    }

    /// The acceptance bar for #1082: a document the importer accepts, imported,
    /// exported, re-imported and exported again, is byte-identical the second
    /// time. Anything non-deterministic (a timestamp, a map iteration order) or
    /// anything the importer silently drops shows up here as a diff.
    #[cfg(feature = "postgres")]
    mod round_trip {
        use rolter_store::postgres::crypto::Kek;
        use rolter_store::postgres::test_schema::TestSchema;
        use rolter_store::postgres::PostgresConfigStore;
        use rolter_store::ConfigStore;
        use uuid::Uuid;

        use super::FIXTURE;

        #[tokio::test]
        async fn an_export_reimports_to_a_byte_identical_export() {
            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let (org_id, project_id) = bootstrap_org(&pool).await;
            let dir = tempdir("export");
            let path = dir.join("rolter.toml");

            std::fs::write(&path, FIXTURE).unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &path)
                .await
                .unwrap();

            // a credential sealed through the dashboard, so the store hands the
            // renderer a decrypted key exactly as it does in production
            let kek = Kek::from_secret("round-trip-test-kek");
            let (ciphertext, nonce) = kek.encrypt("sk-live-must-never-be-exported").unwrap();
            let provider_id: Uuid =
                sqlx::query_scalar("select id from providers where slug = 'openai-primary'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            sqlx::query(
                "insert into provider_keys (provider_id, ciphertext, nonce) values ($1, $2, $3)",
            )
            .bind(provider_id)
            .bind(&ciphertext)
            .bind(&nonce)
            .execute(&pool)
            .await
            .unwrap();

            let store = PostgresConfigStore::with_kek(pool.clone(), Some(kek));
            let first = super::render(&store.load().await.unwrap());

            // the decrypted key is in the loaded config; it must not be in the
            // document built from it
            assert!(
                !first.contains("sk-live-must-never-be-exported"),
                "a sealed credential was exported in plaintext:\n{first}"
            );
            assert!(first.contains("api_key_env = \"OPENAI_API_KEY\""));
            assert!(
                first.contains("# api_key: not exported"),
                "the provider without an api_key_env carries no omission marker:\n{first}"
            );

            // everything the header promises round-trips is actually in there
            for expected in [
                "schema_version = ",
                "[[providers.readonly]]",
                "[[provider_groups.readonly]]",
                "[[provider_groups.readonly.members]]",
                "[[routes]]",
                "[[routes.targets]]",
                "[routes.params]",
                "[routes.param_policy]",
                "[[model_prices]]",
                "[[prompt_templates.templates]]",
                "[logging.payload_capture]",
            ] {
                assert!(first.contains(expected), "{expected} is missing:\n{first}");
            }
            // and never a database id, which is what makes the file portable
            assert!(
                !first.contains(&provider_id.to_string()),
                "the export leaked a database id:\n{first}"
            );

            std::fs::write(&path, &first).unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &path)
                .await
                .unwrap();
            let second = super::render(&store.load().await.unwrap());

            assert_eq!(
                first, second,
                "re-importing an export changed the deployment"
            );
        }

        fn tempdir(label: &str) -> std::path::PathBuf {
            let dir =
                std::env::temp_dir().join(format!("rolter-export-{label}-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        /// A schema of its own per test: the coverage job runs plain
        /// `cargo test` against a shared database and would otherwise race.
        /// The guard drops the schema when the test finishes (#1364).
        async fn scratch_db() -> Option<TestSchema> {
            let url = std::env::var("ROLTER_TEST_DATABASE_URL").ok().or_else(|| {
                eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
                None
            })?;
            Some(TestSchema::migrated(&url).await)
        }

        async fn bootstrap_org(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
            use rolter_store::postgres::repo::{ProjectRepo, TeamRepo};
            let org_id: Uuid = sqlx::query_scalar(
                "insert into orgs (name, slug) values ('acme', 'acme') returning id",
            )
            .fetch_one(pool)
            .await
            .unwrap();
            let team = TeamRepo(pool).create(org_id, "default").await.unwrap();
            let project = ProjectRepo(pool).create(team.id, "default").await.unwrap();
            (org_id, project.id)
        }
    }
}
