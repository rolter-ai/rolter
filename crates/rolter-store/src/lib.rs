//! Storage abstractions for rolter.
//!
//! The MVP ships an in-memory [`ConfigStore`]. Postgres (source of truth),
//! Redis (cache + pub/sub) and ClickHouse (logs) backends implement the same
//! traits behind cargo features as the control plane is built out.
//!
//! **Internal crate.** It is published only so `cargo install rolter` can
//! resolve, and it offers no stable Rust API: any public item here may change
//! or disappear in any release, including a patch release. Build against
//! rolter's HTTP surfaces instead — see
//! [ADR-0032](https://github.com/rolter-ai/rolter/blob/master/docs/dev-docs/adr/2026-09-09-one-point-oh-compatibility-guarantees.md).

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
pub use rolter_core::telemetry::ConfigProblemKind;
use rolter_core::{GatewayConfig, Result};

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "postgres")]
pub use postgres::PostgresConfigStore;

/// Whose row a line of [`ConfigStore::load_problems`] is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProblemOwner {
    /// a deployment-wide setting, or a row no tenant owns: everyone may read it
    Deployment,
    /// a row owned by an organization, or by one project of it
    Tenant(rolter_core::Tenancy),
    /// a budget or rate limit, named by the scope it caps
    Scope {
        scope: rolter_core::BudgetScope,
        id: String,
    },
}

/// One line of [`ConfigStore::load_problems`] and the row it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProblem {
    pub message: String,
    pub owner: ProblemOwner,
    /// what the line reports, for the control plane's
    /// `rolter_config_problems{kind}` gauge (#3542): a row the loader misread
    /// or left out, or an org row shadowing a bootstrap row
    pub kind: ConfigProblemKind,
}

impl ConfigProblem {
    pub fn deployment(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            owner: ProblemOwner::Deployment,
            kind: ConfigProblemKind::Misread,
        }
    }
}

/// Read/write access to the gateway configuration.
#[async_trait]
pub trait ConfigStore: Send + Sync {
    /// Load the current configuration snapshot.
    ///
    /// Fails, rather than returning a weaker config, when a stored value that
    /// gates traffic cannot be read, such as the security settings'
    /// `required_headers` (#2943): the caller then publishes nothing and the
    /// gateways keep what they have. [`load_problems`](Self::load_problems) says
    /// which value it was.
    async fn load(&self) -> Result<GatewayConfig>;
    /// Persist a new configuration snapshot.
    async fn save(&self, config: GatewayConfig) -> Result<()>;
    /// The store's current config version, bumped on every write. Gateways
    /// poll this (see `GET /internal/snapshot?version=N` in rolter-control)
    /// to decide whether a fresh snapshot needs fetching.
    async fn current_version(&self) -> Result<i64> {
        Ok(1)
    }

    /// Rows [`load`](Self::load) could only map onto the config by guessing,
    /// or had to leave out, one sentence each, for `GET /api/v1/config/problems`.
    ///
    /// `load` has to return a config whatever a stored row says, so a value it
    /// does not recognise falls back to a default, or the row is left out,
    /// rather than failing the snapshot for every tenant. That keeps the fleet
    /// served but makes the guess silent; this is where it is said out loud
    /// (#1902). A row is left out rather than defaulted when the default would
    /// be the permissive reading of it, such as a route whose `advanced`
    /// settings do not parse (#2938), or a provider whose proxy pool does not
    /// (#2943). A setting that gates traffic and cannot be read at all makes
    /// `load` fail instead, and this is then the only place its cause is listed.
    /// Empty for a store with no free-text rows to misread.
    async fn load_problems(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// [`load_problems`](Self::load_problems) with the owner of each sentence,
    /// so the problems endpoint can show a caller only the lines about rows it
    /// may see (#3100). A store that does not know whose a line is keeps the
    /// default, which reads every line of `load_problems` as the deployment's.
    async fn load_problem_records(&self) -> Result<Vec<ConfigProblem>> {
        Ok(self
            .load_problems()
            .await?
            .into_iter()
            .map(ConfigProblem::deployment)
            .collect())
    }

    /// How many of the guardrail rules [`load`](Self::load) returns come from
    /// the bootstrap file. They are the leading ones, ahead of the registry's,
    /// so the policy dry run can tell the two apart without the store handing
    /// out the file config. Zero for a store with no bootstrap file.
    fn file_guardrail_rules(&self) -> usize {
        0
    }
}

/// An in-memory [`ConfigStore`] for development and tests.
pub struct InMemoryConfigStore {
    inner: Arc<RwLock<GatewayConfig>>,
    version: AtomicI64,
}

impl InMemoryConfigStore {
    /// Create a store seeded with `config`.
    pub fn new(config: GatewayConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(config)),
            version: AtomicI64::new(1),
        }
    }
}

#[async_trait]
impl ConfigStore for InMemoryConfigStore {
    async fn load(&self) -> Result<GatewayConfig> {
        Ok(self.inner.read().clone())
    }

    async fn save(&self, config: GatewayConfig) -> Result<()> {
        *self.inner.write() = config;
        self.version.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn current_version(&self) -> Result<i64> {
        Ok(self.version.load(Ordering::SeqCst))
    }
}

/// Layers a read-only bootstrap config over a mutable inner store,
/// LiteLLM-style: file-declared providers/routes are "config models" (owned
/// by the file, immutable at runtime), inner-store rows are "DB models"
/// (full runtime CRUD). `load()` returns the merged view. A provider, route or
/// provider group is named by `(org, name)` (ADR-0040): on a collision in the
/// same namespace the config entry wins and the DB entry is dropped from the
/// effective set, while an org's DB row that shares a name with a global
/// config row is kept and shadows it for that org's keys only, and
/// [`load_problem_records`](ConfigStore::load_problem_records) lists it as a
/// warning (#3516). Writes pass through to the inner store, so config entries
/// can never be edited or deleted through it.
///
/// Every other table has one owner, set out in `load` and in
/// `docs/dev-docs/architecture/config-and-hot-reload.md`: the file for what the
/// gateway process reads at startup, the store for what the dashboard edits.
/// A dashboard-owned table the store has nothing to say about yet falls back to
/// the file's value, so adopting a database never erases an existing policy.
pub struct MergedConfigStore {
    bootstrap: GatewayConfig,
    inner: Arc<dyn ConfigStore>,
}

impl MergedConfigStore {
    /// Create a store merging `bootstrap` (config-owned, wins conflicts)
    /// over `inner` (runtime-owned).
    pub fn new(bootstrap: GatewayConfig, inner: Arc<dyn ConfigStore>) -> Self {
        Self { bootstrap, inner }
    }
}

/// The store's value for a singleton the dashboard owns, unless the store still
/// holds exactly what its migration seeded.
///
/// A value equal to `T::default()` is indistinguishable from a row nobody has
/// saved, and there the file's value is the only real one. The cost is one
/// corner: an admin who saves the defaults back over a file value that differs
/// gets the file's value again, and clears it by editing the file.
fn store_unless_untouched<T: Default + PartialEq>(store: T, file: T) -> T {
    if store == T::default() {
        file
    } else {
        store
    }
}

/// The `[logging]` section: the store's half when it holds anything but its
/// seeded values, the file's otherwise.
///
/// `logging_settings` has no column for the sink or its batching, so
/// `clickhouse_url`, `batch_max`, `flush_ms` and `queue_capacity` can only ever
/// come from the file and are kept whichever side wins the rest.
///
/// Public so the control plane can report the rate the snapshot actually
/// serves without assembling a whole [`ConfigStore::load`] (#3171).
pub fn merge_logging(
    file: rolter_core::LoggingConfig,
    store: rolter_core::LoggingConfig,
) -> rolter_core::LoggingConfig {
    let seeded = rolter_core::LoggingConfig::default();
    if store.sample_rate == seeded.sample_rate
        && store.payload_capture == seeded.payload_capture
        && store.ui_events == seeded.ui_events
    {
        return file;
    }
    rolter_core::LoggingConfig {
        sample_rate: store.sample_rate,
        payload_capture: store.payload_capture,
        ui_events: store.ui_events,
        ..file
    }
}

/// The slug of a prompt template the store projected: its id is
/// `{org_id}:{slug}`, and an org id is a uuid, so it holds no colon.
fn stored_template_slug(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, slug)| slug)
}

/// Combine the file's `[prompt_templates]` with the store's published ones.
///
/// A store with no published template leaves the file's block exactly as it
/// was. Otherwise every stored template is served, and a file template is kept
/// only while the store holds none of that name. The store wins a collision, the
/// reverse of providers and routes, because nothing makes a file template
/// read-only in the dashboard: `rolter-seed --import` (which `easy-up` runs)
/// copies the file's templates into the store, where an admin may publish a
/// newer version, and serving the file's copy beside it would run both sets of
/// decorators on one request.
fn merge_prompt_templates(
    file: rolter_core::PromptTemplatesConfig,
    db: rolter_core::PromptTemplatesConfig,
) -> rolter_core::PromptTemplatesConfig {
    if db.templates.is_empty() {
        return file;
    }
    let held: std::collections::HashSet<String> = db
        .templates
        .iter()
        .map(|template| stored_template_slug(&template.id).to_string())
        .collect();
    let mut templates = db.templates;
    // a disabled block adds nothing to the request path, so its templates stay
    // out rather than being switched on by the store's
    if file.enabled {
        templates.extend(
            file.templates
                .into_iter()
                .filter(|template| !held.contains(template.id.trim())),
        );
    }
    rolter_core::PromptTemplatesConfig {
        enabled: true,
        templates,
    }
}

/// Combine the file's plugin instances with the registry's enabled ones.
///
/// Same rule as [`merge_prompt_templates`]: every registry instance is served,
/// and a file instance survives unless the registry holds one for the same org,
/// project and slug.
fn merge_plugins(
    file: rolter_core::PluginsConfig,
    db: rolter_core::PluginsConfig,
) -> rolter_core::PluginsConfig {
    let identity = |plugin: &rolter_core::PluginInstanceConfig| {
        (
            plugin.org_id.clone(),
            plugin.project_id.clone(),
            plugin.slug.clone(),
        )
    };
    let held: std::collections::HashSet<_> = db.instances.iter().map(identity).collect();
    let mut instances = db.instances;
    instances.extend(
        file.instances
            .into_iter()
            .filter(|plugin| !held.contains(&identity(plugin))),
    );
    rolter_core::PluginsConfig { instances }
}

/// The effective slug of a provider group: its own, else its slugified name.
fn group_slug(group: &rolter_core::ProviderGroupConfig) -> String {
    group
        .slug
        .clone()
        .unwrap_or_else(|| rolter_core::slug::slugify(&group.name))
}

/// The `(org, name)` pairs the bootstrap file holds per kind, which a store
/// row must not repeat to be served.
struct FileNames {
    providers: std::collections::HashSet<(String, String)>,
    routes: std::collections::HashSet<(String, String)>,
    provider_groups: std::collections::HashSet<(String, String)>,
}

impl FileNames {
    fn of(file: &GatewayConfig) -> Self {
        Self {
            providers: file
                .providers
                .iter()
                .map(|p| Self::key(&p.tenancy, &p.name))
                .collect(),
            routes: file
                .routes
                .iter()
                .map(|r| Self::key(&r.tenancy, &r.model))
                .collect(),
            provider_groups: file
                .provider_groups
                .iter()
                .map(|g| Self::key(&g.tenancy, &group_slug(g)))
                .collect(),
        }
    }

    fn key(tenancy: &Option<rolter_core::Tenancy>, name: &str) -> (String, String) {
        (
            rolter_core::namespace::org_of(tenancy.as_ref()).to_string(),
            name.to_string(),
        )
    }
}

/// One warning per store row owned by an org that shares its kind and name
/// with a deployment-wide row of the bootstrap file (#3516).
///
/// [`MergedConfigStore::load`] serves both: the org's row for that org's keys,
/// the file's for every other key. ADR-0040 section 6 refuses such a row when
/// it is created, so one exists only when the file gained the name later or
/// the row was written around the API, and an operator should hear of it.
fn bootstrap_shadows(file: &GatewayConfig, db: &GatewayConfig) -> Vec<ConfigProblem> {
    let global = |names: &std::collections::HashSet<(String, String)>, name: &str| {
        names.contains(&(rolter_core::namespace::GLOBAL.to_string(), name.to_string()))
    };
    let names = FileNames::of(file);
    let mut shadows = Vec::new();
    let mut push = |kind: &str, tenancy: &Option<rolter_core::Tenancy>, name: &str| {
        let Some(owner) = tenancy.as_ref().filter(|t| !t.org_id.is_empty()) else {
            return;
        };
        shadows.push(ConfigProblem {
            message: format!(
                "warning: organization {}'s {kind} '{name}' shadows the readonly {kind} of the \
                 same name in the control plane's config file for that organization's keys; \
                 every other key is served the file's {kind} (#3516)",
                owner.org_id
            ),
            owner: ProblemOwner::Tenant(owner.clone()),
            kind: ConfigProblemKind::BootstrapShadow,
        });
    };
    for provider in &db.providers {
        if global(&names.providers, &provider.name) {
            push("provider", &provider.tenancy, &provider.name);
        }
    }
    for route in &db.routes {
        if global(&names.routes, &route.model) {
            push("route", &route.tenancy, &route.model);
        }
    }
    for group in &db.provider_groups {
        let slug = group_slug(group);
        if global(&names.provider_groups, &slug) {
            push("provider group", &group.tenancy, &slug);
        }
    }
    shadows
}

#[async_trait]
impl ConfigStore for MergedConfigStore {
    fn file_guardrail_rules(&self) -> usize {
        self.bootstrap.guardrails.rules.len()
    }

    async fn load(&self) -> Result<GatewayConfig> {
        let db = self.inner.load().await?;
        let mut merged = self.bootstrap.clone();
        // a store row is dropped only for a file row of the same kind and name
        // in the same namespace (#3516): an org's own row shadows the file's
        // for that org's keys, as ADR-0040 section 1 names rows by `(org, name)`
        let file = FileNames::of(&self.bootstrap);
        merged
            .providers
            .extend(db.providers.into_iter().filter(|p| {
                !file
                    .providers
                    .contains(&FileNames::key(&p.tenancy, &p.name))
            }));
        // a provider the store left out stays out for the file's references too
        merged
            .left_out_providers
            .extend(db.left_out_providers.iter().cloned());
        merged.routes.extend(
            db.routes
                .into_iter()
                .filter(|r| !file.routes.contains(&FileNames::key(&r.tenancy, &r.model))),
        );
        // provider groups: readonly config groups win a slug collision, db groups
        // extend the effective set (ADR-0022). compare on the effective slug
        merged
            .provider_groups
            .extend(db.provider_groups.into_iter().filter(|g| {
                !file
                    .provider_groups
                    .contains(&FileNames::key(&g.tenancy, &group_slug(g)))
            }));
        merged.virtual_keys.extend(
            db.virtual_keys
                .into_iter()
                .filter(|k| !self.bootstrap.virtual_keys.iter().any(|c| c.key == k.key)),
        );
        // db-only snapshot fields (#623): the inner store populates these, the
        // bootstrap toml does not own them, so they must be carried through or the
        // gateway silently loses every runtime virtual key, price, budget and
        // rate limit whenever a bootstrap config is present. same "bootstrap wins
        // a collision, db extends the rest" rule as above.
        merged.db_virtual_keys.extend(
            db.db_virtual_keys
                .into_iter()
                .filter(|k| !self.bootstrap.db_virtual_keys.iter().any(|c| c.id == k.id)),
        );
        // MCP authorization is database-owned. Never let a bootstrap file
        // override server policy or inject credential-bearing sessions into a
        // control-plane-managed snapshot.
        merged.mcp_servers = db.mcp_servers;
        merged.mcp_oauth_sessions = db.mcp_oauth_sessions;
        merged.mcp_tool_groups = db.mcp_tool_groups;
        merged.mcp_settings = db.mcp_settings;
        // a bootstrap-only marker (an exported file carries it): the snapshot
        // carries the same fact inside each server, never this table (#3398)
        merged.mcp_manifest_enforcement = db.mcp_manifest_enforcement;
        merged.mcp_gateway_settings = db.mcp_gateway_settings;
        // the importer's spelling of the tool groups (#3514): the snapshot
        // carries the store's own, as `mcp_tool_groups`, above
        merged.mcp_gateway_tool_groups = db.mcp_gateway_tool_groups;
        merged
            .model_prices
            .extend(db.model_prices.into_iter().filter(|p| {
                !self
                    .bootstrap
                    .model_prices
                    .iter()
                    .any(|c| c.model == p.model)
            }));
        merged.budgets.extend(
            db.budgets
                .into_iter()
                .filter(|b| !self.bootstrap.budgets.iter().any(|c| c.id == b.id)),
        );
        merged.rate_limits.extend(
            db.rate_limits
                .into_iter()
                .filter(|r| !self.bootstrap.rate_limits.iter().any(|c| c.id == r.id)),
        );
        // runtime policies are database-owned and hot reloadable. Carry them
        // through even when a bootstrap file supplies immutable providers and
        // routes, then apply the global gates to the complete effective route set.
        //
        // each is a single row that its migration seeds with the default, so a
        // store nobody has written projects exactly `T::default()`. That is the
        // one state in which the file still speaks (#2931): copying the store
        // over it would turn a `[retry]` or `[logging]` section into the
        // defaults without a word. Once an admin saves anything else the store
        // wins outright
        merged.retry = store_unless_untouched(db.retry, merged.retry);
        merged.timeouts = store_unless_untouched(db.timeouts, merged.timeouts);
        merged.queue = store_unless_untouched(db.queue, merged.queue);
        merged.compatibility = store_unless_untouched(db.compatibility, merged.compatibility);
        merged.adaptive_routing =
            store_unless_untouched(db.adaptive_routing, merged.adaptive_routing);
        merged.logging = merge_logging(merged.logging, db.logging);
        // file-owned guardrail rules remain immutable and win a name collision;
        // registry rules extend the ordered policy. This keeps adopting the
        // dashboard from erasing an existing deployment policy. Likewise, an
        // enabled file-owned webhook remains authoritative; the registry owns
        // the effective webhook only when the bootstrap hook is disabled.
        merged.guardrails.enabled |= db.guardrails.enabled;
        merged
            .guardrails
            .rules
            .extend(db.guardrails.rules.into_iter().filter(|rule| {
                !self
                    .bootstrap
                    .guardrails
                    .rules
                    .iter()
                    .any(|item| item.name == rule.name)
            }));
        // semantic guards (#459): the registry's providers and policies join
        // the file's, and the file wins a name — or, for an enabled policy, a
        // scope — so adopting the dashboard never displaces what the file
        // enforces. the file's `[guardrail_webhook]` keeps running beside
        // them as one more provider
        merge_semantic_guards(
            &mut merged.semantic_guards,
            db.semantic_guards,
            &self.bootstrap.guardrail_webhook,
        );
        // Client Settings, Model Settings and Security never reached the
        // gateway in this mode (#2922), and follow the same rule; for
        // `[security]` the alternative was "no rules" on upgrade
        merged.client = store_unless_untouched(db.client, merged.client);
        merged.security = store_unless_untouched(db.security, merged.security);
        merged.model_defaults = store_unless_untouched(db.model_defaults, merged.model_defaults);
        // registry-backed tables: the store's rows are what the dashboard
        // shows, so they all reach the gateway, and a file entry the store does
        // not already carry is kept rather than dropped
        merged.prompt_templates =
            merge_prompt_templates(merged.prompt_templates, db.prompt_templates);
        merged.plugins = merge_plugins(merged.plugins, db.plugins);
        merged.feature_flags = db.feature_flags;
        merged.apply_feature_flags();
        Ok(merged)
    }

    async fn save(&self, config: GatewayConfig) -> Result<()> {
        self.inner.save(config).await
    }

    async fn current_version(&self) -> Result<i64> {
        self.inner.current_version().await
    }

    async fn load_problems(&self) -> Result<Vec<String>> {
        Ok(self
            .load_problem_records()
            .await?
            .into_iter()
            .map(|problem| problem.message)
            .collect())
    }

    async fn load_problem_records(&self) -> Result<Vec<ConfigProblem>> {
        let mut problems = self.inner.load_problem_records().await?;
        // a store that cannot load says why in its own records; the overlap
        // is then moot, since nothing is being served
        if let Ok(db) = self.inner.load().await {
            problems.extend(bootstrap_shadows(&self.bootstrap, &db));
        }
        Ok(problems)
    }
}

/// Fold the registry's semantic guards into the file's: a registry provider
/// or policy whose name the file already uses is dropped, as is a registry
/// provider named like the file's `[guardrail_webhook]`, and an enabled
/// registry policy for a scope an enabled file policy already covers.
fn merge_semantic_guards(
    file: &mut rolter_core::SemanticGuardsConfig,
    db: rolter_core::SemanticGuardsConfig,
    webhook: &rolter_core::GuardrailWebhookConfig,
) {
    let webhook_name = webhook.enabled.then(|| webhook.name.trim());
    let providers: Vec<_> = db
        .providers
        .into_iter()
        .filter(|provider| {
            let name = provider.name.trim();
            webhook_name != Some(name) && !file.providers.iter().any(|p| p.name.trim() == name)
        })
        .collect();
    let policies: Vec<_> = db
        .policies
        .into_iter()
        .filter(|policy| {
            !file.policies.iter().any(|owned| {
                owned.name.trim() == policy.name.trim()
                    || (owned.enabled
                        && policy.enabled
                        && owned.scope == policy.scope
                        && owned.scope_id.as_deref().map(str::trim)
                            == policy.scope_id.as_deref().map(str::trim))
            })
        })
        .collect();
    file.providers.extend(providers);
    file.policies.extend(policies);
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A decimal literal for tests. `rust_decimal`'s `dec!` macro would read
    /// slightly better, but its `macros` feature pulls `rust_decimal_macros`,
    /// `proc-macro-crate`, `toml_edit` and `borsh` into the dependency graph in
    /// production position, which is a poor trade for test ergonomics (#967).
    fn d(literal: &str) -> rust_decimal::Decimal {
        literal.parse().expect("a valid decimal literal")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn roundtrips_config() {
        // note: tokio is pulled in transitively only for the test harness here;
        // keep this test self-contained without external services.
        let store = InMemoryConfigStore::new(GatewayConfig::default());
        let mut cfg = store.load().await.unwrap();
        cfg.server.port = 9999;
        store.save(cfg).await.unwrap();
        assert_eq!(store.load().await.unwrap().server.port, 9999);
    }

    fn route(model: &str) -> rolter_core::ModelRoute {
        rolter_core::ModelRoute {
            model: model.to_string(),
            strategy: Default::default(),
            targets: vec![],
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            tenancy: None,
        }
    }

    fn provider(name: &str) -> rolter_core::ProviderConfig {
        rolter_core::ProviderConfig {
            name: name.to_string(),
            kind: rolter_core::ProviderKind::Openai,
            api_base: "https://example.com".to_string(),
            ..Default::default()
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_config_wins_and_db_extends() {
        let mut bootstrap = GatewayConfig::default();
        bootstrap.providers.push(provider("openai"));
        bootstrap.routes.push(route("gpt-4o"));

        let mut db = GatewayConfig::default();
        // colliding entries: config must win, these must be dropped
        db.providers.push(provider("openai"));
        db.routes.push(route("gpt-4o"));
        // db-only additions: must appear in the merged view
        db.providers.push(provider("anthropic"));
        db.routes.push(route("claude"));

        let inner = Arc::new(InMemoryConfigStore::new(db));
        let store = MergedConfigStore::new(bootstrap, inner.clone());

        let merged = store.load().await.unwrap();
        assert_eq!(merged.providers.len(), 2);
        assert_eq!(merged.routes.len(), 2);
        let models: Vec<_> = merged.routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, vec!["gpt-4o", "claude"]);

        // runtime additions land in the inner store and show up without restart
        let mut updated = inner.load().await.unwrap();
        updated.routes.push(route("mistral"));
        store.save(updated).await.unwrap();
        assert_eq!(store.load().await.unwrap().routes.len(), 3);
        assert_eq!(store.current_version().await.unwrap(), 2);
    }

    fn owned_by_org_a() -> Option<rolter_core::Tenancy> {
        Some(rolter_core::Tenancy {
            org_id: "org-a".to_string(),
            project_id: None,
        })
    }

    /// An org's row that shares a kind and name with a file row is served
    /// beside it and named as a warning; only a same-namespace row is dropped
    /// (#3516).
    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_keeps_an_org_row_a_file_row_shares_a_name_with() {
        let mut bootstrap = GatewayConfig::default();
        bootstrap.providers.push(provider("openai"));
        bootstrap.routes.push(route("gpt-4o"));
        bootstrap.provider_groups.push(group("pool"));

        let mut db = GatewayConfig::default();
        let mut org_provider = provider("openai");
        org_provider.tenancy = owned_by_org_a();
        org_provider.api_base = "https://org-a.example.com".to_string();
        let mut org_route = route("gpt-4o");
        org_route.tenancy = owned_by_org_a();
        let mut org_group = group("pool");
        org_group.tenancy = owned_by_org_a();
        db.providers.push(org_provider);
        db.routes.push(org_route);
        db.provider_groups.push(org_group);
        // a global store row the file repeats is still dropped
        db.routes.push(route("gpt-4o"));

        let store = MergedConfigStore::new(bootstrap, Arc::new(InMemoryConfigStore::new(db)));
        let merged = store.load().await.unwrap();
        let orgs = |tenancies: Vec<Option<&rolter_core::Tenancy>>| -> Vec<String> {
            tenancies
                .into_iter()
                .map(|t| rolter_core::namespace::org_of(t).to_string())
                .collect()
        };
        assert_eq!(
            orgs(merged.routes.iter().map(|r| r.tenancy.as_ref()).collect()),
            ["", "org-a"]
        );
        assert_eq!(
            orgs(
                merged
                    .providers
                    .iter()
                    .map(|p| p.tenancy.as_ref())
                    .collect()
            ),
            ["", "org-a"]
        );
        assert_eq!(
            orgs(
                merged
                    .provider_groups
                    .iter()
                    .map(|g| g.tenancy.as_ref())
                    .collect()
            ),
            ["", "org-a"]
        );

        // the org's key reaches the org's row, any other key the file's
        let mut providers = rolter_core::namespace::Namespaced::new();
        for p in &merged.providers {
            let org = rolter_core::namespace::org_of(p.tenancy.as_ref());
            providers.insert(org, &p.name, p.api_base.as_str()).unwrap();
        }
        assert_eq!(
            providers.get("org-a", "openai"),
            Some(&"https://org-a.example.com")
        );
        assert_eq!(
            providers.get("org-b", "openai"),
            Some(&"https://example.com")
        );
        assert_eq!(providers.get("", "openai"), Some(&"https://example.com"));

        let problems = store.load_problem_records().await.unwrap();
        assert_eq!(problems.len(), 3, "{problems:?}");
        for (problem, kind) in problems.iter().zip(["provider", "route", "provider group"]) {
            assert!(problem.message.starts_with("warning: "), "{problem:?}");
            assert!(
                problem.message.contains(&format!("org-a's {kind} '")),
                "{problem:?}"
            );
            assert_eq!(
                problem.owner,
                ProblemOwner::Tenant(owned_by_org_a().unwrap())
            );
        }
        assert_eq!(store.load_problems().await.unwrap().len(), 3);
    }

    fn group(slug: &str) -> rolter_core::ProviderGroupConfig {
        rolter_core::ProviderGroupConfig {
            name: slug.to_string(),
            slug: Some(slug.to_string()),
            strategy: Default::default(),
            members: vec![rolter_core::GroupMember {
                provider: "a".to_string(),
                model: None,
                weight: 1,
            }],
            tenancy: None,
            ..Default::default()
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_readonly_group_wins_and_db_groups_extend() {
        let mut bootstrap = GatewayConfig::default();
        bootstrap.provider_groups.push(group("vllm-cluster"));

        let mut db = GatewayConfig::default();
        db.provider_groups.push(group("vllm-cluster")); // collides → dropped
        db.provider_groups.push(group("vllm-nsk")); // db-only → kept

        let store = MergedConfigStore::new(bootstrap, Arc::new(InMemoryConfigStore::new(db)));
        let merged = store.load().await.unwrap();
        let slugs: Vec<_> = merged
            .provider_groups
            .iter()
            .map(|g| g.slug.as_deref().unwrap())
            .collect();
        assert_eq!(slugs, vec!["vllm-cluster", "vllm-nsk"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_preserves_file_guardrails_and_extends_registry_rules() {
        let rule = |name: &str| rolter_core::GuardrailRule {
            name: name.to_string(),
            builtin: Some(rolter_core::BuiltinRule::Email),
            pattern: None,
            stage: Default::default(),
            action: Default::default(),
            replacement: None,
            include_system: false,
        };
        let mut bootstrap = GatewayConfig::default();
        bootstrap.guardrails.enabled = true;
        bootstrap.guardrails.rules.push(rule("file-rule"));
        let mut db = GatewayConfig::default();
        db.guardrails.enabled = true;
        db.guardrails.rules.push(rule("file-rule"));
        db.guardrails.rules.push(rule("registry-rule"));

        let store = MergedConfigStore::new(bootstrap, Arc::new(InMemoryConfigStore::new(db)));
        let names: Vec<_> = store
            .load()
            .await
            .unwrap()
            .guardrails
            .rules
            .into_iter()
            .map(|rule| rule.name)
            .collect();
        assert_eq!(names, vec!["file-rule", "registry-rule"]);
    }

    fn db_vkey(id: &str, hash: &str) -> rolter_core::VirtualKeyRecord {
        rolter_core::VirtualKeyRecord {
            key_hash: hash.to_string(),
            id: id.to_string(),
            ..Default::default()
        }
    }

    fn price_val(model: &str, input: rust_decimal::Decimal) -> rolter_core::ModelPriceConfig {
        rolter_core::ModelPriceConfig {
            model: model.to_string(),
            input_per_mtok: input,
            output_per_mtok: d("0.0"),
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            currency: "USD".to_string(),
        }
    }

    fn budget(id: &str) -> rolter_core::BudgetConfig {
        rolter_core::BudgetConfig {
            scope: rolter_core::BudgetScope::Org,
            id: id.to_string(),
            limit_usd: d("10.0"),
            period: Default::default(),
            unpriced_policy: None,
            within: None,
        }
    }

    fn rate_limit(id: &str) -> rolter_core::RateLimitConfig {
        rolter_core::RateLimitConfig {
            scope: rolter_core::BudgetScope::Org,
            id: id.to_string(),
            rpm: Some(60),
            tpm: None,
            within: None,
        }
    }

    // regression for #623: a DB-backed inner store populates db_virtual_keys,
    // model_prices, budgets and rate_limits — none of which the bootstrap toml
    // owns. the merge must carry them through, or runtime virtual keys 401 at
    // the gateway (and prices/budgets/limits silently vanish) whenever a
    // bootstrap config is present.
    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_carries_db_only_snapshot_fields() {
        let bootstrap = GatewayConfig::default();

        let mut db = GatewayConfig::default();
        db.db_virtual_keys.push(db_vkey("vk1", "hash-1"));
        db.model_prices.push(price_val("gpt-4o", d("3.0")));
        db.budgets.push(budget("b1"));
        db.rate_limits.push(rate_limit("rl1"));
        db.mcp_servers.push(rolter_core::McpServerConfig {
            id: "mcp1".to_string(),
            org_id: "org1".to_string(),
            slug: "docs".to_string(),
            url: "https://mcp.example.com".to_string(),
            transport: "streamable_http".to_string(),
            required_scopes: vec!["tools:read".to_string()],
            auth_kind: rolter_core::McpAuthKind::Oauth,
            ..Default::default()
        });
        db.mcp_oauth_sessions
            .push(rolter_core::McpOAuthSessionConfig {
                id: "session1".to_string(),
                server_id: "mcp1".to_string(),
                user_id: "user1".to_string(),
                scopes: vec!["tools:read".to_string()],
                expires_at: "2099-01-01T00:00:00Z".parse().unwrap(),
                access_token: "secret".to_string(),
            });

        let store = MergedConfigStore::new(bootstrap, Arc::new(InMemoryConfigStore::new(db)));
        let merged = store.load().await.unwrap();

        assert_eq!(merged.db_virtual_keys.len(), 1, "db virtual key dropped");
        assert_eq!(merged.db_virtual_keys[0].key_hash, "hash-1");
        assert_eq!(merged.model_prices.len(), 1, "db model price dropped");
        assert_eq!(merged.budgets.len(), 1, "db budget dropped");
        assert_eq!(merged.rate_limits.len(), 1, "db rate limit dropped");
        assert_eq!(merged.mcp_servers.len(), 1, "db MCP server dropped");
        assert_eq!(merged.mcp_oauth_sessions.len(), 1, "db MCP session dropped");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_applies_db_runtime_policies_and_feature_gates() {
        let mut bootstrap = GatewayConfig::default();
        bootstrap.cache.enabled = true;
        bootstrap.routes.push(route("cached"));
        bootstrap.routes[0].strategy = rolter_core::BalancingStrategy::CacheAware;

        let mut db = GatewayConfig::default();
        db.retry.max_retries = 7;
        db.timeouts.request_secs = 123;
        db.queue.capacity = 999;
        db.logging.sample_rate = 0.25;
        db.feature_flags.response_cache = false;
        db.feature_flags.cache_aware_routing = false;

        let store = MergedConfigStore::new(bootstrap, Arc::new(InMemoryConfigStore::new(db)));
        let merged = store.load().await.unwrap();

        assert_eq!(merged.retry.max_retries, 7);
        assert_eq!(merged.timeouts.request_secs, 123);
        assert_eq!(merged.queue.capacity, 999);
        assert_eq!(merged.logging.sample_rate, 0.25);
        assert!(!merged.cache.enabled);
        assert_eq!(
            merged.routes[0].strategy,
            rolter_core::BalancingStrategy::PowerOfTwo
        );
    }

    // the db-only fields follow the same "bootstrap wins a collision, db
    // extends the rest" rule as providers/routes/groups (#623 audit).
    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_bootstrap_wins_over_db_prices_and_limits() {
        let mut bootstrap = GatewayConfig::default();
        bootstrap.model_prices.push(price_val("gpt-4o", d("1.0")));
        bootstrap.budgets.push(budget("b1"));
        bootstrap.rate_limits.push(rate_limit("rl1"));

        let mut db = GatewayConfig::default();
        db.model_prices.push(price_val("gpt-4o", d("999.0"))); // collides on model → dropped
        db.model_prices.push(price_val("claude", d("2.0"))); // db-only → kept
        db.budgets.push(budget("b1")); // collides on id → dropped
        db.budgets.push(budget("b2")); // db-only → kept
        db.rate_limits.push(rate_limit("rl1")); // collides on id → dropped
        db.rate_limits.push(rate_limit("rl2")); // db-only → kept

        let store = MergedConfigStore::new(bootstrap, Arc::new(InMemoryConfigStore::new(db)));
        let merged = store.load().await.unwrap();

        let prices: std::collections::HashMap<_, _> = merged
            .model_prices
            .iter()
            .map(|p| (p.model.clone(), p.input_per_mtok))
            .collect();
        assert_eq!(
            prices.get("gpt-4o"),
            Some(&d("1.0")),
            "config price must win"
        );
        assert_eq!(
            prices.get("claude"),
            Some(&d("2.0")),
            "db-only price must survive"
        );
        assert_eq!(merged.model_prices.len(), 2);
        assert_eq!(merged.budgets.len(), 2);
        assert_eq!(merged.rate_limits.len(), 2);
    }
    // #2922: the dashboard owns these tables, so with a bootstrap file the
    // snapshot has to carry what the store holds, not the file's copy.
    fn merged_with(file: GatewayConfig, db: GatewayConfig) -> MergedConfigStore {
        MergedConfigStore::new(file, Arc::new(InMemoryConfigStore::new(db)))
    }

    /// `[[mcp_manifest_enforcement]]` is read by the importer only (#3398): the
    /// snapshot carries the same fact inside each MCP server.
    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_never_ships_the_bootstrap_mcp_manifest_marker() {
        let file = GatewayConfig {
            mcp_manifest_enforcement: vec![rolter_core::McpManifestEnforcementConfig {
                org: "acme".to_string(),
                enforce: false,
            }],
            ..Default::default()
        };
        let merged = merged_with(file, GatewayConfig::default())
            .load()
            .await
            .unwrap();
        assert!(
            merged.mcp_manifest_enforcement.is_empty(),
            "a bootstrap-only table reached the snapshot"
        );
    }

    /// `[[mcp_gateway_settings]]` is read by the importer only (#3482): the
    /// snapshot carries the same values per organization as `mcp_settings`.
    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_never_ships_the_bootstrap_mcp_gateway_settings() {
        let file = GatewayConfig {
            mcp_gateway_settings: vec![rolter_core::McpGatewaySettingsConfig {
                org: "acme".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let merged = merged_with(file, GatewayConfig::default())
            .load()
            .await
            .unwrap();
        assert!(
            merged.mcp_gateway_settings.is_empty(),
            "a bootstrap-only table reached the snapshot"
        );
    }

    /// `[[mcp_gateway_tool_groups]]` is read by the importer only (#3514):
    /// the snapshot carries the store's groups as `mcp_tool_groups`.
    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_never_ships_the_bootstrap_mcp_gateway_tool_groups() {
        let file = GatewayConfig {
            mcp_gateway_tool_groups: vec![rolter_core::McpGatewayToolGroupConfig {
                org: "acme".to_string(),
                slug: "read-only".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let merged = merged_with(file, GatewayConfig::default())
            .load()
            .await
            .unwrap();
        assert!(
            merged.mcp_gateway_tool_groups.is_empty(),
            "a bootstrap-only table reached the snapshot"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_serves_the_stores_client_settings() {
        let mut file = GatewayConfig::default();
        file.client.request_id_header = "x-file".to_string();
        file.client.forwarded_headers = vec!["x-file-only".to_string()];
        let mut db = GatewayConfig::default();
        db.client.request_id_header = "x-db".to_string();
        db.client.public_base_url = Some("https://gw.example.com".to_string());

        let merged = merged_with(file, db).load().await.unwrap();

        assert_eq!(merged.client.request_id_header, "x-db");
        assert_eq!(
            merged.client.public_base_url.as_deref(),
            Some("https://gw.example.com")
        );
        // the store wins the whole screen, not field by field: a header the
        // dashboard's list no longer holds must not come back from the file
        assert!(merged.client.forwarded_headers.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_keeps_the_files_client_settings_while_the_store_is_untouched() {
        let mut file = GatewayConfig::default();
        file.client.request_id_header = "x-file".to_string();
        file.client
            .injected_headers
            .insert("x-team".to_string(), "platform".to_string());

        let merged = merged_with(file, GatewayConfig::default())
            .load()
            .await
            .unwrap();

        assert_eq!(merged.client.request_id_header, "x-file");
        assert_eq!(merged.client.injected_headers["x-team"], "platform");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_serves_the_stores_model_defaults() {
        let mut file = GatewayConfig::default();
        file.model_defaults.enabled = true;
        file.model_defaults.temperature = Some(0.9);
        let mut db = GatewayConfig::default();
        // the dashboard switched the feature off but left a value behind
        db.model_defaults.temperature = Some(0.2);

        let merged = merged_with(file, db).load().await.unwrap();

        assert!(!merged.model_defaults.enabled);
        assert_eq!(merged.model_defaults.temperature, Some(0.2));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_keeps_the_files_model_defaults_while_the_store_is_untouched() {
        let mut file = GatewayConfig::default();
        file.model_defaults.enabled = true;
        file.model_defaults.max_tokens = Some(512);

        let merged = merged_with(file, GatewayConfig::default())
            .load()
            .await
            .unwrap();

        assert!(merged.model_defaults.enabled);
        assert_eq!(merged.model_defaults.max_tokens, Some(512));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_serves_the_stores_security_policy() {
        let mut file = GatewayConfig::default();
        file.security
            .required_headers
            .insert("x-mesh".to_string(), "file".to_string());
        file.security.auth_bypass_routes = vec!["/v1/models".to_string()];
        let mut db = GatewayConfig::default();
        db.security
            .required_headers
            .insert("x-waf".to_string(), "db".to_string());

        let merged = merged_with(file, db).load().await.unwrap();

        assert_eq!(merged.security.required_headers.len(), 1);
        assert_eq!(merged.security.required_headers["x-waf"], "db");
        assert!(merged.security.auth_bypass_routes.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_keeps_the_files_security_policy_while_the_store_is_untouched() {
        let mut file = GatewayConfig::default();
        file.security
            .required_headers
            .insert("x-mesh".to_string(), "file".to_string());
        file.security.auth_bypass_routes = vec!["/v1/models".to_string()];

        let merged = merged_with(file.clone(), GatewayConfig::default())
            .load()
            .await
            .unwrap();

        assert_eq!(merged.security, file.security);
    }

    fn template(id: &str, version: u32) -> rolter_core::PromptTemplate {
        rolter_core::PromptTemplate {
            id: id.to_string(),
            version,
            routes: Vec::new(),
            scopes: Vec::new(),
            variables: Vec::new(),
            decorators: vec![rolter_core::Decorator {
                role: Default::default(),
                position: Default::default(),
                content: format!("from {id} v{version}"),
            }],
        }
    }

    fn templates(enabled: bool, items: Vec<rolter_core::PromptTemplate>) -> GatewayConfig {
        GatewayConfig {
            prompt_templates: rolter_core::PromptTemplatesConfig {
                enabled,
                templates: items,
            },
            ..Default::default()
        }
    }

    fn template_ids(config: &GatewayConfig) -> Vec<String> {
        config
            .prompt_templates
            .templates
            .iter()
            .map(|t| format!("{}@{}", t.id, t.version))
            .collect()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_serves_prompt_templates_published_in_the_store() {
        let file = templates(true, vec![template("from-file", 1)]);
        let db = templates(true, vec![template("org-1:from-dashboard", 3)]);

        let merged = merged_with(file, db).load().await.unwrap();

        assert!(merged.prompt_templates.enabled);
        assert_eq!(
            template_ids(&merged),
            vec!["org-1:from-dashboard@3", "from-file@1"],
            "the dashboard's template must reach the gateway, and the file's must survive"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_prompt_template_in_the_store_replaces_the_files_copy() {
        // `rolter-seed --import` copied `support` into the store and an admin
        // then published v2; running v1 from the file beside it would stack
        // both sets of decorators on one request
        let file = templates(true, vec![template("support", 1), template("kept", 1)]);
        let db = templates(true, vec![template("org-1:support", 2)]);

        let merged = merged_with(file, db).load().await.unwrap();

        assert_eq!(template_ids(&merged), vec!["org-1:support@2", "kept@1"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_leaves_the_files_prompt_templates_alone_when_the_store_has_none() {
        let disabled = templates(false, vec![template("parked", 1)]);
        let merged = merged_with(disabled, GatewayConfig::default())
            .load()
            .await
            .unwrap();
        assert!(!merged.prompt_templates.enabled);
        assert_eq!(template_ids(&merged), vec!["parked@1"]);

        let enabled = templates(true, vec![template("live", 1)]);
        let merged = merged_with(enabled, GatewayConfig::default())
            .load()
            .await
            .unwrap();
        assert!(merged.prompt_templates.enabled);
        assert_eq!(template_ids(&merged), vec!["live@1"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_does_not_switch_on_a_disabled_file_template_block() {
        let file = templates(false, vec![template("parked", 1)]);
        let db = templates(true, vec![template("org-1:live", 1)]);

        let merged = merged_with(file, db).load().await.unwrap();

        assert!(merged.prompt_templates.enabled);
        assert_eq!(template_ids(&merged), vec!["org-1:live@1"]);
    }

    fn plugin(org: &str, slug: &str, endpoint: &str) -> rolter_core::PluginInstanceConfig {
        rolter_core::PluginInstanceConfig {
            slug: slug.to_string(),
            org_id: org.to_string(),
            project_id: None,
            stage: rolter_core::PluginStage::PreRoute,
            position: 0,
            failure_mode: rolter_core::FailureMode::FailOpen,
            endpoint: endpoint.to_string(),
            auth: None,
        }
    }

    fn plugins(items: Vec<rolter_core::PluginInstanceConfig>) -> GatewayConfig {
        GatewayConfig {
            plugins: rolter_core::PluginsConfig { instances: items },
            ..Default::default()
        }
    }

    fn plugin_endpoints(config: &GatewayConfig) -> Vec<(String, String)> {
        config
            .plugins
            .instances
            .iter()
            .map(|p| (p.slug.clone(), p.endpoint.clone()))
            .collect()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_serves_plugins_registered_in_the_store() {
        let file = plugins(vec![plugin("org-1", "from-file", "https://file.example")]);
        let db = plugins(vec![plugin("org-1", "from-registry", "https://db.example")]);

        let merged = merged_with(file, db).load().await.unwrap();

        assert_eq!(
            plugin_endpoints(&merged),
            vec![
                (
                    "from-registry".to_string(),
                    "https://db.example".to_string()
                ),
                ("from-file".to_string(), "https://file.example".to_string()),
            ],
            "the registry's plugin must reach the gateway, and the file's must survive"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_plugin_in_the_registry_replaces_the_files_instance_of_the_same_identity()
    {
        let file = plugins(vec![
            plugin("org-1", "dup", "https://file.example"),
            // same slug in another org is a different instance
            plugin("org-2", "dup", "https://other-org.example"),
        ]);
        let db = plugins(vec![plugin("org-1", "dup", "https://db.example")]);

        let merged = merged_with(file, db).load().await.unwrap();

        assert_eq!(
            plugin_endpoints(&merged),
            vec![
                ("dup".to_string(), "https://db.example".to_string()),
                ("dup".to_string(), "https://other-org.example".to_string()),
            ]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_keeps_the_files_plugins_while_the_registry_is_empty() {
        let file = plugins(vec![plugin("org-1", "from-file", "https://file.example")]);

        let merged = merged_with(file, GatewayConfig::default())
            .load()
            .await
            .unwrap();

        assert_eq!(
            plugin_endpoints(&merged),
            vec![("from-file".to_string(), "https://file.example".to_string())]
        );
    }

    // #2931: the runtime policies were copied from the store whether or not
    // anyone had saved them, so an untouched store's defaults replaced the file
    async fn merged_untouched(file: GatewayConfig) -> GatewayConfig {
        merged_with(file, GatewayConfig::default())
            .load()
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_keeps_the_files_retry_while_the_store_is_untouched() {
        let mut file = GatewayConfig::default();
        file.retry.max_retries = 9;
        file.retry.max_backoff_ms = 30_000;
        let merged = merged_untouched(file.clone()).await;
        assert_eq!(merged.retry, file.retry);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_keeps_the_files_timeouts_while_the_store_is_untouched() {
        let mut file = GatewayConfig::default();
        file.timeouts.connect_secs = 3;
        file.timeouts.request_secs = 600;
        let merged = merged_untouched(file.clone()).await;
        assert_eq!(merged.timeouts, file.timeouts);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_keeps_the_files_queue_while_the_store_is_untouched() {
        let mut file = GatewayConfig::default();
        file.queue.capacity = 4096;
        file.queue.workers = 32;
        let merged = merged_untouched(file.clone()).await;
        assert_eq!(merged.queue, file.queue);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_keeps_the_files_compatibility_while_the_store_is_untouched() {
        let mut file = GatewayConfig::default();
        file.compatibility.default_max_tokens = 11;
        let merged = merged_untouched(file.clone()).await;
        assert_eq!(merged.compatibility, file.compatibility);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_serves_the_stores_compatibility() {
        let mut file = GatewayConfig::default();
        file.compatibility.default_max_tokens = 11;
        let mut db = GatewayConfig::default();
        db.compatibility.default_max_tokens = 22;
        let merged = merged_with(file, db).load().await.unwrap();
        assert_eq!(merged.compatibility.default_max_tokens, 22);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_keeps_the_files_adaptive_routing_while_the_store_is_untouched() {
        let mut file = GatewayConfig::default();
        file.adaptive_routing.enabled = true;
        file.adaptive_routing.min_samples = 7;
        let merged = merged_untouched(file.clone()).await;
        assert_eq!(merged.adaptive_routing, file.adaptive_routing);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_serves_the_stores_adaptive_routing() {
        let mut file = GatewayConfig::default();
        file.adaptive_routing.enabled = true;
        let mut db = GatewayConfig::default();
        db.adaptive_routing.min_samples = 9;
        let merged = merged_with(file, db).load().await.unwrap();
        assert!(!merged.adaptive_routing.enabled);
        assert_eq!(merged.adaptive_routing.min_samples, 9);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_keeps_the_files_logging_while_the_store_is_untouched() {
        let mut file = GatewayConfig::default();
        file.logging.clickhouse_url = Some("http://ch:8123".to_string());
        file.logging.batch_max = 7;
        file.logging.sample_rate = 0.5;
        file.logging.payload_capture.enabled = true;
        file.logging.ui_events = false;
        let merged = merged_untouched(file.clone()).await;
        assert_eq!(merged.logging, file.logging);
    }

    // the store has no column for the sink, so its half of the section wins
    // without taking the file's `clickhouse_url` and batching with it
    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_serves_the_stores_logging_settings_beside_the_files_sink() {
        let mut file = GatewayConfig::default();
        file.logging.clickhouse_url = Some("http://ch:8123".to_string());
        file.logging.batch_max = 7;
        file.logging.flush_ms = 250;
        file.logging.queue_capacity = 64;
        file.logging.sample_rate = 0.5;
        let mut db = GatewayConfig::default();
        db.logging.sample_rate = 0.1;
        db.logging.payload_capture.enabled = true;
        db.logging.ui_events = false;

        let merged = merged_with(file, db).load().await.unwrap();

        assert_eq!(merged.logging.sample_rate, 0.1);
        assert!(merged.logging.payload_capture.enabled);
        assert!(!merged.logging.ui_events);
        assert_eq!(
            merged.logging.clickhouse_url.as_deref(),
            Some("http://ch:8123")
        );
        assert_eq!(merged.logging.batch_max, 7);
        assert_eq!(merged.logging.flush_ms, 250);
        assert_eq!(merged.logging.queue_capacity, 64);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_serves_the_stores_retry_timeouts_and_queue() {
        let mut file = GatewayConfig::default();
        file.retry.max_retries = 9;
        file.timeouts.request_secs = 600;
        file.queue.capacity = 4096;
        let mut db = GatewayConfig::default();
        db.retry.max_retries = 1;
        db.timeouts.request_secs = 30;
        db.queue.capacity = 8;

        let merged = merged_with(file, db).load().await.unwrap();

        assert_eq!(merged.retry.max_retries, 1);
        assert_eq!(merged.timeouts.request_secs, 30);
        assert_eq!(merged.queue.capacity, 8);
    }
}
