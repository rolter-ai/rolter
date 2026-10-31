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
//! `seed::seed` consumes, so the document it produces is *importable*
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
use axum::extract::{Query, State};
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
# targets, weighted variants, parameter defaults and override policy, model
# prices, published prompt templates, the payload-capture policy and the
# dashboard UX-event opt-out.
#
# `[[mcp_manifest_enforcement]]` records, for each org that never saved MCP
# settings, whether it enforces its servers' tool manifests. An import only
# ever relaxes the org it targets with `enforce = false`; it never makes one
# stricter.
#
# `[[mcp_gateway_settings]]` records the MCP settings an org saved. Unlike the
# marker it is the desired state: an import sets the org it targets to exactly
# these values, and no other org.
#
# `[[mcp_gateway_tool_groups]]` records each org's MCP tool groups: the tools
# each allows, by server slug, and the teams, projects and virtual keys it is
# assigned to. A key is named by its project and its name, or the prefix the
# dashboard shows when it has none, never by its secret. Keys are not exported,
# so a key assignment survives a round trip only where the key exists; an
# import skips, with a warning, an assignment it cannot resolve. A name or
# prefix matching several keys refuses the import before any write; give
# those keys distinct names before importing.
#
# Budgets and rate limits round-trip too, and name what they cap the way the
# importer finds it, never by database id: `scope = \"team\"` with `id =
# \"acme/platform\"` is the team called `platform` of the org with slug `acme`
# (`org`, `org/team`, `org/team/project`, `org/slug` for a business unit or a
# customer). A cap whose scope does not resolve on the receiving deployment is
# left alone, not half applied. A cap on a person is named by the account's
# email (`scope = \"user\"`, `id = \"<account email>\"`), narrowed to an org or a
# project by the same slug path, `within = { org = \"acme\" }`. The import finds
# an existing account by that email and never creates one; `--without-person-caps`
# leaves these caps, and with them every address, out of the file. A cap on a virtual
# key is not exported: keys are never exported, so it would name nothing.
#
# Deliberately not exported, because the importer does not consume them and a
# file that carried them would promise more than a re-import delivers: virtual
# keys, MCP servers and OAuth sessions, guardrails, users, teams, projects and
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
    render_with(config, &ScopeRefs::default())
}

/// What a budget or rate limit's scope id is called outside this database
/// (#1312): `id`s as the importer resolves them, by scope. A scope with no
/// entry has no portable name, which is how a cap is kept out of the file
/// rather than written under an id that means nothing elsewhere.
#[derive(Debug, Default, Clone)]
pub struct ScopeRefs {
    names: std::collections::HashMap<(rolter_core::BudgetScope, String), String>,
    /// the slug of the org each tenancy entity belongs to, whether or not it has
    /// a portable name, so a per-org export (#3440) can tell whose a cap is
    #[cfg(feature = "postgres")]
    owners: std::collections::HashMap<(rolter_core::BudgetScope, String), String>,
    /// id-free comments for portable-file omissions found while loading refs
    omissions: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl ScopeRefs {
    #[cfg(feature = "postgres")]
    fn record_omission(&self, comment: String) {
        self.omissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(comment);
    }

    fn omission_comments(&self) -> Vec<String> {
        let mut comments = self
            .omissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        comments.sort();
        comments
    }

    /// Record that `scope` entity `id` is called `reference` in a portable file.
    /// A name containing `/` is not recorded: the reference is a `/`-joined
    /// path, so it could not be told apart from a deeper one.
    pub fn insert(&mut self, scope: rolter_core::BudgetScope, id: &str, reference: String) {
        let segments = match scope {
            rolter_core::BudgetScope::Org | rolter_core::BudgetScope::Key => 1,
            rolter_core::BudgetScope::Team
            | rolter_core::BudgetScope::BusinessUnit
            | rolter_core::BudgetScope::Customer => 2,
            rolter_core::BudgetScope::Project => 3,
            // a person is named by the account's email, which is no path: the
            // importer reads it whole, so it only has to be one token (#3311)
            rolter_core::BudgetScope::User => {
                if !reference.is_empty() && !reference.chars().any(char::is_whitespace) {
                    self.names.insert((scope, id.to_string()), reference);
                }
                return;
            }
            // a scope a newer release stored (#3347) has no portable name
            rolter_core::BudgetScope::Unknown => return,
        };
        // the org slug cannot hold a `/`, so a name that does is what makes
        // the count wrong
        if reference.split('/').count() == segments && !reference.split('/').any(str::is_empty) {
            self.names.insert((scope, id.to_string()), reference);
        }
    }

    fn get(&self, scope: rolter_core::BudgetScope, id: &str) -> Option<&str> {
        self.names.get(&(scope, id.to_string())).map(String::as_str)
    }

    /// Record that `scope` entity `id` belongs to the org with slug `org`, and
    /// what it is called in a portable file, as [`ScopeRefs::insert`].
    #[cfg(feature = "postgres")]
    fn insert_owned(
        &mut self,
        scope: rolter_core::BudgetScope,
        id: &str,
        org: &str,
        reference: String,
    ) {
        self.owners.insert((scope, id.to_string()), org.to_string());
        self.insert(scope, id, reference);
    }

    #[cfg(feature = "postgres")]
    fn owner(&self, scope: rolter_core::BudgetScope, id: &str) -> Option<&str> {
        self.owners
            .get(&(scope, id.to_string()))
            .map(String::as_str)
    }

    /// Whether a cap belongs in the export of `org` (#3440).
    ///
    /// A cap is the org's when the entity it caps is: a team, project, business
    /// unit or customer of the org, the org itself, or a key minted in it. A cap
    /// on a person is the org's when its `within` names the org or one of its
    /// projects, or, with no `within`, when the person is a member of the org. A
    /// cap on a scope this build does not know cannot be placed, so it is in no
    /// org's file. A key owned by no org (the gateway's own file) is placed in
    /// every file, as the unscoped export places it.
    #[cfg(feature = "postgres")]
    fn in_org(
        &self,
        org: &OrgView,
        scope: rolter_core::BudgetScope,
        id: &str,
        within: Option<&rolter_core::ScopeWithin>,
    ) -> bool {
        use rolter_core::{BudgetScope, ScopeWithin};
        match scope {
            BudgetScope::Unknown => false,
            BudgetScope::Key => org.key_orgs.get(id).is_none_or(|owner| *owner == org.id),
            BudgetScope::User => match within {
                Some(ScopeWithin::Org(of)) => self.owner(BudgetScope::Org, of) == Some(&org.slug),
                Some(ScopeWithin::Project(of)) => {
                    self.owner(BudgetScope::Project, of) == Some(&org.slug)
                }
                _ => org.members.contains(id),
            },
            _ => self.owner(scope, id) == Some(&org.slug),
        }
    }

    /// A cap's `within`, as the slug path the importer resolves (#3311): `None`
    /// when the cap has none, `Some(None)` when it has one that names nothing
    /// portable, which keeps the cap out of the file.
    fn within(
        &self,
        within: Option<&rolter_core::ScopeWithin>,
    ) -> Option<Option<rolter_core::ScopeWithin>> {
        use rolter_core::{BudgetScope, ScopeWithin};
        match within {
            None => None,
            Some(ScopeWithin::Org(id)) => Some(
                self.get(BudgetScope::Org, id)
                    .map(|name| ScopeWithin::Org(name.to_string())),
            ),
            Some(ScopeWithin::Project(id)) => Some(
                self.get(BudgetScope::Project, id)
                    .map(|name| ScopeWithin::Project(name.to_string())),
            ),
        }
    }
}

/// [`render`], with the names the caps' scopes go by (#1312).
pub fn render_with(config: &GatewayConfig, refs: &ScopeRefs) -> String {
    render_document(config, refs, false, &[])
}

/// [`render_with`], optionally leaving the payload-capture table out because it
/// is limited to keys and so cannot be split per org (#3440). `shadowed` names
/// the deployment-wide rows a per-org file leaves out because the org has a row
/// of the same name (#3481).
fn render_document(
    config: &GatewayConfig,
    refs: &ScopeRefs,
    omit_capture: bool,
    shadowed: &[String],
) -> String {
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
    render_shadowed(&mut out, shadowed);
    render_providers(&mut out, config);
    render_provider_groups(&mut out, config);
    render_routes(&mut out, config);
    render_model_prices(&mut out, config);
    render_caps(&mut out, config, refs);
    render_mcp_manifest_enforcement(&mut out, config);
    render_mcp_gateway_settings(&mut out, config);
    render_mcp_gateway_tool_groups(&mut out, config);
    render_prompt_templates(&mut out, config);
    render_logging(&mut out, config, omit_capture);
    for comment in refs.omission_comments() {
        out.push_str(&format!("\n# not exported: {comment}\n"));
    }
    out
}

/// The comment naming the deployment-wide rows a per-org file leaves out
/// because the org shadows them (#3481). Nothing is written when none were.
fn render_shadowed(out: &mut String, shadowed: &[String]) {
    if shadowed.is_empty() {
        return;
    }
    out.push_str(
        "\n# left out because this organization has a row of the same name, which \
         shadows it (ADR-0040), so importing this file reproduces the organization's \
         view:\n",
    );
    for name in shadowed {
        out.push_str(&format!("#   {name}\n"));
    }
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

/// One `api_keys` entry as the export writes it: the variable name and nothing
/// else, so an inline `key` has no field to land in.
#[derive(Serialize)]
struct EnvKey<'a> {
    env: &'a str,
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
        //
        // written in the plural spelling the migration chain produces (#1720),
        // so `rolter check --migrations` reports the file as up to date. one
        // entry with the default weight is exactly what the store holds
        match provider.api_key_env_name() {
            Some(env) => key(out, "api_keys", &[EnvKey { env }]),
            None => out.push_str(
                "# api_key: not exported; any credential for this provider stays sealed in \
                 the control-plane store\n",
            ),
        }
        // the pool the provider actually uses, in the plural spelling: a legacy
        // `egress_proxy` is a one-element pool exactly when the list is empty
        let proxies = provider.egress_proxy_pool();
        if !proxies.is_empty() {
            key(out, "egress_proxies", &proxies);
        }
        // inline tables, so they cannot capture the keys after them; every
        // field is written out, which a re-import reads back identically
        if let Some(kv) = &provider.kv_events {
            key(out, "kv_events", kv);
        }
        if let Some(lmcache) = &provider.lmcache {
            key(out, "lmcache", lmcache);
        }
        // a file names no projects, so the scope round-trips as "the project
        // this file is imported into" (#1919)
        if provider.project_scoped {
            key(out, "project_scoped", &true);
        }
        // the host-pin opt-out round-trips, or a re-import would leave the
        // provider out of the snapshot (#1133)
        if provider.allow_custom_api_base {
            key(out, "allow_custom_api_base", &true);
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
        if group.project_scoped {
            key(out, "project_scoped", &true);
        }
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
                    OverrideMode::Unknown => "unknown",
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
            // a restricted fallback stays one through a round trip (#1584)
            if !target.takes_over.is_empty() {
                key(out, "takes_over", &target.takes_over);
            }
        }
        // declared order, never sorted: it is the fallback chain, and a
        // variant's targets are its routing order (#391)
        let exported: std::collections::HashSet<&str> = config
            .providers
            .iter()
            .chain(&config.provider_defaults)
            .map(|p| p.name.as_str())
            .collect();
        for variant in &route.variants {
            out.push_str("\n[[routes.variants]]\n");
            key(out, "name", &variant.name);
            key(out, "weight", &variant.weight);
            if !variant.params.is_empty() {
                out.push_str("\n[routes.variants.params]\n");
                let mut names: Vec<&String> = variant.params.keys().collect();
                names.sort();
                for name in names {
                    match variant.params.get(name) {
                        Some(value) if !value.is_null() => key(out, name, value),
                        _ => {
                            out.push_str(&format!(
                                "# {name}: not representable in toml, omitted\n"
                            ));
                        }
                    }
                }
            }
            for target in &variant.targets {
                // a stored target on a provider the snapshot left out is kept,
                // not dropped: the importer replaces a route's whole variant
                // set, so dropping it here would delete it on the next import.
                // naming a provider the file does not declare makes the
                // importer leave this route's variants as they are instead
                if !exported.contains(target.provider.as_str()) {
                    out.push_str(&format!(
                        "\n# provider '{}' is not in this export (left out of the snapshot); \
                         the importer leaves this route's variants as stored",
                        target.provider
                    ));
                }
                out.push_str("\n[[routes.variants.targets]]\n");
                key(out, "provider", &target.provider);
                if let Some(model) = &target.model {
                    key(out, "model", model);
                }
                key(out, "weight", &target.weight);
                if !target.takes_over.is_empty() {
                    key(out, "takes_over", &target.takes_over);
                }
            }
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
        if let Some(write) = price.cache_write_per_mtok {
            key(out, "cache_write_per_mtok", &write);
        }
        if let Some(write) = price.cache_write_1h_per_mtok {
            key(out, "cache_write_1h_per_mtok", &write);
        }
        if let Some(rate) = price.image_per_unit {
            key(out, "image_per_unit", &rate);
        }
        if let Some(rate) = price.audio_input_per_minute {
            key(out, "audio_input_per_minute", &rate);
        }
        if let Some(rate) = price.speech_per_mchar {
            key(out, "speech_per_mchar", &rate);
        }
        if let Some(rate) = price.audio_input_per_mtok {
            key(out, "audio_input_per_mtok", &rate);
        }
        if let Some(rate) = price.audio_output_per_mtok {
            key(out, "audio_output_per_mtok", &rate);
        }
        if let Some(rate) = price.cached_audio_input_per_mtok {
            key(out, "cached_audio_input_per_mtok", &rate);
        }
        key(out, "currency", &price.currency);
    }
}

/// `[[budgets]]` and `[[rate_limits]]`, each under the portable name of its
/// scope, sorted so two exports of an unchanged deployment are byte-identical.
/// A cap with no portable name is counted in one comment instead of being
/// written under an id that means nothing on another deployment.
fn render_caps(out: &mut String, config: &GatewayConfig, refs: &ScopeRefs) {
    // a cap travels when its scope, and its `within` if it has one, have names
    let named = |scope, id: &str, within: Option<&rolter_core::ScopeWithin>| {
        let reference = refs.get(scope, id)?;
        let within = match refs.within(within) {
            None => None,
            Some(named) => Some(named?),
        };
        Some((reference, within))
    };
    // the sort key spells `within` out, so two caps of one person on different
    // places keep a fixed order
    let place = |within: &Option<rolter_core::ScopeWithin>| {
        within
            .as_ref()
            .map(rolter_core::ScopeWithin::key_segment)
            .unwrap_or_default()
    };
    let mut budgets = Vec::new();
    let mut skipped_budgets = 0usize;
    for b in &config.budgets {
        match named(b.scope, &b.id, b.within.as_ref()) {
            Some((reference, within)) => {
                budgets.push((b.scope.key_label(), reference, place(&within), within, b))
            }
            None => skipped_budgets += 1,
        }
    }
    budgets.sort_by(|a, b| {
        (&a.0, a.1, &a.2, period_rank(a.4.period), a.4.limit_usd).cmp(&(
            &b.0,
            b.1,
            &b.2,
            period_rank(b.4.period),
            b.4.limit_usd,
        ))
    });
    for (_, reference, _, within, b) in budgets {
        out.push_str("\n[[budgets]]\n");
        key(out, "scope", &b.scope);
        key(out, "id", reference);
        if let Some(within) = &within {
            key(out, "within", within);
        }
        key(out, "limit_usd", &b.limit_usd);
        key(out, "period", &b.period);
        if let Some(policy) = &b.unpriced_policy {
            key(out, "unpriced_policy", policy);
        }
    }
    let mut limits = Vec::new();
    let mut skipped_limits = 0usize;
    for l in &config.rate_limits {
        match named(l.scope, &l.id, l.within.as_ref()) {
            Some((reference, within)) => {
                limits.push((l.scope.key_label(), reference, place(&within), within, l))
            }
            None => skipped_limits += 1,
        }
    }
    limits.sort_by(|a, b| {
        (&a.0, a.1, &a.2, a.4.rpm, a.4.tpm).cmp(&(&b.0, b.1, &b.2, b.4.rpm, b.4.tpm))
    });
    for (_, reference, _, within, l) in limits {
        out.push_str("\n[[rate_limits]]\n");
        key(out, "scope", &l.scope);
        key(out, "id", reference);
        if let Some(within) = &within {
            key(out, "within", within);
        }
        if let Some(rpm) = l.rpm {
            key(out, "rpm", &rpm);
        }
        if let Some(tpm) = l.tpm {
            key(out, "tpm", &tpm);
        }
    }
    if skipped_budgets + skipped_limits > 0 {
        out.push_str(&format!(
            "\n# not exported: {skipped_budgets} budget(s) and {skipped_limits} rate limit(s) on a \
             virtual key (keys are never exported), on an account (or any account, when person \
             caps are left out) or place with no portable name, or on another scope with no \
             name that survives a move\n"
        ));
    }
}

/// `[[mcp_manifest_enforcement]]`: the marker of each org that never saved MCP
/// settings (#3398). Without it a strict org reads back as unconfigured, and a
/// re-import that creates the org anew would give a grandfathered one the
/// strict default.
fn render_mcp_manifest_enforcement(out: &mut String, config: &GatewayConfig) {
    let mut rows: Vec<_> = config.mcp_manifest_enforcement.iter().collect();
    rows.sort_by(|a, b| a.org.cmp(&b.org));
    for row in rows {
        out.push_str("\n[[mcp_manifest_enforcement]]\n");
        key(out, "org", &row.org);
        key(out, "enforce", &row.enforce);
    }
}

/// `[[mcp_gateway_settings]]`: the settings each org saved on the MCP Settings
/// screen (#3482). Without them a round trip into a fresh database resets the
/// org to the new-org defaults.
fn render_mcp_gateway_settings(out: &mut String, config: &GatewayConfig) {
    let mut rows: Vec<_> = config.mcp_gateway_settings.iter().collect();
    rows.sort_by(|a, b| a.org.cmp(&b.org));
    for row in rows {
        out.push_str("\n[[mcp_gateway_settings]]\n");
        key(out, "org", &row.org);
        key(out, "default_transport", &row.default_transport);
        key(out, "connect_timeout_ms", &row.connect_timeout_ms);
        key(out, "request_timeout_ms", &row.request_timeout_ms);
        key(out, "max_retries", &row.max_retries);
        key(out, "default_failure_mode", &row.default_failure_mode);
        key(out, "allow_unlisted_tools", &row.allow_unlisted_tools);
    }
}

/// `[[mcp_gateway_tool_groups]]`: each org's MCP tool groups with their
/// assignments (#3514). Without them a round trip drops every tool
/// restriction, and the org's keys can call every tool its servers expose.
fn render_mcp_gateway_tool_groups(out: &mut String, config: &GatewayConfig) {
    let mut rows: Vec<_> = config.mcp_gateway_tool_groups.iter().collect();
    rows.sort_by(|a, b| (&a.org, &a.slug).cmp(&(&b.org, &b.slug)));
    for row in rows {
        out.push_str("\n[[mcp_gateway_tool_groups]]\n");
        key(out, "org", &row.org);
        key(out, "slug", &row.slug);
        key(out, "name", &row.name);
        if !row.description.is_empty() {
            key(out, "description", &row.description);
        }
        key(out, "enabled", &row.enabled);
        let mut tools = row.tools.clone();
        tools.sort_by(|a, b| (&a.server, &a.tool).cmp(&(&b.server, &b.tool)));
        key(out, "tools", &tools);
        for (name, list) in [("teams", &row.teams), ("projects", &row.projects)] {
            if !list.is_empty() {
                let mut list = list.clone();
                list.sort();
                key(out, name, &list);
            }
        }
        if !row.keys.is_empty() {
            let mut keys = row.keys.clone();
            keys.sort_by(|a, b| {
                (&a.project, &a.name, &a.prefix).cmp(&(&b.project, &b.name, &b.prefix))
            });
            // duplicate public references are written once; an import refuses
            // one that matches several keys rather than guessing its subject
            keys.dedup();
            key(out, "keys", &keys);
        }
    }
}

fn period_rank(period: rolter_core::BudgetPeriod) -> u8 {
    match period {
        rolter_core::BudgetPeriod::Daily => 0,
        rolter_core::BudgetPeriod::Monthly => 1,
        rolter_core::BudgetPeriod::Total => 2,
        rolter_core::BudgetPeriod::Unknown => 3,
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

fn render_logging(out: &mut String, config: &GatewayConfig, omit_capture: bool) {
    // always written, even at the default: the importer applies ui_events only
    // when the file names it, so an omitted key would let a re-import onto a
    // deployment with the stream off leave it off where this one had it on
    out.push_str("\n[logging]\n");
    key(out, "ui_events", &config.logging.ui_events);
    if omit_capture {
        // the importer applies a capture table the file declares to the whole
        // deployment, so a file without one leaves capture as it is
        out.push_str(
            "\n# [logging.payload_capture] is not exported: this deployment limits capture to \
             virtual keys across organizations, which a per-organization file cannot \
             split. Export the whole deployment to carry it.\n",
        );
        return;
    }
    let capture = &config.logging.payload_capture;
    out.push_str("\n[logging.payload_capture]\n");
    key(out, "enabled", &capture.enabled);
    key(out, "max_bytes", &capture.max_bytes);
    key(out, "redact_fields", &capture.redact_fields);
    key(out, "redact_detectors", &capture.redact_detectors);
    key(out, "models", &capture.models);
    out.push_str("# virtual key ids are this deployment's own; drop them when promoting\n");
    key(out, "virtual_key_ids", &capture.virtual_key_ids);
    // array-of-tables entries attach to the table above, so they come last
    for pattern in &capture.redact_patterns {
        out.push_str("\n[[logging.payload_capture.redact_patterns]]\n");
        key(out, "name", &pattern.name);
        key(out, "pattern", &pattern.pattern);
    }
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
        // a value a newer release stored (#3347); importing the file refuses
        // it rather than guessing
        BalancingStrategy::Unknown => "unknown",
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

/// The names the deployment's tenancy goes by in a portable file: org slug,
/// team and project names under it, and business unit and customer slugs.
/// Read from the tables directly, since the snapshot a [`GatewayConfig`] is
/// built from carries ids and nothing that names them (#1312).
#[cfg(feature = "postgres")]
pub async fn load_scope_refs(
    pool: &sqlx::PgPool,
    with_people: bool,
) -> rolter_core::Result<ScopeRefs> {
    use rolter_core::BudgetScope;
    use rolter_store::postgres::store_err;
    let mut refs = ScopeRefs::default();
    let rows: Vec<(uuid::Uuid, String)> = sqlx::query_as("select id, slug from orgs")
        .fetch_all(pool)
        .await
        .map_err(store_err)?;
    for (id, slug) in rows {
        refs.insert_owned(BudgetScope::Org, &id.to_string(), &slug.clone(), slug);
    }
    let rows: Vec<(uuid::Uuid, String, String)> =
        sqlx::query_as("select t.id, o.slug, t.name from teams t join orgs o on o.id = t.org_id")
            .fetch_all(pool)
            .await
            .map_err(store_err)?;
    for (id, org, team) in rows {
        refs.insert_owned(
            BudgetScope::Team,
            &id.to_string(),
            &org,
            format!("{org}/{team}"),
        );
    }
    let rows: Vec<(uuid::Uuid, String, String, String)> = sqlx::query_as(
        "select p.id, o.slug, t.name, p.name from projects p
         join teams t on t.id = p.team_id join orgs o on o.id = t.org_id",
    )
    .fetch_all(pool)
    .await
    .map_err(store_err)?;
    for (id, org, team, project) in rows {
        refs.insert_owned(
            BudgetScope::Project,
            &id.to_string(),
            &org,
            format!("{org}/{team}/{project}"),
        );
    }
    // a person is named by the account's email, so only the accounts that hold
    // a cap are read: a deployment's whole user table has no business in a
    // configuration file (#3311)
    if with_people {
        let rows: Vec<(uuid::Uuid, String)> = sqlx::query_as(
            "select id, email from users where id in (
                 select scope_id from budgets where scope_type = 'user'
                 union select scope_id from rate_limits where scope_type = 'user')",
        )
        .fetch_all(pool)
        .await
        .map_err(store_err)?;
        for (id, email) in rows {
            refs.insert(BudgetScope::User, &id.to_string(), email);
        }
    }
    for (table, scope) in [
        ("business_units", BudgetScope::BusinessUnit),
        ("customers", BudgetScope::Customer),
    ] {
        let rows: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select x.id, o.slug, x.slug from {table} x join orgs o on o.id = x.org_id"
        )))
        .fetch_all(pool)
        .await
        .map_err(store_err)?;
        for (id, org, slug) in rows {
            refs.insert_owned(scope, &id.to_string(), &org, format!("{org}/{slug}"));
        }
    }
    Ok(refs)
}

/// What an export covers. The default is the whole deployment.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    /// Export this organization's rows only (#3440), by slug. Names are per org
    /// (#1857) and a file carries none, so a deployment whose orgs share a
    /// route or provider name is exported one org per file. Rows owned by no
    /// org (those of the gateway's own config file) are written as they are in
    /// the unscoped export.
    pub org: Option<String>,
    /// Leave caps on a person out (#3396). Such a cap names the account by its
    /// email, and a team that commits `rolter.toml` may keep addresses out of
    /// it (#1312). The caps are counted in the trailing comment as key caps are,
    /// and no address is read from the database at all.
    pub without_person_caps: bool,
}

/// Why an export failed, for a caller that answers the refusal to export the
/// whole deployment differently from every other failure (#3480).
#[cfg(feature = "postgres")]
#[derive(Debug)]
pub enum ExportError {
    /// two organizations share a name, so the whole deployment is not one
    /// loadable file; exporting a single organization is. Carries the message
    /// naming each shared name
    NeedsOrganization(String),
    /// a deployment-wide row the file keeps names a row the organization
    /// shadows, so an import would silently rebind it to the organization's
    /// row. Carries the message naming both rows
    ShadowedBinding(String),
    Other(rolter_core::Error),
}

#[cfg(feature = "postgres")]
impl From<rolter_core::Error> for ExportError {
    fn from(err: rolter_core::Error) -> Self {
        Self::Other(err)
    }
}

#[cfg(feature = "postgres")]
impl From<ExportError> for rolter_core::Error {
    fn from(err: ExportError) -> Self {
        match err {
            ExportError::NeedsOrganization(message) | ExportError::ShadowedBinding(message) => {
                Self::Config(message)
            }
            ExportError::Other(err) => err,
        }
    }
}

/// The deployment's configuration as an importable document, caps included.
/// The one call both the endpoint and `rolter config export` make, so they
/// cannot drift.
#[cfg(feature = "postgres")]
pub async fn export_document(
    pool: &sqlx::PgPool,
    config: &GatewayConfig,
    options: &ExportOptions,
) -> rolter_core::Result<String> {
    export_document_typed(pool, config, options)
        .await
        .map_err(Into::into)
}

/// [`export_document`], keeping the refusal to export the whole deployment
/// apart so the endpoint can give it a stable code.
#[cfg(feature = "postgres")]
pub(crate) async fn export_document_typed(
    pool: &sqlx::PgPool,
    config: &GatewayConfig,
    options: &ExportOptions,
) -> Result<String, ExportError> {
    let mut config = config.clone();
    let refs = load_scope_refs(pool, !options.without_person_caps).await?;
    let mut scope = OrgScope::default();
    if let Some(slug) = &options.org {
        let view = load_org_view(pool, slug, &config).await?;
        scope = scope_to_org(&mut config, &view, &refs)?;
    }
    let clashes = clashing_names(&config);
    if !clashes.is_empty() {
        // `--org` settles a clash when every organization's own file loads: the
        // rows it shares with another organization, or with a no-org row it
        // shadows, stop being in one file. A slug shared with a no-org row of
        // another name stays, and wants a rename
        let needs_org = options.org.is_none() && an_org_settles_every_clash(&config);
        let hint = if needs_org {
            format!(
                "{} is used by more than one row (two organizations, or an organization and \
                 a deployment-wide row it shadows), and a file carries no organization, so it \
                 would not load; export one organization at a time with `--org <slug>` \
                 (`?org=<slug>` on the endpoint)",
                clashes.join(", ")
            )
        } else {
            format!(
                "{} is used by a row owned by no organization (the gateway's own config file) \
                 and by another row that `--org` does not set aside, so it would not load; \
                 rename one of them",
                clashes.join(", ")
            )
        };
        let message = format!("this deployment cannot be exported as one file: {hint} (#3440)");
        return Err(if needs_org {
            ExportError::NeedsOrganization(message)
        } else {
            ExportError::Other(rolter_core::Error::Config(message))
        });
    }
    with_stored_variants(pool, &mut config).await?;
    config.mcp_manifest_enforcement = load_mcp_manifest_enforcement(pool, options).await?;
    config.mcp_gateway_settings = load_mcp_gateway_settings(pool, options).await?;
    config.mcp_gateway_tool_groups = load_mcp_gateway_tool_groups(pool, options, &refs).await?;
    Ok(render_document(
        &config,
        &refs,
        scope.omit_capture,
        &scope.shadowed,
    ))
}

/// The marker of each org that has not saved MCP settings, which is the only
/// org the marker decides for (#3398): one with a saved row follows that row.
#[cfg(feature = "postgres")]
async fn load_mcp_manifest_enforcement(
    pool: &sqlx::PgPool,
    options: &ExportOptions,
) -> rolter_core::Result<Vec<rolter_core::McpManifestEnforcementConfig>> {
    use rolter_store::postgres::store_err;
    let rows: Vec<(String, bool)> = sqlx::query_as(
        "select o.slug, o.mcp_enforce_manifests from orgs o
         where not exists (select 1 from mcp_gateway_settings s where s.org_id = o.id)
           and ($1::text is null or o.slug = $1)
         order by o.slug",
    )
    .bind(&options.org)
    .fetch_all(pool)
    .await
    .map_err(store_err)?;
    Ok(rows
        .into_iter()
        .map(|(org, enforce)| rolter_core::McpManifestEnforcementConfig { org, enforce })
        .collect())
}

/// The settings each org saved, which are the only ones the marker above does
/// not decide for (#3482).
#[cfg(feature = "postgres")]
async fn load_mcp_gateway_settings(
    pool: &sqlx::PgPool,
    options: &ExportOptions,
) -> rolter_core::Result<Vec<rolter_core::McpGatewaySettingsConfig>> {
    use rolter_store::postgres::store_err;
    let rows: Vec<(String, String, i32, i32, i32, String, bool)> = sqlx::query_as(
        "select o.slug, s.default_transport, s.connect_timeout_ms, s.request_timeout_ms,
                s.max_retries, s.default_failure_mode, s.allow_unlisted_tools
         from mcp_gateway_settings s join orgs o on o.id = s.org_id
         where $1::text is null or o.slug = $1
         order by o.slug",
    )
    .bind(&options.org)
    .fetch_all(pool)
    .await
    .map_err(store_err)?;
    Ok(rows
        .into_iter()
        .map(
            |(
                org,
                default_transport,
                connect_timeout_ms,
                request_timeout_ms,
                max_retries,
                default_failure_mode,
                allow_unlisted_tools,
            )| rolter_core::McpGatewaySettingsConfig {
                org,
                default_transport,
                connect_timeout_ms,
                request_timeout_ms,
                max_retries,
                default_failure_mode,
                allow_unlisted_tools,
            },
        )
        .collect())
}

/// Each org's MCP tool groups with their assignments (#3514), every reference
/// by the name an import resolves: a server by its slug, a team or project by
/// its slug path, a key by its project and its name or prefix. A reference
/// that has no such name (a tool of a deleted server, a team whose name holds
/// a `/`) is left out, since it would name nothing on another deployment.
#[cfg(feature = "postgres")]
async fn load_mcp_gateway_tool_groups(
    pool: &sqlx::PgPool,
    options: &ExportOptions,
    refs: &ScopeRefs,
) -> rolter_core::Result<Vec<rolter_core::McpGatewayToolGroupConfig>> {
    use rolter_core::{BudgetScope, McpGatewayKeyRefConfig, McpGatewayToolRefConfig};
    use rolter_store::postgres::store_err;
    let groups: Vec<(
        uuid::Uuid,
        String,
        String,
        String,
        String,
        bool,
        serde_json::Value,
    )> = sqlx::query_as(
        "select g.id, o.slug, g.slug, g.name, g.description, g.enabled, g.tools
             from mcp_tool_groups g join orgs o on o.id = g.org_id
             where $1::text is null or o.slug = $1
             order by o.slug, g.slug",
    )
    .bind(&options.org)
    .fetch_all(pool)
    .await
    .map_err(store_err)?;
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let servers: std::collections::HashMap<uuid::Uuid, String> =
        sqlx::query_as::<_, (uuid::Uuid, String)>("select id, slug from mcp_servers")
            .fetch_all(pool)
            .await
            .map_err(store_err)?
            .into_iter()
            .collect();
    let assignments: Vec<(uuid::Uuid, String, uuid::Uuid)> =
        sqlx::query_as("select group_id, subject_kind, subject_id from mcp_tool_group_assignments")
            .fetch_all(pool)
            .await
            .map_err(store_err)?;
    let keys: std::collections::HashMap<uuid::Uuid, (uuid::Uuid, Option<String>, String)> =
        sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid, Option<String>, String)>(
            "select k.id, k.project_id, k.name, k.key_prefix from virtual_keys k
             where k.id in (select subject_id from mcp_tool_group_assignments
                            where subject_kind = 'virtual_key')",
        )
        .fetch_all(pool)
        .await
        .map_err(store_err)?
        .into_iter()
        .map(|(id, project, name, prefix)| (id, (project, name, prefix)))
        .collect();
    Ok(groups
        .into_iter()
        .map(|(id, org, slug, name, description, enabled, tools)| {
            // the store only ever holds the API's `{server_id, tool}` pairs;
            // one written around it is left out, as the snapshot leaves it out
            let tools = serde_json::from_value::<Vec<rolter_core::McpToolRefConfig>>(tools)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|tool| {
                    let server = servers.get(&uuid::Uuid::parse_str(&tool.server_id).ok()?)?;
                    Some(McpGatewayToolRefConfig {
                        server: server.clone(),
                        tool: tool.tool,
                    })
                })
                .collect();
            let mut group = rolter_core::McpGatewayToolGroupConfig {
                org,
                slug,
                name,
                description,
                enabled,
                tools,
                ..Default::default()
            };
            for (_, kind, subject) in assignments.iter().filter(|(group, _, _)| *group == id) {
                let named = match kind.as_str() {
                    "team" => refs
                        .get(BudgetScope::Team, &subject.to_string())
                        .map(|team| group.teams.push(team.to_string())),
                    "project" => refs
                        .get(BudgetScope::Project, &subject.to_string())
                        .map(|project| group.projects.push(project.to_string())),
                    "virtual_key" => keys.get(subject).and_then(|(project, key_name, prefix)| {
                        let project = refs.get(BudgetScope::Project, &project.to_string())?;
                        // the key's public identifier: its name, else the
                        // prefix the dashboard shows
                        let key_name = key_name.clone().filter(|name| !name.trim().is_empty());
                        group.keys.push(McpGatewayKeyRefConfig {
                            project: project.to_string(),
                            prefix: key_name.is_none().then(|| prefix.clone()),
                            name: key_name,
                        });
                        Some(())
                    }),
                    _ => None,
                };
                if named.is_none() {
                    let subject = match kind.as_str() {
                        "team" => "team",
                        "project" => "project",
                        "virtual_key" => "virtual key",
                        _ => "subject",
                    };
                    refs.record_omission(format!(
                        "MCP tool group {subject} assignment, no portable subject name"
                    ));
                    // the file would name nothing an import can find, so the
                    // operator hears that the assignment does not travel
                    tracing::warn!(
                        org = %group.org,
                        group = %group.slug,
                        kind = %kind,
                        "an MCP tool group assignment has no portable name and is not exported"
                    );
                }
            }
            group
        })
        .collect())
}

/// An org as a per-org export needs to know it (#3440): which people are
/// members and which virtual keys were minted in it.
#[cfg(feature = "postgres")]
struct OrgView {
    id: String,
    slug: String,
    /// ids of the accounts holding a membership of the org, or of one of its
    /// teams or projects
    members: std::collections::HashSet<String>,
    /// virtual key id to the id of the org it was minted in
    key_orgs: std::collections::HashMap<String, String>,
}

#[cfg(feature = "postgres")]
async fn load_org_view(
    pool: &sqlx::PgPool,
    slug: &str,
    config: &GatewayConfig,
) -> rolter_core::Result<OrgView> {
    use rolter_store::postgres::store_err;
    let org_id: Option<uuid::Uuid> = sqlx::query_scalar("select id from orgs where slug = $1")
        .bind(slug)
        .fetch_optional(pool)
        .await
        .map_err(store_err)?;
    let org_id = org_id.ok_or_else(|| {
        rolter_core::Error::Config(format!("no organization has the slug '{slug}'"))
    })?;
    let members: Vec<uuid::Uuid> = sqlx::query_scalar(
        "select distinct m.user_id from memberships m
         left join teams t on t.id = m.team_id
         left join projects p on p.id = m.project_id
         left join teams pt on pt.id = p.team_id
         where coalesce(m.org_id, t.org_id, pt.org_id) = $1",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
    .map_err(store_err)?;
    Ok(OrgView {
        id: org_id.to_string(),
        slug: slug.to_string(),
        members: members.into_iter().map(|id| id.to_string()).collect(),
        key_orgs: config
            .db_virtual_keys
            .iter()
            .map(|key| (key.id.clone(), key.org_id.clone()))
            .collect(),
    })
}

/// What [`scope_to_org`] decided about the parts of the file that are not just
/// a filter on rows.
#[cfg(feature = "postgres")]
#[derive(Debug, Default)]
struct OrgScope {
    /// the payload-capture policy is limited to keys, so the file omits it
    omit_capture: bool,
    /// deployment-wide rows left out because the org has one of the same name,
    /// such as `route 'gpt-4o'`, sorted
    shadowed: Vec<String>,
}

/// Leave in `config` the rows that belong in the file of the org `org_id`
/// (#3440): its own, and those no org owns, minus the no-org rows it shadows
/// (#3481). Returns what was left out, such as `route 'gpt-4o'`, sorted.
///
/// A route and a provider are identified by name, a provider group by slug
/// (the identity `clashing_names` and the importer use), so a group is shadowed
/// by a group of the same slug whatever it is called.
///
/// Refuses when a no-org row the file keeps names a row that was left out: the
/// import turns the kept row into the org's own copy, and the copy would resolve
/// that name to the org's row, with its credentials and `api_base`. The file
/// cannot reproduce the binding, so the message names both rows.
#[cfg(any(feature = "postgres", test))]
fn narrow_to_org_rows(config: &mut GatewayConfig, org_id: &str) -> Result<Vec<String>, String> {
    use std::collections::{BTreeSet, HashSet};
    let owned = |tenancy: &Option<rolter_core::Tenancy>| {
        tenancy.as_ref().is_none_or(|owner| owner.org_id == org_id)
    };
    let own = |tenancy: &Option<rolter_core::Tenancy>| {
        tenancy.as_ref().is_some_and(|owner| owner.org_id == org_id)
    };
    config.providers.retain(|p| owned(&p.tenancy));
    config.provider_groups.retain(|g| owned(&g.tenancy));
    config.routes.retain(|r| owned(&r.tenancy));
    // a stored template's id is `{org_id}:{slug}`; one from a file has no org
    config.prompt_templates.templates.retain(|t| {
        t.id.rsplit_once(':')
            .is_none_or(|(owner, _)| owner == org_id)
    });

    let mut shadowed = BTreeSet::new();
    let routes: HashSet<String> = config
        .routes
        .iter()
        .filter(|r| own(&r.tenancy))
        .map(|r| r.model.clone())
        .collect();
    config.routes.retain(|r| {
        let keep = own(&r.tenancy) || !routes.contains(&r.model);
        if !keep {
            shadowed.insert(format!("route '{}'", r.model));
        }
        keep
    });
    let providers: HashSet<String> = config
        .providers
        .iter()
        .filter(|p| own(&p.tenancy))
        .map(|p| p.name.clone())
        .collect();
    let mut dropped_providers = HashSet::new();
    config.providers.retain(|p| {
        let keep = own(&p.tenancy) || !providers.contains(&p.name);
        if !keep {
            shadowed.insert(format!("provider '{}'", p.name));
            dropped_providers.insert(p.name.clone());
        }
        keep
    });
    let groups: HashSet<String> = config
        .provider_groups
        .iter()
        .filter(|g| own(&g.tenancy))
        .map(group_slug)
        .collect();
    config.provider_groups.retain(|g| {
        let keep = own(&g.tenancy) || !groups.contains(&group_slug(g));
        if !keep {
            shadowed.insert(format!("provider group '{}'", group_slug(g)));
        }
        keep
    });

    let binds = |name: &str| dropped_providers.contains(name);
    for route in config.routes.iter().filter(|r| r.tenancy.is_none()) {
        let targets = route
            .targets
            .iter()
            .chain(route.variants.iter().flat_map(|v| v.targets.iter()));
        if let Some(target) = targets.into_iter().find(|t| binds(&t.provider)) {
            return Err(binding_message(
                &format!("route '{}'", route.model),
                &target.provider,
            ));
        }
    }
    for group in config
        .provider_groups
        .iter()
        .filter(|g| g.tenancy.is_none())
    {
        if let Some(member) = group.members.iter().find(|m| binds(&m.provider)) {
            return Err(binding_message(
                &format!("provider group '{}'", group_slug(group)),
                &member.provider,
            ));
        }
    }
    Ok(shadowed.into_iter().collect())
}

#[cfg(any(feature = "postgres", test))]
fn binding_message(row: &str, provider: &str) -> String {
    format!(
        "{row} (owned by no organization) uses provider '{provider}', which this organization \
         shadows with its own provider '{provider}'; an import would bind the organization's \
         copy of the {row} to the organization's provider and change its credentials and \
         api_base, and the file cannot say otherwise. Rename the organization's provider, or \
         give the {row} a provider of another name (#3481)"
    )
}

/// Whether exporting one organization settles every clash of an unscoped
/// export: each organization's own file, with the no-org rows it shadows left
/// out, loads on its own. Then the way out is to pick an organization, rather
/// than to rename a row.
#[cfg(any(feature = "postgres", test))]
fn an_org_settles_every_clash(config: &GatewayConfig) -> bool {
    use std::collections::BTreeSet;
    let orgs: BTreeSet<&str> = config
        .providers
        .iter()
        .map(|p| &p.tenancy)
        .chain(config.provider_groups.iter().map(|g| &g.tenancy))
        .chain(config.routes.iter().map(|r| &r.tenancy))
        .filter_map(|t| t.as_ref().map(|t| t.org_id.as_str()))
        .collect();
    !orgs.is_empty()
        && orgs.into_iter().all(|org| {
            let mut narrowed = config.clone();
            // a refused binding is a different problem from a clash, and the
            // organization's export says which
            narrow_to_org_rows(&mut narrowed, org).is_err() || clashing_names(&narrowed).is_empty()
        })
}

/// Leave in `config` what belongs in the file of `org` (#3440): its providers,
/// groups, routes, prompt templates and caps, and the rows no org owns that it
/// does not shadow. Prices, currency and logging are deployment-wide and stay.
#[cfg(feature = "postgres")]
fn scope_to_org(
    config: &mut GatewayConfig,
    org: &OrgView,
    refs: &ScopeRefs,
) -> Result<OrgScope, ExportError> {
    let shadowed = narrow_to_org_rows(config, &org.id).map_err(ExportError::ShadowedBinding)?;
    config
        .budgets
        .retain(|b| refs.in_org(org, b.scope, &b.id, b.within.as_ref()));
    config
        .rate_limits
        .retain(|l| refs.in_org(org, l.scope, &l.id, l.within.as_ref()));
    // capture is one deployment-wide policy, which the importer applies whole.
    // one limited to keys spans orgs, so a per-org file leaves it out
    Ok(OrgScope {
        omit_capture: !config.logging.payload_capture.virtual_key_ids.is_empty(),
        shadowed,
    })
}

/// The names two rows share in `config`, which a flat file cannot carry.
///
/// Names are per org (#1857), but the export writes every org's rows into one
/// file with no org on them, so a route, provider or provider-group name, or a
/// provider or group slug, that two orgs share (or an org shares with a
/// readonly row) would make a file `validate` refuses. One entry per clash,
/// such as `route 'gpt-4o'`, sorted.
#[cfg(any(feature = "postgres", test))]
fn clashing_names(config: &GatewayConfig) -> Vec<String> {
    use std::collections::{BTreeSet, HashSet};
    fn repeated<'a>(kind: &str, names: impl Iterator<Item = &'a str>, out: &mut BTreeSet<String>) {
        let mut seen = HashSet::new();
        for name in names {
            if !seen.insert(name) {
                out.insert(format!("{kind} '{name}'"));
            }
        }
    }
    let mut clashes = BTreeSet::new();
    repeated(
        "route",
        config.routes.iter().map(|r| r.model.as_str()),
        &mut clashes,
    );
    repeated(
        "provider",
        config.providers.iter().map(|p| p.name.as_str()),
        &mut clashes,
    );
    let slugs: Vec<String> = config
        .providers
        .iter()
        .map(provider_slug)
        .chain(config.provider_groups.iter().map(|g| {
            g.slug
                .clone()
                .unwrap_or_else(|| rolter_core::slug::slugify(&g.name))
        }))
        .collect();
    repeated("slug", slugs.iter().map(String::as_str), &mut clashes);
    // two orgs publishing the same template slug would be one template in a
    // flat file, which the importer merges into a single org's row
    repeated(
        "prompt template",
        config
            .prompt_templates
            .templates
            .iter()
            .map(|t| template_slug(&t.id)),
        &mut clashes,
    );
    clashes.into_iter().collect()
}

/// Replace each database route's variants with the set as stored (#391).
///
/// The loaded config is what the gateway serves, which drops a variant target
/// on a provider left out of the snapshot, and a variant left with no target.
/// `rolter-seed --import` replaces a route's whole variant set, so an export of
/// the served set would delete those on the next import. The stored set keeps
/// them, and [`render`] marks a target whose provider the file does not carry.
#[cfg(feature = "postgres")]
async fn with_stored_variants(
    pool: &sqlx::PgPool,
    config: &mut GatewayConfig,
) -> rolter_core::Result<()> {
    use rolter_store::postgres::store_err;

    // keyed by the route's project as well as its model: two orgs can each have a
    // `gpt-4o` (#1857), and a model name alone would hand one org the other's
    // variants (#3440)
    let variants: Vec<(String, String, uuid::Uuid, String, i32, serde_json::Value)> =
        sqlx::query_as(
            "select r.project_id::text, r.model, v.id, v.name, v.weight, v.params
         from route_variants v join routes r on r.id = v.route_id
         where r.enabled order by r.model, v.position",
        )
        .fetch_all(pool)
        .await
        .map_err(store_err)?;
    type TargetRow = (uuid::Uuid, String, Option<String>, i32, Vec<String>);
    let targets: Vec<TargetRow> = sqlx::query_as(
        "select vt.variant_id, p.name, vt.upstream_model, vt.weight, vt.takes_over
         from route_variant_targets vt join providers p on p.id = vt.provider_id
         order by vt.variant_id, vt.position",
    )
    .fetch_all(pool)
    .await
    .map_err(store_err)?;
    let mut by_route: std::collections::HashMap<(String, String), Vec<rolter_core::Variant>> =
        std::collections::HashMap::new();
    for (project, model, id, name, weight, params) in variants {
        by_route
            .entry((project, model))
            .or_default()
            .push(rolter_core::Variant {
                name,
                weight: weight.max(0) as u32,
                // the column is held to an object by a check constraint
                params: serde_json::from_value(params).unwrap_or_default(),
                targets: targets
                    .iter()
                    .filter(|t| t.0 == id)
                    .filter_map(|(_, provider, model, weight, takes_over)| {
                        // a class the gateway cannot read is left out with its
                        // target, as the snapshot leaves it out (#3373); the
                        // control plane never stores one
                        let takes_over = takes_over
                            .iter()
                            .map(|token| token.parse())
                            .collect::<Result<Vec<_>, String>>()
                            .map_err(|problem| {
                                tracing::warn!(
                                    provider = %provider,
                                    "leaving a variant target out of the export: {problem}"
                                );
                            })
                            .ok()?;
                        Some(rolter_core::Target {
                            provider: provider.clone(),
                            model: model.clone(),
                            weight: (*weight).max(0) as u32,
                            takes_over,
                        })
                    })
                    .collect(),
            });
    }
    for route in &mut config.routes {
        // only a database route has a tenancy; a file route keeps its own
        if let Some(project) = route.tenancy.as_ref().and_then(|t| t.project_id.clone()) {
            route.variants = by_route
                .remove(&(project, route.model.clone()))
                .unwrap_or_default();
        }
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) fn router() -> Router<ControlState> {
    Router::new().route("/api/v1/config/export", get(export_config))
}

/// stable code of the 400 an export of the whole deployment answers while two
/// organizations share a name: the caller exports one organization instead
/// (`?org=<slug>`). Part of the API and never renamed (#3480)
#[cfg(feature = "postgres")]
pub(crate) const CONFIG_EXPORT_NEEDS_ORG: &str = "config_export_needs_org";

/// stable code of the 400 a per-organization export answers when a deployment-wide
/// row it keeps uses a provider the organization shadows: the file could not
/// reproduce the binding. Part of the API and never renamed (#3481)
#[cfg(feature = "postgres")]
pub(crate) const CONFIG_EXPORT_BINDING_SHADOWED: &str = "config_export_binding_shadowed";

/// A 400 in the envelope every refusal renders, with a code a client can
/// translate.
#[cfg(feature = "postgres")]
fn coded_refusal(code: &'static str, message: String) -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

/// Query of `GET /api/v1/config/export`.
#[cfg(feature = "postgres")]
#[derive(serde::Deserialize)]
struct ExportQuery {
    /// export this organization's rows only, by slug (#3440)
    org: Option<String>,
    /// leave caps on a person, which name an account by email, out (#3396)
    #[serde(default)]
    without_person_caps: bool,
}

/// The whole deployment's configuration as an importable `rolter.toml`, or one
/// organization's with `?org=<slug>`.
///
/// Superadmin-only and whole-deployment: the document spans every org's
/// providers and groups, so there is no tenancy scope narrow enough to
/// authorize it. Served as `application/toml` with a download filename so a
/// browser saves it rather than rendering it.
#[cfg(feature = "postgres")]
async fn export_config(
    principal: Principal,
    State(state): State<ControlState>,
    Query(query): Query<ExportQuery>,
) -> ApiResult<Response> {
    authorize_superadmin(&principal, superadmin_cap!("config_export", Read))?;
    let config = state.store.load().await?;
    let options = ExportOptions {
        org: query.org,
        without_person_caps: query.without_person_caps,
    };
    let document = match export_document_typed(crate::crud::pool(&state), &config, &options).await {
        Ok(document) => document,
        // the same envelope every refusal renders, with a code the dashboard
        // can translate (#3480)
        Err(ExportError::NeedsOrganization(message)) => {
            return Ok(coded_refusal(CONFIG_EXPORT_NEEDS_ORG, message));
        }
        Err(ExportError::ShadowedBinding(message)) => {
            return Ok(coded_refusal(CONFIG_EXPORT_BINDING_SHADOWED, message));
        }
        Err(ExportError::Other(err)) => return Err(err.into()),
    };
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
        document,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::{ParamPolicy, ProviderKind, Target};

    /// Two orgs sharing a name is a deployment one flat file cannot carry
    /// (#1857, #3440): the export names every clash rather than writing a
    /// file that would not load.
    #[test]
    fn names_two_orgs_share_are_named_as_clashes() {
        let mut config = GatewayConfig::default();
        for org in ["org-a", "org-b"] {
            let tenancy = Some(rolter_core::Tenancy {
                org_id: org.to_string(),
                project_id: None,
            });
            config.providers.push(rolter_core::ProviderConfig {
                name: "openai".to_string(),
                kind: ProviderKind::Openai,
                api_base: "https://api.openai.com".to_string(),
                tenancy: tenancy.clone(),
                ..Default::default()
            });
            config.routes.push(
                serde_json::from_value(serde_json::json!({
                    "model": "gpt-4o",
                    "targets": [{"provider": "openai"}],
                    "tenancy": {"org_id": org},
                }))
                .unwrap(),
            );
        }
        assert_eq!(
            clashing_names(&config),
            ["provider 'openai'", "route 'gpt-4o'", "slug 'openai'"]
        );
        config.providers[1].name = "openai-b".to_string();
        config.providers[1].slug = Some("openai-b".to_string());
        config.routes[1].model = "gpt-4o-b".to_string();
        assert!(clashing_names(&config).is_empty());
    }

    /// Two orgs publishing the same template slug are one template in a flat
    /// file, which the importer would merge into a single org's row (#3440).
    fn tenanted(org: &str) -> Option<rolter_core::Tenancy> {
        Some(rolter_core::Tenancy {
            org_id: org.to_string(),
            project_id: None,
        })
    }

    fn group(name: &str, slug: &str, provider: &str, org: Option<&str>) -> ProviderGroupConfig {
        ProviderGroupConfig {
            name: name.to_string(),
            slug: Some(slug.to_string()),
            members: vec![rolter_core::GroupMember {
                provider: provider.to_string(),
                model: None,
                weight: 1,
            }],
            tenancy: org.and_then(tenanted),
            ..Default::default()
        }
    }

    /// A group is identified by its slug (#3481): a global group of another
    /// name but the same slug is shadowed, and one of the same name but another
    /// slug is not.
    #[test]
    fn a_group_is_shadowed_by_slug_not_by_display_name() {
        let mut config = GatewayConfig {
            provider_groups: vec![
                group("Primary", "us", "a", Some("org-a")),
                group("Europe", "eu2", "a", Some("org-a")),
                group("Primary", "eu", "a", None),
                group("EU", "eu2", "a", None),
            ],
            ..Default::default()
        };
        let shadowed = narrow_to_org_rows(&mut config, "org-a").unwrap();
        assert_eq!(shadowed, ["provider group 'eu2'"]);
        let slugs: Vec<_> = config.provider_groups.iter().map(group_slug).collect();
        assert_eq!(slugs, ["us", "eu2", "eu"]);
        assert!(clashing_names(&config).is_empty());
    }

    /// A kept deployment-wide row that uses a provider the org shadows would
    /// be bound to the org's provider by an import (#3481), so the export
    /// refuses and names both.
    #[test]
    fn a_kept_row_bound_to_a_shadowed_provider_is_refused() {
        let provider = |org: Option<&str>| rolter_core::ProviderConfig {
            name: "openai".to_string(),
            tenancy: org.and_then(tenanted),
            ..Default::default()
        };
        let mut config = GatewayConfig {
            providers: vec![provider(Some("org-a")), provider(None)],
            routes: vec![route("gpt-4o")],
            ..Default::default()
        };
        config.routes[0].targets =
            vec![serde_json::from_value(serde_json::json!({"provider": "openai"})).unwrap()];
        let err = narrow_to_org_rows(&mut config.clone(), "org-a").unwrap_err();
        assert!(err.contains("route 'gpt-4o'"), "{err}");
        assert!(err.contains("provider 'openai'"), "{err}");
        // a group member binds the same way
        config.routes.clear();
        config.provider_groups = vec![group("G", "g", "openai", None)];
        let err = narrow_to_org_rows(&mut config, "org-a").unwrap_err();
        assert!(err.contains("provider group 'g'"), "{err}");
    }

    /// `--org` only settles a clash it separates: a slug shared with a no-org
    /// row of another name is the operator's to rename.
    #[test]
    fn an_org_settles_a_shadow_but_not_a_shared_slug() {
        let provider = |name: &str, slug: &str, org: Option<&str>| rolter_core::ProviderConfig {
            name: name.to_string(),
            slug: Some(slug.to_string()),
            tenancy: org.and_then(tenanted),
            ..Default::default()
        };
        let mut config = GatewayConfig {
            providers: vec![
                provider("openai", "openai", Some("org-a")),
                provider("openai", "openai", None),
            ],
            ..Default::default()
        };
        assert!(an_org_settles_every_clash(&config));
        config.providers = vec![
            provider("openai", "gpt", Some("org-a")),
            provider("azure", "gpt", None),
        ];
        assert!(!clashing_names(&config).is_empty());
        assert!(!an_org_settles_every_clash(&config));
    }

    #[test]
    fn a_prompt_template_slug_two_orgs_share_is_a_clash() {
        let mut config = GatewayConfig::default();
        for org in ["org-a", "org-b"] {
            config.prompt_templates.templates.push(
                serde_json::from_value(serde_json::json!({
                    "id": format!("{org}:greet"),
                    "version": 1,
                }))
                .unwrap(),
            );
        }
        assert_eq!(clashing_names(&config), ["prompt template 'greet'"]);
    }

    /// A capture policy limited to keys is left out of a per-org file, one not
    /// limited stays, as the rest of logging does (#3440).
    #[test]
    fn the_capture_table_is_left_out_only_when_asked() {
        let mut config = GatewayConfig::default();
        config.logging.payload_capture.enabled = true;
        let refs = ScopeRefs::default();
        let whole = render_document(&config, &refs, false, &[]);
        assert!(whole.contains("\n[logging.payload_capture]\n"), "{whole}");
        let omitted = render_document(&config, &refs, true, &[]);
        assert!(
            !omitted.contains("\n[logging.payload_capture]"),
            "{omitted}"
        );
        assert!(omitted.contains("[logging]\nui_events"), "{omitted}");
        assert!(omitted.contains("is not exported"), "{omitted}");
    }

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

[[routes]]
model = "split"
strategy = "round_robin"
[[routes.variants]]
name = "stable"
weight = 9
[[routes.variants.targets]]
provider = "openai-primary"
model = "gpt-4o"
weight = 1
[[routes.variants]]
name = "canary"
weight = 1
[routes.variants.params]
temperature = 0.7
[[routes.variants.targets]]
provider = "vllm-1"
model = "meta-llama/Llama-3.1-8B-Instruct"
weight = 1
[[routes.variants.targets]]
provider = "openai-primary"
weight = 2

[[model_prices]]
model = "gpt-4o"
input_per_mtok = 2.5
output_per_mtok = 10.0
cache_write_per_mtok = 3.125
cache_write_1h_per_mtok = 5.5
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

[logging]
ui_events = false

[logging.payload_capture]
enabled = true
max_bytes = 32768
redact_fields = ["authorization"]
models = ["gpt-4o"]

[[mcp_gateway_settings]]
org = "acme"
default_transport = "sse"
connect_timeout_ms = 2500
request_timeout_ms = 45000
max_retries = 2
default_failure_mode = "fail_open"
allow_unlisted_tools = true

[[budgets]]
scope = "org"
id = "acme"
limit_usd = 500.0
period = "monthly"
unpriced_policy = "block"

[[budgets]]
scope = "project"
id = "acme/default/default"
limit_usd = 25.5
period = "daily"

[[rate_limits]]
scope = "team"
id = "acme/default"
rpm = 600
tpm = 150000

[[budgets]]
scope = "user"
id = "alice@example.com"
limit_usd = 50.0
period = "monthly"

[[budgets]]
scope = "user"
id = "alice@example.com"
within = { project = "acme/default/default" }
limit_usd = 10.0
period = "daily"

[[rate_limits]]
scope = "user"
id = "alice@example.com"
within = { org = "acme" }
rpm = 60

[[budgets]]
scope = "user"
id = "ghost@example.com"
limit_usd = 99.0
period = "monthly"
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
            tenancy: None,
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

    /// #2876: the cache-write rate leaves with its price, and a price without
    /// one does not grow a key, so an export of an old deployment is unchanged.
    #[test]
    fn a_cache_write_rate_is_exported_only_when_there_is_one() {
        let mut config = GatewayConfig::from_toml_str(FIXTURE).expect("the fixture must parse");
        let rendered = render(&config);
        assert!(
            rendered.contains("cache_write_per_mtok = \"3.125\"\n"),
            "{rendered}"
        );

        config.model_prices[0].cache_write_per_mtok = None;
        let rendered = render(&config);
        assert!(!rendered.contains("cache_write_per_mtok"), "{rendered}");
    }

    /// #2891: the 1 hour rate leaves with its price too, and independently of
    /// the 5 minute one.
    #[test]
    fn a_one_hour_cache_write_rate_is_exported_only_when_there_is_one() {
        let mut config = GatewayConfig::from_toml_str(FIXTURE).expect("the fixture must parse");
        let rendered = render(&config);
        assert!(
            rendered.contains("cache_write_1h_per_mtok = \"5.5\"\n"),
            "{rendered}"
        );

        config.model_prices[0].cache_write_1h_per_mtok = None;
        let rendered = render(&config);
        assert!(!rendered.contains("cache_write_1h_per_mtok"), "{rendered}");
        assert!(rendered.contains("cache_write_per_mtok"), "{rendered}");
    }

    /// #2937: the unit rates leave with their price, and only when set.
    #[test]
    fn unit_rates_are_exported_only_when_they_are_set() {
        let mut config = GatewayConfig::from_toml_str(FIXTURE).expect("the fixture must parse");
        let rendered = render(&config);
        for key in [
            "image_per_unit",
            "audio_input_per_minute",
            "speech_per_mchar",
        ] {
            assert!(!rendered.contains(key), "{key}: {rendered}");
        }

        let price = &mut config.model_prices[0];
        price.image_per_unit = Some(rust_decimal::Decimal::new(4, 2));
        price.audio_input_per_minute = Some(rust_decimal::Decimal::new(6, 3));
        price.speech_per_mchar = Some(rust_decimal::Decimal::new(15, 0));
        let rendered = render(&config);
        assert!(
            rendered.contains("image_per_unit = \"0.04\"\n"),
            "{rendered}"
        );
        assert!(
            rendered.contains("audio_input_per_minute = \"0.006\"\n"),
            "{rendered}"
        );
        assert!(
            rendered.contains("speech_per_mchar = \"15\"\n"),
            "{rendered}"
        );
        // and what was written is a file the loader reads back
        let back = GatewayConfig::from_toml_str(&rendered).expect("the export must parse");
        assert_eq!(
            back.model_prices[0].image_per_unit,
            config.model_prices[0].image_per_unit
        );
    }

    /// #1882: the audio token rates leave with their price, and only when set.
    #[test]
    fn audio_rates_are_exported_only_when_they_are_set() {
        let mut config = GatewayConfig::from_toml_str(FIXTURE).expect("the fixture must parse");
        let rendered = render(&config);
        for key in [
            "audio_input_per_mtok",
            "audio_output_per_mtok",
            "cached_audio_input_per_mtok",
        ] {
            assert!(!rendered.contains(key), "{key}: {rendered}");
        }

        let price = &mut config.model_prices[0];
        price.audio_input_per_mtok = Some(rust_decimal::Decimal::new(32, 0));
        price.audio_output_per_mtok = Some(rust_decimal::Decimal::new(64, 0));
        price.cached_audio_input_per_mtok = Some(rust_decimal::Decimal::new(4, 1));
        let rendered = render(&config);
        assert!(
            rendered.contains("audio_input_per_mtok = \"32\"\n"),
            "{rendered}"
        );
        assert!(
            rendered.contains("audio_output_per_mtok = \"64\"\n"),
            "{rendered}"
        );
        assert!(
            rendered.contains("cached_audio_input_per_mtok = \"0.4\"\n"),
            "{rendered}"
        );
        let back = GatewayConfig::from_toml_str(&rendered).expect("the export must parse");
        assert_eq!(
            back.model_prices[0].cached_audio_input_per_mtok,
            config.model_prices[0].cached_audio_input_per_mtok
        );
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
        assert!(rendered.contains("api_keys = [{ env = \"ANTHROPIC_API_KEY\" }]"));
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
            rendered.contains("api_keys = [{ env = \"OPENAI_KEY_A\" }]"),
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
                        ..Default::default()
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
        // #2236: the stored telemetry sources must survive the round trip
        vllm.kv_events = Some(rolter_core::KvEventsConfig {
            endpoint: "tcp://vllm-1:5557".to_string(),
            topic: "kv".to_string(),
            max_blocks: 500,
            stale_secs: 12,
        });
        vllm.lmcache = Some(rolter_core::LmCacheConfig {
            endpoint: "http://lmcache:9000".to_string(),
            refresh_secs: 3,
            stale_secs: 15,
        });
        let price = config
            .model_prices
            .first_mut()
            .expect("the fixture has a price");
        price.cached_input_per_mtok = Some(rust_decimal::Decimal::new(125, 2));
        price.cache_write_per_mtok = Some(rust_decimal::Decimal::new(375, 2));
        price.cache_write_1h_per_mtok = Some(rust_decimal::Decimal::new(600, 2));
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

    /// #2236: a provider's telemetry sources leave with it and read back whole,
    /// as inline tables that cannot capture the keys written after them.
    #[test]
    fn provider_telemetry_settings_round_trip() {
        let rendered = render(&representative_config());
        let back = GatewayConfig::from_toml_str(&rendered).expect("the export must re-parse");
        let vllm = back
            .providers
            .iter()
            .find(|p| p.name == "vllm-1")
            .expect("the provider survives");
        let kv = vllm.kv_events.as_ref().expect("kv_events survives");
        assert_eq!(
            (
                kv.endpoint.as_str(),
                kv.topic.as_str(),
                kv.max_blocks,
                kv.stale_secs
            ),
            ("tcp://vllm-1:5557", "kv", 500, 12)
        );
        let lmcache = vllm.lmcache.as_ref().expect("lmcache survives");
        assert_eq!((lmcache.refresh_secs, lmcache.stale_secs), (3, 15));
        // the export writes the pool the provider actually used, as the plural
        // `egress_proxies`; the legacy single proxy beside a non-empty list was
        // never part of it
        assert_eq!(
            vllm.egress_proxy_pool(),
            vec!["http://egress-a:3128", "http://egress-b:3128"]
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

            // the person the file's per-person caps name; `ghost@example.com`
            // has no account here, so its cap is skipped, never created (#3311)
            let alice: Uuid = sqlx::query_scalar(
                "insert into users (email, password_hash, is_superadmin)
                 values ('alice@example.com', null, false) returning id",
            )
            .fetch_one(&pool)
            .await
            .unwrap();

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
            let first =
                super::export_document(&pool, &store.load().await.unwrap(), &Default::default())
                    .await
                    .unwrap();

            // the decrypted key is in the loaded config; it must not be in the
            // document built from it
            assert!(
                !first.contains("sk-live-must-never-be-exported"),
                "a sealed credential was exported in plaintext:\n{first}"
            );
            assert!(first.contains("api_keys = [{ env = \"OPENAI_API_KEY\" }]"));
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
                // declared order survives, and with it the fallback chain
                "[[routes.variants]]\nname = \"stable\"\nweight = 9",
                "[[routes.variants]]\nname = \"canary\"\nweight = 1\n\n[routes.variants.params]\ntemperature = 0.7",
                "[[routes.variants.targets]]\nprovider = \"vllm-1\"",
                "[[model_prices]]",
                "[[budgets]]\nscope = \"org\"\nid = \"acme\"",
                "id = \"acme/default/default\"",
                "[[rate_limits]]\nscope = \"team\"\nid = \"acme/default\"",
                // a person is named by email, narrowed by a slug path (#3311)
                "[[budgets]]\nscope = \"user\"\nid = \"alice@example.com\"\nlimit_usd = \"50.0000\"",
                "within = { project = \"acme/default/default\" }",
                "[[rate_limits]]\nscope = \"user\"\nid = \"alice@example.com\"\nwithin = { org = \"acme\" }\nrpm = 60",
                // numeric(12,6) comes back with its scale
                "cache_write_per_mtok = \"3.125000\"",
                "cache_write_1h_per_mtok = \"5.500000\"",
                "[[prompt_templates.templates]]",
                "[logging]\nui_events = false",
                "[logging.payload_capture]",
            ] {
                assert!(first.contains(expected), "{expected} is missing:\n{first}");
            }
            // and never a database id, which is what makes the file portable
            assert!(
                !first.contains(&provider_id.to_string()),
                "the export leaked a database id:\n{first}"
            );
            assert!(
                !first.contains(&alice.to_string()),
                "the export named a person by database id:\n{first}"
            );
            // an account the file names but this deployment lacks got no cap
            // and was not created
            assert!(!first.contains("ghost@example.com"), "{first}");
            let ghosts: i64 =
                sqlx::query_scalar("select count(*) from users where email = 'ghost@example.com'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(ghosts, 0, "an import created an account for a cap");

            std::fs::write(&path, &first).unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &path)
                .await
                .unwrap();
            let second =
                super::export_document(&pool, &store.load().await.unwrap(), &Default::default())
                    .await
                    .unwrap();

            assert_eq!(
                first, second,
                "re-importing an export changed the deployment"
            );

            // the caps are the other half of the promotion story: export,
            // wipe them, import, export again, and nothing differs (#1312)
            sqlx::query("delete from budgets")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("delete from rate_limits")
                .execute(&pool)
                .await
                .unwrap();
            let wiped =
                super::export_document(&pool, &store.load().await.unwrap(), &Default::default())
                    .await
                    .unwrap();
            assert!(!wiped.contains("[[budgets]]"), "the wipe left a budget");
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &path)
                .await
                .unwrap();
            let third =
                super::export_document(&pool, &store.load().await.unwrap(), &Default::default())
                    .await
                    .unwrap();
            assert_eq!(first, third, "export, wipe, import changed the caps");
            // a second import of the same file writes nothing: the person's
            // three caps are still three rows
            let user_caps: i64 = sqlx::query_scalar(
                "select (select count(*) from budgets where scope_type = 'user')
                      + (select count(*) from rate_limits where scope_type = 'user')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(user_caps, 3);
            // a database id never stands in for a name
            let org_uuid = org_id.to_string();
            assert!(!third.contains(&org_uuid), "{third}");
        }

        /// Two orgs each with an `openai` provider and a `gpt-4o` route (#3440):
        /// the unscoped export refuses and names the option, and each org's own
        /// export carries that org's rows and nothing of the other's, then
        /// imports back into it unchanged.
        #[tokio::test]
        async fn two_orgs_sharing_names_export_one_file_each() {
            use super::ExportOptions;

            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let dir = tempdir("per-org");
            let store = PostgresConfigStore::new(pool.clone());

            let mut orgs = Vec::new();
            for slug in ["org-a", "org-b"] {
                orgs.push((slug, bootstrap_named_org(&pool, slug).await));
            }
            let (_, (org_a, project_a)) = orgs[0];
            let (_, (org_b, project_b)) = orgs[1];

            // three accounts: a member of each org, and one of neither
            let mut people = Vec::new();
            for email in [
                "alice.a@corp.example",
                "bob.b@corp.example",
                "carol.none@corp.example",
            ] {
                let id: Uuid = sqlx::query_scalar(
                    "insert into users (email, password_hash, is_superadmin)
                     values ($1, null, false) returning id",
                )
                .bind(email)
                .fetch_one(&pool)
                .await
                .unwrap();
                people.push(id);
            }
            for (user, org) in [(people[0], org_a), (people[1], org_b)] {
                sqlx::query(
                    "insert into memberships (user_id, org_id, role) values ($1, $2, 'member')",
                )
                .bind(user)
                .bind(org)
                .execute(&pool)
                .await
                .unwrap();
            }
            // a virtual key minted in each org
            let mut keys = Vec::new();
            for (n, project) in [project_a, project_b].into_iter().enumerate() {
                let key = rolter_store::postgres::repo::VirtualKeyRepo(&pool)
                    .create(
                        project,
                        rolter_store::postgres::repo::NewVirtualKey {
                            key_hash: &format!("hash-{n}"),
                            key_prefix: &format!("rk-{n}"),
                            name: None,
                            models: &[],
                            providers: &[],
                            cache_enabled: None,
                            created_by: None,
                            expires_at: None,
                            purpose: None,
                        },
                    )
                    .await
                    .unwrap();
                keys.push(key.id);
            }

            // each org's file: the shared names, then what only it has
            for (n, (slug, (org_id, project_id))) in orgs.iter().enumerate() {
                let (variant, group, template, team_cap, project_cap, person) = if n == 0 {
                    (
                        "canary",
                        "cluster-a",
                        "greet-a",
                        6.5,
                        8.5,
                        "alice.a@corp.example",
                    )
                } else {
                    (
                        "ab-test",
                        "cluster-b",
                        "greet-b",
                        7.5,
                        9.5,
                        "bob.b@corp.example",
                    )
                };
                let capture = if n == 1 {
                    format!(
                        "[logging.payload_capture]\nenabled = true\nvirtual_key_ids = [\"{}\", \"{}\"]\n",
                        keys[0], keys[1]
                    )
                } else {
                    String::new()
                };
                let path = dir.join(format!("{slug}.toml"));
                std::fs::write(
                    &path,
                    format!(
                        r#"
[[providers]]
name = "openai"
slug = "openai"
kind = "openai"
api_base = "https://{slug}.example.com"
api_key_env = "OPENAI_API_KEY"

[[provider_groups]]
name = "{group}"
slug = "{group}"
strategy = "round_robin"
[[provider_groups.members]]
provider = "openai"
model = "gpt-4o"

[[routes]]
model = "gpt-4o"
[[routes.targets]]
provider = "openai"
model = "gpt-4o"
[[routes.variants]]
name = "{variant}"
weight = 1
[[routes.variants.targets]]
provider = "openai"
model = "gpt-4o"
weight = 1

[prompt_templates]
enabled = true
[[prompt_templates.templates]]
id = "{template}"
version = 1
[[prompt_templates.templates.decorators]]
role = "system"
position = "prepend"
content = "hello from {slug}"

[[budgets]]
scope = "team"
id = "{slug}/default"
limit_usd = {team_cap}
period = "daily"
[[budgets]]
scope = "project"
id = "{slug}/default/default"
limit_usd = {project_cap}
period = "daily"
[[budgets]]
scope = "user"
id = "{person}"
limit_usd = 3.5
period = "daily"
{extra}
{capture}
"#,
                        extra = if n == 1 {
                            "[[budgets]]\nscope = \"user\"\nid = \"carol.none@corp.example\"\nlimit_usd = 4.5\nperiod = \"daily\"\n"
                        } else {
                            ""
                        },
                    ),
                )
                .unwrap();
                crate::seed::import_bootstrap_toml(&pool, *org_id, *project_id, &path)
                    .await
                    .unwrap();
            }

            let served = store.load().await.unwrap();
            let refused = super::export_document(&pool, &served, &ExportOptions::default())
                .await
                .unwrap_err()
                .to_string();
            assert!(refused.contains("route 'gpt-4o'"), "{refused}");
            assert!(
                refused.contains("--org <slug>"),
                "the error names no option: {refused}"
            );

            // what only one org has, and what must therefore stay in its file
            let own: [&[&str]; 2] = [
                &[
                    "https://org-a.example.com",
                    "cluster-a",
                    "greet-a",
                    "canary",
                    "id = \"org-a/default\"",
                    "id = \"org-a/default/default\"",
                    "limit_usd = \"6.5000\"",
                    "limit_usd = \"8.5000\"",
                    "alice.a@corp.example",
                ],
                &[
                    "https://org-b.example.com",
                    "cluster-b",
                    "greet-b",
                    "ab-test",
                    "id = \"org-b/default\"",
                    "id = \"org-b/default/default\"",
                    "limit_usd = \"7.5000\"",
                    "limit_usd = \"9.5000\"",
                    "bob.b@corp.example",
                ],
            ];
            for (n, (slug, (org_id, project_id))) in orgs.iter().enumerate() {
                let options = ExportOptions {
                    org: Some(slug.to_string()),
                    ..Default::default()
                };
                let first = super::export_document(&pool, &served, &options)
                    .await
                    .unwrap();
                for token in own[n] {
                    assert!(
                        first.contains(token),
                        "{slug}: {token} is missing:\n{first}"
                    );
                }
                for token in own[1 - n] {
                    assert!(!first.contains(token), "{slug}: carries {token}:\n{first}");
                }
                // a person of neither org is in neither file; the other org's
                // key ids stay out, and so does its mention in any comment
                assert!(!first.contains("carol.none@corp.example"), "{first}");
                assert!(
                    !first.contains(&format!("org-{}", if n == 0 { 'b' } else { 'a' })),
                    "{first}"
                );
                // capture is limited to keys across both orgs, so no per-org file
                // declares it (a re-import would apply it to everyone), and the
                // file says why
                for key in &keys {
                    assert!(!first.contains(&key.to_string()), "{first}");
                }
                assert!(!first.contains("\n[logging.payload_capture]"), "{first}");
                assert!(
                    first.contains("# [logging.payload_capture] is not exported"),
                    "{first}"
                );
                // the stored variants are the org's own: one variant, not both
                assert_eq!(first.matches("[[routes.variants]]").count(), 1, "{first}");

                // the file loads on its own, and imports back into the org
                let path = dir.join(format!("{slug}-export.toml"));
                std::fs::write(&path, &first).unwrap();
                rolter_core::GatewayConfig::from_toml_str(&first)
                    .unwrap()
                    .validate()
                    .unwrap();
                crate::seed::import_bootstrap_toml(&pool, *org_id, *project_id, &path)
                    .await
                    .unwrap();
                let served = store.load().await.unwrap();
                let second = super::export_document(&pool, &served, &options)
                    .await
                    .unwrap();
                assert_eq!(first, second, "re-importing the {slug} export changed it");
            }

            // the re-imports left the deployment's capture policy as it was
            let limited: Option<i32> = sqlx::query_scalar(
                "select cardinality(payload_capture_virtual_key_ids) from logging_settings limit 1",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                limited,
                Some(2),
                "a per-org import changed the capture policy"
            );

            let unknown = ExportOptions {
                org: Some("nobody".to_string()),
                ..Default::default()
            };
            let err = super::export_document(&pool, &served, &unknown)
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains("'nobody'"), "{err}");
        }

        /// An org that shadows a deployment-wide route, provider and provider
        /// group (#3481): the per-org file leaves the shadowed rows out and names
        /// them, keeps the deployment-wide rows it does not shadow, and imports
        /// back into the org. Groups are shadowed by slug, not display name.
        #[tokio::test]
        async fn an_org_shadowing_deployment_wide_rows_exports_without_them() {
            use super::{ExportError, ExportOptions};

            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let dir = tempdir("shadow");
            let store = PostgresConfigStore::new(pool.clone());
            let (org_id, project_id) = bootstrap_named_org(&pool, "shadower").await;
            let path = dir.join("shadower.toml");
            std::fs::write(
                &path,
                r#"
[[providers]]
name = "openai"
slug = "openai"
kind = "openai"
api_base = "https://org.example.com"
api_key_env = "OPENAI_API_KEY"

[[provider_groups]]
name = "Primary"
slug = "us"
strategy = "round_robin"
[[provider_groups.members]]
provider = "openai"
model = "gpt-4o"

[[provider_groups]]
name = "Europe"
slug = "eu2"
strategy = "round_robin"
[[provider_groups.members]]
provider = "openai"
model = "gpt-4o"

[[routes]]
model = "gpt-4o"
[[routes.targets]]
provider = "openai"
model = "gpt-4o"
"#,
            )
            .unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &path)
                .await
                .unwrap();

            // the rows a `--config` file would add: the same names with no org,
            // and rows the org does not shadow
            let mut served = store.load().await.unwrap();
            let org_group = |slug: &str| {
                served
                    .provider_groups
                    .iter()
                    .find(|g| g.slug.as_deref() == Some(slug))
                    .cloned()
                    .unwrap()
            };
            // the org's `Primary`/`us` is not shadowed by a global `Primary`/`eu`,
            // and its `Europe`/`eu2` is shadowed by a global `EU`/`eu2`
            let mut kept_group = org_group("us");
            kept_group.tenancy = None;
            kept_group.slug = Some("eu".to_string());
            kept_group.members[0].provider = "mistral".to_string();
            let mut shadowed_group = org_group("eu2");
            shadowed_group.tenancy = None;
            shadowed_group.name = "EU".to_string();
            let mut global_provider = served.providers[0].clone();
            global_provider.tenancy = None;
            global_provider.api_base = "https://global.example.com".to_string();
            let mut global_route = served.routes[0].clone();
            global_route.tenancy = None;
            let mut kept_provider = global_provider.clone();
            kept_provider.name = "mistral".to_string();
            kept_provider.slug = Some("mistral".to_string());
            let mut kept_route = global_route.clone();
            kept_route.model = "mistral-large".to_string();
            kept_route.targets[0].provider = "mistral".to_string();
            served.providers.extend([global_provider, kept_provider]);
            served.routes.extend([global_route, kept_route]);
            served.provider_groups.extend([kept_group, shadowed_group]);

            // the whole deployment is refused, with the code that says to pick
            // an organization, because one organization's file settles it
            let refused = super::export_document_typed(&pool, &served, &ExportOptions::default())
                .await
                .unwrap_err();
            let ExportError::NeedsOrganization(message) = refused else {
                panic!("not the pick-an-organization refusal: {refused:?}");
            };
            assert!(message.contains("route 'gpt-4o'"), "{message}");
            assert!(message.contains("--org"), "{message}");
            assert!(!message.contains("rename"), "{message}");

            let options = ExportOptions {
                org: Some("shadower".to_string()),
                ..Default::default()
            };
            let first = super::export_document(&pool, &served, &options)
                .await
                .unwrap();
            // the global openai is gone: one provider of that name, the org's
            assert_eq!(first.matches("name = \"openai\"").count(), 1, "{first}");
            assert!(first.contains("https://org.example.com"), "{first}");
            assert_eq!(first.matches("[[routes]]").count(), 2, "{first}");
            assert!(first.contains("model = \"mistral-large\""), "{first}");
            assert!(first.contains("#   provider 'openai'\n"), "{first}");
            assert!(first.contains("#   route 'gpt-4o'\n"), "{first}");
            assert!(first.contains("#   provider group 'eu2'\n"), "{first}");
            // the global group of another slug is kept, and not named
            assert!(!first.contains("#   provider group 'eu'\n"), "{first}");
            assert!(first.contains("slug = \"eu\""), "{first}");
            assert!(!first.contains("#   route 'mistral-large'"), "{first}");
            rolter_core::GatewayConfig::from_toml_str(&first)
                .unwrap()
                .validate()
                .unwrap();

            let file = dir.join("export.toml");
            std::fs::write(&file, &first).unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &file)
                .await
                .unwrap();

            // the import made the org own a copy of every deployment-wide row
            // the file kept. Where the target still serves those originals, they
            // are shadowed in turn, and the export names them as well
            let mut still_served = store.load().await.unwrap();
            still_served.providers.extend(
                served
                    .providers
                    .iter()
                    .filter(|p| p.tenancy.is_none())
                    .cloned(),
            );
            still_served.routes.extend(
                served
                    .routes
                    .iter()
                    .filter(|r| r.tenancy.is_none())
                    .cloned(),
            );
            still_served.provider_groups.extend(
                served
                    .provider_groups
                    .iter()
                    .filter(|g| g.tenancy.is_none())
                    .cloned(),
            );
            let again = super::export_document(&pool, &still_served, &options)
                .await
                .unwrap();
            assert!(again.contains("#   provider 'mistral'\n"), "{again}");
            assert_ne!(first, again);

            // the same bytes come back when the target serves only the originals
            // the org shadows (the kept ones moved into the org by the import)
            let mut reloaded = store.load().await.unwrap();
            reloaded.providers.extend(
                served
                    .providers
                    .iter()
                    .filter(|p| p.tenancy.is_none() && p.name == "openai")
                    .cloned(),
            );
            reloaded.routes.extend(
                served
                    .routes
                    .iter()
                    .filter(|r| r.tenancy.is_none() && r.model == "gpt-4o")
                    .cloned(),
            );
            reloaded.provider_groups.extend(
                served
                    .provider_groups
                    .iter()
                    .filter(|g| g.tenancy.is_none() && g.name == "EU")
                    .cloned(),
            );
            let second = super::export_document(&pool, &reloaded, &options)
                .await
                .unwrap();
            assert_eq!(first, second, "re-importing the export changed it");
        }

        /// An org's route and provider that a control plane's config file later
        /// repeats with no org (#3516): the merged store serves both, the org's
        /// rows for its keys, lists the overlap, and the per-org export writes
        /// the org's rows, so re-importing it leaves them as they were.
        #[tokio::test]
        async fn an_org_row_a_bootstrap_file_repeats_is_served_and_exported() {
            use super::ExportOptions;
            use rolter_core::GatewayConfig;

            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let dir = tempdir("bootstrap-shadow");
            let (org_id, project_id) = bootstrap_named_org(&pool, "shadower").await;
            let org_file = dir.join("org.toml");
            let org_rows = |base: &str| {
                format!(
                    r#"
[[providers]]
name = "openai"
slug = "openai"
kind = "openai"
api_base = "{base}"
api_key_env = "OPENAI_API_KEY"

[[routes]]
model = "gpt-4o"
[[routes.targets]]
provider = "openai"
model = "gpt-4o"
"#
                )
            };
            std::fs::write(&org_file, org_rows("https://org.example.com")).unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &org_file)
                .await
                .unwrap();

            // the bootstrap file gains the same names afterwards, with no org
            let bootstrap =
                GatewayConfig::from_toml_str(&org_rows("https://file.example.com")).unwrap();
            let store = rolter_store::MergedConfigStore::new(
                bootstrap,
                std::sync::Arc::new(PostgresConfigStore::new(pool.clone())),
            );
            let served = store.load().await.unwrap();
            let org = org_id.to_string();
            let mut providers = rolter_core::namespace::Namespaced::new();
            for p in &served.providers {
                let owner = rolter_core::namespace::org_of(p.tenancy.as_ref());
                providers
                    .insert(owner, &p.name, p.api_base.as_str())
                    .unwrap();
            }
            assert_eq!(
                providers.get(&org, "openai"),
                Some(&"https://org.example.com")
            );
            assert_eq!(
                providers.get("", "openai"),
                Some(&"https://file.example.com")
            );
            let mut routes = rolter_core::namespace::Namespaced::new();
            for r in &served.routes {
                let owner = rolter_core::namespace::org_of(r.tenancy.as_ref());
                routes.insert(owner, &r.model, owner.to_string()).unwrap();
            }
            assert_eq!(routes.get(&org, "gpt-4o"), Some(&org));
            assert_eq!(routes.get("", "gpt-4o"), Some(&String::new()));

            let problems = store.load_problems().await.unwrap();
            let shadows: Vec<_> = problems
                .iter()
                .filter(|line| line.contains("shadows the readonly"))
                .collect();
            assert_eq!(shadows.len(), 2, "{problems:?}");
            assert!(
                shadows
                    .iter()
                    .any(|line| line.contains(&format!("organization {org}'s route 'gpt-4o'"))),
                "{shadows:?}"
            );

            let options = ExportOptions {
                org: Some("shadower".to_string()),
                ..Default::default()
            };
            let document = super::export_document(&pool, &served, &options)
                .await
                .unwrap();
            assert!(document.contains("https://org.example.com"), "{document}");
            assert!(!document.contains("https://file.example.com"), "{document}");

            // re-importing the export leaves the org's own row as it was
            let exported = dir.join("export.toml");
            std::fs::write(&exported, &document).unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &exported)
                .await
                .unwrap();
            let reloaded = store.load().await.unwrap();
            let org_provider = reloaded
                .providers
                .iter()
                .find(|p| p.tenancy.is_some() && p.name == "openai")
                .unwrap();
            assert_eq!(org_provider.api_base, "https://org.example.com");
        }

        /// With person caps left out the file names no account, and the trailing
        /// comment counts them (#3396). The default still writes them (#3305).
        #[tokio::test]
        async fn no_address_is_exported_when_person_caps_are_left_out() {
            use super::ExportOptions;

            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let (org_id, project_id) = bootstrap_org(&pool).await;
            let dir = tempdir("no-people");
            let path = dir.join("rolter.toml");
            let address = "carol.cap@corp.example";
            sqlx::query(
                "insert into users (email, password_hash, is_superadmin)
                 values ($1, null, false)",
            )
            .bind(address)
            .execute(&pool)
            .await
            .unwrap();
            std::fs::write(&path, FIXTURE.replace("alice@example.com", address)).unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &path)
                .await
                .unwrap();

            let served = PostgresConfigStore::new(pool.clone()).load().await.unwrap();
            let with = super::export_document(&pool, &served, &ExportOptions::default())
                .await
                .unwrap();
            assert!(with.contains(&format!("id = \"{address}\"")), "{with}");

            let without = super::export_document(
                &pool,
                &served,
                &ExportOptions {
                    without_person_caps: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert!(
                !without.contains(address),
                "an address was exported:\n{without}"
            );
            assert!(
                !without.contains('@'),
                "an address-shaped string was exported:\n{without}"
            );
            // (the header's own example names a user, so look for a table)
            assert!(!without.contains("\nscope = \"user\""), "{without}");
            // the other caps are still there, and the person's are counted
            assert!(without.contains("scope = \"team\""), "{without}");
            assert!(
                without.contains("# not exported: 2 budget(s) and 1 rate limit(s)"),
                "{without}"
            );
            // a file without the addresses still loads
            rolter_core::GatewayConfig::from_toml_str(&without).unwrap();
        }

        /// The marker is exported for an org that never saved MCP settings, in
        /// both values, and not for one that did (#3398).
        #[tokio::test]
        async fn the_mcp_manifest_marker_is_exported_for_an_org_without_settings() {
            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let (strict, _) = bootstrap_named_org(&pool, "strict").await;
            let (lax, _) = bootstrap_named_org(&pool, "lax").await;
            let (saved, _) = bootstrap_named_org(&pool, "saved").await;
            sqlx::query("update orgs set mcp_enforce_manifests = false where id = $1")
                .bind(lax)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("insert into mcp_gateway_settings (org_id) values ($1)")
                .bind(saved)
                .execute(&pool)
                .await
                .unwrap();
            let _ = strict;

            let store = PostgresConfigStore::new(pool.clone());
            let served = store.load().await.unwrap();
            let all = super::export_document(&pool, &served, &Default::default())
                .await
                .unwrap();
            assert!(
                all.contains("[[mcp_manifest_enforcement]]\norg = \"lax\"\nenforce = false\n"),
                "{all}"
            );
            assert!(
                all.contains("[[mcp_manifest_enforcement]]\norg = \"strict\"\nenforce = true\n"),
                "{all}"
            );
            assert!(
                !all.contains("[[mcp_manifest_enforcement]]\norg = \"saved\""),
                "an org with saved settings follows them, not the marker:\n{all}"
            );
            assert!(
                all.contains("[[mcp_gateway_settings]]\norg = \"saved\"\n"),
                "{all}"
            );
            let one = super::export_document(
                &pool,
                &served,
                &super::ExportOptions {
                    org: Some("lax".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert!(one.contains("org = \"lax\""), "{one}");
            assert!(!one.contains("org = \"strict\""), "{one}");
        }

        /// Saved MCP gateway settings survive export, wipe, import, export
        /// (#3482): the import sets the targeted org to exactly the file, in
        /// either direction, and touches no other org.
        #[tokio::test]
        async fn saved_mcp_gateway_settings_round_trip_for_one_org() {
            use super::ExportOptions;

            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let dir = tempdir("mcp-settings");
            let (org_a, project_a) = bootstrap_named_org(&pool, "org-a").await;
            let (org_b, _) = bootstrap_named_org(&pool, "org-b").await;
            for (org, transport, retries, mode, allow) in [
                (org_a, "sse", 3, "fail_open", true),
                (org_b, "websocket", 0, "fail_closed", false),
            ] {
                sqlx::query(
                    "insert into mcp_gateway_settings
                     (org_id, default_transport, connect_timeout_ms, request_timeout_ms,
                      max_retries, default_failure_mode, allow_unlisted_tools)
                     values ($1, $2, 2500, 45000, $3, $4, $5)",
                )
                .bind(org)
                .bind(transport)
                .bind(retries)
                .bind(mode)
                .bind(allow)
                .execute(&pool)
                .await
                .unwrap();
            }
            let store = PostgresConfigStore::new(pool.clone());
            let options = ExportOptions {
                org: Some("org-a".to_string()),
                ..Default::default()
            };
            let first = super::export_document(&pool, &store.load().await.unwrap(), &options)
                .await
                .unwrap();
            assert!(
                first.contains(
                    "[[mcp_gateway_settings]]\norg = \"org-a\"\ndefault_transport = \"sse\"\n\
                     connect_timeout_ms = 2500\nrequest_timeout_ms = 45000\nmax_retries = 3\n\
                     default_failure_mode = \"fail_open\"\nallow_unlisted_tools = true\n"
                ),
                "{first}"
            );
            assert!(!first.contains("org-b"), "{first}");
            let all = super::export_document(
                &pool,
                &store.load().await.unwrap(),
                &ExportOptions::default(),
            )
            .await
            .unwrap();
            assert!(all.contains("org = \"org-b\"\ndefault_transport = \"websocket\""));
            let file = dir.join("org-a.toml");
            std::fs::write(&file, &first).unwrap();

            // wipe, import, export: the same bytes, and org-b left alone
            sqlx::query("delete from mcp_gateway_settings")
                .execute(&pool)
                .await
                .unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_a, project_a, &file)
                .await
                .unwrap();
            let again = super::export_document(&pool, &store.load().await.unwrap(), &options)
                .await
                .unwrap();
            assert_eq!(first, again, "export, wipe, import changed the settings");
            let rows: i64 = sqlx::query_scalar("select count(*) from mcp_gateway_settings")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(rows, 1, "an import wrote another org's settings");

            // an import sets exactly the file: stricter than the row it finds
            // as readily as laxer, and a repeat writes nothing
            let strict = first
                .replace(
                    "allow_unlisted_tools = true",
                    "allow_unlisted_tools = false",
                )
                .replace("fail_open", "fail_closed");
            let file = dir.join("strict.toml");
            std::fs::write(&file, &strict).unwrap();
            let version = || async {
                sqlx::query_scalar::<_, i64>("select version from config_version")
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            };
            crate::seed::import_bootstrap_toml(&pool, org_a, project_a, &file)
                .await
                .unwrap();
            let after = super::export_document(&pool, &store.load().await.unwrap(), &options)
                .await
                .unwrap();
            assert!(after.contains("allow_unlisted_tools = false"), "{after}");
            assert!(after.contains("default_failure_mode = \"fail_closed\""));
            // an entry that already matches writes nothing (the file's other
            // sections, `[logging]` among them, bump the version on their own)
            let only = dir.join("only.toml");
            std::fs::write(
                &only,
                &strict[strict.find("\n[[mcp_gateway_settings]]\n").unwrap()
                    ..strict.find("\n[logging]\n").unwrap()],
            )
            .unwrap();
            let before = version().await;
            crate::seed::import_bootstrap_toml(&pool, org_a, project_a, &only)
                .await
                .unwrap();
            assert_eq!(version().await, before, "an unchanged entry bumped config");

            // a value the MCP Settings screen would refuse fails the import
            // before anything is written: the provider the file also names is
            // not created, and the saved settings are as they were
            let bad = dir.join("bad.toml");
            std::fs::write(
                &bad,
                format!(
                    "[[providers]]\nname = \"leaky\"\nkind = \"openai\"\n\
                     api_base = \"https://leaky.example.com\"\n\n{}",
                    strict.replace("max_retries = 3", "max_retries = 9")
                ),
            )
            .unwrap();
            let before = super::export_document(&pool, &store.load().await.unwrap(), &options)
                .await
                .unwrap();
            let refused = crate::seed::import_bootstrap_toml(&pool, org_a, project_a, &bad)
                .await
                .unwrap_err()
                .to_string();
            assert!(refused.contains("max_retries"), "{refused}");
            let after = super::export_document(&pool, &store.load().await.unwrap(), &options)
                .await
                .unwrap();
            assert_eq!(before, after, "a refused import wrote something");
            assert!(!after.contains("leaky"), "{after}");

            // a file naming another org never reaches it
            let foreign = strict.replace("org = \"org-a\"", "org = \"org-b\"");
            let file = dir.join("foreign.toml");
            std::fs::write(&file, &foreign).unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_a, project_a, &file)
                .await
                .unwrap();
            // (org-b's row went with the wipe, and the import did not bring it back)
            let b: i64 =
                sqlx::query_scalar("select count(*) from mcp_gateway_settings where org_id = $1")
                    .bind(org_b)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(b, 0, "an import wrote another org's settings");
        }

        /// An org's MCP tool groups, a team, project and key assignment each,
        /// survive export, wipe, import, export byte for byte, and a repeat
        /// import writes nothing (#3514).
        #[tokio::test]
        async fn mcp_tool_groups_round_trip_with_their_assignments() {
            use super::ExportOptions;

            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let dir = tempdir("mcp-tool-groups");
            let world = tool_group_world(&pool).await;
            let store = PostgresConfigStore::new(pool.clone());
            let options = ExportOptions {
                org: Some("org-a".to_string()),
                ..Default::default()
            };
            let export = || async {
                super::export_document(&pool, &store.load().await.unwrap(), &options)
                    .await
                    .unwrap()
            };
            let first = export().await;
            let unnamed = concat!("sk-", "rolter-ab");
            assert!(
                first.contains(&format!(
                    "[[mcp_gateway_tool_groups]]\norg = \"org-a\"\nslug = \"off\"\n\
                     name = \"Switched off\"\nenabled = false\ntools = []\n\n\
                     [[mcp_gateway_tool_groups]]\norg = \"org-a\"\nslug = \"read-only\"\n\
                     name = \"Read only\"\ndescription = \"no writes\"\nenabled = true\n\
                     tools = [{{ server = \"github\", tool = \"fetch\" }}, \
                     {{ server = \"github\", tool = \"search\" }}]\n\
                     teams = [\"org-a/default\"]\nprojects = [\"org-a/default/default\"]\n\
                     keys = [{{ project = \"org-a/default/default\", prefix = \"{unnamed}\" }}, \
                     {{ project = \"org-a/default/default\", name = \"ci\" }}]\n"
                )),
                "{first}"
            );
            // no row id, server id, key digest or secret is written
            for id in [world.group, world.server, world.ci_key] {
                assert!(!first.contains(&id.to_string()), "{first}");
            }
            assert!(!first.contains("key_hash"), "{first}");
            // the other org's group is not in this org's file
            assert!(!first.contains("org-b"), "{first}");
            let file = dir.join("org-a.toml");
            std::fs::write(&file, &first).unwrap();

            sqlx::query("delete from mcp_tool_groups where org_id = $1")
                .bind(world.org_a)
                .execute(&pool)
                .await
                .unwrap();
            crate::seed::import_bootstrap_toml(&pool, world.org_a, world.project_a, &file)
                .await
                .unwrap();
            assert_eq!(
                first,
                export().await,
                "export, wipe, import changed the groups"
            );
            // org-b's group is untouched
            let b: i64 =
                sqlx::query_scalar("select count(*) from mcp_tool_groups where org_id = $1")
                    .bind(world.org_b)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(b, 1);

            // a repeat import of the same groups writes nothing
            let only = dir.join("only.toml");
            std::fs::write(
                &only,
                &first[first.find("\n[[mcp_gateway_tool_groups]]\n").unwrap()
                    ..first.find("\n[logging]\n").unwrap()],
            )
            .unwrap();
            let version = || async {
                sqlx::query_scalar::<_, i64>("select version from config_version")
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            };
            let before = version().await;
            crate::seed::import_bootstrap_toml(&pool, world.org_a, world.project_a, &only)
                .await
                .unwrap();
            assert_eq!(version().await, before, "an unchanged group bumped config");

            // an edited file is applied: a tool and an assignment dropped
            let edited = dir.join("edited.toml");
            std::fs::write(
                &edited,
                std::fs::read_to_string(&only)
                    .unwrap()
                    .replace(", { server = \"github\", tool = \"search\" }", "")
                    .replace("teams = [\"org-a/default\"]\n", ""),
            )
            .unwrap();
            crate::seed::import_bootstrap_toml(&pool, world.org_a, world.project_a, &edited)
                .await
                .unwrap();
            let after = export().await;
            assert!(!after.contains("tool = \"search\""), "{after}");
            assert!(!after.contains("teams = "), "{after}");
            assert!(
                after.contains("projects = [\"org-a/default/default\"]"),
                "{after}"
            );
        }

        /// Into a deployment missing one key the group is still created with
        /// its other assignments, and the missing one is skipped with a
        /// warning that names it (#3514).
        #[tokio::test]
        async fn an_mcp_tool_group_keeps_its_group_when_a_key_is_missing() {
            use super::ExportOptions;

            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let dir = tempdir("mcp-tool-groups-missing");
            let world = tool_group_world(&pool).await;
            let store = PostgresConfigStore::new(pool.clone());
            let options = ExportOptions {
                org: Some("org-a".to_string()),
                ..Default::default()
            };
            let first = super::export_document(&pool, &store.load().await.unwrap(), &options)
                .await
                .unwrap();
            let file = dir.join("org-a.toml");
            std::fs::write(&file, &first).unwrap();

            // the receiving deployment has no `ci` key
            sqlx::query("delete from mcp_tool_groups where org_id = $1")
                .bind(world.org_a)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("delete from virtual_keys where id = $1")
                .bind(world.ci_key)
                .execute(&pool)
                .await
                .unwrap();
            let output = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
            #[derive(Clone)]
            struct Capture(std::sync::Arc<parking_lot::Mutex<Vec<u8>>>);
            impl std::io::Write for Capture {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    self.0.lock().extend_from_slice(bytes);
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let captured = output.clone();
            let subscriber = tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_max_level(tracing::Level::WARN)
                .with_writer(move || Capture(captured.clone()))
                .finish();
            {
                let _guard = tracing::subscriber::set_default(subscriber);
                crate::seed::import_bootstrap_toml(&pool, world.org_a, world.project_a, &file)
                    .await
                    .unwrap();
            }
            let log = String::from_utf8(output.lock().clone()).unwrap();
            assert!(
                log.contains("skipping an MCP tool group assignment")
                    && log.contains("key=ci")
                    && log.contains("group=\"read-only\""),
                "{log}"
            );

            let assigned: Vec<String> = sqlx::query_scalar(
                "select a.subject_kind from mcp_tool_group_assignments a
                 join mcp_tool_groups g on g.id = a.group_id
                 where g.org_id = $1 and g.slug = 'read-only' order by a.subject_kind",
            )
            .bind(world.org_a)
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(assigned, ["project", "team", "virtual_key"]);
            let again = super::export_document(&pool, &store.load().await.unwrap(), &options)
                .await
                .unwrap();
            assert!(!again.contains("name = \"ci\""), "{again}");
            assert!(again.contains("tool = \"search\""), "{again}");
        }

        /// A tool on a server the organization does not have fails the import
        /// for the organization before any group is written, naming the group,
        /// the server and the tool: a server created later under that slug
        /// would otherwise start ungoverned (#3514).
        #[tokio::test]
        async fn a_tool_on_a_missing_server_fails_the_orgs_tool_group_import() {
            use super::ExportOptions;

            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let dir = tempdir("mcp-tool-groups-server");
            let world = tool_group_world(&pool).await;
            let store = PostgresConfigStore::new(pool.clone());
            let options = ExportOptions {
                org: Some("org-a".to_string()),
                ..Default::default()
            };
            // the file also loosens the org's saved MCP settings, which the
            // refusal must not let through either
            sqlx::query(
                "insert into mcp_gateway_settings (org_id, allow_unlisted_tools) values ($1, false)",
            )
            .bind(world.org_a)
            .execute(&pool)
            .await
            .unwrap();
            let first = super::export_document(&pool, &store.load().await.unwrap(), &options)
                .await
                .unwrap();
            assert!(first.contains("allow_unlisted_tools = false"), "{first}");
            let file = dir.join("org-a.toml");
            std::fs::write(
                &file,
                first
                    .replace(
                        "{ server = \"github\", tool = \"fetch\" }",
                        "{ server = \"gitlab\", tool = \"fetch\" }",
                    )
                    .replace(
                        "allow_unlisted_tools = false",
                        "allow_unlisted_tools = true",
                    ),
            )
            .unwrap();
            sqlx::query("delete from mcp_tool_groups where org_id = $1")
                .bind(world.org_a)
                .execute(&pool)
                .await
                .unwrap();
            let refused =
                crate::seed::import_bootstrap_toml(&pool, world.org_a, world.project_a, &file)
                    .await
                    .unwrap_err()
                    .to_string();
            for named in ["read-only", "gitlab", "fetch"] {
                assert!(refused.contains(named), "{refused}");
            }
            // not even the group without that tool was written, and nothing
            // else of the file was either
            let groups: i64 =
                sqlx::query_scalar("select count(*) from mcp_tool_groups where org_id = $1")
                    .bind(world.org_a)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(groups, 0);
            let allow: bool = sqlx::query_scalar(
                "select allow_unlisted_tools from mcp_gateway_settings where org_id = $1",
            )
            .bind(world.org_a)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(!allow, "a refused file loosened the org's MCP settings");
        }

        #[test]
        fn omission_comments_are_sorted_without_losing_duplicates() {
            let refs = super::super::ScopeRefs::default();
            refs.record_omission("z assignment".into());
            refs.record_omission("a assignment".into());
            refs.record_omission("z assignment".into());
            assert_eq!(
                refs.omission_comments(),
                ["a assignment", "z assignment", "z assignment"]
            );
        }

        /// An assignment whose team has no portable name (`A/B`) is not in the
        /// file, and a re-import of that file into the same deployment keeps
        /// it rather than handing the team the organization-wide set (#3514).
        #[tokio::test]
        async fn a_re_import_keeps_an_assignment_the_file_cannot_name() {
            use super::ExportOptions;
            use rolter_store::postgres::repo::TeamRepo;

            let db = scratch_db()
                .await
                .expect("this test needs ROLTER_TEST_DATABASE_URL");
            let pool = db.pool().clone();
            let dir = tempdir("mcp-tool-groups-unportable");
            let world = tool_group_world(&pool).await;
            let team = TeamRepo(&pool).create(world.org_a, "A/B").await.unwrap();
            sqlx::query(
                "insert into mcp_tool_group_assignments (group_id, subject_kind, subject_id)
                 values ($1, 'team', $2)",
            )
            .bind(world.group)
            .bind(team.id)
            .execute(&pool)
            .await
            .unwrap();
            let store = PostgresConfigStore::new(pool.clone());
            let options = ExportOptions {
                org: Some("org-a".to_string()),
                ..Default::default()
            };
            let first = super::export_document(&pool, &store.load().await.unwrap(), &options)
                .await
                .unwrap();
            assert!(!first.contains("A/B"), "{first}");
            assert!(
                first.contains(
                    "# not exported: MCP tool group team assignment, no portable subject name"
                ),
                "{first}"
            );
            let file = dir.join("org-a.toml");
            std::fs::write(&file, &first).unwrap();

            crate::seed::import_bootstrap_toml(&pool, world.org_a, world.project_a, &file)
                .await
                .unwrap();
            let kept: i64 = sqlx::query_scalar(
                "select count(*) from mcp_tool_group_assignments
                 where group_id = $1 and subject_kind = 'team' and subject_id = $2",
            )
            .bind(world.group)
            .bind(team.id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                kept, 1,
                "the re-import dropped an assignment it could not name"
            );
        }

        #[tokio::test]
        async fn an_ambiguous_key_prefix_cannot_grant_another_keys_tools() {
            ambiguous_key_import_is_refused(false).await;
        }

        #[tokio::test]
        async fn an_ambiguous_key_name_cannot_grant_another_keys_tools() {
            ambiguous_key_import_is_refused(true).await;
        }

        async fn ambiguous_key_import_is_refused(named: bool) {
            use super::ExportOptions;

            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let dir = tempdir("mcp-tool-groups-ambiguous");
            let world = tool_group_world(&pool).await;
            let intended: Uuid = if named {
                world.ci_key
            } else {
                sqlx::query_scalar(
                    "select id from virtual_keys where project_id = $1 and name is null",
                )
                .bind(world.project_a)
                .fetch_one(&pool)
                .await
                .unwrap()
            };
            let write_group: Uuid = sqlx::query_scalar(
                "insert into mcp_tool_groups (org_id, name, slug, tools)
                 values ($1, 'Write only', 'write-only', $2) returning id",
            )
            .bind(world.org_a)
            .bind(serde_json::json!([{"server_id": world.server, "tool": "write"}]))
            .fetch_one(&pool)
            .await
            .unwrap();
            sqlx::query(
                "insert into mcp_tool_group_assignments (group_id, subject_kind, subject_id)
                 values ($1, 'virtual_key', $2)",
            )
            .bind(write_group)
            .bind(intended)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "insert into mcp_gateway_settings (org_id, allow_unlisted_tools) values ($1, false)",
            )
            .bind(world.org_a)
            .execute(&pool)
            .await
            .unwrap();
            let store = PostgresConfigStore::new(pool.clone());
            let options = ExportOptions {
                org: Some("org-a".to_string()),
                ..Default::default()
            };
            let first = super::export_document(&pool, &store.load().await.unwrap(), &options)
                .await
                .unwrap();
            let file = dir.join("org-a.toml");
            assert!(first.contains("allow_unlisted_tools = false"));
            // keep the no-op assertion about MCP policy; unchanged logging
            // sections still write on import independently of this fix (#3577)
            let policy = &first[first.find("\n[[mcp_gateway_settings]]\n").unwrap()
                ..first.find("\n[logging]\n").unwrap()];
            std::fs::write(
                &file,
                policy.replace(
                    "allow_unlisted_tools = false",
                    "allow_unlisted_tools = true",
                ),
            )
            .unwrap();
            // the target already holds another key with the same public reference
            let other: Uuid = sqlx::query_scalar(
                "insert into virtual_keys (project_id, key_hash, key_prefix, name)
                 values ($1, $2, $3, $4) returning id",
            )
            .bind(world.project_a)
            .bind(format!("digest-{}", Uuid::new_v4()))
            .bind(concat!("sk-", "rolter-ab"))
            .bind(named.then_some("ci"))
            .fetch_one(&pool)
            .await
            .unwrap();
            sqlx::query(
                "insert into mcp_tool_group_assignments (group_id, subject_kind, subject_id)
                 values ($1, 'virtual_key', $2)",
            )
            .bind(world.group)
            .bind(other)
            .execute(&pool)
            .await
            .unwrap();
            let team: Uuid = sqlx::query_scalar("select team_id from projects where id = $1")
                .bind(world.project_a)
                .fetch_one(&pool)
                .await
                .unwrap();
            let caller = [
                ("virtual_key", other.to_string()),
                ("project", world.project_a.to_string()),
                ("team", team.to_string()),
            ];
            // policy unions all three subjects: adding write-only would grant write
            let tools = || async {
                let config = store.load().await.unwrap();
                config
                    .mcp_tool_groups
                    .iter()
                    .filter(|group| {
                        !group.disabled
                            && group.assignments.iter().any(|subject| {
                                caller
                                    .iter()
                                    .any(|(kind, id)| subject.kind == *kind && &subject.id == id)
                            })
                    })
                    .flat_map(|group| group.tools.iter().map(|tool| tool.tool.clone()))
                    .collect::<std::collections::BTreeSet<_>>()
            };
            let read_only =
                std::collections::BTreeSet::from(["fetch".to_string(), "search".to_string()]);
            assert_eq!(tools().await, read_only);
            let version = || async {
                sqlx::query_scalar::<_, i64>("select version from config_version")
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            };
            let before = version().await;
            for _ in 0..2 {
                let result =
                    crate::seed::import_bootstrap_toml(&pool, world.org_a, world.project_a, &file)
                        .await;
                assert_eq!(
                    tools().await,
                    read_only,
                    "a refused import widened another key's tools"
                );
                let refused = result.unwrap_err().to_string();
                for named in ["org-a/", "org-a/default/default", "2 keys"] {
                    assert!(refused.contains(named), "{refused}");
                }
                assert_eq!(
                    version().await,
                    before,
                    "a refused import wrote configuration"
                );
            }
            // a distinct name resolves either collision; valid repeated imports are no-ops
            sqlx::query("update virtual_keys set name = 'another-key' where id = $1")
                .bind(other)
                .execute(&pool)
                .await
                .unwrap();
            crate::seed::import_bootstrap_toml(&pool, world.org_a, world.project_a, &file)
                .await
                .unwrap();
            assert_eq!(tools().await, read_only);
            let before = version().await;
            crate::seed::import_bootstrap_toml(&pool, world.org_a, world.project_a, &file)
                .await
                .unwrap();
            assert_eq!(version().await, before);
        }

        /// What the tool-group tests export: two orgs, org-a with a `github`
        /// server, a named and an unnamed key in its default project, a group
        /// assigned to its team, project and both keys, and a disabled one.
        struct ToolGroupWorld {
            org_a: Uuid,
            org_b: Uuid,
            project_a: Uuid,
            server: Uuid,
            group: Uuid,
            ci_key: Uuid,
        }

        async fn tool_group_world(pool: &sqlx::PgPool) -> ToolGroupWorld {
            let (org_a, project_a) = bootstrap_named_org(pool, "org-a").await;
            let (org_b, _) = bootstrap_named_org(pool, "org-b").await;
            let team_a: Uuid = sqlx::query_scalar("select team_id from projects where id = $1")
                .bind(project_a)
                .fetch_one(pool)
                .await
                .unwrap();
            let server: Uuid = sqlx::query_scalar(
                "insert into mcp_servers (org_id, name, slug, url)
                 values ($1, 'GitHub', 'github', 'https://mcp.example/github') returning id",
            )
            .bind(org_a)
            .fetch_one(pool)
            .await
            .unwrap();
            let key = |name: Option<&'static str>| async move {
                sqlx::query_scalar::<_, Uuid>(
                    "insert into virtual_keys (project_id, key_hash, key_prefix, name)
                     values ($1, $2, $3, $4) returning id",
                )
                .bind(project_a)
                .bind(format!("digest-{}", Uuid::new_v4()))
                .bind(concat!("sk-", "rolter-ab"))
                .bind(name)
                .fetch_one(pool)
                .await
                .unwrap()
            };
            let ci_key = key(Some("ci")).await;
            let unnamed = key(None).await;
            let tools = serde_json::json!([
                {"server_id": server, "tool": "search"},
                {"server_id": server, "tool": "fetch"},
            ]);
            let group: Uuid = sqlx::query_scalar(
                "insert into mcp_tool_groups (org_id, name, slug, description, tools)
                 values ($1, 'Read only', 'read-only', 'no writes', $2) returning id",
            )
            .bind(org_a)
            .bind(&tools)
            .fetch_one(pool)
            .await
            .unwrap();
            for (kind, subject) in [
                ("team", team_a),
                ("project", project_a),
                ("virtual_key", ci_key),
                ("virtual_key", unnamed),
            ] {
                sqlx::query(
                    "insert into mcp_tool_group_assignments (group_id, subject_kind, subject_id)
                     values ($1, $2, $3)",
                )
                .bind(group)
                .bind(kind)
                .bind(subject)
                .execute(pool)
                .await
                .unwrap();
            }
            for (org, slug, name, enabled) in [
                (org_a, "off", "Switched off", false),
                (org_b, "elsewhere", "Elsewhere", true),
            ] {
                sqlx::query(
                    "insert into mcp_tool_groups (org_id, name, slug, enabled)
                     values ($1, $2, $3, $4)",
                )
                .bind(org)
                .bind(name)
                .bind(slug)
                .bind(enabled)
                .execute(pool)
                .await
                .unwrap();
            }
            ToolGroupWorld {
                org_a,
                org_b,
                project_a,
                server,
                group,
                ci_key,
            }
        }

        /// A cap whose scope does not resolve is left alone rather than
        /// half applied, a database id is not a name, and a second import of
        /// the same file writes nothing.
        #[tokio::test]
        async fn a_cap_naming_nothing_here_is_left_alone() {
            let Some(db) = scratch_db().await else {
                return;
            };
            let pool = db.pool().clone();
            let (org_id, project_id) = bootstrap_org(&pool).await;
            let dir = tempdir("export-caps");
            let path = dir.join("rolter.toml");
            std::fs::write(
                &path,
                format!(
                    r#"
[[budgets]]
scope = "team"
id = "elsewhere/platform"
limit_usd = 1.0

[[budgets]]
scope = "org"
id = "{org_id}"
limit_usd = 2.0

[[budgets]]
scope = "key"
id = "some-key"
limit_usd = 3.0

[[budgets]]
scope = "org"
id = "acme"
limit_usd = 4.0

[[rate_limits]]
scope = "project"
id = "acme/default/missing"
rpm = 10
"#
                ),
            )
            .unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &path)
                .await
                .unwrap();
            let rows: Vec<(String, String)> =
                sqlx::query_as("select scope_type, limit_usd::text from budgets")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            assert_eq!(rows, vec![("org".to_string(), "4.0000".to_string())]);
            let limits: i64 = sqlx::query_scalar("select count(*) from rate_limits")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(limits, 0);

            // desired state: an edit lands on the row, and an unchanged file
            // does not move the config version
            let version = || async {
                sqlx::query_scalar::<_, i64>("select version from config_version where id = 1")
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            };
            let before = version().await;
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &path)
                .await
                .unwrap();
            assert_eq!(version().await, before, "an unchanged import bumped");
            let edited = std::fs::read_to_string(&path)
                .unwrap()
                .replace("limit_usd = 4.0", "limit_usd = 40.0");
            std::fs::write(&path, edited).unwrap();
            crate::seed::import_bootstrap_toml(&pool, org_id, project_id, &path)
                .await
                .unwrap();
            let limit: String =
                sqlx::query_scalar("select limit_usd::text from budgets where scope_type = 'org'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(limit, "40.0000");
            let count: i64 = sqlx::query_scalar("select count(*) from budgets")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(count, 1, "an edit must not add a row");
        }

        /// Caps on a key or a person have no name that survives a move, so
        /// they are counted in a comment instead of written under an id.
        #[test]
        fn caps_without_a_portable_name_are_not_exported() {
            use rolter_core::{BudgetScope, GatewayConfig};
            let mut config = GatewayConfig::default();
            config.budgets.push(
                serde_json::from_value(serde_json::json!({
                    "scope": "key", "id": "11111111-1111-1111-1111-111111111111",
                    "limit_usd": 5, "period": "monthly"
                }))
                .unwrap(),
            );
            let mut refs = super::super::ScopeRefs::default();
            refs.insert(BudgetScope::Team, "t-1", "acme/a/b".to_string());
            refs.insert(BudgetScope::Team, "t-2", "acme/platform".to_string());
            let out = super::super::render_with(&config, &refs);
            assert!(!out.contains("[[budgets]]"), "{out}");
            assert!(
                out.contains("# not exported: 1 budget(s) and 0 rate limit(s)"),
                "{out}"
            );
            // a name with a slash would read as a deeper path, so it is not
            // recorded at all
            assert!(refs.get(BudgetScope::Team, "t-1").is_none());
            assert_eq!(refs.get(BudgetScope::Team, "t-2"), Some("acme/platform"));
        }

        /// A person's cap is named by email, a narrowed one carries its place
        /// by slug path, and one whose place has no name stays out of the file
        /// (#3311).
        #[test]
        fn a_per_person_cap_is_named_by_email_and_its_place_by_slug() {
            use rolter_core::{BudgetScope, GatewayConfig};
            let mut config = GatewayConfig::default();
            let cap = |within: serde_json::Value| {
                serde_json::from_value(serde_json::json!({
                    "scope": "user", "id": "u-1", "limit_usd": 5, "period": "monthly",
                    "within": within
                }))
                .unwrap()
            };
            config.budgets.push(cap(serde_json::json!({"org": "o-1"})));
            config
                .budgets
                .push(cap(serde_json::json!({"project": "p-gone"})));
            let mut refs = super::super::ScopeRefs::default();
            refs.insert(BudgetScope::User, "u-1", "alice@example.com".to_string());
            refs.insert(BudgetScope::Org, "o-1", "acme".to_string());
            let out = super::super::render_with(&config, &refs);
            assert!(
                out.contains(
                    "[[budgets]]\nscope = \"user\"\nid = \"alice@example.com\"\n\
                     within = { org = \"acme\" }"
                ),
                "{out}"
            );
            assert!(!out.contains("p-gone") && !out.contains("u-1"), "{out}");
            assert!(out.contains("# not exported: 1 budget(s)"), "{out}");
            // an address with whitespace in it is no token an importer could
            // read back, so it is not recorded
            refs.insert(BudgetScope::User, "u-2", "a b@example.com".to_string());
            assert!(refs.get(BudgetScope::User, "u-2").is_none());
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
            let url = rolter_store::postgres::test_database::url()
                .await
                .or_else(|| {
                    eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
                    None
                })?;
            Some(TestSchema::migrated(&url).await)
        }

        async fn bootstrap_org(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
            bootstrap_named_org(pool, "acme").await
        }

        async fn bootstrap_named_org(pool: &sqlx::PgPool, slug: &str) -> (Uuid, Uuid) {
            use rolter_store::postgres::repo::{ProjectRepo, TeamRepo};
            let org_id: Uuid =
                sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
                    .bind(slug)
                    .fetch_one(pool)
                    .await
                    .unwrap();
            let team = TeamRepo(pool).create(org_id, "default").await.unwrap();
            let project = ProjectRepo(pool).create(team.id, "default").await.unwrap();
            (org_id, project.id)
        }
    }
}
