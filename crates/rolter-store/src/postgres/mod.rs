//! Postgres-backed [`ConfigStore`], gated behind the `postgres` feature.

pub mod crypto;
pub mod kek_audit;
pub mod models;
pub mod repo;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rolter_core::failover::FailureClass;
use rolter_core::{
    AdvancedModelConfig, BackpressurePolicy, BalancingStrategy, BudgetConfig, BudgetPeriod,
    BudgetScope, BuiltinRule, Decorator, Error, FailureMode, FeatureFlagsConfig, GatewayConfig,
    GroupMember, GuardAction, GuardProviderKind, GuardStage, GuardrailPolicy, GuardrailRule,
    GuardrailWebhookConfig, GuardrailsConfig, McpAuthKind, McpOAuthSessionConfig,
    McpOrgSettingsConfig, McpServerConfig, McpToolGroupConfig, McpToolGroupSubjectConfig,
    McpToolRefConfig, ModelPolicy, ModelPriceConfig, ModelRoute, ParamPolicy, PluginInstanceConfig,
    PluginStage, PluginsConfig, PolicyScope, PromptTemplate, PromptTemplateActivationScope,
    PromptTemplatesConfig, ProviderConfig, ProviderGroupConfig, ProviderKind, RateLimitConfig,
    Result, ScopeWithin, SemanticGuardsConfig, StreamingStrategy, Target, TemplateVariable,
    Tenancy, UnpricedPolicy, ViolationAction, VirtualKeyRecord, WebhookAuth, WebhookStage,
};
use rust_decimal::Decimal;
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

use crate::postgres::models::{
    AdaptiveRoutingPolicy, Budget, ClientSettings, CompatibilityPolicy, FeatureFlags,
    GuardrailProvider, GuardrailRule as GuardrailRuleRow, LoggingSettings, ModelDefaults,
    ModelPrice, PluginInstance, RateLimit, RuntimePolicy, SecurityPolicyRow,
};
use crate::ConfigStore;

/// Map a `sqlx` failure onto the store's error type: a unique violation onto
/// [`Error::AlreadyExists`], anything else onto [`Error::Store`].
///
/// Public so the control plane's own queries (the ones outside a repository)
/// answer a unique violation the way the repositories do, with a `409` rather
/// than a `500` (#2729).
pub fn store_err(err: sqlx::Error) -> Error {
    repo::support::store_err(err)
}

/// Connection-pool budget for the control plane's Postgres pool.
///
/// One pool serves everything the control plane does: `/internal/snapshot`,
/// which every gateway in the fleet polls, the whole dashboard CRUD surface,
/// SCIM, SSO, MCP and the RBAC guard's per-request membership lookups. Under
/// fan-out a fixed ceiling turns into an acquire-timeout queue with no lever to
/// pull, which is why every field here is configurable (#1052).
///
/// [`Default`] reproduces the previously hardcoded budget exactly, so an
/// existing deployment that configures nothing is unaffected.
#[derive(Debug, Clone, Copy)]
pub struct PoolConfig {
    /// upper bound on open connections. Sized against Postgres `max_connections`
    /// divided across every control replica, not against gateway count
    pub max_connections: u32,
    /// connections kept open while idle. Above zero, first-request latency
    /// after a quiet period skips the connect handshake
    pub min_connections: u32,
    /// how long a caller waits for a free connection before failing. Bounded
    /// so pool exhaustion surfaces as a fast, attributable error instead of an
    /// opaque stall
    pub acquire_timeout: std::time::Duration,
    /// close a connection idle for longer than this. `None` keeps idle
    /// connections forever
    pub idle_timeout: Option<std::time::Duration>,
    /// retire a connection older than this regardless of use, so a pool cannot
    /// pin itself to a database instance across a failover. `None` disables
    pub max_lifetime: Option<std::time::Duration>,
    /// longest a single statement may run before Postgres cancels it. `None`
    /// leaves the server default, which is to wait forever
    pub statement_timeout: Option<std::time::Duration>,
    /// longest a statement may wait for a row or table lock held by another
    /// transaction. `None` leaves the server default, which is to wait forever
    pub lock_timeout: Option<std::time::Duration>,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            // the historical hardcoded value
            max_connections: 10,
            min_connections: 0,
            acquire_timeout: std::time::Duration::from_secs(30),
            idle_timeout: Some(std::time::Duration::from_secs(600)),
            max_lifetime: Some(std::time::Duration::from_secs(1800)),
            statement_timeout: None,
            lock_timeout: None,
        }
    }
}

/// Connect to Postgres with the default pool budget.
pub async fn connect(database_url: &str) -> Result<PgPool> {
    connect_with(database_url, PoolConfig::default()).await
}

/// Connect to Postgres with an explicit pool budget. See [`PoolConfig`].
pub async fn connect_with(database_url: &str, config: PoolConfig) -> Result<PgPool> {
    let mut options = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(config.acquire_timeout);
    // sqlx takes `Option` here, but spelling it out keeps "unset means never
    // expire" explicit rather than implied by a missing builder call
    options = options.idle_timeout(config.idle_timeout);
    options = options.max_lifetime(config.max_lifetime);
    if config.statement_timeout.is_some() || config.lock_timeout.is_some() {
        // session settings, so they hold for every statement the connection
        // runs, a transaction's included. `set_config` takes the value as a
        // bind parameter, so no setting is ever spliced into sql text
        let settings: Vec<(&'static str, String)> = [
            ("statement_timeout", config.statement_timeout),
            ("lock_timeout", config.lock_timeout),
        ]
        .into_iter()
        .filter_map(|(name, bound)| bound.map(|d| (name, d.as_millis().to_string())))
        .collect();
        options = options.after_connect(move |conn, _meta| {
            let settings = settings.clone();
            Box::pin(async move {
                for (name, millis) in settings {
                    sqlx::query("select set_config($1, $2, false)")
                        .bind(name)
                        .bind(millis)
                        .execute(&mut *conn)
                        .await?;
                }
                Ok(())
            })
        });
    }
    options.connect(database_url).await.map_err(store_err)
}

/// A point-in-time reading of a pool's occupancy, for the metrics exporter.
///
/// `connections` counts every connection the pool holds open and `idle` how
/// many of those are free right now; `connections - idle` is therefore what is
/// checked out. When `connections` sits at `max` while `idle` is zero, callers
/// are queueing on `acquire_timeout` and the ceiling is the bottleneck.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolStats {
    pub connections: u64,
    pub idle: u64,
    pub max: u64,
}

/// Sample `pool` for the metrics exporter. Cheap: reads atomics, no query.
pub fn pool_stats(pool: &PgPool) -> PoolStats {
    PoolStats {
        connections: u64::from(pool.size()),
        idle: pool.num_idle() as u64,
        max: u64::from(pool.options().get_max_connections()),
    }
}

/// Run pending migrations against `pool`. The migration set lives in this
/// crate's own `migrations/` directory so it is embedded at compile time and
/// packaged with the published crate; `docker-compose` mounts the same dir for
/// its initdb bootstrap.
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|err| Error::Store(err.to_string()))
}

/// Embedded migration versions the database has not applied successfully.
///
/// Used by the control plane's readiness probe: a pod whose database is behind
/// the binary it runs cannot serve `/internal/snapshot` or the CRUD API
/// correctly, so it must not be marked ready. A missing `_sqlx_migrations`
/// table — a database nothing has ever migrated — reports every version as
/// pending rather than erroring, which is the same answer for the caller.
pub async fn pending_migrations(pool: &PgPool) -> Result<Vec<i64>> {
    let migrator = sqlx::migrate!("./migrations");
    let applied: Vec<i64> =
        match sqlx::query_scalar::<_, i64>("select version from _sqlx_migrations where success")
            .fetch_all(pool)
            .await
        {
            Ok(rows) => rows,
            // 42P01 is undefined_table: nothing has migrated this database yet
            Err(sqlx::Error::Database(err)) if err.code().as_deref() == Some("42P01") => Vec::new(),
            Err(err) => return Err(store_err(err)),
        };
    Ok(migrator
        .iter()
        .map(|m| m.version)
        .filter(|version| !applied.contains(version))
        .collect())
}

/// Test-only helpers for building isolated, migrated pools. Every test gets its
/// own schema pinned via `search_path`, so plain `cargo test` (which runs tests
/// as threads in one process — e.g. the coverage job) never races on a shared
/// `public` schema during DDL, and gets it back when the test finishes.
///
/// Available to other crates' tests behind the `test-support` feature; see
/// `docs/dev-docs/development/testing.md`.
#[cfg(any(test, feature = "test-support"))]
pub mod test_schema;

/// Test-only resolution of *which* database the postgres tests run against: a
/// database per worktree, derived from the workspace path rather than
/// configured, so parallel worktrees stop sharing one (#1430).
#[cfg(any(test, feature = "test-support"))]
pub mod test_database;

/// Providers the loader left out, as `(owning org, name)`.
type LeftOut = HashSet<(Uuid, String)>;

/// Whether the provider `name` of `org` was left out. A stored target or
/// member names its provider by id, so the org it reaches is exactly the
/// provider's own, with no resolution to do.
fn is_left_out(left_out: &LeftOut, org: Uuid, name: &str) -> bool {
    !left_out.is_empty() && left_out.contains(&(org, name.to_string()))
}

#[derive(FromRow)]
struct ProviderRow {
    org_id: Uuid,
    name: String,
    slug: String,
    kind: String,
    api_base: String,
    api_key_env: Option<String>,
    egress_proxy: Option<String>,
    egress_proxies: serde_json::Value,
    kv_events: Option<serde_json::Value>,
    lmcache: Option<serde_json::Value>,
    /// the project the provider is scoped to, `None` for org-wide (#1919)
    project_id: Option<Uuid>,
    /// opt out of the kind's host pin (ADR-0029, #1133)
    allow_custom_api_base: bool,
    /// sealed runtime credential from `provider_keys`, when one is stored
    ciphertext: Option<Vec<u8>>,
    nonce: Option<Vec<u8>>,
}

impl ProviderRow {
    /// Convert a row into config, opening the sealed credential with `kek`
    /// when both are present. A missing KEK or an undecryptable credential
    /// degrades to `api_key: None` with a warning rather than failing the
    /// whole config load, so one bad key cannot take down snapshot serving.
    ///
    /// `egress_proxies` is the pool [`egress_proxies_from_value`] read from the
    /// row, which the caller has already checked: a row whose pool cannot be
    /// read never gets this far.
    fn into_config(
        self,
        kek: Option<&crypto::Kek>,
        egress_proxies: Vec<String>,
    ) -> Result<ProviderConfig> {
        let row = self;
        let kind = match row.kind.as_str() {
            "openai" => ProviderKind::Openai,
            "anthropic" => ProviderKind::Anthropic,
            "openai_compatible" => ProviderKind::OpenaiCompatible,
            "ollama" => ProviderKind::Ollama,
            "ollama_cloud" => ProviderKind::OllamaCloud,
            "llama_cpp" => ProviderKind::LlamaCpp,
            "openrouter" => ProviderKind::Openrouter,
            "tei" => ProviderKind::Tei,
            "azure_openai" => ProviderKind::AzureOpenai,
            "bedrock" => ProviderKind::Bedrock,
            "vertex" => ProviderKind::Vertex,
            "gemini" => ProviderKind::Gemini,
            "gemini_native" => ProviderKind::GeminiNative,
            "gemini_interactions" => ProviderKind::GeminiInteractions,
            "mistral" => ProviderKind::Mistral,
            "groq" => ProviderKind::Groq,
            "xai" => ProviderKind::Xai,
            "meta_llama_api" => ProviderKind::MetaLlamaApi,
            "cohere" => ProviderKind::Cohere,
            "perplexity" => ProviderKind::Perplexity,
            "together" => ProviderKind::Together,
            "fireworks" => ProviderKind::Fireworks,
            "databricks" => ProviderKind::Databricks,
            "aleph_alpha" => ProviderKind::AlephAlpha,
            "nebius" => ProviderKind::Nebius,
            "ovhcloud" => ProviderKind::Ovhcloud,
            "scaleway" => ProviderKind::Scaleway,
            "deepseek" => ProviderKind::Deepseek,
            "qwen" => ProviderKind::Qwen,
            "zhipu" => ProviderKind::Zhipu,
            "kimi" => ProviderKind::Kimi,
            "ernie" => ProviderKind::Ernie,
            "doubao" => ProviderKind::Doubao,
            "hunyuan" => ProviderKind::Hunyuan,
            "yi" => ProviderKind::Yi,
            "minimax" => ProviderKind::Minimax,
            "baichuan" => ProviderKind::Baichuan,
            "gigachat" => ProviderKind::Gigachat,
            "yandex_gpt" => ProviderKind::YandexGpt,
            "cloud_ru" => ProviderKind::CloudRu,
            "mts_ai" => ProviderKind::MtsAi,
            "naver" => ProviderKind::Naver,
            "upstage" => ProviderKind::Upstage,
            "rinna" => ProviderKind::Rinna,
            "rakuten" => ProviderKind::Rakuten,
            "sarvam" => ProviderKind::Sarvam,
            "krutrim" => ProviderKind::Krutrim,
            "falcon" => ProviderKind::Falcon,
            "system_one" => ProviderKind::SystemOne,
            "infinity" => ProviderKind::Infinity,
            // a kind from a newer release, a downgrade or a hand-written row
            // (#3469): read fail-closed as `Unknown`, which the snapshot
            // sanitizer omits with everything that named it, rather than
            // failing every tenant's config load over one row (#3347)
            other => {
                tracing::warn!(provider = %row.name, kind = other,
                    "provider kind this build does not know; the provider is not served");
                ProviderKind::Unknown
            }
        };
        let api_key = match (row.ciphertext.as_deref(), row.nonce.as_deref(), kek) {
            (Some(ciphertext), Some(nonce), Some(kek)) => match kek.decrypt(ciphertext, nonce) {
                Ok(plaintext) => Some(plaintext),
                Err(err) => {
                    tracing::warn!(provider = %row.name, error = %err,
                        "stored provider key could not be decrypted; serving provider without it");
                    None
                }
            },
            (Some(_), _, None) => {
                tracing::warn!(provider = %row.name,
                    "provider has a stored key but {} is unset; serving provider without it",
                    crypto::KEK_ENV);
                None
            }
            _ => None,
        };
        let kv_events = telemetry_setting(&row.name, "kv_events", row.kv_events);
        let lmcache = telemetry_setting(&row.name, "lmcache", row.lmcache);
        Ok(ProviderConfig {
            // for a provider `project_id` is the scope, not a home project: set,
            // it serves only that project's keys
            project_scoped: row.project_id.is_some(),
            tenancy: Some(Tenancy {
                org_id: row.org_id.to_string(),
                project_id: row.project_id.map(|id| id.to_string()),
            }),
            name: row.name,
            slug: Some(row.slug),
            kind,
            api_base: row.api_base,
            api_key,
            api_key_env: row.api_key_env,
            egress_proxy: row.egress_proxy,
            egress_proxies,
            kv_events,
            lmcache,
            ca_bundles: None,
            api_keys: Vec::new(),
            also_track_via_llm_call: false,
            llm_probe_model: None,
            status_page_url: None,
            role_profile: None,
            model_role_profiles: Default::default(),
            allow_custom_api_base: row.allow_custom_api_base,
        })
    }
}

#[derive(FromRow)]
struct RouteRow {
    id: Uuid,
    project_id: Uuid,
    org_id: Uuid,
    model: String,
    strategy: String,
    params: serde_json::Value,
    param_policy: serde_json::Value,
    advanced: serde_json::Value,
}

#[derive(FromRow)]
struct TargetRow {
    route_id: Uuid,
    provider_org_id: Uuid,
    provider_name: String,
    upstream_model: Option<String>,
    weight: i32,
    takes_over: Vec<String>,
}

#[derive(FromRow)]
struct VariantRow {
    id: Uuid,
    route_id: Uuid,
    name: String,
    weight: i32,
    params: serde_json::Value,
}

#[derive(FromRow)]
struct VariantTargetRow {
    variant_id: Uuid,
    provider_org_id: Uuid,
    provider_name: String,
    upstream_model: Option<String>,
    weight: i32,
    takes_over: Vec<String>,
}

#[derive(FromRow)]
struct ProviderGroupRow {
    id: Uuid,
    org_id: Uuid,
    name: String,
    slug: String,
    strategy: String,
    project_id: Option<Uuid>,
}

#[derive(FromRow)]
struct ProviderGroupMemberRow {
    group_id: Uuid,
    provider_org_id: Uuid,
    provider_name: String,
    upstream_model: Option<String>,
    weight: i32,
}

#[derive(FromRow)]
struct VirtualKeyRow {
    id: Uuid,
    key_hash: String,
    models: Vec<String>,
    providers: Vec<String>,
    disabled: bool,
    expires_at: Option<DateTime<Utc>>,
    cache_enabled: Option<bool>,
    project_id: Uuid,
    team_id: Uuid,
    org_id: Uuid,
    created_by: Option<Uuid>,
    business_unit_id: Option<Uuid>,
    customer_id: Option<Uuid>,
    previous_key_hash: Option<String>,
    previous_expires_at: Option<DateTime<Utc>>,
}

/// One access-profile policy row, tagged with the user it reaches. A user
/// holding several profiles produces several rows (#791).
#[derive(FromRow)]
struct OwnerPolicyRow {
    user_id: Uuid,
    allowed_models: Vec<String>,
    denied_models: Vec<String>,
    allowed_routes: Vec<String>,
    denied_routes: Vec<String>,
}

/// Map the `auth_kind` column onto the config enum.
///
/// The column's check constraint is the authority on the set, and a value
/// outside it can only mean this binary predates a kind the control plane
/// knows. That maps to [`McpAuthKind::Unknown`], which the proxy refuses
/// rather than guesses at — presenting no credential to a server that expects
/// one fails visibly, whereas picking the wrong scheme could leak the
/// credential into the wrong header.
fn parse_mcp_auth_kind(raw: &str) -> McpAuthKind {
    match raw {
        "none" => McpAuthKind::None,
        "bearer" => McpAuthKind::Bearer,
        "header" => McpAuthKind::Header,
        "oauth" => McpAuthKind::Oauth,
        _ => McpAuthKind::Unknown,
    }
}

#[derive(FromRow)]
struct McpServerSnapshotRow {
    id: Uuid,
    org_id: Uuid,
    slug: String,
    url: String,
    transport: String,
    required_scopes: Vec<String>,
    auth_kind: String,
    auth_header_name: Option<String>,
    credential_ciphertext: Option<Vec<u8>>,
    credential_nonce: Option<Vec<u8>>,
    connect_timeout_ms: Option<i32>,
    request_timeout_ms: Option<i32>,
    max_retries: Option<i32>,
    tools: Vec<String>,
    enforce_manifest_by_default: bool,
}

#[derive(FromRow)]
struct McpSettingsSnapshotRow {
    org_id: Uuid,
    connect_timeout_ms: i32,
    request_timeout_ms: i32,
    max_retries: i32,
    allow_unlisted_tools: bool,
}

#[derive(FromRow)]
struct McpToolGroupSnapshotRow {
    id: Uuid,
    org_id: Uuid,
    slug: String,
    enabled: bool,
    tools: serde_json::Value,
}

#[derive(FromRow)]
struct McpSessionSnapshotRow {
    id: Uuid,
    server_id: Uuid,
    user_id: Uuid,
    scopes: Vec<String>,
    expires_at: DateTime<Utc>,
    access_ciphertext: Vec<u8>,
    access_nonce: Vec<u8>,
}

#[derive(FromRow)]
struct PublishedPromptTemplateRow {
    template_id: Uuid,
    org_id: Uuid,
    slug: String,
    version: i32,
    variables: serde_json::Value,
    decorators: serde_json::Value,
}

#[derive(Clone, FromRow)]
struct PromptTemplateScopeRow {
    template_id: Uuid,
    version: i32,
    scope_type: String,
    scope_id: Uuid,
}

#[derive(FromRow)]
struct RouteScopeRow {
    route_id: Uuid,
    project_id: Uuid,
    model: String,
}

/// Read a stored `strategy` column back into its enum.
///
/// This defers to the same `serde` derive that writes the value, rather than
/// repeating the mapping by hand. The hand-written version silently fell behind
/// twice — `lora_aware` (#896) and `predicted_latency` (#912) were both accepted
/// by the check constraint and offered in the dashboard while this function
/// still rejected them, and because every route loads through one query, a
/// single route on a new strategy failed `/internal/snapshot` outright. The
/// whole fleet then held its last good config with nothing but a 500 to say why.
///
/// A value this build does not know, one a newer release wrote, reads as
/// [`BalancingStrategy::Unknown`] (#3347): the snapshot sanitizer then omits
/// that route and names it, and the rest of the config still loads.
fn parse_strategy(s: &str) -> Result<BalancingStrategy> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|_| Error::Store(format!("unknown balancing strategy '{s}'")))
}

/// A [`ConfigStore`] backed by Postgres. `load` composes a [`GatewayConfig`]
/// from the `providers`, `routes`/`route_targets`, `model_prices`, `virtual_keys`
/// and published `prompt_templates` tables.
///
/// Virtual keys are exposed as [`rolter_core::VirtualKeyRecord`]s carrying only
/// the one-way `key_hash` plus scope identity — never the plaintext. Since the
/// gateway authenticates by peppered digest, the stored hash is sufficient to
/// verify presented keys (the control plane must hash with the same pepper).
pub struct PostgresConfigStore {
    pool: PgPool,
    /// key-encryption key for opening sealed provider credentials; read from
    /// [`crypto::KEK_ENV`] at construction. `None` serves providers without
    /// their stored keys (with a warning)
    kek: Option<crypto::Kek>,
}

impl PostgresConfigStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            kek: crypto::Kek::from_env(),
        }
    }

    /// Construct with an explicit KEK instead of reading the environment;
    /// mainly for tests, where mutating process-wide env vars races.
    pub fn with_kek(pool: PgPool, kek: Option<crypto::Kek>) -> Self {
        Self { pool, kek }
    }

    /// The providers to serve, and the `(org, name)` of the ones left out
    /// because their `egress_proxies` could not be read (#2943). A caller that
    /// goes on to build route targets or group members from the provider rows
    /// must skip those providers, or the snapshot ends up naming a provider it
    /// does not carry. Two orgs may each have a provider of one name (#1857),
    /// so the set is keyed by the owning org too: one org's unreadable row
    /// must not take another org's namesake out of its routes (#3464).
    async fn load_providers(&self) -> Result<(Vec<ProviderConfig>, LeftOut)> {
        let rows: Vec<ProviderRow> = sqlx::query_as(
            "select p.org_id, p.name, p.slug, p.kind, p.api_base, p.api_key_env, p.egress_proxy,
                    p.egress_proxies, p.kv_events, p.lmcache, p.project_id,
                    p.allow_custom_api_base, pk.ciphertext, pk.nonce
             from providers p
             left join provider_keys pk on pk.provider_id = p.id
             order by p.name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        let mut providers = Vec::with_capacity(rows.len());
        let mut left_out = LeftOut::new();
        for row in rows {
            match egress_proxies_from_value(&row.name, &row.egress_proxies) {
                Ok(proxies) => providers.push(row.into_config(self.kek.as_ref(), proxies)?),
                Err(problem) => {
                    tracing::warn!("{problem}");
                    left_out.insert((row.org_id, row.name));
                }
            }
        }
        Ok((providers, left_out))
    }

    async fn load_feature_flags(&self) -> Result<FeatureFlags> {
        sqlx::query_as(
            "select response_cache, cache_aware_routing, circuit_breaker, active_health_checks, \
                    complexity_routing, guardrails, updated_at \
             from feature_flags where id = true",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(store_err)
    }

    async fn load_runtime_policy(&self) -> Result<RuntimePolicy> {
        sqlx::query_as(
            "select retry_max_retries, retry_base_ms, retry_max_ms, timeout_connect_s, \
                    timeout_request_s, queue_enabled, queue_capacity, queue_workers, \
                    queue_backpressure, queue_block_ms, updated_at \
             from runtime_policy where id = true",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(store_err)
    }

    async fn load_compatibility_policy(&self) -> Result<CompatibilityPolicy> {
        sqlx::query_as(
            "select anthropic_version, default_max_tokens, updated_at \
             from compatibility_policy where id = true",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(store_err)
    }

    async fn load_client_settings(&self) -> Result<ClientSettings> {
        sqlx::query_as(
            "select public_base_url, forwarded_headers, injected_headers, request_id_header, \
                    updated_at \
             from client_settings where id = true",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(store_err)
    }

    /// The policy half of `security_settings`. The ciphertext columns are not
    /// selected at all: the migration's own comment promises snapshots receive
    /// policy only, and the surest way to keep that promise is for the query
    /// never to name them (#1162).
    async fn load_security_policy(&self) -> Result<SecurityPolicyRow> {
        sqlx::query_as(
            "select required_headers, auth_bypass_routes \
             from security_settings where id = true",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(store_err)
    }

    async fn load_model_defaults(&self) -> Result<ModelDefaults> {
        sqlx::query_as(
            "select enabled, default_model, default_temperature, default_top_p, \
                    default_max_tokens, updated_at \
             from model_defaults where id = true",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(store_err)
    }

    async fn load_adaptive_routing_policy(&self) -> Result<AdaptiveRoutingPolicy> {
        sqlx::query_as(
            "select enabled, latency_weight, cost_weight, load_weight, exploration_ratio, \
                    min_samples, updated_at \
             from adaptive_routing_policy where id = true",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(store_err)
    }

    async fn load_guardrails(&self) -> Result<GuardrailsConfig> {
        let rows: Vec<GuardrailRuleRow> = sqlx::query_as(
            "select id, name, enabled, source_type, builtin, pattern, stage, action, replacement, \
                    include_system, position, created_at, updated_at \
             from guardrail_rules where enabled order by position, name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        let rules = rows
            .into_iter()
            .map(|row| {
                let builtin = match row.builtin.as_deref() {
                    Some("email") => Some(BuiltinRule::Email),
                    Some("phone") => Some(BuiltinRule::Phone),
                    Some("api_token") => Some(BuiltinRule::ApiToken),
                    Some("payment_card") => Some(BuiltinRule::PaymentCard),
                    _ => None,
                };
                GuardrailRule {
                    name: row.name,
                    builtin,
                    pattern: row.pattern,
                    stage: if row.stage == "post_call" {
                        GuardStage::PostCall
                    } else {
                        GuardStage::PreCall
                    },
                    action: match row.action.as_str() {
                        "block" => GuardAction::Block,
                        "redact" => GuardAction::Redact,
                        _ => GuardAction::Annotate,
                    },
                    replacement: row.replacement,
                    include_system: row.include_system,
                }
            })
            .collect::<Vec<_>>();
        Ok(GuardrailsConfig {
            enabled: !rules.is_empty(),
            rules,
            ..GuardrailsConfig::default()
        })
    }

    /// Every registry guardrail provider and policy, for the gateway's
    /// semantic guards (#459). Disabled providers are carried too, so a policy
    /// that names one still validates; the gateway runs enabled ones only.
    /// Disabled policies are carried for the same reason, and because a
    /// provider any policy lists is confined to policies.
    async fn load_semantic_guards(&self) -> Result<SemanticGuardsConfig> {
        let rows: Vec<GuardrailProvider> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {} from guardrail_providers order by name",
            repo::GUARDRAIL_PROVIDER_COLUMNS
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        let names: std::collections::HashMap<Uuid, String> =
            rows.iter().map(|row| (row.id, row.name.clone())).collect();
        let providers = rows.into_iter().map(provider_from_row).collect();
        let policies = repo::GuardrailRepo(&self.pool)
            .list_policies()
            .await?
            .into_iter()
            .map(|row| GuardrailPolicy {
                name: row.name,
                enabled: row.enabled,
                scope: PolicyScope::parse(&row.scope_kind).unwrap_or_default(),
                scope_id: row.scope_id,
                providers: row
                    .provider_ids
                    .iter()
                    .filter_map(|id| names.get(id).cloned())
                    .collect(),
                // the control plane validated these on the way in; a value
                // that is not a number in range is left out rather than read
                // as some other threshold
                thresholds: row
                    .thresholds
                    .as_object()
                    .map(|map| {
                        map.iter()
                            .filter_map(|(category, value)| {
                                let value = value.as_f64()?;
                                (value.is_finite() && (0.0..=1.0).contains(&value))
                                    .then(|| (category.clone(), value))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                on_violation: if row.on_violation == "annotate" {
                    ViolationAction::Annotate
                } else {
                    ViolationAction::Block
                },
                streaming: if row.streaming == "buffer" {
                    StreamingStrategy::Buffer
                } else {
                    StreamingStrategy::Reject
                },
            })
            .collect();
        Ok(SemanticGuardsConfig {
            providers,
            policies,
            // a per-gateway resource knob, set in the file only; the merge
            // keeps the file's value
            local_concurrency: None,
            // gateway-side only, never in a snapshot (#3454)
            model_root: None,
        })
    }

    /// Every enabled webhook plugin instance across every org, for the
    /// gateway's dispatch runtime (#509). Ordered so a stable snapshot needs
    /// no further sorting; [`PluginsConfig::for_stage`] re-sorts by position
    /// within the matched (org, project, stage) group regardless.
    async fn load_plugins(&self) -> Result<PluginsConfig> {
        let rows: Vec<PluginInstance> = sqlx::query_as(
            "select id, org_id, project_id, name, slug, description, kind, stage, enabled, \
                    position, failure_mode, endpoint, secret_env, config, created_at, updated_at \
             from plugin_instances where enabled and kind = 'webhook' order by org_id, position, name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        let instances = rows
            .into_iter()
            .map(|row| PluginInstanceConfig {
                slug: row.slug,
                org_id: row.org_id.to_string(),
                project_id: row.project_id.map(|id| id.to_string()),
                stage: PluginStage::from_db_str(&row.stage),
                position: row.position,
                failure_mode: if row.failure_mode == "fail_closed" {
                    FailureMode::FailClosed
                } else {
                    FailureMode::FailOpen
                },
                endpoint: row.endpoint,
                auth: row
                    .secret_env
                    .map(|token_env| WebhookAuth::Bearer { token_env }),
            })
            .collect();
        Ok(PluginsConfig { instances })
    }

    async fn load_logging_settings(&self) -> Result<LoggingSettings> {
        sqlx::query_as(
            "select sample_rate, payload_capture_enabled, payload_capture_max_bytes, \
                    payload_capture_redact_fields, payload_capture_redact_detectors, \
                    payload_capture_redact_patterns, payload_capture_models, payload_capture_virtual_key_ids, \
                    retention_days, payload_retention_hours, ui_events, updated_at \
             from logging_settings where id = true",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(store_err)
    }

    /// The enabled routes with the org they belong to, as `load_routes` and
    /// `load_problems` both read them, so the two cannot disagree about which
    /// routes exist.
    async fn enabled_route_rows(&self) -> Result<Vec<RouteRow>> {
        sqlx::query_as(
            "select r.id, r.project_id, t.org_id, r.model, r.strategy, r.params, r.param_policy,
                    r.advanced
             from routes r
             join projects p on p.id = r.project_id
             join teams t on t.id = p.team_id
             where r.enabled order by r.model",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)
    }

    /// The rows whose stored enum value this build does not know, one
    /// problem each, naming the entity, the value and what happens to the row
    /// (#3446). Since #3347 such a value no longer fails the snapshot, so
    /// without these lines the row would simply stop being served: a route on
    /// an unknown strategy and a provider of an unknown kind (#3469) are left
    /// out by the sanitizer, and a budget or rate limit on an unknown scope
    /// (#3470) is enforced against every request.
    async fn unknown_value_problems(
        &self,
        budgets: &[Budget],
    ) -> Result<Vec<crate::ConfigProblem>> {
        use crate::{ConfigProblem, ProblemOwner};
        let mut problems = Vec::new();
        for row in self.enabled_route_rows().await? {
            if parse_strategy(&row.strategy)? == BalancingStrategy::Unknown {
                problems.push(ConfigProblem {
                    kind: crate::ConfigProblemKind::Misread,
                    message: format!(
                        "route '{}' has balancing strategy '{}', which this build does not \
                         know; the route is left out of the snapshot until the control plane \
                         is upgraded or the strategy is changed",
                        row.model, row.strategy
                    ),
                    owner: ProblemOwner::Tenant(Tenancy {
                        org_id: row.org_id.to_string(),
                        project_id: Some(row.project_id.to_string()),
                    }),
                });
            }
        }
        let providers: Vec<(String, Uuid, Option<Uuid>, String)> =
            sqlx::query_as("select name, org_id, project_id, kind from providers order by name")
                .fetch_all(&self.pool)
                .await
                .map_err(store_err)?;
        for (name, org, project, kind) in providers {
            let read: ProviderKind =
                serde_json::from_value(serde_json::Value::String(kind.clone()))
                    .unwrap_or(ProviderKind::Unknown);
            if read == ProviderKind::Unknown {
                problems.push(ConfigProblem {
                    kind: crate::ConfigProblemKind::Misread,
                    message: format!(
                        "provider '{name}' has kind '{kind}', which this build does not know; \
                         it is left out of the snapshot, and so is what named it, until the \
                         control plane is upgraded or the kind is changed"
                    ),
                    owner: ProblemOwner::Tenant(Tenancy {
                        org_id: org.to_string(),
                        project_id: project.map(|p| p.to_string()),
                    }),
                });
            }
        }
        // the loader keeps these as `Unknown`, which the gateway enforces on
        // every request it serves; no known scope names a tenant to own them
        problems.extend(
            budgets
                .iter()
                .filter(|row| BudgetScope::from_scope_type(&row.scope_type).is_none())
                .map(|row| {
                    ConfigProblem::deployment(format!(
                        "budget '{}' has scope '{}', which this build does not know; until the \
                         control plane is upgraded or the budget is recreated on a known scope, \
                         every request the gateways serve, every tenant's, counts against it",
                        row.id, row.scope_type
                    ))
                }),
        );
        // the MCP proxy refuses every call through such a server (#3424)
        let servers: Vec<(String, Uuid, String)> =
            sqlx::query_as("select slug, org_id, auth_kind from mcp_servers order by slug")
                .fetch_all(&self.pool)
                .await
                .map_err(store_err)?;
        for (slug, org, auth_kind) in servers {
            if parse_mcp_auth_kind(&auth_kind) == McpAuthKind::Unknown {
                problems.push(ConfigProblem {
                    kind: crate::ConfigProblemKind::Misread,
                    message: format!(
                        "mcp server '{slug}' has auth kind '{auth_kind}', which this build does \
                         not know; every call through it is refused until the control plane is \
                         upgraded or the auth kind is changed"
                    ),
                    owner: ProblemOwner::Tenant(Tenancy {
                        org_id: org.to_string(),
                        project_id: None,
                    }),
                });
            }
        }
        let limits: Vec<(Uuid, String)> =
            sqlx::query_as("select id, scope_type from rate_limits order by created_at")
                .fetch_all(&self.pool)
                .await
                .map_err(store_err)?;
        problems.extend(
            limits
                .into_iter()
                .filter(|(_, scope)| BudgetScope::from_scope_type(scope).is_none())
                .map(|(id, scope)| {
                    ConfigProblem::deployment(format!(
                        "rate limit '{id}' has scope '{scope}', which this build does not know; \
                         until the control plane is upgraded or the limit is recreated on a known \
                         scope, every request the gateways serve, every tenant's, counts against \
                         it"
                    ))
                }),
        );
        Ok(problems)
    }

    /// `left_out` names the providers [`Self::load_providers`] did not return.
    async fn load_routes(&self, left_out: &LeftOut) -> Result<Vec<ModelRoute>> {
        Ok(self.assemble_routes(left_out).await?.0)
    }

    /// The routes [`Self::load_routes`] serves, and what it dropped from them
    /// on the way: a target whose `takes_over` does not parse, and a route left
    /// with only restricted targets (#3321). `load_problem_records` reports the
    /// second half, so the two cannot disagree about what the snapshot left
    /// out. Each problem is owned by its route's tenant.
    async fn assemble_routes(
        &self,
        left_out: &LeftOut,
    ) -> Result<(Vec<ModelRoute>, Vec<crate::ConfigProblem>)> {
        let route_rows = self.enabled_route_rows().await?;

        let target_rows: Vec<TargetRow> = sqlx::query_as(
            "select rt.route_id, p.org_id as provider_org_id, p.name as provider_name,
                    rt.upstream_model, rt.weight, rt.takes_over
             from route_targets rt
             join providers p on p.id = rt.provider_id
             order by rt.route_id, rt.created_at, rt.id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;

        // the weighted variants (#391), in declared order with their targets
        // in routing order: the gateway samples by weight and falls back
        // through the rest in that order, so the order is the routing
        let variant_rows: Vec<VariantRow> = sqlx::query_as(
            "select id, route_id, name, weight, params
             from route_variants order by route_id, position",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        let variant_target_rows: Vec<VariantTargetRow> = sqlx::query_as(
            "select vt.variant_id, p.org_id as provider_org_id, p.name as provider_name,
                    vt.upstream_model, vt.weight, vt.takes_over
             from route_variant_targets vt
             join providers p on p.id = vt.provider_id
             order by vt.variant_id, vt.position",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;

        let mut routes = Vec::with_capacity(route_rows.len());
        let mut problems = Vec::new();
        for r in route_rows {
            let tenancy = Tenancy {
                org_id: r.org_id.to_string(),
                project_id: Some(r.project_id.to_string()),
            };
            let mut route_problems = Vec::new();
            let strategy = parse_strategy(&r.strategy)?;
            // a route whose settings do not parse is left out, not served on
            // the permissive defaults: those are public visibility, no
            // allow-lists and no guardrail override (#2938)
            let settings = match route_settings_from_row(&r) {
                Ok(settings) => settings,
                Err(problem) => {
                    tracing::warn!(route = %r.model, "{problem}");
                    continue;
                }
            };
            let targets = target_rows
                .iter()
                .filter(|t| t.route_id == r.id)
                // a provider that was left out of the snapshot cannot be named
                // by a target: the dangling name would fail validation of the
                // whole snapshot, not just this route (#2943)
                .filter(|t| !is_left_out(left_out, t.provider_org_id, &t.provider_name))
                // a target on another org's provider would spend that
                // org's credential; the write path refuses one now, and a
                // row written before it did is dropped here (#1844)
                .filter(|t| {
                    let same_org = t.provider_org_id == r.org_id;
                    if !same_org {
                        tracing::warn!(route = %r.model, provider = %t.provider_name,
                            "dropping a route target on another org's provider");
                    }
                    same_org
                })
                .filter_map(|t| {
                    // a class the gateway cannot read leaves the target out
                    // rather than serving it as an ordinary member of the
                    // pool: an expensive fallback must fail closed (#1584)
                    let takes_over = match t
                        .takes_over
                        .iter()
                        .map(|token| token.parse::<FailureClass>())
                        .collect::<std::result::Result<Vec<_>, _>>()
                    {
                        Ok(classes) => classes,
                        Err(problem) => {
                            let problem = format!(
                                "route '{}' target on provider '{}' omitted from the snapshot: {problem}",
                                r.model, t.provider_name
                            );
                            tracing::warn!("{problem}");
                            route_problems.push(problem);
                            return None;
                        }
                    };
                    Some(Target {
                        provider: t.provider_name.clone(),
                        model: t.upstream_model.clone(),
                        weight: t.weight.max(0) as u32,
                        takes_over,
                    })
                })
                .collect::<Vec<_>>();
            let variants = route_variants(
                &r,
                &variant_rows,
                &variant_target_rows,
                left_out,
                &mut route_problems,
            );
            // a route left with only restricted targets could never dial
            // anything, and one such route would fail validation of the whole
            // snapshot, so it is left out instead (#1584). a route with
            // variants dials only them, so its pool does not count (#3430)
            let stranded =
                rolter_core::config::only_restricted_targets_dialled(&targets, &variants);
            if stranded {
                let problem = format!(
                    "route '{}' omitted from the snapshot: {}",
                    r.model,
                    rolter_core::failover::all_targets_restricted(&r.model)
                );
                tracing::warn!("{problem}");
                route_problems.push(problem);
            }
            problems.extend(
                route_problems
                    .into_iter()
                    .map(|message| crate::ConfigProblem {
                        kind: crate::ConfigProblemKind::Misread,
                        message,
                        owner: crate::ProblemOwner::Tenant(tenancy.clone()),
                    }),
            );
            if stranded {
                continue;
            }
            routes.push(ModelRoute {
                model: r.model,
                strategy,
                targets,
                params: settings.params,
                param_policy: settings.param_policy,
                advanced: settings.advanced,
                variants,
                // response-cache opt-in is config-only for now; a db-backed
                // cache policy lands with its own store follow-up
                cache: None,
                tenancy: Some(tenancy),
            });
        }
        Ok((routes, problems))
    }

    /// Load provider groups and their members into `ProviderGroupConfig`
    /// (ADR-0017 addendum, ADR-0022). A member with a null `upstream_model`
    /// forwards the requested model as-is.
    async fn load_provider_groups(&self, left_out: &LeftOut) -> Result<Vec<ProviderGroupConfig>> {
        let group_rows: Vec<ProviderGroupRow> = sqlx::query_as(
            "select id, org_id, name, slug, strategy, project_id from provider_groups order by name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;

        let member_rows: Vec<ProviderGroupMemberRow> = sqlx::query_as(
            "select m.group_id, p.org_id as provider_org_id, p.name as provider_name,
                    m.upstream_model, m.weight
             from provider_group_members m
             join providers p on p.id = m.provider_id
             order by m.position",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;

        group_rows
            .into_iter()
            .map(|g| {
                // validate + map the strategy the same way routes do
                let strategy = parse_strategy(&g.strategy)?;
                let members = member_rows
                    .iter()
                    .filter(|m| m.group_id == g.id)
                    // same rule as a route target: never name a provider that
                    // was left out of the snapshot (#2943)
                    .filter(|m| !is_left_out(left_out, m.provider_org_id, &m.provider_name))
                    // and never another org's provider
                    .filter(|m| {
                        let same_org = m.provider_org_id == g.org_id;
                        if !same_org {
                            tracing::warn!(group = %g.slug, provider = %m.provider_name,
                                "dropping a group member on another org's provider");
                        }
                        same_org
                    })
                    .map(|m| GroupMember {
                        provider: m.provider_name.clone(),
                        model: m.upstream_model.clone(),
                        weight: m.weight.max(1) as u32,
                    })
                    .collect();
                Ok(ProviderGroupConfig {
                    name: g.name,
                    slug: Some(g.slug),
                    strategy,
                    members,
                    project_scoped: g.project_id.is_some(),
                    tenancy: Some(Tenancy {
                        org_id: g.org_id.to_string(),
                        project_id: g.project_id.map(|id| id.to_string()),
                    }),
                })
            })
            .collect()
    }

    /// Load database-defined virtual keys with their resolved scope chain
    /// (project → team → org). Only the one-way `key_hash` is exposed; the
    /// gateway matches presented keys against it by peppered digest.
    ///
    /// A personal key (one with a creator) is served only while that person
    /// still belongs to the key's project: their account is active, and a
    /// membership or an access-profile role — assigned to them or to a team
    /// they belong to — still reaches the project, or they are a superadmin.
    /// Offboarding therefore stops their keys at the next snapshot, and
    /// reinstating them brings the same keys back, because nothing about the
    /// key itself changed (#1841). A shared key (no creator) belongs to the
    /// project and is never affected by who minted it. Deleting the account
    /// disables its keys outright (migration 0075), since the foreign key would
    /// otherwise turn them into shared ones.
    async fn load_virtual_keys(&self) -> Result<Vec<VirtualKeyRecord>> {
        let rows: Vec<VirtualKeyRow> = sqlx::query_as(
            "select vk.id, vk.key_hash, vk.models, vk.providers, vk.disabled, vk.expires_at, \
                    vk.cache_enabled, vk.project_id, p.team_id, t.org_id, vk.created_by, \
                    vk.business_unit_id, vk.customer_id, \
                    vk.previous_key_hash, vk.previous_expires_at \
             from virtual_keys vk \
             join projects p on p.id = vk.project_id \
             join teams t on t.id = p.team_id \
             left join users u on u.id = vk.created_by \
             where vk.created_by is null \
                or (u.deactivated_at is null and ( \
                    u.is_superadmin \
                    or exists (select 1 from memberships m \
                                where m.user_id = vk.created_by \
                                  and (m.org_id = t.org_id or m.team_id = p.team_id \
                                       or m.project_id = vk.project_id)) \
                    or exists (select 1 from access_profile_assignments a \
                                 join access_profile_roles apr on apr.profile_id = a.profile_id \
                                where (a.user_id = vk.created_by or a.team_id in ( \
                                           select m.team_id from memberships m \
                                            where m.user_id = vk.created_by \
                                              and m.team_id is not null \
                                           union \
                                           select mp.team_id from memberships m \
                                             join projects mp on mp.id = m.project_id \
                                            where m.user_id = vk.created_by)) \
                                  and (apr.org_id = t.org_id or apr.team_id = p.team_id \
                                       or apr.project_id = vk.project_id)))) \
             order by vk.created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;

        let owners: Vec<Uuid> = {
            let mut ids: Vec<Uuid> = rows.iter().filter_map(|r| r.created_by).collect();
            ids.sort();
            ids.dedup();
            ids
        };
        let policies = self.access_policies_for_owners(&owners).await?;

        Ok(rows
            .into_iter()
            .map(|r| VirtualKeyRecord {
                access_policy: r
                    .created_by
                    .and_then(|owner| policies.get(&owner))
                    .filter(|policy| !policy.is_unrestricted())
                    .cloned(),
                key_hash: r.key_hash,
                id: r.id.to_string(),
                org_id: r.org_id.to_string(),
                team_id: r.team_id.to_string(),
                project_id: r.project_id.to_string(),
                user_id: r.created_by.map(|id| id.to_string()).unwrap_or_default(),
                models: r.models,
                providers: r.providers,
                disabled: r.disabled,
                expires_at: r.expires_at,
                cache: r.cache_enabled,
                business_unit_id: r
                    .business_unit_id
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                customer_id: r.customer_id.map(|id| id.to_string()).unwrap_or_default(),
                previous: match (r.previous_key_hash, r.previous_expires_at) {
                    (Some(key_hash), Some(valid_until)) => Some(rolter_core::PreviousKeySecret {
                        key_hash,
                        valid_until,
                    }),
                    _ => None,
                },
            })
            .collect())
    }

    /// Merged access-profile policy for each of `owners`, keyed by user id.
    ///
    /// One query for the whole key set rather than one per key: a deployment
    /// can hold thousands of virtual keys and this runs on every snapshot
    /// build. Users with no profile are simply absent from the map, which the
    /// caller reads as unrestricted.
    ///
    /// A profile reaches a user either directly or through a team they belong
    /// to, including membership held at project level — the same two paths
    /// `AccessProfileRepo::policies_for_user` resolves for the control plane.
    async fn access_policies_for_owners(
        &self,
        owners: &[Uuid],
    ) -> Result<HashMap<Uuid, ModelPolicy>> {
        if owners.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<OwnerPolicyRow> = sqlx::query_as(
            "select distinct o.id as user_id, p.allowed_models, p.denied_models, \
                    p.allowed_routes, p.denied_routes \
             from unnest($1::uuid[]) as o(id) \
             join access_profile_assignments a \
               on a.user_id = o.id \
               or a.team_id in ( \
                   select m.team_id from memberships m \
                    where m.user_id = o.id and m.team_id is not null \
                   union \
                   select pr.team_id from memberships m \
                     join projects pr on pr.id = m.project_id \
                    where m.user_id = o.id \
               ) \
             join access_profile_policies p on p.profile_id = a.profile_id",
        )
        .bind(owners)
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;

        let mut by_owner: HashMap<Uuid, Vec<ModelPolicy>> = HashMap::new();
        for row in rows {
            by_owner.entry(row.user_id).or_default().push(ModelPolicy {
                allowed_models: row.allowed_models,
                denied_models: row.denied_models,
                allowed_routes: row.allowed_routes,
                denied_routes: row.denied_routes,
            });
        }
        Ok(by_owner
            .into_iter()
            .map(|(owner, policies)| (owner, ModelPolicy::merge(policies)))
            .collect())
    }

    /// MCP servers for the snapshot, with any static credential unsealed.
    ///
    /// The credential is sealed at rest under the KEK and the data plane needs
    /// the plaintext to present it, so it is decrypted here and travels over
    /// `/internal/snapshot` — the same route an MCP OAuth access token already
    /// takes, and the channel that already exists to carry secrets from the
    /// control plane to the gateway (#952).
    ///
    /// A row whose credential cannot be unsealed keeps its other fields and
    /// loses only the secret. The server is *not* dropped from the snapshot:
    /// the proxy refuses a `bearer`/`header` server with no credential with a
    /// message naming the problem, which beats the server silently not
    /// existing. A missing KEK is the same case for every row at once, so it is
    /// logged once rather than per server.
    async fn load_mcp_servers(&self) -> Result<Vec<McpServerConfig>> {
        let rows: Vec<McpServerSnapshotRow> = sqlx::query_as(
            "select id, org_id, slug, url, transport, required_scopes, \
                    auth_kind, auth_header_name, credential_ciphertext, credential_nonce, \
                    connect_timeout_ms, request_timeout_ms, max_retries, tools, \
                    coalesce((select o.mcp_enforce_manifests from orgs o \
                              where o.id = mcp_servers.org_id), false) \
                        as enforce_manifest_by_default \
             from mcp_servers where enabled order by org_id, slug",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        if self.kek.is_none() && rows.iter().any(|row| row.credential_ciphertext.is_some()) {
            tracing::warn!(
                "MCP servers carry static credentials but {} is unset; they are omitted from the \
                 snapshot and those servers will be refused at the proxy",
                crypto::KEK_ENV
            );
        }
        Ok(rows
            .into_iter()
            .map(|row| {
                let credential = match (
                    self.kek.as_ref(),
                    row.credential_ciphertext.as_ref(),
                    row.credential_nonce.as_ref(),
                ) {
                    (Some(kek), Some(ciphertext), Some(nonce)) => {
                        match kek.decrypt(ciphertext, nonce) {
                            Ok(plaintext) => Some(plaintext),
                            Err(error) => {
                                tracing::warn!(
                                    server_id = %row.id,
                                    error = %error,
                                    "stored MCP credential could not be decrypted; omitting it"
                                );
                                None
                            }
                        }
                    }
                    _ => None,
                };
                McpServerConfig {
                    id: row.id.to_string(),
                    org_id: row.org_id.to_string(),
                    slug: row.slug,
                    url: row.url,
                    transport: row.transport,
                    required_scopes: row.required_scopes,
                    auth_kind: parse_mcp_auth_kind(&row.auth_kind),
                    auth_header_name: row.auth_header_name,
                    credential,
                    // the check constraints in migration 0069 bound each of
                    // these to a sane range, so a negative value means the row
                    // was written around them; clamp at zero rather than
                    // wrapping into an enormous timeout
                    connect_timeout_ms: row.connect_timeout_ms.map(|ms| ms.max(0) as u64),
                    request_timeout_ms: row.request_timeout_ms.map(|ms| ms.max(0) as u64),
                    max_retries: row.max_retries.map(|n| n.max(0) as u32),
                    tools: row.tools,
                    enforce_manifest_by_default: row.enforce_manifest_by_default,
                }
            })
            .collect())
    }

    /// Enabled tool groups for the snapshot (#1403), and the disabled ones that
    /// callers are assigned to, marked `disabled` (#3295). A reference that does not parse is left
    /// out with a warning rather than failing the snapshot; the API only ever
    /// writes well-formed ones, so this is a row written around it.
    async fn load_mcp_tool_groups(&self) -> Result<Vec<McpToolGroupConfig>> {
        let rows: Vec<McpToolGroupSnapshotRow> = sqlx::query_as(
            "select id, org_id, slug, enabled, tools from mcp_tool_groups g where enabled \
             or exists (select 1 from mcp_tool_group_assignments a where a.group_id = g.id) \
             order by org_id, slug",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        // a disabled group governs no one else, but the callers assigned to it
        // stay narrowed (to none of its tools) on the servers it names, so it
        // travels with its assignments, marked disabled, rather than handing
        // them the organization-wide set or the whole server back
        let assignments: Vec<(Uuid, String, Uuid)> = sqlx::query_as(
            "select a.group_id, a.subject_kind, a.subject_id \
             from mcp_tool_group_assignments a \
             order by a.group_id, a.subject_kind, a.subject_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let assignments = assignments
                    .iter()
                    .filter(|(group_id, _, _)| *group_id == row.id)
                    .map(|(_, kind, subject)| McpToolGroupSubjectConfig {
                        kind: kind.clone(),
                        id: subject.to_string(),
                    })
                    .collect();
                let tools = match serde_json::from_value::<Vec<McpToolRefConfig>>(row.tools) {
                    Ok(tools) => tools,
                    Err(error) => {
                        tracing::warn!(
                            group_id = %row.id,
                            error = %error,
                            "MCP tool group holds references that do not parse; ignoring them"
                        );
                        Vec::new()
                    }
                };
                McpToolGroupConfig {
                    id: row.id.to_string(),
                    org_id: row.org_id.to_string(),
                    slug: row.slug,
                    tools,
                    assignments,
                    disabled: !row.enabled,
                }
            })
            .collect())
    }

    /// The organizations that have saved MCP settings. An organization with no
    /// row is left out and is served the defaults by the gateway, exactly as
    /// the settings endpoint reports it.
    async fn load_mcp_settings(&self) -> Result<Vec<McpOrgSettingsConfig>> {
        let rows: Vec<McpSettingsSnapshotRow> = sqlx::query_as(
            "select org_id, connect_timeout_ms, request_timeout_ms, max_retries, \
                    allow_unlisted_tools \
             from mcp_gateway_settings order by org_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .map(|row| McpOrgSettingsConfig {
                org_id: row.org_id.to_string(),
                // bounded by check constraints; clamp a row written around them
                connect_timeout_ms: row.connect_timeout_ms.max(0) as u64,
                request_timeout_ms: row.request_timeout_ms.max(0) as u64,
                max_retries: row.max_retries.max(0) as u32,
                // deprecated (#3527): the stored `default_failure_mode` is
                // still the API's and the export's, but the snapshot always
                // says fail closed, so a gateway that predates the change
                // refuses an unreadable body too during a rolling upgrade
                fail_open: false,
                allow_unlisted_tools: row.allow_unlisted_tools,
            })
            .collect())
    }

    async fn load_mcp_oauth_sessions(&self) -> Result<Vec<McpOAuthSessionConfig>> {
        let rows: Vec<McpSessionSnapshotRow> = sqlx::query_as(
            "select distinct on (g.server_id, g.user_id) \
                    s.id, g.server_id, g.user_id, s.scopes, s.expires_at, \
                    s.access_ciphertext, s.access_nonce \
             from mcp_oauth_sessions s \
             join mcp_oauth_grants g on g.id = s.grant_id \
             join mcp_servers srv on srv.id = g.server_id \
             where s.revoked_at is null and g.revoked_at is null \
               and s.expires_at > now() and s.scopes <@ g.scopes \
               and srv.required_scopes <@ s.scopes \
             order by g.server_id, g.user_id, s.created_at desc",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        let Some(kek) = self.kek.as_ref() else {
            if !rows.is_empty() {
                tracing::warn!(
                    "MCP OAuth sessions exist but {} is unset; omitting them from the snapshot",
                    crypto::KEK_ENV
                );
            }
            return Ok(Vec::new());
        };
        Ok(rows
            .into_iter()
            .filter_map(
                |row| match kek.decrypt(&row.access_ciphertext, &row.access_nonce) {
                    Ok(access_token) => Some(McpOAuthSessionConfig {
                        id: row.id.to_string(),
                        server_id: row.server_id.to_string(),
                        user_id: row.user_id.to_string(),
                        scopes: row.scopes,
                        expires_at: row.expires_at,
                        access_token,
                    }),
                    Err(error) => {
                        tracing::warn!(
                            session_id = %row.id,
                            error = %error,
                            "stored MCP access token could not be decrypted; omitting the session"
                        );
                        None
                    }
                },
            )
            .collect())
    }

    async fn load_model_prices(&self) -> Result<Vec<ModelPriceConfig>> {
        let rows = repo::ModelPriceRepo(&self.pool).list().await?;
        Ok(rows
            .iter()
            .filter_map(|row| match price_from_row(row) {
                Ok(price) => Some(price),
                // one unreadable price is one unpriced model, not a reason to
                // withhold every tenant's config (#2889)
                Err(problem) => {
                    tracing::warn!(model = %row.model, "{problem}");
                    None
                }
            })
            .collect())
    }

    async fn load_budgets(&self) -> Result<Vec<BudgetConfig>> {
        let rows: Vec<Budget> = sqlx::query_as(
            // limit_usd is numeric(12,4); decode it as text (Budget.limit_usd is a
            // String) or sqlx errors and the whole snapshot 500s — freezing every
            // polling gateway on its last config the moment any budget exists
            "select id, scope_type, scope_id, limit_usd::text as limit_usd, period,
                    unpriced_policy, created_at, within_org_id, within_project_id
             from budgets order by created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let scope = match r.scope_type.as_str() {
                    "org" => BudgetScope::Org,
                    "team" => BudgetScope::Team,
                    "project" => BudgetScope::Project,
                    "virtual_key" => BudgetScope::Key,
                    "business_unit" => BudgetScope::BusinessUnit,
                    "customer" => BudgetScope::Customer,
                    "user" => BudgetScope::User,
                    // a scope from a newer release, a downgrade or a hand edit
                    // (#3470): kept as `Unknown`, which the gateway enforces as
                    // binding on every request, rather than skipped, which
                    // would not enforce the cap at all (#3347)
                    _ => BudgetScope::Unknown,
                };
                BudgetConfig {
                    scope,
                    id: r.scope_id.to_string(),
                    // decimal stored as text; a malformed value disables the cap
                    // a limit that does not parse means "no effective cap", the same
                    // fail-open this had as `f64::INFINITY`; `numeric(12,4)`
                    // cannot actually produce one
                    limit_usd: r.limit_usd.parse().unwrap_or(Decimal::MAX),
                    period: parse_period(&r.period),
                    unpriced_policy: r.unpriced_policy.as_deref().and_then(parse_unpriced_policy),
                    within: scope_within(r.within_org_id, r.within_project_id),
                }
            })
            .collect())
    }

    async fn load_rate_limits(&self) -> Result<Vec<RateLimitConfig>> {
        let rows: Vec<RateLimit> = sqlx::query_as(
            "select id, scope_type, scope_id, rpm, tpm, created_at, within_org_id, within_project_id
             from rate_limits order by created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let scope = match r.scope_type.as_str() {
                    "org" => BudgetScope::Org,
                    "team" => BudgetScope::Team,
                    "project" => BudgetScope::Project,
                    "virtual_key" => BudgetScope::Key,
                    "business_unit" => BudgetScope::BusinessUnit,
                    "customer" => BudgetScope::Customer,
                    "user" => BudgetScope::User,
                    // a scope from a newer release, a downgrade or a hand edit
                    // (#3470): kept as `Unknown`, which the gateway enforces as
                    // binding on every request, rather than skipped, which
                    // would not enforce the cap at all (#3347)
                    _ => BudgetScope::Unknown,
                };
                RateLimitConfig {
                    scope,
                    id: r.scope_id.to_string(),
                    // non-positive caps are meaningless; treat them as unset
                    rpm: r.rpm.filter(|v| *v > 0).map(|v| v as u32),
                    tpm: r.tpm.filter(|v| *v > 0).map(|v| v as u32),
                    within: scope_within(r.within_org_id, r.within_project_id),
                }
            })
            .collect())
    }

    /// Load published prompt templates with tenant-aware activation bindings.
    async fn load_prompt_templates(&self) -> Result<PromptTemplatesConfig> {
        let templates: Vec<PublishedPromptTemplateRow> = sqlx::query_as(
            "select t.id as template_id, t.org_id, t.slug, v.version, v.variables, v.decorators
             from prompt_templates t
             join prompt_template_versions v
               on v.template_id = t.id
              and v.version = t.published_version
             order by t.slug",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        if templates.is_empty() {
            return Ok(PromptTemplatesConfig::default());
        }

        let scopes: Vec<PromptTemplateScopeRow> = sqlx::query_as(
            "select s.template_id, s.version, s.scope_type, s.scope_id
             from prompt_template_scopes s
             join prompt_templates t on t.id = s.template_id
             where s.version = t.published_version
             order by s.template_id, s.version, s.scope_type, s.scope_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;

        let route_rows: Vec<RouteScopeRow> = sqlx::query_as(
            "select id as route_id, project_id, model
             from routes
             where enabled",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        let route_identity: HashMap<Uuid, (Uuid, String)> = route_rows
            .iter()
            .map(|route| (route.route_id, (route.project_id, route.model.clone())))
            .collect();

        let mut scopes_by_template: HashMap<(Uuid, i32), Vec<PromptTemplateScopeRow>> =
            HashMap::new();
        for scope in scopes {
            scopes_by_template
                .entry((scope.template_id, scope.version))
                .or_default()
                .push(scope);
        }

        let mut compiled = Vec::new();
        for row in templates {
            if row.version <= 0 {
                tracing::warn!(
                    template_id = %row.template_id,
                    version = row.version,
                    "skipping prompt template with non-positive published version"
                );
                continue;
            }
            let variables: Vec<TemplateVariable> = match serde_json::from_value(row.variables) {
                Ok(variables) => variables,
                Err(err) => {
                    tracing::warn!(
                        template_id = %row.template_id,
                        error = %err,
                        "skipping prompt template with invalid variables json"
                    );
                    continue;
                }
            };
            let decorators: Vec<Decorator> = match serde_json::from_value(row.decorators) {
                Ok(decorators) => decorators,
                Err(err) => {
                    tracing::warn!(
                        template_id = %row.template_id,
                        error = %err,
                        "skipping prompt template with invalid decorators json"
                    );
                    continue;
                }
            };
            if decorators.is_empty() {
                tracing::warn!(
                    template_id = %row.template_id,
                    "skipping prompt template with empty decorators"
                );
                continue;
            }

            let scope_rows = scopes_by_template
                .get(&(row.template_id, row.version))
                .cloned()
                .unwrap_or_default();
            if scope_rows.is_empty() {
                tracing::warn!(
                    template_id = %row.template_id,
                    "skipping published prompt template without scope bindings"
                );
                continue;
            }

            let mut activations = Vec::with_capacity(scope_rows.len());
            for scope in scope_rows {
                match scope.scope_type.as_str() {
                    "org" => activations.push(PromptTemplateActivationScope::Org {
                        id: scope.scope_id.to_string(),
                    }),
                    "project" => activations.push(PromptTemplateActivationScope::Project {
                        id: scope.scope_id.to_string(),
                    }),
                    "route" => {
                        if let Some((project_id, model)) = route_identity.get(&scope.scope_id) {
                            activations.push(PromptTemplateActivationScope::Route {
                                project_id: project_id.to_string(),
                                model: model.clone(),
                            });
                        }
                    }
                    "virtual_key" => activations.push(PromptTemplateActivationScope::VirtualKey {
                        id: scope.scope_id.to_string(),
                    }),
                    other => {
                        tracing::warn!(
                            template_id = %row.template_id,
                            scope_type = other,
                            "unknown prompt-template scope type skipped"
                        );
                    }
                }
            }

            if activations.is_empty() {
                tracing::warn!(
                    template_id = %row.template_id,
                    "skipping prompt template because no resolvable route scopes were found"
                );
                continue;
            }

            compiled.push(PromptTemplate {
                id: format!("{}:{}", row.org_id, row.slug),
                version: row.version as u32,
                routes: Vec::new(),
                scopes: activations,
                variables,
                decorators,
            });
        }
        compiled.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.version.cmp(&right.version))
        });

        Ok(PromptTemplatesConfig {
            enabled: !compiled.is_empty(),
            templates: compiled,
        })
    }
}

/// The jsonb columns of one `routes` row, read into the config types.
struct RouteSettings {
    params: HashMap<String, serde_json::Value>,
    param_policy: ParamPolicy,
    advanced: AdvancedModelConfig,
}

/// Read a `routes` row's jsonb columns into the config types, or say which one
/// cannot be read.
///
/// The control plane parses every write through these same types, so a row that
/// fails here was written around it (SQL, a seed) or no longer fits a type that
/// changed since. Reading it as the type's default, as this used to, is not a
/// neutral fallback, since every default here is the permissive one (#2938):
///
/// - `advanced` carries who may see the route (`visibility`: `project_only`, the
///   team, key and user allow-lists), the guardrail overrides and the route's
///   limits, and `AdvancedModelConfig::default()` is public, unrestricted and
///   unlimited
/// - `param_policy` defaults to `allow`, so callers could override every admin
///   default in `params`
/// - `params` carries those admin defaults (a `max_tokens` that bounds spend, a
///   pinned `temperature`), and an empty map injects none of them
///
/// Each therefore fails closed the same way: the route is left out of the
/// snapshot, so its model answers as unknown until the row is repaired, rather
/// than being served wide open. That is the same shape as a route with no usable
/// target (#926) and as a price that is not a number (#2889), and the problem
/// line names the column so the operator knows which to fix.
fn route_settings_from_row(row: &RouteRow) -> std::result::Result<RouteSettings, String> {
    use serde::Deserialize;

    // deserializing from the borrowed value keeps the load path from cloning
    // each blob
    fn column<'de, T: Deserialize<'de>>(
        row: &'de RouteRow,
        name: &str,
        value: &'de serde_json::Value,
    ) -> std::result::Result<T, String> {
        T::deserialize(value).map_err(|error| {
            format!(
                "route '{}' ({}) has a {name} value that does not parse ({error}), so the route \
                 is left out of the snapshot and its model is not served; repair the {name} \
                 value or delete the route",
                row.model, row.id
            )
        })
    }
    let param_policy: ParamPolicy = column(row, "param_policy", &row.param_policy)?;
    // a mode this build does not know decodes as `Unknown` rather than failing
    // (#3347); the store still reads it as a row it cannot read, so it is left
    // out and named like any other
    if param_policy.mode == rolter_core::OverrideMode::Unknown {
        return Err(format!(
            "route '{}' ({}) has a param_policy value that does not parse (a mode this build \
             does not know), so the route is left out of the snapshot and its model is not \
             served; repair the param_policy value or delete the route",
            row.model, row.id
        ));
    }
    Ok(RouteSettings {
        params: column(row, "params", &row.params)?,
        param_policy,
        advanced: column(row, "advanced", &row.advanced)?,
    })
}

/// The variants of route `r` as the gateway reads them (#391), built from the
/// rows `load_routes` read for every route.
///
/// A variant target drops out on the same rules as a route target: a provider
/// left out of the snapshot, or one of another org. A variant those rules
/// leave with no target is dropped whole, as `GatewayConfig`'s own prune does,
/// since validation reads a targetless variant as a defect of the snapshot.
/// The params column is held to a json object by a check constraint
/// (`0138`), and any object reads as a param map, so it cannot fail to parse.
///
/// A target whose stored `takes_over` does not parse is left out, as on the
/// classic pool, with a line in `problems` (#3373).
fn route_variants(
    r: &RouteRow,
    variant_rows: &[VariantRow],
    target_rows: &[VariantTargetRow],
    left_out: &LeftOut,
    problems: &mut Vec<String>,
) -> Vec<rolter_core::Variant> {
    use serde::Deserialize;

    let mut variants = Vec::new();
    for v in variant_rows.iter().filter(|v| v.route_id == r.id) {
        let params =
            HashMap::<String, serde_json::Value>::deserialize(&v.params).unwrap_or_default();
        let targets: Vec<Target> = target_rows
            .iter()
            .filter(|t| t.variant_id == v.id)
            .filter(|t| !is_left_out(left_out, t.provider_org_id, &t.provider_name))
            .filter(|t| {
                let same_org = t.provider_org_id == r.org_id;
                if !same_org {
                    tracing::warn!(route = %r.model, variant = %v.name,
                        provider = %t.provider_name,
                        "dropping a variant target on another org's provider");
                }
                same_org
            })
            .filter_map(|t| {
                // a class the gateway cannot read leaves the target out rather
                // than serving it as an ordinary member (#1584, #3373)
                let takes_over = match t
                    .takes_over
                    .iter()
                    .map(|token| token.parse::<FailureClass>())
                    .collect::<std::result::Result<Vec<_>, _>>()
                {
                    Ok(classes) => classes,
                    Err(problem) => {
                        let problem = format!(
                            "route '{}' variant '{}' target on provider '{}' omitted from the \
                             snapshot: {problem}",
                            r.model, v.name, t.provider_name
                        );
                        tracing::warn!("{problem}");
                        problems.push(problem);
                        return None;
                    }
                };
                Some(Target {
                    provider: t.provider_name.clone(),
                    model: t.upstream_model.clone(),
                    weight: t.weight.max(0) as u32,
                    takes_over,
                })
            })
            .collect();
        if targets.is_empty() {
            tracing::warn!(route = %r.model, variant = %v.name,
                "dropping a variant with no target the snapshot can serve");
            continue;
        }
        variants.push(rolter_core::Variant {
            name: v.name.clone(),
            weight: v.weight.max(0) as u32,
            targets,
            params,
        });
    }
    variants
}

/// What kind of JSON value this is, for a problem line that must not echo it:
/// a stored value can carry a proxy URL with credentials or a shared secret.
fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Read a provider's `kv_events` or `lmcache` column. These only feed the
/// cache-aware scorers, so a value that cannot be read costs the provider its
/// telemetry (the strategy falls back to its other signals), not its place in
/// the snapshot, and the log says which row to fix.
fn telemetry_setting<T: serde::de::DeserializeOwned>(
    provider: &str,
    column: &str,
    value: Option<serde_json::Value>,
) -> Option<T> {
    match serde_json::from_value(value?) {
        Ok(setting) => Some(setting),
        Err(err) => {
            tracing::warn!(provider, column, error = %err,
                "stored provider setting could not be read; serving provider without it");
            None
        }
    }
}

/// Read `providers.egress_proxies` into the provider's proxy pool, or say why
/// it cannot be read.
///
/// The column is a `jsonb` array, and the migration that added it checks only
/// that (0018). An array holding anything but strings used to load as an empty
/// pool, which is the "no proxy configured" reading: the provider fell back to
/// its single `egress_proxy`, or to a direct connection, so traffic the operator
/// had forced through a proxy left the network without it (#2943). The default
/// is the permissive one, so it gets the answer routes got in #2938: the
/// provider is left out of the snapshot and its routes do not use it, which an
/// operator can see, rather than reaching the upstream around the pool. The
/// control plane writes the column from a `Vec<String>`, so reaching this takes
/// a row written by SQL, a seed or an import, and migration 0086 refuses it from
/// then on.
fn egress_proxies_from_value(
    provider: &str,
    value: &serde_json::Value,
) -> std::result::Result<Vec<String>, String> {
    let unreadable = |detail: String| {
        format!(
            "provider '{provider}' has an egress_proxies value that cannot be read ({detail}), so \
             the provider is left out of the snapshot and nothing is routed to it, rather than \
             reaching its upstream without the proxy pool; set egress_proxies to a list of proxy \
             URLs or delete the provider"
        )
    };
    let Some(entries) = value.as_array() else {
        return Err(unreadable(format!(
            "it is {}, not a list",
            json_kind(value)
        )));
    };
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            entry.as_str().map(str::to_string).ok_or_else(|| {
                unreadable(format!(
                    "entry {index} is {}, not a string",
                    json_kind(entry)
                ))
            })
        })
        .collect()
}

/// Read `security_settings.required_headers` into the ingress rule, or say why
/// it cannot be read.
///
/// This is a gate: a request that does not carry every header here, with its
/// exact value, is refused before authentication. The column is a `jsonb` with
/// no shape check (0021), and the loader used to read what it could and drop the
/// rest, so a value that was not an object loaded as no rule at all and an entry
/// whose value was not a string vanished from the rule, with nothing said
/// anywhere (#2943). Every one of those turns a refusal the operator configured
/// into an admission.
///
/// So an unreadable value is an error, and `load` returns it: no snapshot is
/// built, none is published, and a gateway keeps the config it last received,
/// gate included. Neither answer that keeps serving is safe. Reading the value
/// as no rule opens the deployment, and guessing a value locks it, for every
/// tenant, on a reading nobody wrote. The line names the setting and each entry
/// at fault but never a value, since an operator may well have put a shared
/// secret there.
///
/// A value counts as readable when it is the shape the API writes: an object
/// whose names are header names and whose values are strings that are not blank.
/// A name that differs only in case or padding from another is the same header,
/// and two values for one header is a rule that cannot be read either.
fn required_headers_from_value(
    value: &serde_json::Value,
) -> std::result::Result<BTreeMap<String, String>, String> {
    let unreadable = |detail: String| {
        format!(
            "security setting required_headers cannot be read ({detail}), so no config snapshot \
             is published and gateways keep the config they last received: dropping the rule \
             would admit requests it exists to refuse. Replace it with PUT \
             /api/v1/security-settings, or in the dashboard under Security"
        )
    };
    let Some(object) = value.as_object() else {
        return Err(unreadable(format!(
            "it is {}, not an object of header names to values",
            json_kind(value)
        )));
    };
    let mut headers = BTreeMap::new();
    let mut reasons = Vec::new();
    for (name, value) in object {
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() {
            reasons.push("an entry has an empty header name".to_string());
            continue;
        }
        let Some(value) = value.as_str().filter(|value| !value.trim().is_empty()) else {
            reasons.push(format!(
                "header '{name}' has a value that is not a non-empty string"
            ));
            continue;
        };
        if headers
            .insert(name.clone(), value.to_string())
            .is_some_and(|earlier| earlier != value)
        {
            reasons.push(format!(
                "header '{name}' is listed twice with different values"
            ));
        }
    }
    if reasons.is_empty() {
        Ok(headers)
    } else {
        Err(unreadable(reasons.join("; ")))
    }
}

/// Read `client_settings.injected_headers`, keeping what is readable and saying
/// what was not.
///
/// Unlike [`required_headers_from_value`] this does not fail the load. An
/// injected header is added to a request on its way to the upstream and gates
/// nothing, so serving without the one that cannot be read is a smaller
/// service, not a wider one; what #2943 asked of it is that the omission is
/// reported rather than silent.
fn injected_headers_from_value(
    value: &serde_json::Value,
) -> (BTreeMap<String, String>, Option<String>) {
    let report = |detail: String| {
        format!(
            "client setting injected_headers has {detail}, so they are not injected; save the \
             client settings again to repair them"
        )
    };
    let Some(object) = value.as_object() else {
        return (
            BTreeMap::new(),
            Some(report(format!(
                "a value that is {}, not an object of header names to values",
                json_kind(value)
            ))),
        );
    };
    let mut headers = BTreeMap::new();
    let mut unreadable = Vec::new();
    for (name, value) in object {
        let name = name.trim().to_ascii_lowercase();
        match value.as_str() {
            Some(value) if !name.is_empty() => {
                headers.insert(name, value.to_string());
            }
            _ => unreadable.push(format!("'{name}'")),
        }
    }
    let problem = (!unreadable.is_empty()).then(|| {
        report(format!(
            "entries that are not a header name with a string value ({})",
            unreadable.join(", ")
        ))
    });
    (headers, problem)
}

/// Read one `model_prices` row into the config type, or say why it cannot be.
///
/// The columns are `numeric(12,6)` cast to text and parsed straight into
/// [`Decimal`], so the exactness the column was chosen for is kept (#967). A
/// `numeric` can also hold `NaN`, which a [`Decimal`] cannot, and the control
/// plane accepted it until #2889. Reading that as zero, as this used to, turned
/// a priced model into a free one without a word; the row is left out instead,
/// which makes the model unpriced, the state `unpriced_policy` has an answer for.
/// A negative rate does parse and is left for
/// [`GatewayConfig::sanitize_for_snapshot`] to omit with its own problem line.
fn price_from_row(row: &ModelPrice) -> std::result::Result<ModelPriceConfig, String> {
    let rate = |field: &str, text: &str| {
        text.parse::<Decimal>().map_err(|_| {
            format!(
                "model price '{}' has a {field} of '{text}', which is not a number a rate can be, \
                 so the model is left out of the snapshot and unpriced; set the rate to a number \
                 or delete the price",
                row.model
            )
        })
    };
    let optional = |field: &str, text: &Option<String>| match text {
        Some(text) => rate(field, text).map(Some),
        None => Ok(None),
    };
    Ok(ModelPriceConfig {
        model: row.model.clone(),
        input_per_mtok: rate("input_per_mtok", &row.input_per_mtok)?,
        output_per_mtok: rate("output_per_mtok", &row.output_per_mtok)?,
        cached_input_per_mtok: optional("cached_input_per_mtok", &row.cached_input_per_mtok)?,
        cache_write_per_mtok: optional("cache_write_per_mtok", &row.cache_write_per_mtok)?,
        cache_write_1h_per_mtok: optional("cache_write_1h_per_mtok", &row.cache_write_1h_per_mtok)?,
        image_per_unit: optional("image_per_unit", &row.image_per_unit)?,
        audio_input_per_minute: optional("audio_input_per_minute", &row.audio_input_per_minute)?,
        speech_per_mchar: optional("speech_per_mchar", &row.speech_per_mchar)?,
        audio_input_per_mtok: optional("audio_input_per_mtok", &row.audio_input_per_mtok)?,
        audio_output_per_mtok: optional("audio_output_per_mtok", &row.audio_output_per_mtok)?,
        cached_audio_input_per_mtok: optional(
            "cached_audio_input_per_mtok",
            &row.cached_audio_input_per_mtok,
        )?,
        // the column has always existed and the dashboard has always written
        // it; it just never reached the config (#650), so every non-USD price
        // was charged as if it were USD
        currency: row.currency.clone(),
    })
}

/// Map the free-text `budgets.period` column to a [`BudgetPeriod`]. Accepts both
/// the human names and the duration shorthands (`1d`, `30d`), defaulting to
/// monthly for anything unrecognized.
///
/// The fallback stays for rows written before the control plane checked the
/// value (#1902): failing the load would withhold every tenant's config over
/// one row, and moving an existing budget onto another window is not a guess
/// to make on the operator's behalf. [`unrecognised_budget_period`] is what
/// says it happened.
/// The place a `user` cap is narrowed to, as the gateway matches it (#1830).
fn scope_within(org: Option<Uuid>, project: Option<Uuid>) -> Option<ScopeWithin> {
    match (org, project) {
        (Some(org), None) => Some(ScopeWithin::Org(org.to_string())),
        (None, Some(project)) => Some(ScopeWithin::Project(project.to_string())),
        // the table's check constraint refuses both, so this is "neither"
        _ => None,
    }
}

fn parse_period(period: &str) -> BudgetPeriod {
    BudgetPeriod::parse(period).unwrap_or(BudgetPeriod::Monthly)
}

/// One config problem per budget whose stored `period` the gateway does not
/// recognise, and so enforces as monthly (#1902).
///
/// Worded like the other lines `GET /api/v1/config/problems` reports, naming
/// the row by id and scope, since two budgets on one scope differ by nothing
/// else an operator can search for.
fn unrecognised_budget_period(row: &Budget) -> String {
    let accepted = BudgetPeriod::SPELLINGS
        .iter()
        .map(|(spelling, _)| *spelling)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "budget '{}' on {} '{}' has period '{}', which the gateway does not \
         recognise, so it is enforced as a monthly cap; set it to one of {accepted}",
        row.id, row.scope_type, row.scope_id, row.period
    )
}

/// Map the `budgets.unpriced_policy` column to an [`UnpricedPolicy`] override.
///
/// Unlike [`parse_period`], an unrecognized value yields `None` rather than a
/// default: `None` means "inherit the deployment-wide setting", which is the
/// conservative reading. Guessing a policy here would let a typo silently
/// decide whether unaccountable traffic is served, and the column carries a
/// check constraint precisely so this branch stays unreachable in practice.
fn parse_unpriced_policy(value: &str) -> Option<UnpricedPolicy> {
    match value.trim().to_ascii_lowercase().as_str() {
        "ignore" => Some(UnpricedPolicy::Ignore),
        "warn" => Some(UnpricedPolicy::Warn),
        "block" => Some(UnpricedPolicy::Block),
        _ => None,
    }
}

/// A registry provider row as the gateway's provider config.
fn provider_from_row(row: GuardrailProvider) -> GuardrailWebhookConfig {
    let auth = match (row.auth_kind.as_str(), row.auth_env) {
        ("bearer", Some(token_env)) => Some(WebhookAuth::Bearer { token_env }),
        ("shared_secret", Some(secret_env)) => Some(WebhookAuth::SharedSecret { secret_env }),
        _ => None,
    };
    GuardrailWebhookConfig {
        enabled: row.enabled,
        name: row.name,
        // a kind this build cannot parse reaches the gateway as one it does
        // not know, which fails closed there (ADR-0039)
        kind: GuardProviderKind::parse(&row.kind).unwrap_or(GuardProviderKind::Unknown),
        url: row.url,
        model_path: row.model_path.unwrap_or_default(),
        model_sha256: row.model_sha256.unwrap_or_default(),
        stage: if row.stage == "post_call" {
            WebhookStage::PostCall
        } else {
            WebhookStage::PreCall
        },
        timeout_ms: row.timeout_ms.max(1) as u64,
        max_retries: row.max_retries.max(0) as u32,
        failure_mode: if row.failure_mode == "fail_closed" {
            FailureMode::FailClosed
        } else {
            FailureMode::FailOpen
        },
        max_body_bytes: row.max_body_bytes.max(1) as usize,
        // not stored: a registry provider scores the default budget and
        // flags a cut without refusing it; only a config file sets these
        // (#3513)
        max_scored_bytes: None,
        on_truncated: rolter_core::OnTruncated::default(),
        auth,
    }
}

/// Read the current global config version. Bumping happens in the database
/// itself: migration 0003 installs triggers that increment the version
/// atomically with any write to providers/routes/route_targets/virtual_keys.
pub async fn current_version(pool: &PgPool) -> Result<i64> {
    sqlx::query_scalar("select version from config_version where id = 1")
        .fetch_one(pool)
        .await
        .map_err(store_err)
}

#[async_trait]
impl ConfigStore for PostgresConfigStore {
    async fn load(&self) -> Result<GatewayConfig> {
        let flags = self.load_feature_flags().await?;
        let runtime_policy = self.load_runtime_policy().await?;
        let logging = self.load_logging_settings().await?;
        let compatibility = self.load_compatibility_policy().await?;
        let client_settings = self.load_client_settings().await?;
        let security = self.load_security_policy().await?;
        // read before anything else is built: a gate that cannot be read means
        // there is no config worth assembling (see `required_headers_from_value`)
        let required_headers =
            required_headers_from_value(&security.required_headers).map_err(Error::Store)?;
        let model_defaults = self.load_model_defaults().await?;
        let adaptive = self.load_adaptive_routing_policy().await?;
        let guardrails = self.load_guardrails().await?;
        let semantic_guards = self.load_semantic_guards().await?;
        let (providers, left_out) = self.load_providers().await?;
        let routes = self.load_routes(&left_out).await?;
        let provider_groups = self.load_provider_groups(&left_out).await?;
        let model_prices = self.load_model_prices().await?;
        let db_virtual_keys = self.load_virtual_keys().await?;
        let mcp_servers = self.load_mcp_servers().await?;
        let mcp_oauth_sessions = self.load_mcp_oauth_sessions().await?;
        let mcp_tool_groups = self.load_mcp_tool_groups().await?;
        let mcp_settings = self.load_mcp_settings().await?;
        let budgets = self.load_budgets().await?;
        let rate_limits = self.load_rate_limits().await?;
        let prompt_templates = self.load_prompt_templates().await?;
        let plugins = self.load_plugins().await?;
        let mut config = GatewayConfig {
            providers,
            left_out_providers: left_out
                .into_iter()
                .map(|(org, name)| (org.to_string(), name))
                .collect(),
            routes,
            provider_groups,
            model_prices,
            db_virtual_keys,
            mcp_servers,
            mcp_oauth_sessions,
            mcp_tool_groups,
            mcp_settings,
            budgets,
            rate_limits,
            prompt_templates,
            guardrails,
            semantic_guards,
            plugins,
            feature_flags: FeatureFlagsConfig {
                response_cache: flags.response_cache,
                cache_aware_routing: flags.cache_aware_routing,
                circuit_breaker: flags.circuit_breaker,
                active_health_checks: flags.active_health_checks,
                complexity_routing: flags.complexity_routing,
                guardrails: flags.guardrails,
            },
            ..GatewayConfig::default()
        };
        config.apply_feature_flags();
        logging.project_onto(&mut config.logging);
        config.retry.max_retries = runtime_policy.retry_max_retries.max(0) as u32;
        config.retry.base_backoff_ms = runtime_policy.retry_base_ms.max(0) as u64;
        config.retry.max_backoff_ms = runtime_policy.retry_max_ms.max(0) as u64;
        config.timeouts.connect_secs = runtime_policy.timeout_connect_s.max(0) as u64;
        config.timeouts.request_secs = runtime_policy.timeout_request_s.max(0) as u64;
        config.queue.enabled = runtime_policy.queue_enabled;
        config.queue.capacity = runtime_policy.queue_capacity.max(1) as usize;
        config.queue.workers = runtime_policy.queue_workers.max(1) as usize;
        config.queue.backpressure = match runtime_policy.queue_backpressure.as_str() {
            "drop" => BackpressurePolicy::Drop,
            "block" => BackpressurePolicy::Block,
            _ => BackpressurePolicy::Error,
        };
        config.queue.block_timeout_ms = runtime_policy.queue_block_ms.max(0) as u64;
        config.compatibility.anthropic_version = compatibility.anthropic_version;
        config.compatibility.default_max_tokens = compatibility.default_max_tokens.max(1) as u32;
        // injected headers add to a request and gate nothing, so an unreadable
        // value degrades to injecting fewer of them, said out loud
        let (injected_headers, injected_problem) =
            injected_headers_from_value(&client_settings.injected_headers);
        if let Some(problem) = injected_problem {
            tracing::warn!("{problem}");
        }
        config.client = rolter_core::ClientConfig {
            public_base_url: client_settings.public_base_url,
            forwarded_headers: client_settings
                .forwarded_headers
                .iter()
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .collect(),
            injected_headers,
            request_id_header: client_settings.request_id_header.to_ascii_lowercase(),
        };
        config.security = rolter_core::SecurityPolicyConfig {
            required_headers,
            auth_bypass_routes: security
                .auth_bypass_routes
                .into_iter()
                .map(|route| route.trim().to_string())
                .filter(|route| !route.is_empty())
                .collect(),
        };
        config.model_defaults = rolter_core::ModelDefaultsConfig {
            enabled: model_defaults.enabled,
            default_model: model_defaults.default_model,
            temperature: model_defaults.default_temperature,
            top_p: model_defaults.default_top_p,
            max_tokens: model_defaults.default_max_tokens.map(|v| v.max(1) as u32),
        };
        config.adaptive_routing = rolter_core::AdaptiveRoutingConfig {
            enabled: adaptive.enabled,
            latency_weight: adaptive.latency_weight,
            cost_weight: adaptive.cost_weight,
            load_weight: adaptive.load_weight,
            exploration_ratio: adaptive.exploration_ratio,
            min_samples: adaptive.min_samples.max(0) as u32,
        }
        .sanitized();
        Ok(config)
    }

    async fn save(&self, _config: GatewayConfig) -> Result<()> {
        Err(Error::Store(
            "PostgresConfigStore is read-only; use the control-plane CRUD API to mutate providers/routes"
                .into(),
        ))
    }

    async fn current_version(&self) -> Result<i64> {
        sqlx::query_scalar("select version from config_version where id = 1")
            .fetch_one(&self.pool)
            .await
            .map_err(store_err)
    }

    async fn load_problems(&self) -> Result<Vec<String>> {
        Ok(self
            .load_problem_records()
            .await?
            .into_iter()
            .map(|problem| problem.message)
            .collect())
    }

    async fn load_problem_records(&self) -> Result<Vec<crate::ConfigProblem>> {
        use crate::{ConfigProblem, ProblemOwner};
        let budgets: Vec<Budget> = sqlx::query_as(
            "select id, scope_type, scope_id, limit_usd::text as limit_usd, period,
                    unpriced_policy, created_at, within_org_id, within_project_id
             from budgets order by created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        let mut problems: Vec<ConfigProblem> = budgets
            .iter()
            .filter(|row| BudgetPeriod::parse(&row.period).is_none())
            .map(|row| ConfigProblem {
                kind: crate::ConfigProblemKind::Misread,
                message: unrecognised_budget_period(row),
                owner: match BudgetScope::from_scope_type(&row.scope_type) {
                    Some(scope) => ProblemOwner::Scope {
                        scope,
                        id: row.scope_id.to_string(),
                    },
                    // a scope the gateway does not enforce names no tenant
                    None => ProblemOwner::Deployment,
                },
            })
            .collect();
        let prices = repo::ModelPriceRepo(&self.pool).list().await?;
        // prices are deployment-wide: the table has no owner
        problems.extend(
            prices
                .iter()
                .filter_map(|row| price_from_row(row).err())
                .map(ConfigProblem::deployment),
        );
        // a route left out because its settings would not parse, which the
        // snapshot cannot say since it never sees the row (#2938)
        problems.extend(self.enabled_route_rows().await?.iter().filter_map(|row| {
            route_settings_from_row(row)
                .err()
                .map(|message| ConfigProblem {
                    kind: crate::ConfigProblemKind::Misread,
                    message,
                    owner: ProblemOwner::Tenant(rolter_core::Tenancy {
                        org_id: row.org_id.to_string(),
                        project_id: Some(row.project_id.to_string()),
                    }),
                })
        }));
        // the security gate and the egress pools (#2943). a gate that cannot be
        // read makes `load` fail outright, so this is the one place an operator
        // can read why
        problems.extend(
            required_headers_from_value(&self.load_security_policy().await?.required_headers)
                .err()
                .map(ConfigProblem::deployment),
        );
        problems.extend(
            injected_headers_from_value(&self.load_client_settings().await?.injected_headers)
                .1
                .into_iter()
                .map(ConfigProblem::deployment),
        );
        let proxies: Vec<(String, Uuid, Option<Uuid>, serde_json::Value)> = sqlx::query_as(
            "select name, org_id, project_id, egress_proxies from providers order by name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        let mut left_out = LeftOut::new();
        problems.extend(proxies.iter().filter_map(|(name, org, project, value)| {
            egress_proxies_from_value(name, value).err().map(|message| {
                // the provider `load_providers` leaves out for it
                left_out.insert((*org, name.clone()));
                ConfigProblem {
                    kind: crate::ConfigProblemKind::Misread,
                    message,
                    owner: ProblemOwner::Tenant(rolter_core::Tenancy {
                        org_id: org.to_string(),
                        project_id: project.map(|p| p.to_string()),
                    }),
                }
            })
        }));
        // a stored value from a newer release, a downgrade or a hand-written
        // row that this build reads as `Unknown` (#3446). a `takes_over` class
        // is reported with its target below, and a budget period above
        problems.extend(self.unknown_value_problems(&budgets).await?);
        // a target or route the snapshot drops for `takes_over` (#3321). a
        // failure to read them is itself the problem to report: failing the
        // whole list would hide every other explanation with it
        match self.assemble_routes(&left_out).await {
            Ok((_, dropped)) => problems.extend(dropped),
            Err(error) => {
                tracing::warn!("the routes could not be read for their problems: {error}");
                problems.push(ConfigProblem::deployment(format!(
                    "the routes could not be read, so a target or route left out of the \
                     snapshot is not listed here: {error}"
                )));
            }
        }
        Ok(problems)
    }
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

    /// Every strategy the enum has, as the string stored in the `strategy`
    /// column.
    ///
    /// The `match` is what makes this maintainable: it is exhaustive, so adding
    /// a `BalancingStrategy` variant fails to compile here until the new wire
    /// value is written down, and the test below then proves it round-trips.
    /// Requires no database, so it guards the read path even on a checkout with
    /// `ROLTER_TEST_DATABASE_URL` unset.
    fn wire_name(s: BalancingStrategy) -> &'static str {
        match s {
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
            // never stored: it stands for a value a newer release wrote (#3347)
            BalancingStrategy::Unknown => unreachable!("Unknown has no column value"),
        }
    }

    const ALL_STRATEGIES: &[BalancingStrategy] = &[
        BalancingStrategy::RoundRobin,
        BalancingStrategy::Random,
        BalancingStrategy::PowerOfTwo,
        BalancingStrategy::ConsistentHash,
        BalancingStrategy::CacheAware,
        BalancingStrategy::Weighted,
        BalancingStrategy::Pipeline,
        BalancingStrategy::Cheapest,
        BalancingStrategy::Fastest,
        BalancingStrategy::PreciseCacheAware,
        BalancingStrategy::LmcacheAware,
        BalancingStrategy::Adaptive,
        BalancingStrategy::LoraAware,
        BalancingStrategy::PredictedLatency,
    ];

    // `lora_aware` (#896) and `predicted_latency` (#912) both shipped a check
    // constraint and a dashboard option while `parse_strategy` still rejected
    // them. Every route loads through one query, so one route on a new strategy
    // failed `/internal/snapshot` for the whole fleet.
    #[test]
    fn every_strategy_the_column_can_hold_parses_back() {
        for &s in ALL_STRATEGIES {
            let stored = wire_name(s);
            let parsed = parse_strategy(stored)
                .unwrap_or_else(|e| panic!("stored strategy '{stored}' does not parse: {e}"));
            assert_eq!(parsed, s, "'{stored}' parsed to the wrong variant");
        }
    }

    #[test]
    fn the_serialized_form_is_the_name_the_column_stores() {
        for &s in ALL_STRATEGIES {
            let json = serde_json::to_string(&s).expect("strategy serializes");
            assert_eq!(json, format!("\"{}\"", wire_name(s)));
        }
    }

    /// A strategy a newer release stored reads as `Unknown`, which the snapshot
    /// sanitizer omits, rather than failing the whole load (#3347).
    #[test]
    fn an_unknown_strategy_reads_as_unknown_rather_than_failing_the_load() {
        assert_eq!(
            parse_strategy("teleport").expect("a strategy from a newer release still loads"),
            BalancingStrategy::Unknown
        );
    }

    /// An isolated, migrated schema of this test's own. Bind the guard for the
    /// whole test: the schema is dropped with it.
    async fn fresh_db() -> super::test_schema::TestSchema {
        let url = test_database::url()
            .await
            .expect("ROLTER_TEST_DATABASE_URL not set; skipping");
        super::test_schema::TestSchema::migrated(&url).await
    }

    /// #1925: a write stuck behind another transaction's row lock, or a
    /// statement that runs too long, ends with a timeout the API can code
    /// rather than waiting forever.
    #[tokio::test]
    async fn a_pool_with_bounds_ends_a_stuck_write_with_a_store_timeout() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let url = test_database::url().await.expect("configured above");
        let bounded = connect_with(
            &super::test_schema::with_search_path(&url, db.schema()),
            PoolConfig {
                max_connections: 2,
                statement_timeout: Some(std::time::Duration::from_millis(800)),
                lock_timeout: Some(std::time::Duration::from_millis(300)),
                ..PoolConfig::default()
            },
        )
        .await
        .expect("bounded pool connects");

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();

        // another transaction holds the row; the update must give up
        let mut holder = db.pool().begin().await.unwrap();
        sqlx::query("select id from orgs where id = $1 for update")
            .bind(org_id)
            .fetch_one(&mut *holder)
            .await
            .unwrap();
        let started = std::time::Instant::now();
        let err = sqlx::query("update orgs set name = 'renamed' where id = $1")
            .bind(org_id)
            .execute(&bounded)
            .await
            .map_err(store_err)
            .expect_err("the row is locked");
        assert!(matches!(err, Error::StoreTimeout(_)), "{err:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        holder.rollback().await.unwrap();

        // and a statement that outlasts its bound is cancelled the same way
        let err = sqlx::query("select pg_sleep(30)")
            .execute(&bounded)
            .await
            .map_err(store_err)
            .expect_err("pg_sleep outlasts the statement bound");
        assert!(matches!(err, Error::StoreTimeout(_)), "{err:?}");

        // with the lock released the same write goes through
        sqlx::query("update orgs set name = 'renamed' where id = $1")
            .bind(org_id)
            .execute(&bounded)
            .await
            .expect("the lock is gone");
    }

    #[tokio::test]
    async fn triggers_bump_version_atomically_with_writes() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let v0 = current_version(&pool).await.unwrap();

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        // orgs don't feed the gateway snapshot: no bump
        assert_eq!(current_version(&pool).await.unwrap(), v0);

        sqlx::query(
            "insert into providers (org_id, name, slug, kind, api_base)
             values ($1, 'openai', 'openai', 'openai', 'https://api.openai.com')",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 1);

        // a rolled-back write must not bump the version
        let mut txn = pool.begin().await.unwrap();
        sqlx::query(
            "insert into providers (org_id, name, slug, kind, api_base)
             values ($1, 'ghost', 'ghost', 'openai', 'https://ghost.example.com')",
        )
        .bind(org_id)
        .execute(&mut *txn)
        .await
        .unwrap();
        txn.rollback().await.unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 1);

        sqlx::query("delete from providers where name = 'openai'")
            .execute(&pool)
            .await
            .unwrap();
        // the provider delete cascades to provider_keys, whose statement trigger
        // bumps the version even when no key rows exist, and sweeps the
        // provider's labels (#985), whose trigger does the same
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 4);
    }

    /// #933: a virtual key created in the dashboard was rejected for up to the
    /// gateway's 5s poll interval, which is indistinguishable from a wrong key
    /// — and since the Keys screen shows a key's full value only at creation,
    /// the natural reaction (assume the copy went wrong, mint another) burns a
    /// key the operator cannot recover on every retry.
    ///
    /// A key write has to bump `config_version` in the same transaction, the
    /// way every other data-plane-visible table does, so the control plane has
    /// a new version to publish on `rolter.config` and the gateway refetches
    /// immediately instead of waiting for its poll. This pins both halves: the
    /// bump, and the key being in the snapshot the bumped version serves.
    #[tokio::test]
    async fn a_virtual_key_write_bumps_the_version_and_lands_in_the_next_snapshot() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let (_user_id, project_id) = tenancy_with_owned_key(&pool, "unrelated-existing-key").await;

        let before = current_version(&pool).await.unwrap();
        sqlx::query(
            "insert into virtual_keys (project_id, key_hash, key_prefix, name)
             values ($1, 'brand-new-hash', 'sk-rolter-abc', 'minted-in-the-dashboard')",
        )
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
        let after = current_version(&pool).await.unwrap();
        assert!(
            after > before,
            "a virtual-key insert left config_version at {before}: the gateway has nothing to \
             refetch on, so the key stays rejected until the next poll"
        );

        // the bump is only useful if the snapshot that version names actually
        // carries the key — a version bump pointing at a snapshot without it
        // would fail exactly the same way
        let store = PostgresConfigStore::new(pool.clone());
        let snapshot = store.load().await.unwrap();
        assert!(
            snapshot
                .db_virtual_keys
                .iter()
                .any(|k| k.key_hash == "brand-new-hash"),
            "the key bumped the version but is not in the snapshot that version serves"
        );

        // disable and delete are data-plane-visible too: a revoked key must
        // stop working as promptly as a new one starts
        let disabled_from = current_version(&pool).await.unwrap();
        sqlx::query("update virtual_keys set disabled = true where key_hash = 'brand-new-hash'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            current_version(&pool).await.unwrap() > disabled_from,
            "disabling a key did not bump the version"
        );

        let deleted_from = current_version(&pool).await.unwrap();
        sqlx::query("delete from virtual_keys where key_hash = 'brand-new-hash'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            current_version(&pool).await.unwrap() > deleted_from,
            "deleting a key did not bump the version"
        );
    }

    /// A personal key follows its creator (#1841). The snapshot decides that
    /// from `users` and `access_profile_roles`, so a write to either has to
    /// bump the version or no gateway refetches; and deleting the account
    /// disables the key instead of letting the foreign key orphan it into a
    /// shared one.
    #[tokio::test]
    async fn a_personal_keys_creator_decides_whether_the_snapshot_serves_it() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let store = PostgresConfigStore::new(pool.clone());
        let (user_id, project_id) = tenancy_with_owned_key(&pool, "hash-leaver").await;
        async fn served(store: &PostgresConfigStore) -> Option<VirtualKeyRecord> {
            store
                .load_virtual_keys()
                .await
                .unwrap()
                .into_iter()
                .find(|k| k.key_hash == "hash-leaver")
        }
        let bumped = |from: i64, what: &'static str| {
            let pool = pool.clone();
            async move {
                assert!(
                    current_version(&pool).await.unwrap() > from,
                    "{what} left config_version at {from}"
                );
            }
        };
        assert!(served(&store).await.is_some());

        let v = current_version(&pool).await.unwrap();
        sqlx::query("update users set deactivated_at = now() where id = $1")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        bumped(v, "deactivating the creator").await;
        assert!(
            served(&store).await.is_none(),
            "a deactivated creator's key is served"
        );

        let v = current_version(&pool).await.unwrap();
        sqlx::query("update users set deactivated_at = null where id = $1")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        bumped(v, "reactivating the creator").await;
        assert!(
            served(&store).await.is_some(),
            "reactivation does not bring the key back"
        );

        // only a profile role reaches the project: assigned to a team the
        // creator belongs to through another project, scoped at this one
        let team_id: Uuid = sqlx::query_scalar("select team_id from projects where id = $1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let org_id: Uuid = sqlx::query_scalar("select id from orgs limit 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let other_project: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'other') returning id",
        )
        .bind(team_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("delete from memberships where user_id = $1")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "insert into memberships (user_id, project_id, role) values ($1, $2, 'viewer')",
        )
        .bind(user_id)
        .bind(other_project)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            served(&store).await.is_none(),
            "a creator with no reach keeps their key"
        );
        let role_id: Uuid = sqlx::query_scalar(
            "insert into custom_roles (org_id, slug, name) values ($1, 'dev', 'Dev') returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let profile_id: Uuid = sqlx::query_scalar(
            "insert into access_profiles (org_id, slug, name) values ($1, 'devs', 'devs') returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("insert into access_profile_assignments (profile_id, team_id) values ($1, $2)")
            .bind(profile_id)
            .bind(team_id)
            .execute(&pool)
            .await
            .unwrap();
        let v = current_version(&pool).await.unwrap();
        sqlx::query(
            "insert into access_profile_roles (profile_id, role_id, project_id) values ($1, $2, $3)",
        )
        .bind(profile_id)
        .bind(role_id)
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
        bumped(v, "composing a role into a profile").await;
        assert!(
            served(&store).await.is_some(),
            "a team-assigned profile role at the project does not reach it"
        );
        let v = current_version(&pool).await.unwrap();
        sqlx::query("delete from access_profile_roles where profile_id = $1")
            .bind(profile_id)
            .execute(&pool)
            .await
            .unwrap();
        bumped(v, "removing a role from a profile").await;
        assert!(served(&store).await.is_none());

        // deleting the account: the key stays a row, disabled, not shared
        sqlx::query(
            "insert into memberships (user_id, project_id, role) values ($1, $2, 'member')",
        )
        .bind(user_id)
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(!served(&store).await.unwrap().disabled);
        sqlx::query("delete from users where id = $1")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        let orphan = served(&store)
            .await
            .expect("the key row outlives its creator");
        assert!(
            orphan.disabled,
            "a deleted creator's key is served as a shared key"
        );
        assert!(orphan.user_id.is_empty());
    }

    /// #1643: a provider group created through the dashboard was reachable in
    /// `/internal/snapshot` yet 404'd at the gateway until someone restarted it.
    /// The data plane indexes `config.provider_groups` for `group-slug/model`
    /// addressing, so the group's tables feed the snapshot — but neither had a
    /// `bump_config_version()` trigger, leaving the gateway's watcher with no
    /// version change to refetch on. Any later write to another config table
    /// masked it, which is what made it look intermittent.
    ///
    /// Every write shape gets its own assertion because each is a separate
    /// operator action: creating the group, adding members, retuning a weight
    /// or the strategy, and removing a member all have to reach the fleet.
    #[tokio::test]
    async fn provider_group_writes_bump_the_version_and_land_in_the_next_snapshot() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let mut provider_ids = Vec::new();
        for n in 1..=2 {
            let id: Uuid = sqlx::query_scalar(
                "insert into providers (org_id, name, slug, kind, api_base)
                 values ($1, $2, $3, 'openai', 'http://vllm.internal') returning id",
            )
            .bind(org_id)
            .bind(format!("vllm-a100-0{n}"))
            .bind(format!("vllm-a100-0{n}"))
            .fetch_one(&pool)
            .await
            .unwrap();
            provider_ids.push(id);
        }

        let before = current_version(&pool).await.unwrap();
        let group_id: Uuid = sqlx::query_scalar(
            "insert into provider_groups (org_id, name, slug, strategy)
             values ($1, 'Llama fleet', 'llama-fleet', 'weighted') returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            current_version(&pool).await.unwrap() > before,
            "creating a provider group left config_version at {before}: the gateway has \
             nothing to refetch on, so the group 404s until someone restarts it"
        );

        let before = current_version(&pool).await.unwrap();
        for (position, provider_id) in provider_ids.iter().enumerate() {
            sqlx::query(
                "insert into provider_group_members (group_id, provider_id, weight, position)
                 values ($1, $2, $3, $4)",
            )
            .bind(group_id)
            .bind(provider_id)
            .bind(3 - position as i32)
            .bind(position as i32)
            .execute(&pool)
            .await
            .unwrap();
        }
        assert!(
            current_version(&pool).await.unwrap() > before,
            "adding members to a group did not bump config_version: an empty group never \
             routes, so the group stays dead at the gateway"
        );

        // retuning a member's weight is the write an operator repeats most, and
        // it is invisible unless it bumps too
        let before = current_version(&pool).await.unwrap();
        sqlx::query("update provider_group_members set weight = 7 where group_id = $1")
            .bind(group_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            current_version(&pool).await.unwrap() > before,
            "changing a member weight did not bump config_version"
        );

        let before = current_version(&pool).await.unwrap();
        sqlx::query("update provider_groups set strategy = 'round_robin' where id = $1")
            .bind(group_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            current_version(&pool).await.unwrap() > before,
            "changing a group strategy did not bump config_version"
        );

        // a rolled-back write must not bump, the same as every other table
        let rolled_back_from = current_version(&pool).await.unwrap();
        let mut txn = pool.begin().await.unwrap();
        sqlx::query(
            "insert into provider_groups (org_id, name, slug) values ($1, 'ghost', 'ghost')",
        )
        .bind(org_id)
        .execute(&mut *txn)
        .await
        .unwrap();
        txn.rollback().await.unwrap();
        assert_eq!(
            current_version(&pool).await.unwrap(),
            rolled_back_from,
            "a rolled-back group insert bumped config_version"
        );

        // the bump is only worth anything if the snapshot that version names
        // carries the group — a version pointing at a snapshot without it fails
        // exactly the same way
        let store = PostgresConfigStore::new(pool.clone());
        let snapshot = store.load().await.unwrap();
        let group = snapshot
            .provider_groups
            .iter()
            .find(|g| g.slug.as_deref() == Some("llama-fleet"))
            .expect("the group bumped the version but is not in the snapshot it serves");
        assert_eq!(group.members.len(), 2, "the snapshot dropped a member");

        let before = current_version(&pool).await.unwrap();
        sqlx::query("delete from provider_group_members where group_id = $1 and provider_id = $2")
            .bind(group_id)
            .bind(provider_ids[1])
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            current_version(&pool).await.unwrap() > before,
            "removing a member did not bump config_version: the gateway keeps sending traffic \
             to a box the operator took out of the fleet"
        );

        let before = current_version(&pool).await.unwrap();
        sqlx::query("delete from provider_groups where id = $1")
            .bind(group_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            current_version(&pool).await.unwrap() > before,
            "deleting a group did not bump config_version"
        );
    }

    /// org → team → project → user, with a virtual key the user owns.
    /// Returns `(user_id, project_id)`.
    async fn tenancy_with_owned_key(pool: &PgPool, key_hash: &str) -> (Uuid, Uuid) {
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let team_id: Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org_id)
                .fetch_one(pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'api') returning id",
        )
        .bind(team_id)
        .fetch_one(pool)
        .await
        .unwrap();
        let user_id: Uuid =
            sqlx::query_scalar("insert into users (email) values ('dev@example.com') returning id")
                .fetch_one(pool)
                .await
                .unwrap();
        // a personal key is served only while its creator belongs to the
        // project (#1841). An org role reaches it without placing the creator
        // in any team, which the team-assigned profile tests rely on
        sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
            .bind(user_id)
            .bind(org_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "insert into virtual_keys (project_id, key_hash, key_prefix, name, created_by)
             values ($1, $2, 'sk-test', 'test key', $3)",
        )
        .bind(project_id)
        .bind(key_hash)
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
        (user_id, project_id)
    }

    /// Attach a profile carrying `policy` to `user_id`, returning its id.
    async fn profile_for_user(
        pool: &PgPool,
        user_id: Uuid,
        slug: &str,
        allowed_models: &[&str],
        denied_models: &[&str],
        denied_routes: &[&str],
    ) -> Uuid {
        let org_id: Uuid = sqlx::query_scalar("select id from orgs limit 1")
            .fetch_one(pool)
            .await
            .unwrap();
        let profile_id: Uuid = sqlx::query_scalar(
            "insert into access_profiles (org_id, slug, name) values ($1, $2, $2) returning id",
        )
        .bind(org_id)
        .bind(slug)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into access_profile_policies
                 (profile_id, allowed_models, denied_models, denied_routes)
             values ($1, $2, $3, $4)",
        )
        .bind(profile_id)
        .bind(
            allowed_models
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
        .bind(
            denied_models
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
        .bind(
            denied_routes
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("insert into access_profile_assignments (profile_id, user_id) values ($1, $2)")
            .bind(profile_id)
            .bind(user_id)
            .execute(pool)
            .await
            .unwrap();
        profile_id
    }

    /// #2962: a route's targets reach the snapshot ordered by `created_at`
    /// then `id`, and a weight edit does not move one.
    #[tokio::test]
    async fn route_targets_load_in_a_stable_order() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let store = PostgresConfigStore {
            pool: pool.clone(),
            kek: None,
        };
        let org = repo::OrgRepo(&pool).create("acme", "acme").await.unwrap();
        let team = repo::TeamRepo(&pool).create(org.id, "core").await.unwrap();
        let project = repo::ProjectRepo(&pool)
            .create(team.id, "default")
            .await
            .unwrap();
        let route = repo::RouteRepo(&pool)
            .create(project.id, "gpt-4o", "consistent_hash")
            .await
            .unwrap();
        // inserted newest first, so heap order is the reverse of the wanted one (a is oldest)
        let names = ["a", "b", "c"];
        for (age, name) in names.iter().enumerate().rev() {
            let provider = repo::ProviderRepo(&pool)
                .create(
                    org.id,
                    repo::NewProvider {
                        name,
                        slug: name,
                        kind: "openai",
                        api_base: "https://api.openai.com",
                        api_key_env: Some("OPENAI_API_KEY"),
                        egress_proxy: None,
                        egress_proxies: &[],
                        kv_events: None,
                        lmcache: None,
                        project_id: None,
                        allow_custom_api_base: false,
                    },
                )
                .await
                .unwrap();
            sqlx::query(
                "insert into route_targets (route_id, provider_id, weight, created_at)
                 values ($1, $2, 1, now() - make_interval(secs => $3))",
            )
            .bind(route.id)
            .bind(provider.id)
            .bind((names.len() - age) as f64)
            .execute(&pool)
            .await
            .unwrap();
        }
        let order = |routes: Vec<ModelRoute>| -> Vec<String> {
            routes[0]
                .targets
                .iter()
                .map(|t| t.provider.clone())
                .collect()
        };
        let before = order(store.load_routes(&HashSet::new()).await.unwrap());
        assert_eq!(before, ["a", "b", "c"].map(String::from));

        // the oldest target is rewritten, which moves its heap tuple
        sqlx::query(
            "update route_targets set weight = 5
             where id = (select id from route_targets order by created_at limit 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let after = order(store.load_routes(&HashSet::new()).await.unwrap());
        assert_eq!(after, before, "a weight edit keeps the target order");
    }

    /// #391: a variant target drops out of the snapshot on the rules a route
    /// target does, a provider of another org and a provider left out of the
    /// snapshot, and a variant those rules leave with no target is dropped
    /// whole rather than served empty. The variants keep their declared order.
    #[tokio::test]
    async fn variant_targets_load_on_the_route_target_rules() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let store = PostgresConfigStore {
            pool: pool.clone(),
            kek: None,
        };
        let provider = |org_id: Uuid, name: &'static str| {
            let pool = pool.clone();
            async move {
                repo::ProviderRepo(&pool)
                    .create(
                        org_id,
                        repo::NewProvider {
                            name,
                            slug: name,
                            kind: "openai",
                            api_base: "https://api.openai.com",
                            api_key_env: Some("OPENAI_API_KEY"),
                            egress_proxy: None,
                            egress_proxies: &[],
                            kv_events: None,
                            lmcache: None,
                            project_id: None,
                            allow_custom_api_base: false,
                        },
                    )
                    .await
                    .unwrap()
                    .id
            }
        };
        let org = repo::OrgRepo(&pool).create("acme", "acme").await.unwrap();
        let other = repo::OrgRepo(&pool)
            .create("globex", "globex")
            .await
            .unwrap();
        let team = repo::TeamRepo(&pool).create(org.id, "core").await.unwrap();
        let project = repo::ProjectRepo(&pool)
            .create(team.id, "default")
            .await
            .unwrap();
        let route = repo::RouteRepo(&pool)
            .create(project.id, "split", "round_robin")
            .await
            .unwrap();
        let own = provider(org.id, "own").await;
        let dropped = provider(org.id, "dropped").await;
        let foreign = provider(other.id, "foreign").await;
        let target = |provider_id: Uuid| repo::RouteVariantTargetSpec {
            provider_id,
            upstream_model: None,
            weight: 1,
            takes_over: Vec::new(),
        };
        let variant =
            |name: &str, targets: Vec<repo::RouteVariantTargetSpec>| repo::RouteVariantSpec {
                name: name.to_string(),
                weight: 1,
                params: serde_json::json!({}),
                targets,
            };
        // the write path refuses a foreign provider, so it is written here
        // directly, as a row from before that check would be
        repo::RouteVariantRepo(&pool)
            .replace(
                route.id,
                &[
                    variant("mixed", vec![target(foreign), target(own), target(dropped)]),
                    variant("stranded", vec![target(foreign), target(dropped)]),
                    variant("plain", vec![target(own)]),
                ],
            )
            .await
            .unwrap();

        let left_out: LeftOut = [(org.id, "dropped".to_string())].into();
        let routes = store.load_routes(&left_out).await.unwrap();
        let split = routes.iter().find(|r| r.model == "split").unwrap();
        let shape: Vec<(String, Vec<String>)> = split
            .variants
            .iter()
            .map(|v| {
                let providers = v.targets.iter().map(|t| t.provider.clone()).collect();
                (v.name.clone(), providers)
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("mixed".to_string(), vec!["own".to_string()]),
                ("plain".to_string(), vec!["own".to_string()]),
            ],
            "a foreign or left-out target is dropped, and so is a variant left with none"
        );
    }

    #[tokio::test]
    async fn snapshot_carries_the_key_owners_access_policy() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let store = PostgresConfigStore {
            pool: pool.clone(),
            kek: None,
        };
        let (user_id, _) = tenancy_with_owned_key(&pool, "hash-owner-policy").await;

        // no profiles yet: the key is unrestricted, exactly as before #791
        let keys = store.load_virtual_keys().await.unwrap();
        assert_eq!(keys.len(), 1);
        assert!(keys[0].access_policy.is_none());

        profile_for_user(&pool, user_id, "restricted", &["gpt-*"], &["gpt-4o"], &[]).await;

        let keys = store.load_virtual_keys().await.unwrap();
        let policy = keys[0]
            .access_policy
            .as_ref()
            .expect("owner's policy reaches the snapshot");
        assert!(policy.permits_model("gpt-4o-mini"));
        // deny wins over the allow-list that also matches
        assert!(!policy.permits_model("gpt-4o"));
        // outside the allow-list entirely
        assert!(!policy.permits_model("claude-opus"));
    }

    #[tokio::test]
    async fn several_profiles_union_onto_one_key() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let store = PostgresConfigStore {
            pool: pool.clone(),
            kek: None,
        };
        let (user_id, _) = tenancy_with_owned_key(&pool, "hash-multi-profile").await;

        profile_for_user(&pool, user_id, "openai", &["gpt-*"], &[], &[]).await;
        profile_for_user(
            &pool,
            user_id,
            "anthropic",
            &["claude-*"],
            &[],
            &["internal-*"],
        )
        .await;

        let keys = store.load_virtual_keys().await.unwrap();
        let policy = keys[0].access_policy.as_ref().expect("merged policy");
        // union, not intersection: a second profile may only widen
        assert!(policy.permits_model("gpt-4o"));
        assert!(policy.permits_model("claude-opus"));
        assert!(!policy.permits_model("llama-3"));
        // the route deny from one profile still applies
        assert!(!policy.permits_route("internal-tools"));
    }

    #[tokio::test]
    async fn a_team_assigned_profile_reaches_the_keys_owner() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let store = PostgresConfigStore {
            pool: pool.clone(),
            kek: None,
        };
        let (user_id, project_id) = tenancy_with_owned_key(&pool, "hash-team-profile").await;
        let team_id: Uuid = sqlx::query_scalar("select team_id from projects where id = $1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let org_id: Uuid = sqlx::query_scalar("select id from orgs limit 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        let profile_id: Uuid = sqlx::query_scalar(
            "insert into access_profiles (org_id, slug, name)
             values ($1, 'team-wide', 'team-wide') returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into access_profile_policies (profile_id, denied_models)
             values ($1, '{secret-*}')",
        )
        .bind(profile_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("insert into access_profile_assignments (profile_id, team_id) values ($1, $2)")
            .bind(profile_id)
            .bind(team_id)
            .execute(&pool)
            .await
            .unwrap();

        // the profile is on the team; the user is not in it yet
        let keys = store.load_virtual_keys().await.unwrap();
        assert!(keys[0].access_policy.is_none());

        sqlx::query("insert into memberships (user_id, team_id, role) values ($1, $2, 'member')")
            .bind(user_id)
            .bind(team_id)
            .execute(&pool)
            .await
            .unwrap();

        let keys = store.load_virtual_keys().await.unwrap();
        let policy = keys[0]
            .access_policy
            .as_ref()
            .expect("team membership carries the profile through");
        assert!(!policy.permits_model("secret-model"));
        assert!(policy.permits_model("gpt-4o"));
    }

    #[tokio::test]
    async fn access_policy_writes_bump_version() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let (user_id, project_id) = tenancy_with_owned_key(&pool, "hash-bump").await;
        let team_id: Uuid = sqlx::query_scalar("select team_id from projects where id = $1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();

        // the gateway now reads these tables, so every write has to propagate
        let v0 = current_version(&pool).await.unwrap();
        profile_for_user(&pool, user_id, "bumping", &["gpt-*"], &[], &[]).await;
        assert!(
            current_version(&pool).await.unwrap() > v0,
            "profile, policy and assignment writes must bump config_version"
        );

        let v1 = current_version(&pool).await.unwrap();
        sqlx::query("insert into memberships (user_id, team_id, role) values ($1, $2, 'member')")
            .bind(user_id)
            .bind(team_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            current_version(&pool).await.unwrap() > v1,
            "a membership change alters someone's effective policy and must bump"
        );

        // custom roles stay control-plane only: no bump, per ADR-0023
        let org_id: Uuid = sqlx::query_scalar("select id from orgs limit 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let v2 = current_version(&pool).await.unwrap();
        sqlx::query(
            "insert into custom_roles (org_id, slug, name) values ($1, 'auditor', 'Auditor')",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            current_version(&pool).await.unwrap(),
            v2,
            "custom roles decide control-plane authorization only"
        );
    }

    /// #1919: a provider or group scoped to a project reaches the gateway
    /// through the snapshot, so a change of scope must bump `config_version`;
    /// the scope can only name a project of the row's own org; and the project
    /// cannot be deleted from under it, while deleting the whole org still works.
    #[tokio::test]
    async fn project_scope_bumps_the_version_stays_in_its_org_and_blocks_project_delete() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let org = |slug: &'static str| {
            let pool = pool.clone();
            async move {
                let org: Uuid = sqlx::query_scalar(
                    "insert into orgs (name, slug) values ($1, $1) returning id",
                )
                .bind(slug)
                .fetch_one(&pool)
                .await
                .unwrap();
                let team: Uuid = sqlx::query_scalar(
                    "insert into teams (org_id, name) values ($1, 'core') returning id",
                )
                .bind(org)
                .fetch_one(&pool)
                .await
                .unwrap();
                let project: Uuid = sqlx::query_scalar(
                    "insert into projects (team_id, name) values ($1, 'app') returning id",
                )
                .bind(team)
                .fetch_one(&pool)
                .await
                .unwrap();
                (org, project)
            }
        };
        let (org_a, project_a) = org("scope-a").await;
        let (_org_b, project_b) = org("scope-b").await;
        let provider: Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, slug, kind, api_base)
             values ($1, 'edge', 'edge', 'openai_compatible', 'http://127.0.0.1:9')
             returning id",
        )
        .bind(org_a)
        .fetch_one(&pool)
        .await
        .unwrap();
        let group: Uuid = sqlx::query_scalar(
            "insert into provider_groups (org_id, name, slug) values ($1, 'pool', 'pool')
             returning id",
        )
        .bind(org_a)
        .fetch_one(&pool)
        .await
        .unwrap();
        // rows that predate the column are org-wide
        let scope: Option<Uuid> = sqlx::query_scalar("select project_id from providers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(scope, None);

        for (table, id) in [("providers", provider), ("provider_groups", group)] {
            let before = current_version(&pool).await.unwrap();
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "update {table} set project_id = $1 where id = $2"
            )))
            .bind(project_a)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
            assert!(
                current_version(&pool).await.unwrap() > before,
                "scoping a row in {table} did not bump config_version"
            );
            // another org's project is refused by the database itself
            let crossed = sqlx::query(sqlx::AssertSqlSafe(format!(
                "update {table} set project_id = $1 where id = $2"
            )))
            .bind(project_b)
            .bind(id)
            .execute(&pool)
            .await;
            assert!(crossed.is_err(), "{table} accepted another org's project");
        }

        // restricting the delete is what stops a project's private credential
        // from widening to org-wide or vanishing with it
        let deleted = sqlx::query("delete from projects where id = $1")
            .bind(project_a)
            .execute(&pool)
            .await;
        assert!(deleted.is_err(), "a project was deleted under its provider");
        // and the loader hands the scope to the gateway
        let store = PostgresConfigStore::with_kek(pool.clone(), None);
        let config = store.load().await.unwrap();
        let loaded = &config.providers[0];
        assert_eq!(
            loaded
                .tenancy
                .as_ref()
                .and_then(|t| t.project_id.as_deref()),
            Some(project_a.to_string().as_str())
        );
        assert!(loaded.project_scoped);

        // deleting the org cascades to both and must not trip over the check
        sqlx::query("delete from orgs where id = $1")
            .bind(org_a)
            .execute(&pool)
            .await
            .expect("deleting an org with project-scoped providers");
    }

    #[tokio::test]
    async fn prompt_template_writes_bump_version() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let v0 = current_version(&pool).await.unwrap();

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let template_id: Uuid = sqlx::query_scalar(
            "insert into prompt_templates (org_id, name, slug, description)
             values ($1, 'support baseline', 'support-baseline', 'desc')
             returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 1);

        sqlx::query(
            "insert into prompt_template_versions (template_id, version, variables, decorators)
             values
               ($1, 1, '[]'::jsonb, '[{\"role\":\"system\",\"position\":\"prepend\",\"content\":\"v1\"}]'::jsonb)",
        )
        .bind(template_id)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 2);

        sqlx::query(
            "insert into prompt_template_scopes (
                 template_id, version, scope_type, scope_id, org_id
             )
             values ($1, 1, 'org', $2, $2)",
        )
        .bind(template_id)
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 3);

        sqlx::query(
            "update prompt_templates
             set published_version = 1
             where id = $1",
        )
        .bind(template_id)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 4);
    }

    #[tokio::test]
    async fn skills_writes_bump_version() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let v0 = current_version(&pool).await.unwrap();

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let skill_id: Uuid = sqlx::query_scalar(
            "insert into skills (org_id, name, slug, description)
             values ($1, 'classification baseline', 'classification-baseline', 'desc')
             returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 1);

        sqlx::query(
            "insert into skill_versions (skill_id, version, content, metadata)
             values ($1, 1, 'alpha', '{}'::jsonb)",
        )
        .bind(skill_id)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 2);

        sqlx::query("update skills set published_version = 1 where id = $1")
            .bind(skill_id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 3);
    }

    #[tokio::test]
    async fn loads_providers_and_routes_from_db() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let team_id: Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'default') returning id",
        )
        .bind(team_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let provider_id: Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, slug, kind, api_base, api_key_env)
             values ($1, 'openai', 'openai', 'openai', 'https://api.openai.com', 'OPENAI_API_KEY')
             returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let route_id: Uuid = sqlx::query_scalar(
            "insert into routes (project_id, model, strategy) values ($1, 'gpt-4o', 'power_of_two') returning id",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into route_targets (route_id, provider_id, upstream_model, weight)
             values ($1, $2, 'gpt-4o-2024-08-06', 2)",
        )
        .bind(route_id)
        .bind(provider_id)
        .execute(&pool)
        .await
        .unwrap();

        let store = PostgresConfigStore::new(pool);
        let config = store.load().await.unwrap();

        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].name, "openai");
        assert_eq!(config.providers[0].kind, ProviderKind::Openai);
        assert_eq!(
            config.providers[0].api_key_env.as_deref(),
            Some("OPENAI_API_KEY")
        );

        assert_eq!(config.routes.len(), 1);
        assert_eq!(config.routes[0].model, "gpt-4o");
        assert_eq!(config.routes[0].strategy, BalancingStrategy::PowerOfTwo);
        assert_eq!(config.routes[0].targets.len(), 1);
        assert_eq!(config.routes[0].targets[0].provider, "openai");
        assert_eq!(
            config.routes[0].targets[0].model.as_deref(),
            Some("gpt-4o-2024-08-06")
        );
        assert_eq!(config.routes[0].targets[0].weight, 2);
    }

    #[tokio::test]
    async fn feature_flags_gate_supported_snapshot_subsystems() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let team_id: Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'default') returning id",
        )
        .bind(team_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let provider_id: Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, slug, kind, api_base, api_key_env)
             values ($1, 'openai', 'openai', 'openai', 'https://api.openai.com', 'OPENAI_API_KEY')
             returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let route_id: Uuid = sqlx::query_scalar(
            "insert into routes (project_id, model, strategy, params)
             values ($1, 'gpt-4o', 'cache_aware', $2) returning id",
        )
        .bind(project_id)
        .bind(serde_json::json!({
            "_rolter_complexity": {
                "tiers": [{"name":"simple","max_input_bytes":256,"route":"gpt-4o"}]
            }
        }))
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into route_targets (route_id, provider_id, upstream_model, weight)
             values ($1, $2, 'gpt-4o-2024-08-06', 1)",
        )
        .bind(route_id)
        .bind(provider_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "update feature_flags set
                response_cache = false,
                cache_aware_routing = false,
                circuit_breaker = false,
                active_health_checks = false,
                complexity_routing = false,
                guardrails = false
             where id = true",
        )
        .execute(&pool)
        .await
        .unwrap();

        let store = PostgresConfigStore::new(pool);
        let config = store.load().await.unwrap();
        assert!(!config.cache.enabled);
        assert!(!config.breaker.enabled);
        assert!(!config.health.enabled);
        assert!(!config.guardrails.enabled);
        assert_eq!(config.routes.len(), 1);
        assert_eq!(config.routes[0].strategy, BalancingStrategy::PowerOfTwo);
        assert!(!config.routes[0].params.contains_key("_rolter_complexity"));
    }

    #[tokio::test]
    async fn logging_settings_project_into_snapshot_logging_policy() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        sqlx::query(
            "update logging_settings set
                sample_rate = 0.4,
                payload_capture_enabled = true,
                payload_capture_max_bytes = 2048,
                payload_capture_redact_fields = array['token','secret'],
                payload_capture_models = array['gpt-4o'],
                payload_capture_virtual_key_ids = array['vk-1']
             where id = true",
        )
        .execute(&pool)
        .await
        .unwrap();

        let store = PostgresConfigStore::new(pool);
        let config = store.load().await.unwrap();
        assert_eq!(config.logging.sample_rate, 0.4);
        assert!(config.logging.payload_capture.enabled);
        assert_eq!(config.logging.payload_capture.max_bytes, 2048);
        assert_eq!(
            config.logging.payload_capture.redact_fields,
            vec!["token".to_string(), "secret".to_string()]
        );
        assert_eq!(
            config.logging.payload_capture.models,
            vec!["gpt-4o".to_string()]
        );
        assert_eq!(
            config.logging.payload_capture.virtual_key_ids,
            vec!["vk-1".to_string()]
        );
    }

    /// #1748: `logging.ui_events` is the opt-out the UX ingest endpoint reads,
    /// and before the column existed a postgres deployment always saw the
    /// serde default. The row has to reach the loaded config, and switching it
    /// off has to bump `config_version` like any other write to the table.
    #[tokio::test]
    async fn ui_events_opt_out_projects_into_the_loaded_config() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let store = PostgresConfigStore::new(pool.clone());
        assert!(
            store.load().await.unwrap().logging.ui_events,
            "the migration default must keep the stream on for existing deployments"
        );

        let before = current_version(&pool).await.unwrap();
        sqlx::query("update logging_settings set ui_events = false where id = true")
            .execute(&pool)
            .await
            .unwrap();
        let after = current_version(&pool).await.unwrap();
        assert!(
            after > before,
            "switching ui_events off left config_version at {before}"
        );
        assert!(!store.load().await.unwrap().logging.ui_events);

        // a writer with no opinion on the column keeps what is stored
        let repo = repo::LoggingSettingsRepo(&pool);
        let row = repo
            .update(repo::LoggingSettingsUpdate {
                sample_rate: 1.0,
                payload_capture_enabled: false,
                payload_capture_max_bytes: 32768,
                payload_capture_redact_fields: &[],
                payload_capture_redact_detectors: None,
                payload_capture_redact_patterns: None,
                payload_capture_models: &[],
                payload_capture_virtual_key_ids: &[],
                retention_days: 90,
                payload_retention_hours: 168,
                ui_events: None,
            })
            .await
            .unwrap();
        assert!(!row.ui_events, "a None update re-enabled the stream");
        let row = repo
            .update(repo::LoggingSettingsUpdate {
                sample_rate: 1.0,
                payload_capture_enabled: false,
                payload_capture_max_bytes: 32768,
                payload_capture_redact_fields: &[],
                payload_capture_redact_detectors: None,
                payload_capture_redact_patterns: None,
                payload_capture_models: &[],
                payload_capture_virtual_key_ids: &[],
                retention_days: 90,
                payload_retention_hours: 168,
                ui_events: Some(true),
            })
            .await
            .unwrap();
        assert!(row.ui_events);

        // the value-redaction policy (#1835) defaults to every built-in
        // detector, is replaced when given, kept when not, and reaches the
        // snapshot through the same trigger as the other columns
        assert_eq!(
            row.payload_capture_redact_detectors,
            rolter_core::payload_redaction::detector_names()
        );
        let patterns = [rolter_core::payload_redaction::RedactPattern {
            name: "employee_id".to_string(),
            pattern: r"EMP-\d{6}".to_string(),
        }];
        let detectors = ["aws_key".to_string()];
        let before = current_version(&pool).await.unwrap();
        let update = |detectors, patterns| repo::LoggingSettingsUpdate {
            sample_rate: 1.0,
            payload_capture_enabled: true,
            payload_capture_max_bytes: 32768,
            payload_capture_redact_fields: &[],
            payload_capture_redact_detectors: detectors,
            payload_capture_redact_patterns: patterns,
            payload_capture_models: &[],
            payload_capture_virtual_key_ids: &[],
            retention_days: 90,
            payload_retention_hours: 168,
            ui_events: None,
        };
        repo.update(update(Some(&detectors[..]), Some(&patterns[..])))
            .await
            .unwrap();
        assert!(current_version(&pool).await.unwrap() > before);
        let row = repo.update(update(None, None)).await.unwrap();
        assert_eq!(row.payload_capture_redact_detectors, detectors);
        assert_eq!(row.payload_capture_redact_patterns.0, patterns);
        let capture = store.load().await.unwrap().logging.payload_capture;
        assert_eq!(capture.redact_detectors, detectors);
        assert_eq!(capture.redact_patterns, patterns);
    }

    #[tokio::test]
    async fn runtime_policy_projects_into_snapshot_runtime_controls() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        sqlx::query(
            "update runtime_policy set
                retry_max_retries = 5,
                retry_base_ms = 250,
                retry_max_ms = 4000,
                timeout_connect_s = 15,
                timeout_request_s = 120,
                queue_enabled = false,
                queue_capacity = 512,
                queue_workers = 32,
                queue_backpressure = 'drop',
                queue_block_ms = 750
             where id = true",
        )
        .execute(&pool)
        .await
        .unwrap();

        let store = PostgresConfigStore::new(pool);
        let config = store.load().await.unwrap();
        assert_eq!(config.retry.max_retries, 5);
        assert_eq!(config.retry.base_backoff_ms, 250);
        assert_eq!(config.retry.max_backoff_ms, 4000);
        assert_eq!(config.timeouts.connect_secs, 15);
        assert_eq!(config.timeouts.request_secs, 120);
        assert!(!config.queue.enabled);
        assert_eq!(config.queue.capacity, 512);
        assert_eq!(config.queue.workers, 32);
        assert_eq!(config.queue.backpressure, BackpressurePolicy::Drop);
        assert_eq!(config.queue.block_timeout_ms, 750);
    }

    // regression: the snapshot query must cast numeric price columns to text,
    // otherwise sqlx fails decoding into String and GET /api/v1/models 500s
    // as soon as any model_prices row exists
    #[tokio::test]
    async fn loads_model_prices_from_db() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        sqlx::query(
            "insert into model_prices
                    (model, input_per_mtok, output_per_mtok, cached_input_per_mtok,
                     cache_write_per_mtok, cache_write_1h_per_mtok, currency)
             values ('gpt-4o', 3, 15, 1.5, 3.75, 6, 'USD'),
                    ('gpt-4o-mini', 0.15, 0.6, null, null, null, 'USD')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let store = PostgresConfigStore::new(pool);
        let config = store.load().await.unwrap();

        assert_eq!(config.model_prices.len(), 2);
        assert_eq!(config.model_prices[0].model, "gpt-4o");
        assert_eq!(config.model_prices[0].input_per_mtok, d("3.0"));
        assert_eq!(config.model_prices[0].output_per_mtok, d("15.0"));
        assert_eq!(config.model_prices[0].cached_input_per_mtok, Some(d("1.5")));
        assert_eq!(config.model_prices[0].cache_write_per_mtok, Some(d("3.75")));
        assert_eq!(config.model_prices[0].cache_write_1h_per_mtok, Some(d("6")));
        assert_eq!(config.model_prices[1].model, "gpt-4o-mini");
        assert_eq!(config.model_prices[1].input_per_mtok, d("0.15"));
        assert_eq!(config.model_prices[1].cached_input_per_mtok, None);
        assert_eq!(config.model_prices[1].cache_write_per_mtok, None);
        assert_eq!(config.model_prices[1].cache_write_1h_per_mtok, None);
    }

    /// Every constraint 0084 puts on `model_prices`, to take them off again and
    /// stand in for a database that was written before they existed.
    const RATE_CHECKS: [&str; 4] = [
        "model_prices_input_per_mtok_rate",
        "model_prices_output_per_mtok_rate",
        "model_prices_cached_input_per_mtok_rate",
        "model_prices_cache_write_per_mtok_rate",
    ];

    async fn drop_rate_checks(pool: &PgPool) {
        for name in RATE_CHECKS {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "alter table model_prices drop constraint {name}"
            )))
            .execute(pool)
            .await
            .unwrap();
        }
    }

    /// Which of 0084's rate constraints are validated, by name. 0085's check on
    /// the 1 hour rate is not among them: it came with its column, validated.
    async fn validated_rate_checks(pool: &PgPool) -> Vec<(String, bool)> {
        sqlx::query_as(
            "select conname::text, convalidated from pg_constraint
             where conrelid = 'model_prices'::regclass and conname = any($1)
             order by conname",
        )
        .bind(RATE_CHECKS.map(String::from).to_vec())
        .fetch_all(pool)
        .await
        .unwrap()
    }

    // #2889: the control plane accepted `NaN` as a rate, `numeric` stores it,
    // and a `Decimal` cannot hold it. The loader used to read it as zero, which
    // priced the model at nothing; it now leaves that one row out, says so, and
    // loads the rest
    #[tokio::test]
    async fn a_price_with_a_nan_rate_is_left_out_and_reported_not_priced_at_zero() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        // an older database has no constraint to stop the row going in
        drop_rate_checks(&pool).await;
        sqlx::query(
            "insert into model_prices
                    (model, input_per_mtok, output_per_mtok, cached_input_per_mtok,
                     cache_write_per_mtok)
             values ('good', 3, 15, 1.5, 3.75),
                    ('nan-input', 'NaN', 15, null, null),
                    ('nan-cached', 3, 15, 'NaN', null),
                    ('nan-write', 3, 15, null, 'NaN'),
                    ('negative', -1, 15, null, null)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let store = PostgresConfigStore::new(pool);
        let models: Vec<String> = store
            .load()
            .await
            .unwrap()
            .model_prices
            .into_iter()
            .map(|price| price.model)
            .collect();
        // a negative rate does parse: it is `sanitize_for_snapshot`'s to omit,
        // with its own problem line
        assert_eq!(models, ["good", "negative"]);

        let problems = store.load_problems().await.unwrap();
        assert_eq!(problems.len(), 3, "{problems:?}");
        for (model, field) in [
            ("nan-cached", "cached_input_per_mtok"),
            ("nan-input", "input_per_mtok"),
            ("nan-write", "cache_write_per_mtok"),
        ] {
            assert!(
                problems
                    .iter()
                    .any(|p| p.contains(&format!("'{model}'")) && p.contains(field)),
                "{model}: {problems:?}"
            );
        }
        assert!(
            problems.iter().all(|p| p.contains("unpriced")),
            "{problems:?}"
        );
    }

    // #2889: a migration must not brick a deployment that already holds a bad
    // row, and the rule must still bind every write from then on
    #[tokio::test]
    async fn the_rate_checks_migration_applies_over_a_bad_row_and_binds_new_writes() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        // a database migrated cleanly has every constraint validated
        let clean = validated_rate_checks(&pool).await;
        assert_eq!(clean.len(), 4, "{clean:?}");
        assert!(clean.iter().all(|(_, validated)| *validated), "{clean:?}");

        // the same database as an older control plane left it: no constraints,
        // a NaN input and a negative output (and 0083's own check, which lets
        // NaN through, gone with the rest)
        drop_rate_checks(&pool).await;
        sqlx::query(
            "insert into model_prices (model, input_per_mtok, output_per_mtok)
             values ('good', 3, 15), ('nan-input', 'NaN', 15), ('negative-output', 3, -1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(include_str!(
            "../../migrations/0084_model_price_rate_checks.sql"
        ))
        .execute(&pool)
        .await
        .expect("the migration applies over a row that breaks it");

        // unvalidated where a row breaks the rule, validated where none does
        assert_eq!(
            validated_rate_checks(&pool).await,
            [
                ("model_prices_cache_write_per_mtok_rate".to_string(), true),
                ("model_prices_cached_input_per_mtok_rate".to_string(), true),
                ("model_prices_input_per_mtok_rate".to_string(), false),
                ("model_prices_output_per_mtok_rate".to_string(), false),
            ]
        );

        // every write from now on is held to it, whichever column it is in
        for (column, value) in [
            ("input_per_mtok", "'NaN'"),
            ("input_per_mtok", "-0.5"),
            ("output_per_mtok", "'NaN'"),
            ("output_per_mtok", "-0.5"),
            ("cached_input_per_mtok", "'NaN'"),
            ("cached_input_per_mtok", "-0.5"),
            ("cache_write_per_mtok", "'NaN'"),
            ("cache_write_per_mtok", "-0.5"),
        ] {
            let insert =
                format!("insert into model_prices (model, {column}) values ('new', {value})");
            assert!(
                sqlx::query(sqlx::AssertSqlSafe(insert))
                    .execute(&pool)
                    .await
                    .is_err(),
                "{column} = {value} should be refused"
            );
            let update = format!("update model_prices set {column} = {value} where model = 'good'");
            assert!(
                sqlx::query(sqlx::AssertSqlSafe(update))
                    .execute(&pool)
                    .await
                    .is_err(),
                "{column} = {value} should be refused on update"
            );
        }
        sqlx::query(
            "insert into model_prices (model, input_per_mtok, output_per_mtok, cached_input_per_mtok,
                                       cache_write_per_mtok)
             values ('free', 0, 0, 0, 0)",
        )
        .execute(&pool)
        .await
        .expect("zero is a rate");

        // the operator's fix: correct the rows, then validate
        sqlx::query("update model_prices set input_per_mtok = 3 where model = 'nan-input'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("update model_prices set output_per_mtok = 15 where model = 'negative-output'")
            .execute(&pool)
            .await
            .unwrap();
        for name in RATE_CHECKS {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "alter table model_prices validate constraint {name}"
            )))
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("{name} should validate once the rows are fixed: {e}"));
        }
        let fixed = validated_rate_checks(&pool).await;
        assert!(fixed.iter().all(|(_, validated)| *validated), "{fixed:?}");
    }

    // #2891: the 1 hour rate is held to the same rule from the day it exists:
    // at least zero and not `NaN`, which `numeric` sorts above every number
    #[tokio::test]
    async fn the_one_hour_rate_is_held_to_the_same_check_as_the_others() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        let validated: Vec<(String, bool)> = sqlx::query_as(
            "select conname::text, convalidated from pg_constraint
             where conrelid = 'model_prices'::regclass
               and conname = 'model_prices_cache_write_1h_per_mtok_rate'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            validated,
            [(
                "model_prices_cache_write_1h_per_mtok_rate".to_string(),
                true
            )]
        );

        for value in ["'NaN'", "-0.5"] {
            let insert = format!(
                "insert into model_prices (model, cache_write_1h_per_mtok) values ('new', {value})"
            );
            assert!(
                sqlx::query(sqlx::AssertSqlSafe(insert))
                    .execute(&pool)
                    .await
                    .is_err(),
                "cache_write_1h_per_mtok = {value} should be refused"
            );
        }
        sqlx::query(
            "insert into model_prices (model, input_per_mtok, output_per_mtok, cache_write_1h_per_mtok)
             values ('free', 0, 0, 0), ('unset', 0, 0, null)",
        )
        .execute(&pool)
        .await
        .expect("zero and absent are rates");
    }

    // #2891: like the other rates, a 1 hour rate no `Decimal` can hold is left
    // out of the snapshot and reported, never read as zero, and a negative one
    // is for `sanitize_for_snapshot` to omit
    #[tokio::test]
    async fn a_price_with_a_nan_one_hour_rate_is_left_out_and_reported() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        // a database that predates the check, as the 0084 tests recreate one
        sqlx::query(
            "alter table model_prices drop constraint model_prices_cache_write_1h_per_mtok_rate",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into model_prices (model, input_per_mtok, output_per_mtok, cache_write_1h_per_mtok)
             values ('good', 3, 15, 6), ('nan-long', 3, 15, 'NaN'), ('negative-long', 3, 15, -1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let store = PostgresConfigStore::new(pool);
        let models: Vec<String> = store
            .load()
            .await
            .unwrap()
            .model_prices
            .into_iter()
            .map(|price| price.model)
            .collect();
        assert_eq!(models, ["good", "negative-long"]);

        let problems = store.load_problems().await.unwrap();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("'nan-long'")
                && problems[0].contains("cache_write_1h_per_mtok")
                && problems[0].contains("unpriced"),
            "{problems:?}"
        );
    }

    #[tokio::test]
    async fn loads_published_prompt_templates_from_db() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let team_id: Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'default') returning id",
        )
        .bind(team_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let route_a: Uuid = sqlx::query_scalar(
            "insert into routes (project_id, model, strategy)
             values ($1, 'gpt-4o-mini', 'round_robin')
             returning id",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into routes (project_id, model, strategy)
             values ($1, 'gpt-4o', 'round_robin')",
        )
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();

        let template_id: Uuid = sqlx::query_scalar(
            "insert into prompt_templates (org_id, name, slug, description)
             values ($1, 'support baseline', 'support-baseline', 'desc')
             returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into prompt_template_versions (template_id, version, variables, decorators)
             values
                ($1, 1, '[]'::jsonb, '[]'::jsonb),
                ($1, 2, '[{\"name\":\"tone\",\"required\":true}]'::jsonb,
                        '[{\"role\":\"system\",\"position\":\"prepend\",\"content\":\"tone={{ tone }}\"}]'::jsonb)",
        )
        .bind(template_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into prompt_template_scopes (
                 template_id, version, scope_type, scope_id, project_id, route_id
             )
             values
                ($1, 2, 'project', $2, $2, null),
                ($1, 2, 'route', $3, null, $3)",
        )
        .bind(template_id)
        .bind(project_id)
        .bind(route_a)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("update prompt_templates set published_version = 2 where id = $1")
            .bind(template_id)
            .execute(&pool)
            .await
            .unwrap();

        let global_template_id: Uuid = sqlx::query_scalar(
            "insert into prompt_templates (org_id, name, slug, description)
             values ($1, 'global baseline', 'global-baseline', 'desc')
             returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into prompt_template_versions (template_id, version, variables, decorators)
             values
                ($1, 1, '[]'::jsonb, '[{\"role\":\"system\",\"position\":\"prepend\",\"content\":\"always on\"}]'::jsonb)",
        )
        .bind(global_template_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into prompt_template_scopes (
                 template_id, version, scope_type, scope_id, org_id
             )
             values ($1, 1, 'org', $2, $2)",
        )
        .bind(global_template_id)
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("update prompt_templates set published_version = 1 where id = $1")
            .bind(global_template_id)
            .execute(&pool)
            .await
            .unwrap();

        let store = PostgresConfigStore::new(pool);
        let config = store.load().await.unwrap();
        assert!(config.prompt_templates.enabled);
        assert_eq!(config.prompt_templates.templates.len(), 2);

        let global = config
            .prompt_templates
            .templates
            .iter()
            .find(|template| template.id == format!("{org_id}:global-baseline"))
            .unwrap();
        assert_eq!(global.version, 1);
        assert!(global.routes.is_empty());
        assert_eq!(
            global.scopes,
            vec![PromptTemplateActivationScope::Org {
                id: org_id.to_string()
            }]
        );

        let scoped = config
            .prompt_templates
            .templates
            .iter()
            .find(|template| template.id == format!("{org_id}:support-baseline"))
            .unwrap();
        assert_eq!(scoped.version, 2);
        assert_eq!(
            scoped.scopes,
            vec![
                PromptTemplateActivationScope::Project {
                    id: project_id.to_string()
                },
                PromptTemplateActivationScope::Route {
                    project_id: project_id.to_string(),
                    model: "gpt-4o-mini".to_string()
                }
            ]
        );
        assert_eq!(scoped.variables.len(), 1);
        assert_eq!(scoped.decorators.len(), 1);
    }

    // regression: like model_prices, the snapshot query must cast budgets.limit_usd
    // (numeric) to text — Budget.limit_usd is a String — or store.load() errors and
    // every polling gateway freezes on its last config the moment any budget exists
    #[tokio::test]
    async fn loads_budgets_from_db() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into budgets (scope_type, scope_id, limit_usd, period)
             values ('org', $1, 100.5, '30d')",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();

        let store = PostgresConfigStore::new(pool);
        // must not error decoding the numeric column into Budget.limit_usd (String)
        let config = store.load().await.unwrap();

        assert_eq!(config.budgets.len(), 1);
        assert_eq!(config.budgets[0].limit_usd, d("100.5"));
    }

    // governance dimensions carry their own caps, so a business-unit budget and
    // a customer rate limit must survive the snapshot projection (#539)
    #[tokio::test]
    async fn loads_governance_scoped_budgets_and_limits() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let unit_id: Uuid = sqlx::query_scalar(
            "insert into business_units (org_id, name, slug) values ($1, 'Payments', 'payments') returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let customer_id: Uuid = sqlx::query_scalar(
            "insert into customers (org_id, name, slug) values ($1, 'Acme EU', 'acme-eu') returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into budgets (scope_type, scope_id, limit_usd, period)
             values ('business_unit', $1, 250.0, '30d')",
        )
        .bind(unit_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into rate_limits (scope_type, scope_id, rpm, tpm)
             values ('customer', $1, 60, 90000)",
        )
        .bind(customer_id)
        .execute(&pool)
        .await
        .unwrap();

        let config = PostgresConfigStore::new(pool).load().await.unwrap();

        let budget = config
            .budgets
            .iter()
            .find(|b| b.scope == BudgetScope::BusinessUnit)
            .expect("business-unit budget in snapshot");
        assert_eq!(budget.id, unit_id.to_string());
        assert_eq!(budget.limit_usd, d("250.0"));

        let limit = config
            .rate_limits
            .iter()
            .find(|l| l.scope == BudgetScope::Customer)
            .expect("customer rate limit in snapshot");
        assert_eq!(limit.id, customer_id.to_string());
        assert_eq!(limit.rpm, Some(60));
    }

    // a budget stored before the control plane checked its period keeps being
    // enforced as it was, monthly, but is reported rather than passing as the
    // 7d cap the row claims to be (#1902)
    #[tokio::test]
    async fn an_unrecognised_budget_period_is_enforced_monthly_and_reported() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let mut ids = Vec::new();
        for period in ["30d", "Daily", "7d", "dialy"] {
            let id: Uuid = sqlx::query_scalar(
                "insert into budgets (scope_type, scope_id, limit_usd, period)
                 values ('org', $1, 10, $2) returning id",
            )
            .bind(org_id)
            .bind(period)
            .fetch_one(&pool)
            .await
            .unwrap();
            ids.push(id);
        }

        let store = PostgresConfigStore::new(pool);
        let periods: Vec<BudgetPeriod> = store
            .load()
            .await
            .unwrap()
            .budgets
            .iter()
            .map(|b| b.period)
            .collect();
        assert_eq!(
            periods,
            [
                BudgetPeriod::Monthly,
                BudgetPeriod::Daily,
                BudgetPeriod::Monthly,
                BudgetPeriod::Monthly,
            ]
        );

        let problems = store.load_problems().await.unwrap();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains(&ids[2].to_string()), "{}", problems[0]);
        assert!(problems[0].contains("period '7d'"), "{}", problems[0]);
        assert!(problems[0].contains("monthly"), "{}", problems[0]);
        assert!(problems[1].contains("period 'dialy'"), "{}", problems[1]);
        assert!(
            problems[1].contains(&format!("org '{org_id}'")),
            "{}",
            problems[1]
        );
    }

    /// #3446: a stored value this build reads as `Unknown` no longer fails the
    /// snapshot, so the problems list is where the row is explained: each
    /// kind names its entity, the value and what happens to the row, owned by
    /// its tenant where it has one. The values are written with plain SQL past
    /// the check constraints, as a newer release or a downgrade leaves them.
    #[tokio::test]
    async fn rows_read_as_unknown_are_listed_in_the_problems() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        // the constraints a newer release's migrations would have widened
        sqlx::query(
            "do $$ declare c record; begin
               for c in select conname, conrelid::regclass::text as t from pg_constraint
                        where contype = 'c'
                          and conrelid::regclass::text in
                              ('routes', 'providers', 'budgets', 'rate_limits', 'route_targets',
                               'mcp_servers')
               loop execute format('alter table %s drop constraint %I', c.t, c.conname);
               end loop; end $$",
        )
        .execute(&pool)
        .await
        .unwrap();
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let team_id: Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'api') returning id",
        )
        .bind(team_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let known: Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, slug, kind, api_base)
             values ($1, 'known', 'known', 'openai', 'https://a.example') returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into providers (org_id, name, slug, kind, api_base)
             values ($1, 'future', 'future', 'quantum', 'https://q.example')",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into routes (project_id, model, strategy)
             values ($1, 'guessing', 'mind_reading')",
        )
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
        let route: Uuid = sqlx::query_scalar(
            "insert into routes (project_id, model, strategy)
             values ($1, 'kept', 'round_robin') returning id",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into route_targets (route_id, provider_id, weight, takes_over)
             values ($1, $2, 1, array['quantum_flux'])",
        )
        .bind(route)
        .bind(known)
        .execute(&pool)
        .await
        .unwrap();
        let galaxy: Uuid = sqlx::query_scalar(
            "insert into budgets (scope_type, scope_id, limit_usd, period)
             values ('galaxy', $1, 10, 'monthly') returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into budgets (scope_type, scope_id, limit_usd, period)
             values ('org', $1, 10, 'fortnightly')",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
        let limit: Uuid = sqlx::query_scalar(
            "insert into rate_limits (scope_type, scope_id, rpm)
             values ('galaxy', $1, 10) returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into mcp_servers (org_id, name, slug, url, transport, auth_kind)
             values ($1, 'tools', 'tools', 'https://mcp.example', 'streamable_http', 'telepathy')",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();

        let store = PostgresConfigStore::new(pool);
        let problems = store.load_problem_records().await.unwrap();
        let find = |needle: &str| {
            problems
                .iter()
                .find(|p| p.message.contains(needle))
                .unwrap_or_else(|| panic!("no problem names {needle}: {problems:?}"))
        };
        let tenant = crate::ProblemOwner::Tenant(rolter_core::Tenancy {
            org_id: org_id.to_string(),
            project_id: Some(project_id.to_string()),
        });

        let strategy = find("balancing strategy 'mind_reading'");
        assert!(
            strategy.message.contains("route 'guessing'"),
            "{strategy:?}"
        );
        assert!(strategy.message.contains("left out"), "{strategy:?}");
        assert_eq!(strategy.owner, tenant);

        let kind = find("has kind 'quantum'");
        assert!(kind.message.contains("provider 'future'"), "{kind:?}");
        assert!(kind.message.contains("left out"), "{kind:?}");
        assert_eq!(
            kind.owner,
            crate::ProblemOwner::Tenant(rolter_core::Tenancy {
                org_id: org_id.to_string(),
                project_id: None,
            })
        );

        let scope = find(&format!("budget '{galaxy}' has scope 'galaxy'"));
        assert!(scope.message.contains("every request"), "{scope:?}");
        let rate = find(&format!("rate limit '{limit}'"));
        assert!(rate.message.contains("scope 'galaxy'"), "{rate:?}");

        let mcp = find("auth kind 'telepathy'");
        assert!(mcp.message.contains("mcp server 'tools'"), "{mcp:?}");
        assert!(mcp.message.contains("refused"), "{mcp:?}");
        assert_eq!(
            mcp.owner,
            crate::ProblemOwner::Tenant(rolter_core::Tenancy {
                org_id: org_id.to_string(),
                project_id: None,
            })
        );

        let period = find("period 'fortnightly'");
        assert!(period.message.contains("monthly"), "{period:?}");

        let class = find("quantum_flux");
        assert!(class.message.contains("route 'kept'"), "{class:?}");
        assert!(class.message.contains("omitted"), "{class:?}");
        assert_eq!(class.owner, tenant);
    }

    /// #3469: one org's provider row of a kind this build does not know used
    /// to fail the whole load, so no tenant's config was published. It now
    /// reads as `Unknown`, the sanitizer leaves out that provider and the route
    /// that named it, and every other org's config loads and serves.
    #[tokio::test]
    async fn an_unknown_provider_kind_is_left_out_and_the_other_orgs_still_load() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        sqlx::query("alter table providers drop constraint if exists providers_kind_check")
            .execute(&pool)
            .await
            .unwrap();
        let mut projects = Vec::new();
        for (slug, provider, kind) in [("acme", "known", "openai"), ("newer", "future", "quantum")]
        {
            let org: Uuid =
                sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
                    .bind(slug)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let team: Uuid = sqlx::query_scalar(
                "insert into teams (org_id, name) values ($1, 'core') returning id",
            )
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
            let project: Uuid = sqlx::query_scalar(
                "insert into projects (team_id, name) values ($1, 'api') returning id",
            )
            .bind(team)
            .fetch_one(&pool)
            .await
            .unwrap();
            let provider_id: Uuid = sqlx::query_scalar(
                "insert into providers (org_id, name, slug, kind, api_base)
                 values ($1, $2, $2, $3, 'https://a.example') returning id",
            )
            .bind(org)
            .bind(provider)
            .bind(kind)
            .fetch_one(&pool)
            .await
            .unwrap();
            let route: Uuid = sqlx::query_scalar(
                "insert into routes (project_id, model, strategy)
                 values ($1, $2, 'round_robin') returning id",
            )
            .bind(project)
            .bind(format!("{slug}-model"))
            .fetch_one(&pool)
            .await
            .unwrap();
            sqlx::query(
                "insert into route_targets (route_id, provider_id, weight) values ($1, $2, 1)",
            )
            .bind(route)
            .bind(provider_id)
            .execute(&pool)
            .await
            .unwrap();
            projects.push(project);
        }

        let mut config = PostgresConfigStore::new(pool).load().await.unwrap();
        let unknown = config.fail_closed_unknown_report();
        assert_eq!(unknown.kinds.get("provider_kind"), Some(&1), "{unknown:?}");
        let providers: Vec<&str> = config.providers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(providers, ["known"]);
        let routes: Vec<&str> = config.routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(routes, ["acme-model"]);
        config
            .validate_snapshot()
            .expect("the rest of the config is servable");
    }

    /// #3470: a budget or rate limit on a scope this build does not know was
    /// skipped, which left its cap unenforced. It is now loaded as
    /// `BudgetScope::Unknown`, the reading the gateway enforces as binding on
    /// every request (#3347), and the snapshot still validates.
    #[tokio::test]
    async fn a_cap_on_an_unknown_scope_loads_as_binding() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        sqlx::query(
            "do $$ declare c record; begin
               for c in select conname, conrelid::regclass::text as t from pg_constraint
                        where contype = 'c'
                          and conrelid::regclass::text in ('budgets', 'rate_limits')
               loop execute format('alter table %s drop constraint %I', c.t, c.conname);
               end loop; end $$",
        )
        .execute(&pool)
        .await
        .unwrap();
        let scope_id = Uuid::new_v4();
        sqlx::query(
            "insert into budgets (scope_type, scope_id, limit_usd, period)
             values ('galaxy', $1, 5, 'monthly')",
        )
        .bind(scope_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("insert into rate_limits (scope_type, scope_id, rpm) values ('galaxy', $1, 1)")
            .bind(scope_id)
            .execute(&pool)
            .await
            .unwrap();

        let mut config = PostgresConfigStore::new(pool).load().await.unwrap();
        assert_eq!(config.budgets.len(), 1, "{:?}", config.budgets);
        assert_eq!(config.budgets[0].scope, BudgetScope::Unknown);
        assert_eq!(config.budgets[0].id, scope_id.to_string());
        assert_eq!(config.rate_limits.len(), 1, "{:?}", config.rate_limits);
        assert_eq!(config.rate_limits[0].scope, BudgetScope::Unknown);
        assert_eq!(config.rate_limits[0].rpm, Some(1));
        let unknown = config.fail_closed_unknown_report();
        assert_eq!(unknown.kinds.get("budget_scope"), Some(&1), "{unknown:?}");
        assert_eq!(
            unknown.kinds.get("rate_limit_scope"),
            Some(&1),
            "{unknown:?}"
        );
        config
            .validate_snapshot()
            .expect("the caps travel in the snapshot");
    }

    // the adaptive-routing kill switch and blend weights are control-plane
    // owned, so an operator change must reach the polling gateway (#544)
    #[tokio::test]
    async fn adaptive_routing_policy_projects_into_snapshot() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        // the shipped defaults leave adaptive routing off
        let config = PostgresConfigStore::new(pool.clone()).load().await.unwrap();
        assert!(!config.adaptive_routing.enabled);
        assert_eq!(config.adaptive_routing.min_samples, 50);

        sqlx::query(
            "update adaptive_routing_policy set enabled = true, latency_weight = 2.0, \
                    cost_weight = 0, load_weight = 0.5, exploration_ratio = 0.1, \
                    min_samples = 10 where id = true",
        )
        .execute(&pool)
        .await
        .unwrap();

        let config = PostgresConfigStore::new(pool).load().await.unwrap();
        assert!(config.adaptive_routing.enabled);
        assert_eq!(config.adaptive_routing.latency_weight, 2.0);
        assert_eq!(config.adaptive_routing.cost_weight, 0.0);
        assert_eq!(config.adaptive_routing.exploration_ratio, 0.1);
        assert_eq!(config.adaptive_routing.min_samples, 10);
    }

    #[tokio::test]
    async fn guardrail_registry_projects_into_snapshot() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        sqlx::query(
            "insert into guardrail_rules \
             (name, source_type, builtin, stage, action, replacement, position) \
             values ('email-redaction', 'builtin', 'email', 'pre_call', 'redact', \
                     '[MASKED]', 10)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into guardrail_providers \
             (name, enabled, url, failure_mode, auth_kind, auth_env) \
             values ('policy-service', true, 'https://guardrails.internal/evaluate', \
                     'fail_closed', 'bearer', 'ROLTER_GUARDRAIL_TOKEN')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let config = PostgresConfigStore::new(pool).load().await.unwrap();
        assert!(config.guardrails.enabled);
        assert_eq!(config.guardrails.rules.len(), 1);
        assert_eq!(config.guardrails.rules[0].name, "email-redaction");
        assert!(matches!(
            config.guardrails.rules[0].builtin,
            Some(BuiltinRule::Email)
        ));
        // registry providers reach the gateway as semantic guard providers
        // (#459); `[guardrail_webhook]` is the config file's alone
        assert!(!config.guardrail_webhook.enabled);
        let provider = &config.semantic_guards.providers[0];
        assert!(provider.enabled);
        assert_eq!(provider.name, "policy-service");
        assert_eq!(provider.url, "https://guardrails.internal/evaluate");
        assert!(matches!(provider.failure_mode, FailureMode::FailClosed));
        assert!(matches!(
            provider.auth,
            Some(WebhookAuth::Bearer { ref token_env }) if token_env == "ROLTER_GUARDRAIL_TOKEN"
        ));
    }

    // #567 shipped the registry without gateway consumption; #509 wires the
    // dispatch runtime, so an enabled instance must now reach the snapshot
    #[tokio::test]
    async fn plugin_registry_projects_enabled_instances_into_snapshot() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let team_id: Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'default') returning id",
        )
        .bind(team_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::query(
            "insert into plugin_instances \
             (org_id, name, slug, kind, stage, enabled, position, failure_mode, endpoint, \
              secret_env) \
             values ($1, 'audit', 'audit', 'webhook', 'pre_upstream', true, 10, 'fail_closed', \
                     'https://plugins.internal/audit', 'ROLTER_PLUGIN_TOKEN')",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
        // disabled rows never reach the snapshot
        sqlx::query(
            "insert into plugin_instances \
             (org_id, name, slug, kind, stage, enabled, position, failure_mode, endpoint) \
             values ($1, 'disabled', 'disabled', 'webhook', 'pre_route', false, 0, 'fail_open', \
                     'https://plugins.internal/disabled')",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
        // project-scoped rows carry their project id through
        sqlx::query(
            "insert into plugin_instances \
             (org_id, project_id, name, slug, kind, stage, enabled, position, failure_mode, \
              endpoint) \
             values ($1, $2, 'project-audit', 'project-audit', 'webhook', 'post_response', \
                     true, 5, 'fail_open', 'https://plugins.internal/project-audit')",
        )
        .bind(org_id)
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();

        let config = PostgresConfigStore::new(pool).load().await.unwrap();
        assert_eq!(config.plugins.instances.len(), 2);

        let audit = config
            .plugins
            .instances
            .iter()
            .find(|p| p.slug == "audit")
            .unwrap();
        assert_eq!(audit.org_id, org_id.to_string());
        assert!(audit.project_id.is_none());
        assert_eq!(audit.stage, rolter_core::PluginStage::PreUpstream);
        assert!(matches!(audit.failure_mode, FailureMode::FailClosed));
        assert_eq!(audit.endpoint, "https://plugins.internal/audit");
        assert!(matches!(
            audit.auth,
            Some(WebhookAuth::Bearer { ref token_env }) if token_env == "ROLTER_PLUGIN_TOKEN"
        ));

        let project_audit = config
            .plugins
            .instances
            .iter()
            .find(|p| p.slug == "project-audit")
            .unwrap();
        assert_eq!(
            project_audit.project_id.as_deref(),
            Some(project_id.to_string().as_str())
        );
        assert_eq!(project_audit.stage, rolter_core::PluginStage::PostResponse);
    }

    #[tokio::test]
    async fn plugin_instances_writes_bump_config_version() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let v0 = current_version(&pool).await.unwrap();

        let plugin_id: Uuid = sqlx::query_scalar(
            "insert into plugin_instances \
             (org_id, name, slug, kind, stage, enabled, position, failure_mode, endpoint) \
             values ($1, 'audit', 'audit', 'webhook', 'pre_upstream', true, 0, 'fail_open', \
                     'https://plugins.internal/audit') returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 1);

        sqlx::query("update plugin_instances set enabled = false where id = $1")
            .bind(plugin_id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 2);

        sqlx::query("delete from plugin_instances where id = $1")
            .bind(plugin_id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 3);
    }

    // compiled-in translation constants are control-plane owned now, so the
    // persisted policy must reach the snapshot the gateway polls (#546)
    #[tokio::test]
    async fn compatibility_policy_projects_into_snapshot() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        // defaults preserve the previous hardcoded behavior
        let config = PostgresConfigStore::new(pool.clone()).load().await.unwrap();
        assert_eq!(config.compatibility.anthropic_version, "2023-06-01");
        assert_eq!(config.compatibility.default_max_tokens, 1024);

        sqlx::query(
            "update compatibility_policy set anthropic_version = '2024-10-22', \
                    default_max_tokens = 4096 where id = true",
        )
        .execute(&pool)
        .await
        .unwrap();

        let config = PostgresConfigStore::new(pool).load().await.unwrap();
        assert_eq!(config.compatibility.anthropic_version, "2024-10-22");
        assert_eq!(config.compatibility.default_max_tokens, 4096);
    }

    #[tokio::test]
    async fn save_is_read_only() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let store = PostgresConfigStore::new(pool);
        assert!(store.save(GatewayConfig::default()).await.is_err());
    }

    /// The write path refuses a route target or a group member on another
    /// org's provider, but a row written before that guard existed is still in
    /// the table, and the loader is all that keeps it from spending the other
    /// org's credential (#1844). Every other test creates rows through the
    /// guarded API, so this one writes the cross-org rows with plain SQL.
    #[tokio::test]
    async fn the_loader_drops_targets_and_members_on_another_orgs_provider() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        // two orgs, each with one project and one provider
        let mut tenants = Vec::new();
        for org in ["org-a", "org-b"] {
            let org_id: Uuid =
                sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
                    .bind(org)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let team_id: Uuid = sqlx::query_scalar(
                "insert into teams (org_id, name) values ($1, 'core') returning id",
            )
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            let project_id: Uuid = sqlx::query_scalar(
                "insert into projects (team_id, name) values ($1, 'api') returning id",
            )
            .bind(team_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            let provider_id: Uuid = sqlx::query_scalar(
                "insert into providers (org_id, name, slug, kind, api_base)
                 values ($1, $2, $2, 'openai', 'https://example.com') returning id",
            )
            .bind(org_id)
            .bind(format!("{org}-edge"))
            .fetch_one(&pool)
            .await
            .unwrap();
            tenants.push((org_id, project_id, provider_id));
        }
        let (org_a, project_a, own) = tenants[0];
        let foreign = tenants[1].2;

        let route_id: Uuid = sqlx::query_scalar(
            "insert into routes (project_id, model, strategy)
             values ($1, 'gpt-4o', 'round_robin') returning id",
        )
        .bind(project_a)
        .fetch_one(&pool)
        .await
        .unwrap();
        let group_id: Uuid = sqlx::query_scalar(
            "insert into provider_groups (org_id, name, slug, strategy)
             values ($1, 'pool', 'pool', 'round_robin') returning id",
        )
        .bind(org_a)
        .fetch_one(&pool)
        .await
        .unwrap();
        for (position, provider_id) in [own, foreign].into_iter().enumerate() {
            sqlx::query(
                "insert into route_targets (route_id, provider_id, weight) values ($1, $2, 1)",
            )
            .bind(route_id)
            .bind(provider_id)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "insert into provider_group_members (group_id, provider_id, weight, position)
                 values ($1, $2, 1, $3)",
            )
            .bind(group_id)
            .bind(provider_id)
            .bind(position as i32)
            .execute(&pool)
            .await
            .unwrap();
        }

        let store = PostgresConfigStore::new(pool);
        let routes = store.load_routes(&HashSet::new()).await.unwrap();
        let route = routes
            .iter()
            .find(|r| r.model == "gpt-4o")
            .expect("the route itself still loads");
        let targets: Vec<&str> = route.targets.iter().map(|t| t.provider.as_str()).collect();
        assert_eq!(
            targets,
            ["org-a-edge"],
            "a cross-org target survived the load"
        );

        let groups = store.load_provider_groups(&HashSet::new()).await.unwrap();
        let group = groups
            .iter()
            .find(|g| g.slug.as_deref() == Some("pool"))
            .expect("the group itself still loads");
        let members: Vec<&str> = group.members.iter().map(|m| m.provider.as_str()).collect();
        assert_eq!(
            members,
            ["org-a-edge"],
            "a cross-org member survived the load"
        );
    }

    // #2922: `MergedConfigStore` treats a dashboard-owned singleton equal to the
    // default as "nobody has saved this", so a migrated store has to project
    // exactly the default or the file's value would never be used (or, the
    // other way round, would always lose). Pin that against a real schema
    #[tokio::test]
    async fn merged_store_a_migrated_store_projects_the_defaults_of_the_settings_it_owns() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let config = PostgresConfigStore::new(db.pool().clone())
            .load()
            .await
            .unwrap();
        assert_eq!(config.client, rolter_core::ClientConfig::default());
        assert_eq!(
            config.security,
            rolter_core::SecurityPolicyConfig::default()
        );
        assert_eq!(
            config.model_defaults,
            rolter_core::ModelDefaultsConfig::default()
        );
        // #2931: the runtime policies follow the same rule
        assert_eq!(config.retry, rolter_core::RetryConfig::default());
        assert_eq!(config.timeouts, rolter_core::TimeoutConfig::default());
        assert_eq!(config.queue, rolter_core::QueueConfig::default());
        assert_eq!(
            config.compatibility,
            rolter_core::CompatibilityConfig::default()
        );
        assert_eq!(
            config.adaptive_routing,
            rolter_core::AdaptiveRoutingConfig::default()
        );
        assert_eq!(config.logging, rolter_core::LoggingConfig::default());
    }

    fn merged_file_config() -> GatewayConfig {
        let mut file = GatewayConfig::default();
        file.client.request_id_header = "x-file".to_string();
        file.model_defaults.enabled = true;
        file.model_defaults.temperature = Some(0.9);
        file.security
            .required_headers
            .insert("x-mesh".to_string(), "file".to_string());
        file.retry.max_retries = 9;
        file.timeouts.request_secs = 600;
        file.queue.capacity = 4096;
        file.compatibility.default_max_tokens = 11;
        file.adaptive_routing.min_samples = 7;
        file.logging.clickhouse_url = Some("http://ch:8123".to_string());
        file.logging.batch_max = 7;
        file
    }

    #[tokio::test]
    async fn merged_store_over_an_untouched_postgres_store_keeps_the_files_settings() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let file = merged_file_config();
        let merged = crate::MergedConfigStore::new(
            file.clone(),
            std::sync::Arc::new(PostgresConfigStore::new(db.pool().clone())),
        )
        .load()
        .await
        .unwrap();

        assert_eq!(merged.client, file.client);
        assert_eq!(merged.security, file.security);
        assert_eq!(merged.model_defaults, file.model_defaults);
        assert_eq!(merged.retry, file.retry);
        assert_eq!(merged.timeouts, file.timeouts);
        assert_eq!(merged.queue, file.queue);
        assert_eq!(merged.compatibility, file.compatibility);
        assert_eq!(merged.adaptive_routing, file.adaptive_routing);
        assert_eq!(merged.logging, file.logging);
    }

    #[tokio::test]
    async fn merged_store_over_postgres_serves_what_the_dashboard_saved() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        // the same writes the Client Settings, Model Settings and Security
        // screens make
        sqlx::query(
            "update client_settings set request_id_header = 'x-db', \
             injected_headers = '{\"x-team\":\"platform\"}'::jsonb where id = true",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "update model_defaults set enabled = false, default_temperature = 0.2 where id = true",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "update security_settings set required_headers = '{\"x-waf\":\"db\"}'::jsonb, \
             auth_bypass_routes = '{/v1/models}' where id = true",
        )
        .execute(&pool)
        .await
        .unwrap();

        let merged = crate::MergedConfigStore::new(
            merged_file_config(),
            std::sync::Arc::new(PostgresConfigStore::new(pool)),
        )
        .load()
        .await
        .unwrap();

        assert_eq!(merged.client.request_id_header, "x-db");
        assert_eq!(merged.client.injected_headers["x-team"], "platform");
        assert!(!merged.model_defaults.enabled);
        assert_eq!(merged.model_defaults.temperature, Some(0.2));
        assert_eq!(merged.security.required_headers.len(), 1);
        assert_eq!(merged.security.required_headers["x-waf"], "db");
        assert_eq!(merged.security.auth_bypass_routes, ["/v1/models"]);
    }

    #[tokio::test]
    async fn merged_store_over_postgres_serves_the_saved_runtime_policies_beside_the_files_log_sink(
    ) {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();

        for statement in [
            "update runtime_policy set retry_max_retries = 1, timeout_request_s = 30, \
             queue_capacity = 8 where id = true",
            "update compatibility_policy set default_max_tokens = 22 where id = true",
            "update adaptive_routing_policy set min_samples = 9 where id = true",
            "update logging_settings set sample_rate = 0.1 where id = true",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }

        let merged = crate::MergedConfigStore::new(
            merged_file_config(),
            std::sync::Arc::new(PostgresConfigStore::new(pool)),
        )
        .load()
        .await
        .unwrap();

        assert_eq!(merged.retry.max_retries, 1);
        assert_eq!(merged.timeouts.request_secs, 30);
        assert_eq!(merged.queue.capacity, 8);
        assert_eq!(merged.compatibility.default_max_tokens, 22);
        assert_eq!(merged.adaptive_routing.min_samples, 9);
        assert_eq!(merged.logging.sample_rate, 0.1);
        // no column holds these, so the saved row cannot have replaced them
        assert_eq!(
            merged.logging.clickhouse_url.as_deref(),
            Some("http://ch:8123")
        );
        assert_eq!(merged.logging.batch_max, 7);
    }

    #[tokio::test]
    async fn merged_store_over_postgres_serves_registered_plugins_and_published_templates() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::query(
            "insert into plugin_instances \
             (org_id, name, slug, kind, stage, enabled, position, failure_mode, endpoint) \
             values ($1, 'audit', 'audit', 'webhook', 'pre_upstream', true, 10, 'fail_open', \
                     'https://plugins.internal/audit')",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();

        // `support` as `rolter-seed --import` leaves it: published in the store
        // under the slug the file calls it by
        let template_id: Uuid = sqlx::query_scalar(
            "insert into prompt_templates (org_id, name, slug, description) \
             values ($1, 'support', 'support', '') returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into prompt_template_versions (template_id, version, variables, decorators) \
             values ($1, 2, '[]'::jsonb, \
                     '[{\"role\":\"system\",\"position\":\"prepend\",\"content\":\"v2\"}]'::jsonb)",
        )
        .bind(template_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into prompt_template_scopes (template_id, version, scope_type, scope_id, org_id) \
             values ($1, 2, 'org', $2, $2)",
        )
        .bind(template_id)
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("update prompt_templates set published_version = 2 where id = $1")
            .bind(template_id)
            .execute(&pool)
            .await
            .unwrap();

        let template = |id: &str, version: u32| rolter_core::PromptTemplate {
            id: id.to_string(),
            version,
            routes: Vec::new(),
            scopes: Vec::new(),
            variables: Vec::new(),
            decorators: vec![rolter_core::Decorator {
                role: Default::default(),
                position: Default::default(),
                content: format!("file {id}"),
            }],
        };
        let mut file = GatewayConfig {
            prompt_templates: rolter_core::PromptTemplatesConfig {
                enabled: true,
                templates: vec![template("support", 1), template("file-only", 1)],
            },
            ..Default::default()
        };
        file.plugins
            .instances
            .push(rolter_core::PluginInstanceConfig {
                slug: "from-file".to_string(),
                org_id: org_id.to_string(),
                project_id: None,
                stage: rolter_core::PluginStage::PreRoute,
                position: 0,
                failure_mode: FailureMode::FailOpen,
                endpoint: "https://file.example".to_string(),
                auth: None,
            });

        let merged = crate::MergedConfigStore::new(
            file,
            std::sync::Arc::new(PostgresConfigStore::new(pool)),
        )
        .load()
        .await
        .unwrap();

        let templates: Vec<String> = merged
            .prompt_templates
            .templates
            .iter()
            .map(|t| format!("{}@{}", t.id, t.version))
            .collect();
        assert_eq!(
            templates,
            [format!("{org_id}:support@2"), "file-only@1".to_string()],
            "the stored template replaces the file's copy of the same slug; the file-only one stays"
        );
        let plugins: Vec<&str> = merged
            .plugins
            .instances
            .iter()
            .map(|p| p.slug.as_str())
            .collect();
        assert_eq!(plugins, ["audit", "from-file"]);
    }

    /// #1584: `takes_over` reaches the snapshot, a change to it bumps
    /// `config_version`, and both ways a stored row could leave a route unable
    /// to dial anything fail closed: a class the gateway cannot read drops its
    /// target rather than serving it as an ordinary one, and a route whose
    /// every remaining target is restricted is left out rather than failing
    /// validation of the whole snapshot.
    #[tokio::test]
    async fn takes_over_reaches_the_snapshot_and_fails_closed() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let team_id: Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'api') returning id",
        )
        .bind(team_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let mut providers = HashMap::new();
        for name in ["cheap", "premium"] {
            let id: Uuid = sqlx::query_scalar(
                "insert into providers (org_id, name, slug, kind, api_base)
                 values ($1, $2, $2, 'openai', 'https://api.openai.com') returning id",
            )
            .bind(org_id)
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap();
            providers.insert(name, id);
        }
        // (route, [(provider, takes_over)])
        type Targets<'a> = &'a [(&'a str, &'a [&'a str])];
        let routes: [(&str, Targets); 3] = [
            ("guarded", &[("cheap", &[]), ("premium", &["refusal"])]),
            ("bad-class", &[("cheap", &[]), ("premium", &["bogus"])]),
            ("stranded", &[("premium", &["refusal"])]),
        ];
        for (model, targets) in routes {
            let route_id: Uuid = sqlx::query_scalar(
                "insert into routes (project_id, model, strategy) values ($1, $2, 'round_robin')
                 returning id",
            )
            .bind(project_id)
            .bind(model)
            .fetch_one(&pool)
            .await
            .unwrap();
            for (provider, takes_over) in targets {
                sqlx::query(
                    "insert into route_targets (route_id, provider_id, weight, takes_over)
                     values ($1, $2, 1, $3)",
                )
                .bind(route_id)
                .bind(providers[provider])
                .bind(takes_over)
                .execute(&pool)
                .await
                .unwrap();
            }
        }

        // a variant's targets carry the same classes (#3373): `split` has a
        // guarded variant, a variant whose only readable target is ordinary,
        // and `split-stranded` only a restricted target across its variants
        type VariantTargets<'a> = &'a [(&'a str, &'a [(&'a str, &'a [&'a str])])];
        let variant_routes: [(&str, VariantTargets); 3] = [
            (
                "split",
                &[
                    ("a", &[("cheap", &[]), ("premium", &["throttled"])]),
                    ("b", &[("cheap", &[]), ("premium", &["bogus"])]),
                ],
            ),
            ("split-stranded", &[("a", &[("premium", &["refusal"])])]),
            // an ordinary pool target the data plane never dials beside
            // variants that hold only a fallback (#3430)
            ("split-pooled", &[("a", &[("premium", &["refusal"])])]),
        ];
        for (model, variants) in variant_routes {
            let route_id: Uuid = sqlx::query_scalar(
                "insert into routes (project_id, model, strategy) values ($1, $2, 'round_robin')
                 returning id",
            )
            .bind(project_id)
            .bind(model)
            .fetch_one(&pool)
            .await
            .unwrap();
            if model == "split-pooled" {
                sqlx::query(
                    "insert into route_targets (route_id, provider_id, weight, takes_over)
                     values ($1, $2, 1, '{}')",
                )
                .bind(route_id)
                .bind(providers["cheap"])
                .execute(&pool)
                .await
                .unwrap();
            }
            for (position, (name, targets)) in variants.iter().enumerate() {
                let variant_id: Uuid = sqlx::query_scalar(
                    "insert into route_variants (route_id, name, weight, position)
                     values ($1, $2, 1, $3) returning id",
                )
                .bind(route_id)
                .bind(name)
                .bind(position as i32)
                .fetch_one(&pool)
                .await
                .unwrap();
                for (position, (provider, takes_over)) in targets.iter().enumerate() {
                    sqlx::query(
                        "insert into route_variant_targets
                             (variant_id, provider_id, weight, position, takes_over)
                         values ($1, $2, 1, $3, $4)",
                    )
                    .bind(variant_id)
                    .bind(providers[provider])
                    .bind(position as i32)
                    .bind(takes_over)
                    .execute(&pool)
                    .await
                    .unwrap();
                }
            }
        }

        let store = PostgresConfigStore::new(pool.clone());
        let config = store.load().await.unwrap();
        let models: Vec<&str> = config.routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, ["bad-class", "guarded", "split"]);
        let split = &config.routes[2];
        assert_eq!(
            split.variants[0].targets[1].takes_over,
            vec!["throttled".parse().unwrap()]
        );
        assert_eq!(
            split.variants[1].targets.len(),
            1,
            "the unreadable variant target is dropped"
        );
        assert_eq!(config.validate_snapshot(), Ok(()));
        let guarded = &config.routes[1];
        assert_eq!(guarded.targets.len(), 2);
        assert_eq!(
            guarded.targets[1].takes_over,
            vec!["refusal".parse().unwrap()]
        );
        let bad = &config.routes[0];
        assert_eq!(bad.targets.len(), 1, "the unreadable target is dropped");
        assert_eq!(bad.targets[0].provider, "cheap");

        // both are reported, owned by the route's tenant (#3321)
        let problems = store.load_problem_records().await.unwrap();
        let tenant = crate::ProblemOwner::Tenant(Tenancy {
            org_id: org_id.to_string(),
            project_id: Some(project_id.to_string()),
        });
        let dropped = problems
            .iter()
            .find(|p| p.message.contains("route 'bad-class'"))
            .unwrap_or_else(|| panic!("no problem for the dropped target: {problems:?}"));
        assert!(
            dropped.message.contains("provider 'premium'"),
            "{dropped:?}"
        );
        assert!(dropped.message.contains("'bogus'"), "{dropped:?}");
        assert_eq!(dropped.owner, tenant);
        let stranded = problems
            .iter()
            .find(|p| p.message.contains("route 'stranded'"))
            .unwrap_or_else(|| panic!("no problem for the left-out route: {problems:?}"));
        assert!(
            stranded
                .message
                .contains(rolter_core::failover::ALL_TARGETS_RESTRICTED),
            "{stranded:?}"
        );
        assert_eq!(stranded.owner, tenant);
        assert!(
            !problems.iter().any(|p| p.message.contains("'guarded'")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|p| p.message.contains("route 'split-pooled'")
                    && p.message
                        .contains(rolter_core::failover::ALL_TARGETS_RESTRICTED)),
            "an ordinary pool target does not rescue all-restricted variants: {problems:?}"
        );
        let variant_dropped = problems
            .iter()
            .find(|p| p.message.contains("route 'split' variant 'b'"))
            .unwrap_or_else(|| panic!("no problem for the variant target: {problems:?}"));
        assert!(
            variant_dropped.message.contains("'bogus'"),
            "{variant_dropped:?}"
        );
        assert_eq!(variant_dropped.owner, tenant);
        assert!(
            problems
                .iter()
                .any(|p| p.message.contains("route 'split-stranded'")
                    && p.message
                        .contains(rolter_core::failover::ALL_TARGETS_RESTRICTED)),
            "{problems:?}"
        );

        let before = current_version(&pool).await.unwrap();
        sqlx::query(
            "update route_targets set takes_over = '{throttled}' where takes_over = '{refusal}'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(current_version(&pool).await.unwrap() > before);
        // the variant targets' existing trigger (0139) covers the new column
        let before = current_version(&pool).await.unwrap();
        sqlx::query("update route_variant_targets set takes_over = '{timeout}' where takes_over = '{throttled}'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(current_version(&pool).await.unwrap() > before);
    }

    /// #3321: when the assembly cannot read the routes at all, only the
    /// routes' own part of the problem list fails, not the list: every other
    /// explanation is still there, with a line saying the routes could not be
    /// read. A strategy no build knows no longer fails the read since #3347 (it
    /// loads as `Unknown`), so a table the assembly reads is taken away instead.
    #[tokio::test]
    async fn an_unreadable_route_keeps_the_other_problems() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let team_id: Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'api') returning id",
        )
        .bind(team_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into routes (project_id, model, strategy, params)
             values ($1, 'bad-params', 'round_robin', '[\"max_tokens\"]'::jsonb)",
        )
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
        // the assembly reads the variant targets, so without the table it
        // cannot read the routes at all. the schema is this test's own
        sqlx::query("drop table route_variant_targets")
            .execute(&pool)
            .await
            .unwrap();

        let store = PostgresConfigStore::new(pool.clone());
        let problems = store.load_problem_records().await.unwrap();
        assert!(
            problems.iter().any(|p| p.message.contains("bad-params")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|p| p.message.contains("the routes could not be read")
                    && p.message.contains("route_variant_targets")),
            "{problems:?}"
        );
    }

    /// #2938: a route whose jsonb settings do not parse used to load on the
    /// type's default, which is public, unrestricted and free to override. The
    /// control plane parses every write, so the rows here are written with
    /// plain SQL, the way a seed, an import or an older version of the type
    /// could leave them.
    #[tokio::test]
    async fn a_route_whose_settings_do_not_parse_is_left_out_not_served_open() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let team_id: Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'api') returning id",
        )
        .bind(team_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        // (model, enabled, params, param_policy, advanced)
        let rows = [
            // never given any settings: the column defaults are what an empty
            // object means, and that stays a valid route
            ("untouched", true, "{}", "{}", "{}"),
            // a restriction that parses is kept
            (
                "restricted",
                true,
                r#"{"max_tokens": 256}"#,
                r#"{"mode": "deny", "allow": ["temperature"]}"#,
                r#"{"visibility": {"project_only": true, "allowed_team_ids": ["t1"]}}"#,
            ),
            // the restriction is there, but not in a shape the type reads
            (
                "bad-advanced",
                true,
                "{}",
                "{}",
                r#"{"visibility": {"project_only": "yes", "allowed_team_ids": "t1"}}"#,
            ),
            // `params` is a map of defaults, not a list
            ("bad-params", true, r#"["max_tokens"]"#, "{}", "{}"),
            // `mode` is allow or deny, and the default for a missing one is allow
            (
                "bad-policy",
                true,
                r#"{"max_tokens": 256}"#,
                r#"{"mode": "sometimes"}"#,
                "{}",
            ),
            // a disabled route is not loaded, so there is nothing to report
            (
                "bad-but-disabled",
                false,
                "{}",
                "{}",
                r#"{"visibility": 7}"#,
            ),
        ];
        for (model, enabled, params, param_policy, advanced) in rows {
            sqlx::query(
                "insert into routes (project_id, model, strategy, enabled, params, param_policy,
                                     advanced)
                 values ($1, $2, 'round_robin', $3, $4::jsonb, $5::jsonb, $6::jsonb)",
            )
            .bind(project_id)
            .bind(model)
            .bind(enabled)
            .bind(params)
            .bind(param_policy)
            .bind(advanced)
            .execute(&pool)
            .await
            .unwrap();
        }

        let store = PostgresConfigStore::new(pool);
        let routes = store.load().await.unwrap().routes;
        let models: Vec<&str> = routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(
            models,
            ["restricted", "untouched"],
            "a route that cannot be read must not be served on the permissive defaults"
        );
        let restricted = &routes[0];
        assert!(restricted.advanced.visibility.project_only);
        assert_eq!(restricted.advanced.visibility.allowed_team_ids, ["t1"]);
        assert_eq!(restricted.params["max_tokens"], 256);
        assert_eq!(
            restricted.param_policy.mode,
            rolter_core::OverrideMode::Deny
        );

        let problems = store.load_problems().await.unwrap();
        assert_eq!(problems.len(), 3, "{problems:?}");
        for (model, column) in [
            ("bad-advanced", "advanced"),
            ("bad-params", "params"),
            ("bad-policy", "param_policy"),
        ] {
            let line = problems
                .iter()
                .find(|p| p.contains(&format!("'{model}'")))
                .unwrap_or_else(|| panic!("{model} is not reported: {problems:?}"));
            assert!(line.contains(&format!("a {column} value")), "{line}");
            assert!(line.contains("left out of the snapshot"), "{line}");
        }
    }

    /// The three shape checks of migration 0086, dropped so a test can write by
    /// SQL what a seed, an import or an older control plane could have left.
    async fn drop_shape_checks(pool: &PgPool) {
        for (table, constraint) in SHAPE_CHECKS {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "alter table {table} drop constraint {constraint}"
            )))
            .execute(pool)
            .await
            .unwrap();
        }
        // 0018's array check would refuse the non-array rows as well
        sqlx::query("alter table providers drop constraint providers_egress_proxies_array")
            .execute(pool)
            .await
            .unwrap();
    }

    /// (table, constraint) for each check migration 0086 adds.
    const SHAPE_CHECKS: [(&str, &str); 3] = [
        (
            "security_settings",
            "security_settings_required_headers_shape",
        ),
        ("providers", "providers_egress_proxies_strings"),
        ("client_settings", "client_settings_injected_headers_shape"),
    ];

    async fn validated_shape_checks(pool: &PgPool) -> Vec<(String, bool)> {
        sqlx::query_as(
            "select conname::text, convalidated from pg_constraint
             where conname in ('security_settings_required_headers_shape',
                               'providers_egress_proxies_strings',
                               'client_settings_injected_headers_shape')
               and connamespace = current_schema()::regnamespace
             order by conname",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    /// An org with one team and project, for the tests that need a route.
    async fn seed_project(pool: &PgPool) -> (Uuid, Uuid) {
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let team_id: Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org_id)
                .fetch_one(pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'api') returning id",
        )
        .bind(team_id)
        .fetch_one(pool)
        .await
        .unwrap();
        (org_id, project_id)
    }

    #[test]
    fn a_proxy_pool_is_read_only_when_every_entry_is_a_string() {
        use serde_json::json;
        assert_eq!(
            egress_proxies_from_value("edge", &json!([])).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            egress_proxies_from_value("edge", &json!(["http://a:3128", "socks5://b:1080"]))
                .unwrap(),
            ["http://a:3128", "socks5://b:1080"]
        );
        let line = egress_proxies_from_value("edge", &json!(["http://a:3128", 7])).unwrap_err();
        assert!(line.contains("provider 'edge'"), "{line}");
        assert!(line.contains("entry 1 is a number"), "{line}");
        assert!(line.contains("left out of the snapshot"), "{line}");
        // a bare string is the likeliest hand-written mistake, and it may carry
        // credentials, so what it was is not repeated
        let line = egress_proxies_from_value("edge", &json!("http://user:hunter2@proxy:3128"))
            .unwrap_err();
        assert!(line.contains("it is a string, not a list"), "{line}");
        assert!(!line.contains("hunter2"), "{line}");
        for value in [json!({"a": "b"}), json!(null), json!(true), json!(3)] {
            assert!(
                egress_proxies_from_value("edge", &value).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn the_required_headers_rule_is_read_only_when_every_entry_is() {
        use serde_json::json;
        assert!(required_headers_from_value(&json!({})).unwrap().is_empty());
        // names are lowercased and trimmed, the value is kept exactly
        let rule =
            required_headers_from_value(&json!({" X-WAF ": "edge", "x-tenant": " a b "})).unwrap();
        assert_eq!(rule["x-waf"], "edge");
        assert_eq!(rule["x-tenant"], " a b ");
        // one header written twice to the same value is still one rule
        assert_eq!(
            required_headers_from_value(&json!({"x-waf": "edge", "X-Waf": "edge"}))
                .unwrap()
                .len(),
            1
        );

        for (value, detail) in [
            (json!([]), "it is an array, not an object"),
            (json!("x-waf=edge"), "it is a string, not an object"),
            (json!(null), "it is null, not an object"),
            (
                json!({"x-waf": 7}),
                "header 'x-waf' has a value that is not",
            ),
            (
                json!({"x-waf": null}),
                "header 'x-waf' has a value that is not",
            ),
            (
                json!({"x-waf": ["a"]}),
                "header 'x-waf' has a value that is not",
            ),
            (
                json!({"x-waf": ""}),
                "header 'x-waf' has a value that is not",
            ),
            (
                json!({"x-waf": "  "}),
                "header 'x-waf' has a value that is not",
            ),
            (json!({" ": "edge"}), "an entry has an empty header name"),
            (
                json!({"x-waf": "a", "X-Waf": "b"}),
                "header 'x-waf' is listed twice with different values",
            ),
        ] {
            let line = required_headers_from_value(&value).unwrap_err();
            assert!(line.contains(detail), "{value}: {line}");
            assert!(line.contains("no config snapshot is published"), "{line}");
        }

        // every entry at fault is named, and no value is ever repeated
        let line = required_headers_from_value(
            &json!({"x-ok": "kept-secret", "x-a": 1, "x-b": {"nested": "also-secret"}}),
        )
        .unwrap_err();
        assert!(line.contains("'x-a'") && line.contains("'x-b'"), "{line}");
        assert!(!line.contains("secret"), "{line}");
    }

    #[test]
    fn injected_headers_keep_what_reads_and_report_what_does_not() {
        use serde_json::json;
        let (headers, problem) = injected_headers_from_value(&json!({"X-Team": "platform"}));
        assert_eq!(headers["x-team"], "platform");
        assert!(problem.is_none());

        let (headers, problem) =
            injected_headers_from_value(&json!({"x-team": "platform", "x-bad": 3, " ": "v"}));
        assert_eq!(headers.len(), 1);
        let line = problem.expect("an entry that cannot be read is reported");
        assert!(line.contains("'x-bad'"), "{line}");
        assert!(line.contains("not injected"), "{line}");

        let (headers, problem) = injected_headers_from_value(&json!(["x-team"]));
        assert!(headers.is_empty());
        assert!(problem.unwrap().contains("an array"));
    }

    /// #2943: an unreadable `required_headers` used to load as no rule, or lose
    /// the entries it could not read, and the deployment stopped refusing the
    /// requests it was configured to refuse. The load fails instead, so no
    /// snapshot is built and a gateway keeps the one it has, and the store says
    /// why through `load_problems`, which still answers.
    #[tokio::test]
    async fn an_unreadable_required_headers_rule_fails_the_load_instead_of_being_dropped() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let store = PostgresConfigStore::new(pool.clone());

        sqlx::query(
            "update security_settings set required_headers = '{\"x-waf\":\"edge-secret\"}'::jsonb",
        )
        .execute(&pool)
        .await
        .unwrap();
        let config = store.load().await.unwrap();
        assert_eq!(config.security.required_headers["x-waf"], "edge-secret");
        assert!(store.load_problems().await.unwrap().is_empty());

        // the api and the check refuse each of these; write them the way an
        // older database, a seed or hand-run sql could have
        drop_shape_checks(&pool).await;
        for unreadable in [
            // not an object: used to be no rule at all
            "[\"x-waf\"]",
            "\"x-waf=edge-secret\"",
            "null",
            // an object, but an entry the rule cannot be read from: used to be
            // dropped, leaving the rest of the rule or none of it
            "{\"x-waf\":\"edge-secret\",\"x-tenant\":7}",
            "{\"x-waf\":\"edge-secret\",\"x-tenant\":\"\"}",
            "{\"x-waf\":\"edge-secret\",\"x-tenant\":null}",
            "{\"x-waf\":\"edge-secret\",\" \":\"v\"}",
            "{\"x-waf\":\"edge-secret\",\"X-WAF\":\"other\"}",
        ] {
            sqlx::query("update security_settings set required_headers = $1::jsonb")
                .bind(unreadable)
                .execute(&pool)
                .await
                .unwrap();
            let error = store
                .load()
                .await
                .expect_err(&format!("{unreadable} must not load as a weaker rule"));
            assert!(
                matches!(&error, Error::Store(message) if message.contains("required_headers")),
                "{unreadable}: {error}"
            );

            let problems = store.load_problems().await.unwrap();
            assert_eq!(problems.len(), 1, "{unreadable}: {problems:?}");
            assert!(problems[0].contains("required_headers"), "{problems:?}");
            assert!(
                problems[0].contains("no config snapshot is published"),
                "{problems:?}"
            );
            // a value is never repeated: it may be a shared secret
            assert!(!problems[0].contains("edge-secret"), "{problems:?}");
            assert!(!error.to_string().contains("edge-secret"), "{error}");
        }

        // repaired through the same write the api does, it loads again
        sqlx::query(
            "update security_settings set required_headers = '{\"x-waf\":\"edge-secret\"}'::jsonb",
        )
        .execute(&pool)
        .await
        .unwrap();
        let config = store.load().await.unwrap();
        assert_eq!(config.security.required_headers["x-waf"], "edge-secret");
        assert!(store.load_problems().await.unwrap().is_empty());
    }

    /// #2943: a provider whose proxy pool cannot be read used to load with an
    /// empty one and reach its upstream directly. It is left out now, and
    /// nothing that named it is left dangling, so the rest of the snapshot is
    /// still one that validates.
    #[tokio::test]
    async fn a_provider_whose_proxy_pool_does_not_parse_is_left_out_not_served_direct() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let (org_id, project_id) = seed_project(&pool).await;
        drop_shape_checks(&pool).await;

        // (name, egress_proxy, egress_proxies)
        let providers = [
            ("pooled", None, "[\"http://proxy.example.com:3128\"]"),
            ("plain", None, "[]"),
            // the proxy it should have used; the single `egress_proxy` is the
            // fallback the unreadable pool used to fall back to
            (
                "bad-entry",
                Some("http://legacy.example.com:3128"),
                "[\"http://proxy.example.com:3128\", 7]",
            ),
            ("bad-nested", None, "[[\"http://proxy.example.com:3128\"]]"),
            ("bad-shape", None, "{\"http\":\"proxy.example.com\"}"),
            ("bad-scalar", None, "\"http://proxy.example.com:3128\""),
        ];
        let mut ids = std::collections::HashMap::new();
        for (name, egress_proxy, egress_proxies) in providers {
            let id: Uuid = sqlx::query_scalar(
                "insert into providers (org_id, name, slug, kind, api_base, api_key_env,
                                        egress_proxy, egress_proxies)
                 values ($1, $2, $2, 'openai_compatible', 'https://llm.example.com/v1',
                         'UPSTREAM_KEY', $3, $4::jsonb)
                 returning id",
            )
            .bind(org_id)
            .bind(name)
            .bind(egress_proxy)
            .bind(egress_proxies)
            .fetch_one(&pool)
            .await
            .unwrap();
            ids.insert(name, id);
        }
        // a route that can still be served by another target, a route with
        // nowhere left to go, and a group that loses a member
        let targets = |model: &str, names: &[&str]| {
            let model = model.to_string();
            let names: Vec<Uuid> = names.iter().map(|name| ids[name]).collect();
            let pool = pool.clone();
            async move {
                let route_id: Uuid = sqlx::query_scalar(
                    "insert into routes (project_id, model, strategy)
                     values ($1, $2, 'round_robin') returning id",
                )
                .bind(project_id)
                .bind(&model)
                .fetch_one(&pool)
                .await
                .unwrap();
                for provider_id in names {
                    sqlx::query(
                        "insert into route_targets (route_id, provider_id, weight)
                         values ($1, $2, 1)",
                    )
                    .bind(route_id)
                    .bind(provider_id)
                    .execute(&pool)
                    .await
                    .unwrap();
                }
            }
        };
        targets("shared", &["pooled", "bad-entry"]).await;
        targets("only-bad", &["bad-entry", "bad-shape"]).await;
        targets("healthy", &["plain"]).await;
        let group_id: Uuid = sqlx::query_scalar(
            "insert into provider_groups (org_id, name, slug, strategy)
             values ($1, 'pool', 'pool', 'round_robin') returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        for (position, name) in ["pooled", "bad-nested"].into_iter().enumerate() {
            sqlx::query(
                "insert into provider_group_members (group_id, provider_id, weight, position)
                 values ($1, $2, 1, $3)",
            )
            .bind(group_id)
            .bind(ids[name])
            .bind(position as i32)
            .execute(&pool)
            .await
            .unwrap();
        }

        let store = PostgresConfigStore::new(pool);
        let mut config = store.load().await.unwrap();
        let served: Vec<&str> = config.providers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            served,
            ["plain", "pooled"],
            "a provider whose pool cannot be read must not be served around it"
        );
        let pooled = &config.providers[1];
        assert_eq!(pooled.egress_proxies, ["http://proxy.example.com:3128"]);

        // nothing names a provider the snapshot does not carry: the whole
        // snapshot would be refused for it otherwise
        let target_names = |model: &str| -> Vec<String> {
            config
                .routes
                .iter()
                .find(|r| r.model == model)
                .unwrap_or_else(|| panic!("route {model} is missing"))
                .targets
                .iter()
                .map(|t| t.provider.clone())
                .collect()
        };
        assert_eq!(target_names("shared"), ["pooled"]);
        assert!(target_names("only-bad").is_empty());
        assert_eq!(target_names("healthy"), ["plain"]);
        let members: Vec<&str> = config.provider_groups[0]
            .members
            .iter()
            .map(|m| m.provider.as_str())
            .collect();
        assert_eq!(members, ["pooled"]);

        // the control plane's own pruning then finishes the job, and what is
        // left is a snapshot a gateway accepts
        let omitted = config.sanitize_for_snapshot();
        assert!(
            omitted
                .iter()
                .any(|line| line.contains("route 'only-bad' omitted")),
            "{omitted:?}"
        );
        let models: Vec<&str> = config.routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, ["healthy", "shared"]);
        config
            .validate_snapshot()
            .expect("the remaining snapshot is servable");

        let problems = store.load_problems().await.unwrap();
        assert_eq!(problems.len(), 4, "{problems:?}");
        for name in ["bad-entry", "bad-nested", "bad-shape", "bad-scalar"] {
            let line = problems
                .iter()
                .find(|p| p.contains(&format!("provider '{name}'")))
                .unwrap_or_else(|| panic!("{name} is not reported: {problems:?}"));
            assert!(line.contains("egress_proxies"), "{line}");
            assert!(line.contains("left out of the snapshot"), "{line}");
            assert!(!line.contains("proxy.example.com"), "{line}");
        }
    }

    /// #3464: two orgs each have an `openai`, and the store cannot read
    /// org-a's. Only org-a's route and group lose their target: org-b's keep
    /// their own `openai`, and the snapshot that is left is servable.
    #[tokio::test]
    async fn one_orgs_unreadable_provider_leaves_the_other_orgs_namesake_alone() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        drop_shape_checks(&pool).await;
        let mut orgs = Vec::new();
        for (slug, egress_proxies) in [("org-a", "[7]"), ("org-b", "[]")] {
            let org = repo::OrgRepo(&pool).create(slug, slug).await.unwrap();
            let team = repo::TeamRepo(&pool).create(org.id, "core").await.unwrap();
            let project = repo::ProjectRepo(&pool)
                .create(team.id, "app")
                .await
                .unwrap();
            let provider: Uuid = sqlx::query_scalar(
                "insert into providers (org_id, name, slug, kind, api_base, api_key_env,
                                        egress_proxies)
                 values ($1, 'openai', 'openai', 'openai_compatible',
                         'https://llm.example.com/v1', 'UPSTREAM_KEY', $2::jsonb)
                 returning id",
            )
            .bind(org.id)
            .bind(egress_proxies)
            .fetch_one(&pool)
            .await
            .unwrap();
            let route = repo::RouteRepo(&pool)
                .create(project.id, "gpt-4o", "round_robin")
                .await
                .unwrap();
            repo::RouteTargetRepo(&pool)
                .create(route.id, provider, None, 1, &[])
                .await
                .unwrap();
            let group: Uuid = sqlx::query_scalar(
                "insert into provider_groups (org_id, name, slug, strategy)
                 values ($1, 'pool', 'pool', 'round_robin') returning id",
            )
            .bind(org.id)
            .fetch_one(&pool)
            .await
            .unwrap();
            sqlx::query(
                "insert into provider_group_members (group_id, provider_id, weight, position)
                 values ($1, $2, 1, 0)",
            )
            .bind(group)
            .bind(provider)
            .execute(&pool)
            .await
            .unwrap();
            orgs.push(org.id.to_string());
        }
        let (org_a, org_b) = (&orgs[0], &orgs[1]);

        let mut config = PostgresConfigStore::new(pool).load().await.unwrap();
        assert_eq!(
            config.left_out_providers,
            [(org_a.clone(), "openai".to_string())].into()
        );
        let org_of = |t: Option<&rolter_core::Tenancy>| t.map(|t| t.org_id.clone());
        let served: Vec<_> = config
            .providers
            .iter()
            .map(|p| org_of(p.tenancy.as_ref()))
            .collect();
        assert_eq!(served, [Some(org_b.clone())]);
        let mut routes: Vec<_> = config
            .routes
            .iter()
            .map(|r| (org_of(r.tenancy.as_ref()), r.targets.len()))
            .collect();
        routes.sort();
        let mut expected = vec![(Some(org_a.clone()), 0), (Some(org_b.clone()), 1)];
        expected.sort();
        assert_eq!(routes, expected, "only org-a's target is left out");
        let mut groups: Vec<_> = config
            .provider_groups
            .iter()
            .map(|g| (org_of(g.tenancy.as_ref()), g.members.len()))
            .collect();
        groups.sort();
        assert_eq!(groups, expected, "only org-a's member is left out");

        let problems = config.sanitize_for_snapshot();
        let kept: Vec<_> = config
            .routes
            .iter()
            .map(|r| (org_of(r.tenancy.as_ref()), r.targets[0].provider.clone()))
            .collect();
        assert_eq!(
            kept,
            [(Some(org_b.clone()), "openai".to_string())],
            "{problems:?}"
        );
        config
            .validate_snapshot()
            .expect("the remaining snapshot is servable");
    }

    /// #2943: injected headers gate nothing, so an unreadable entry costs the
    /// request that header and is reported, instead of failing the snapshot.
    #[tokio::test]
    async fn unreadable_injected_headers_are_dropped_and_reported() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let store = PostgresConfigStore::new(pool.clone());
        drop_shape_checks(&pool).await;

        sqlx::query(
            "update client_settings set injected_headers =
                '{\"x-team\":\"platform\",\"x-bad\":3}'::jsonb",
        )
        .execute(&pool)
        .await
        .unwrap();
        let config = store.load().await.unwrap();
        assert_eq!(config.client.injected_headers.len(), 1);
        assert_eq!(config.client.injected_headers["x-team"], "platform");
        let problems = store.load_problems().await.unwrap();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("injected_headers"), "{problems:?}");
        assert!(problems[0].contains("'x-bad'"), "{problems:?}");

        sqlx::query("update client_settings set injected_headers = '[\"x-team\"]'::jsonb")
            .execute(&pool)
            .await
            .unwrap();
        let config = store.load().await.unwrap();
        assert!(config.client.injected_headers.is_empty());
        let problems = store.load_problems().await.unwrap();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("an array"), "{problems:?}");
    }

    /// #2943: the database refuses what the loader would have to guess at, and
    /// accepts what the api writes.
    #[tokio::test]
    async fn the_database_refuses_security_and_egress_values_the_loader_cannot_read() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let (org_id, _) = seed_project(&pool).await;

        let clean = validated_shape_checks(&pool).await;
        assert_eq!(clean.len(), 3, "{clean:?}");
        assert!(clean.iter().all(|(_, validated)| *validated), "{clean:?}");

        for value in [
            "{}",
            "{\"x-waf\":\"edge\"}",
            "{\"x-waf\":\"edge\",\"x-tenant\":\"a b\"}",
        ] {
            for (table, column) in [
                ("security_settings", "required_headers"),
                ("client_settings", "injected_headers"),
            ] {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "update {table} set {column} = '{value}'::jsonb"
                )))
                .execute(&pool)
                .await
                .unwrap_or_else(|e| panic!("{table}.{column} = {value} should be accepted: {e}"));
            }
        }
        // an injected value may be empty, a required one may not
        sqlx::query("update client_settings set injected_headers = '{\"x-a\":\"\"}'::jsonb")
            .execute(&pool)
            .await
            .expect("an empty injected value is a header with no value");

        for value in [
            "[]",
            "\"x\"",
            "null",
            "{\"x-a\":1}",
            "{\"x-a\":null}",
            "{\"x-a\":[\"b\"]}",
            "{\"x-a\":{\"b\":\"c\"}}",
            "{\"x-a\":true}",
            "{\"x-ok\":\"b\",\"x-a\":2}",
            "{\" \":\"b\"}",
            "{\"\":\"b\"}",
        ] {
            for (table, column) in [
                ("security_settings", "required_headers"),
                ("client_settings", "injected_headers"),
            ] {
                let error = sqlx::query(sqlx::AssertSqlSafe(format!(
                    "update {table} set {column} = '{value}'::jsonb"
                )))
                .execute(&pool)
                .await
                .expect_err(&format!("{table}.{column} = {value} should be refused"));
                assert!(
                    error.to_string().contains("_shape"),
                    "{table}.{column} = {value}: {error}"
                );
            }
        }
        for value in ["{\"x-a\":\"\"}", "{\"x-a\":\" \"}"] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "update security_settings set required_headers = '{value}'::jsonb"
            )))
            .execute(&pool)
            .await
            .expect_err(&format!(
                "a required header with value {value} locks every request out"
            ));
        }

        for (egress_proxies, accepted) in [
            ("[]", true),
            ("[\"http://proxy.example.com:3128\"]", true),
            ("[\"\"]", true),
            ("[7]", false),
            ("[\"http://proxy.example.com:3128\", null]", false),
            ("[[\"http://proxy.example.com:3128\"]]", false),
            ("[{\"url\":\"http://proxy.example.com:3128\"}]", false),
            ("{\"a\":\"b\"}", false),
            ("\"http://proxy.example.com:3128\"", false),
        ] {
            let outcome = sqlx::query(
                "insert into providers (org_id, name, slug, kind, api_base, egress_proxies)
                 values ($1, gen_random_uuid()::text, gen_random_uuid()::text, 'openai',
                         'https://api.openai.com/v1', $2::jsonb)",
            )
            .bind(org_id)
            .bind(egress_proxies)
            .execute(&pool)
            .await;
            assert_eq!(outcome.is_ok(), accepted, "{egress_proxies}: {outcome:?}");
        }
    }

    /// 0090: a cap a route carried while nothing read it must not start
    /// refusing traffic the day the gateway learns to read it (#2936). Only
    /// `rpm`, `tpm` and `concurrency` go; every other key of the block stays.
    #[tokio::test]
    async fn the_route_limits_migration_clears_stale_caps_and_keeps_the_rest() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let (_, project_id) = seed_project(&pool).await;
        let insert = |model: &'static str, advanced: serde_json::Value| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, Uuid>(
                    "insert into routes (project_id, model, strategy, advanced)
                     values ($1, $2, 'round_robin', $3) returning id",
                )
                .bind(project_id)
                .bind(model)
                .bind(advanced)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        let stale = insert(
            "stale",
            serde_json::json!({
                "limits": {"rpm": 60, "tpm": 1000, "concurrency": 2,
                           "timeout_secs": 30, "output_tokens": 512},
                "headers": {"x-a": "b"}
            }),
        )
        .await;
        let clean = insert("clean", serde_json::json!({"limits": {"retries": 1}})).await;
        let bare = insert("bare", serde_json::json!({})).await;

        sqlx::raw_sql(include_str!(
            "../../migrations/0090_clear_unread_route_limits.sql"
        ))
        .execute(&pool)
        .await
        .expect("the migration applies");

        let advanced = |id: Uuid| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, serde_json::Value>(
                    "select advanced from routes where id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(
            advanced(stale).await,
            serde_json::json!({
                "limits": {"timeout_secs": 30, "output_tokens": 512},
                "headers": {"x-a": "b"}
            })
        );
        assert_eq!(
            advanced(clean).await,
            serde_json::json!({"limits": {"retries": 1}})
        );
        assert_eq!(advanced(bare).await, serde_json::json!({}));
    }

    /// #2943: the migration applies over a row that already breaks it, as any
    /// deployment written to by hand could hold, leaves exactly that check
    /// unvalidated, and still binds every write from then on.
    #[tokio::test]
    async fn the_shape_checks_migration_applies_over_a_bad_row_and_binds_new_writes() {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        let db = fresh_db().await;
        let pool = db.pool().clone();
        let (org_id, _) = seed_project(&pool).await;

        // the database as it was before 0086: no checks, and two bad rows
        drop_shape_checks(&pool).await;
        sqlx::query("update security_settings set required_headers = '{\"x-waf\":7}'::jsonb")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "insert into providers (org_id, name, slug, kind, api_base, egress_proxies)
             values ($1, 'bad', 'bad', 'openai', 'https://api.openai.com/v1', '[7]'::jsonb)",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(include_str!(
            "../../migrations/0086_security_egress_settings_shape.sql"
        ))
        .execute(&pool)
        .await
        .expect("the migration applies over a row that breaks it");

        // unvalidated where a row breaks the rule, validated where none does
        assert_eq!(
            validated_shape_checks(&pool).await,
            [
                ("client_settings_injected_headers_shape".to_string(), true),
                ("providers_egress_proxies_strings".to_string(), false),
                (
                    "security_settings_required_headers_shape".to_string(),
                    false
                ),
            ]
        );

        // every write from now on is held to it
        sqlx::query("update client_settings set injected_headers = '{\"x-a\":1}'::jsonb")
            .execute(&pool)
            .await
            .expect_err("a new bad injected header is refused");
        sqlx::query(
            "insert into providers (org_id, name, slug, kind, api_base, egress_proxies)
             values ($1, 'worse', 'worse', 'openai', 'https://api.openai.com/v1', '[8]'::jsonb)",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .expect_err("a new bad proxy pool is refused");

        // the operator's fix: correct the rows, then validate
        sqlx::query(
            "update security_settings set required_headers = '{\"x-waf\":\"edge\"}'::jsonb",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("update providers set egress_proxies = '[]'::jsonb where name = 'bad'")
            .execute(&pool)
            .await
            .unwrap();
        for (table, constraint) in SHAPE_CHECKS {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "alter table {table} validate constraint {constraint}"
            )))
            .execute(&pool)
            .await
            .unwrap_or_else(|e| {
                panic!("{constraint} should validate once the rows are fixed: {e}")
            });
        }
        let fixed = validated_shape_checks(&pool).await;
        assert!(fixed.iter().all(|(_, validated)| *validated), "{fixed:?}");
    }
}
