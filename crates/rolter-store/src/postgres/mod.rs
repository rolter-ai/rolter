//! Postgres-backed [`ConfigStore`], gated behind the `postgres` feature.

pub mod crypto;
pub mod kek_audit;
pub mod models;
pub mod repo;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rolter_core::{
    BackpressurePolicy, BalancingStrategy, BudgetConfig, BudgetPeriod, BudgetScope, BuiltinRule,
    Decorator, Error, FailureMode, FeatureFlagsConfig, GatewayConfig, GroupMember, GuardAction,
    GuardStage, GuardrailRule, GuardrailWebhookConfig, GuardrailsConfig, McpOAuthSessionConfig,
    McpServerConfig, ModelPolicy, ModelPriceConfig, ModelRoute, PluginInstanceConfig, PluginStage,
    PluginsConfig, PromptTemplate, PromptTemplateActivationScope, PromptTemplatesConfig,
    ProviderConfig, ProviderGroupConfig, ProviderKind, RateLimitConfig, Result, Target,
    TemplateVariable, UnpricedPolicy, VirtualKeyRecord, WebhookAuth, WebhookStage,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use uuid::Uuid;

use crate::postgres::models::{
    AdaptiveRoutingPolicy, Budget, ClientSettings, CompatibilityPolicy, FeatureFlags,
    GuardrailProvider, GuardrailRule as GuardrailRuleRow, LoggingSettings, ModelDefaults,
    ModelPrice, PluginInstance, RateLimit, RuntimePolicy, SecurityPolicyRow,
};
use crate::ConfigStore;

fn store_err(err: sqlx::Error) -> Error {
    Error::Store(err.to_string())
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
/// `public` schema during DDL.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::atomic::{AtomicU32, Ordering};

    use sqlx::PgPool;

    use super::{connect, run_migrations};

    static SCHEMA_SEQ: AtomicU32 = AtomicU32::new(0);

    /// Isolated schema name unique to this process and call, safe to
    /// interpolate (only ascii digits and underscores).
    fn unique_schema() -> String {
        let n = SCHEMA_SEQ.fetch_add(1, Ordering::Relaxed);
        format!("test_{}_{}", std::process::id(), n)
    }

    /// `url` with the connection pinned to `schema` via `search_path`, so
    /// migrations and queries land in the isolated schema rather than `public`.
    pub(crate) fn with_search_path(url: &str, schema: &str) -> String {
        let sep = if url.contains('?') { '&' } else { '?' };
        // percent-encode the space and `=` inside the libpq options string
        format!("{url}{sep}options=-c%20search_path%3D{schema}")
    }

    /// Create a fresh isolated schema and return a migrated pool scoped to it.
    pub(crate) async fn fresh_scoped_pool(url: &str) -> PgPool {
        fresh_scoped_pool_named(url).await.0
    }

    /// As [`fresh_scoped_pool`], also returning the schema name. A test that
    /// shells out to `pg_dump` needs the name; one that only queries does not.
    pub(crate) async fn fresh_scoped_pool_named(url: &str) -> (PgPool, String) {
        let schema = unique_schema();

        // (re)create the isolated schema over a default-search_path connection
        let admin = connect(url).await.expect("connect");
        sqlx::query(&format!("drop schema if exists {schema} cascade"))
            .execute(&admin)
            .await
            .expect("reset schema");
        sqlx::query(&format!("create schema {schema}"))
            .execute(&admin)
            .await
            .expect("create schema");
        admin.close().await;

        // app pool scoped to the isolated schema so migrations run there
        let pool = connect(&with_search_path(url, &schema))
            .await
            .expect("connect scoped");
        run_migrations(&pool).await.expect("run migrations");
        (pool, schema)
    }
}

#[derive(FromRow)]
struct ProviderRow {
    name: String,
    slug: String,
    kind: String,
    api_base: String,
    api_key_env: Option<String>,
    egress_proxy: Option<String>,
    egress_proxies: serde_json::Value,
    /// sealed runtime credential from `provider_keys`, when one is stored
    ciphertext: Option<Vec<u8>>,
    nonce: Option<Vec<u8>>,
}

impl ProviderRow {
    /// Convert a row into config, opening the sealed credential with `kek`
    /// when both are present. A missing KEK or an undecryptable credential
    /// degrades to `api_key: None` with a warning rather than failing the
    /// whole config load, so one bad key cannot take down snapshot serving.
    fn into_config(self, kek: Option<&crypto::Kek>) -> Result<ProviderConfig> {
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
            other => return Err(Error::Store(format!("unknown provider kind '{other}'"))),
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
        Ok(ProviderConfig {
            name: row.name,
            slug: Some(row.slug),
            kind,
            api_base: row.api_base,
            api_key,
            api_key_env: row.api_key_env,
            egress_proxy: row.egress_proxy,
            egress_proxies: serde_json::from_value(row.egress_proxies).unwrap_or_default(),
            kv_events: None,
            lmcache: None,
            ca_bundles: None,
            api_keys: Vec::new(),
            also_track_via_llm_call: false,
            llm_probe_model: None,
            status_page_url: None,
            role_profile: None,
            model_role_profiles: Default::default(),
            // db-backed providers cannot opt out of the host pin yet: the
            // column, the crud surface and the dashboard control are tracked
            // separately, so a stored provider keeps the safe default
            allow_custom_api_base: false,
        })
    }
}

#[derive(FromRow)]
struct RouteRow {
    id: Uuid,
    model: String,
    strategy: String,
    params: serde_json::Value,
    param_policy: serde_json::Value,
    advanced: serde_json::Value,
}

#[derive(FromRow)]
struct TargetRow {
    route_id: Uuid,
    provider_name: String,
    upstream_model: Option<String>,
    weight: i32,
}

#[derive(FromRow)]
struct ProviderGroupRow {
    id: Uuid,
    name: String,
    slug: String,
    strategy: String,
}

#[derive(FromRow)]
struct ProviderGroupMemberRow {
    group_id: Uuid,
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

#[derive(FromRow)]
struct McpServerSnapshotRow {
    id: Uuid,
    org_id: Uuid,
    slug: String,
    url: String,
    transport: String,
    required_scopes: Vec<String>,
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

    async fn load_providers(&self) -> Result<Vec<ProviderConfig>> {
        let rows: Vec<ProviderRow> = sqlx::query_as(
            "select p.name, p.slug, p.kind, p.api_base, p.api_key_env, p.egress_proxy, p.egress_proxies,
                    pk.ciphertext, pk.nonce
             from providers p
             left join provider_keys pk on pk.provider_id = p.id
             order by p.name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        rows.into_iter()
            .map(|row| row.into_config(self.kek.as_ref()))
            .collect()
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
            "select virtual_key_required, required_headers, auth_bypass_routes \
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

    async fn load_guardrail_webhook(&self) -> Result<GuardrailWebhookConfig> {
        let row: Option<GuardrailProvider> = sqlx::query_as(
            "select id, name, enabled, url, stage, timeout_ms, max_retries, failure_mode, \
                    max_body_bytes, auth_kind, auth_env, created_at, updated_at \
             from guardrail_providers where enabled limit 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(store_err)?;
        let Some(row) = row else {
            return Ok(GuardrailWebhookConfig::default());
        };
        let auth = match (row.auth_kind.as_str(), row.auth_env) {
            ("bearer", Some(token_env)) => Some(WebhookAuth::Bearer { token_env }),
            ("shared_secret", Some(secret_env)) => Some(WebhookAuth::SharedSecret { secret_env }),
            _ => None,
        };
        Ok(GuardrailWebhookConfig {
            enabled: true,
            url: row.url,
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
            auth,
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
                    payload_capture_redact_fields, payload_capture_models, payload_capture_virtual_key_ids, \
                    retention_days, payload_retention_hours, updated_at \
             from logging_settings where id = true",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(store_err)
    }

    async fn load_routes(&self) -> Result<Vec<ModelRoute>> {
        let route_rows: Vec<RouteRow> = sqlx::query_as(
            "select id, model, strategy, params, param_policy, advanced
             from routes where enabled order by model",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;

        let target_rows: Vec<TargetRow> = sqlx::query_as(
            "select rt.route_id, p.name as provider_name, rt.upstream_model, rt.weight
             from route_targets rt
             join providers p on p.id = rt.provider_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;

        route_rows
            .into_iter()
            .map(|r| {
                let strategy = parse_strategy(&r.strategy)?;
                let targets = target_rows
                    .iter()
                    .filter(|t| t.route_id == r.id)
                    .map(|t| Target {
                        provider: t.provider_name.clone(),
                        model: t.upstream_model.clone(),
                        weight: t.weight.max(0) as u32,
                    })
                    .collect();
                // jsonb → typed; a malformed value falls back to the permissive
                // default rather than failing the whole config load
                let params = serde_json::from_value(r.params).unwrap_or_default();
                let param_policy = serde_json::from_value(r.param_policy).unwrap_or_default();
                let advanced = serde_json::from_value(r.advanced).unwrap_or_default();
                Ok(ModelRoute {
                    model: r.model,
                    strategy,
                    targets,
                    params,
                    param_policy,
                    advanced,
                    // db-backed variants land with their own store follow-up
                    variants: Default::default(),
                    // response-cache opt-in is config-only for now; a db-backed
                    // cache policy lands with its own store follow-up
                    cache: None,
                })
            })
            .collect()
    }

    /// Load provider groups and their members into `ProviderGroupConfig`
    /// (ADR-0017 addendum, ADR-0022). A member with a null `upstream_model`
    /// forwards the requested model as-is.
    async fn load_provider_groups(&self) -> Result<Vec<ProviderGroupConfig>> {
        let group_rows: Vec<ProviderGroupRow> =
            sqlx::query_as("select id, name, slug, strategy from provider_groups order by name")
                .fetch_all(&self.pool)
                .await
                .map_err(store_err)?;

        let member_rows: Vec<ProviderGroupMemberRow> = sqlx::query_as(
            "select m.group_id, p.name as provider_name, m.upstream_model, m.weight
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
                })
            })
            .collect()
    }

    /// Load database-defined virtual keys with their resolved scope chain
    /// (project → team → org). Only the one-way `key_hash` is exposed; the
    /// gateway matches presented keys against it by peppered digest.
    async fn load_virtual_keys(&self) -> Result<Vec<VirtualKeyRecord>> {
        let rows: Vec<VirtualKeyRow> = sqlx::query_as(
            "select vk.id, vk.key_hash, vk.models, vk.providers, vk.disabled, vk.expires_at, \
                    vk.cache_enabled, vk.project_id, p.team_id, t.org_id, vk.created_by, \
                    vk.business_unit_id, vk.customer_id \
             from virtual_keys vk \
             join projects p on p.id = vk.project_id \
             join teams t on t.id = p.team_id \
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

    async fn load_mcp_servers(&self) -> Result<Vec<McpServerConfig>> {
        let rows: Vec<McpServerSnapshotRow> = sqlx::query_as(
            "select id, org_id, slug, url, transport, required_scopes \
             from mcp_servers where enabled order by org_id, slug",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .map(|row| McpServerConfig {
                id: row.id.to_string(),
                org_id: row.org_id.to_string(),
                slug: row.slug,
                url: row.url,
                transport: row.transport,
                required_scopes: row.required_scopes,
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
        let rows: Vec<ModelPrice> = sqlx::query_as(
            "select id, model, \
                    input_per_mtok::text as input_per_mtok, \
                    output_per_mtok::text as output_per_mtok, \
                    cached_input_per_mtok::text as cached_input_per_mtok, \
                    currency, created_at \
             from model_prices order by model",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .map(|r| ModelPriceConfig {
                model: r.model,
                // decimals are stored as text; a malformed value prices at zero
                input_per_mtok: r.input_per_mtok.parse().unwrap_or(0.0),
                output_per_mtok: r.output_per_mtok.parse().unwrap_or(0.0),
                cached_input_per_mtok: r.cached_input_per_mtok.and_then(|v| v.parse().ok()),
                // the column has always existed and the dashboard has always
                // written it; it just never reached the config (#650), so every
                // non-USD price was charged as if it were USD
                currency: r.currency,
            })
            .collect())
    }

    async fn load_budgets(&self) -> Result<Vec<BudgetConfig>> {
        let rows: Vec<Budget> = sqlx::query_as(
            // limit_usd is numeric(12,4); decode it as text (Budget.limit_usd is a
            // String) or sqlx errors and the whole snapshot 500s — freezing every
            // polling gateway on its last config the moment any budget exists
            "select id, scope_type, scope_id, limit_usd::text as limit_usd, period,
                    unpriced_policy, created_at
             from budgets order by created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let scope = match r.scope_type.as_str() {
                    "org" => BudgetScope::Org,
                    "team" => BudgetScope::Team,
                    "project" => BudgetScope::Project,
                    "virtual_key" => BudgetScope::Key,
                    "business_unit" => BudgetScope::BusinessUnit,
                    "customer" => BudgetScope::Customer,
                    // unknown scope: skip rather than mis-enforce
                    _ => return None,
                };
                Some(BudgetConfig {
                    scope,
                    id: r.scope_id.to_string(),
                    // decimal stored as text; a malformed value disables the cap
                    limit_usd: r.limit_usd.parse().unwrap_or(f64::INFINITY),
                    period: parse_period(&r.period),
                    unpriced_policy: r.unpriced_policy.as_deref().and_then(parse_unpriced_policy),
                })
            })
            .collect())
    }

    async fn load_rate_limits(&self) -> Result<Vec<RateLimitConfig>> {
        let rows: Vec<RateLimit> = sqlx::query_as(
            "select id, scope_type, scope_id, rpm, tpm, created_at
             from rate_limits order by created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let scope = match r.scope_type.as_str() {
                    "org" => BudgetScope::Org,
                    "team" => BudgetScope::Team,
                    "project" => BudgetScope::Project,
                    "virtual_key" => BudgetScope::Key,
                    "business_unit" => BudgetScope::BusinessUnit,
                    "customer" => BudgetScope::Customer,
                    // unknown scope: skip rather than mis-enforce
                    _ => return None,
                };
                Some(RateLimitConfig {
                    scope,
                    id: r.scope_id.to_string(),
                    // non-positive caps are meaningless; treat them as unset
                    rpm: r.rpm.filter(|v| *v > 0).map(|v| v as u32),
                    tpm: r.tpm.filter(|v| *v > 0).map(|v| v as u32),
                })
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

/// Map the free-text `budgets.period` column to a [`BudgetPeriod`]. Accepts both
/// the human names and the legacy duration shorthands (`1d`, `30d`), defaulting
/// to monthly for anything unrecognized.
fn parse_period(period: &str) -> BudgetPeriod {
    match period.trim().to_ascii_lowercase().as_str() {
        "daily" | "1d" | "24h" => BudgetPeriod::Daily,
        "total" | "lifetime" | "all" => BudgetPeriod::Total,
        _ => BudgetPeriod::Monthly,
    }
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
        let model_defaults = self.load_model_defaults().await?;
        let adaptive = self.load_adaptive_routing_policy().await?;
        let guardrails = self.load_guardrails().await?;
        let guardrail_webhook = self.load_guardrail_webhook().await?;
        let providers = self.load_providers().await?;
        let routes = self.load_routes().await?;
        let provider_groups = self.load_provider_groups().await?;
        let model_prices = self.load_model_prices().await?;
        let db_virtual_keys = self.load_virtual_keys().await?;
        let mcp_servers = self.load_mcp_servers().await?;
        let mcp_oauth_sessions = self.load_mcp_oauth_sessions().await?;
        let budgets = self.load_budgets().await?;
        let rate_limits = self.load_rate_limits().await?;
        let prompt_templates = self.load_prompt_templates().await?;
        let plugins = self.load_plugins().await?;
        let mut config = GatewayConfig {
            providers,
            routes,
            provider_groups,
            model_prices,
            db_virtual_keys,
            mcp_servers,
            mcp_oauth_sessions,
            budgets,
            rate_limits,
            prompt_templates,
            guardrails,
            guardrail_webhook,
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
        config.logging.sample_rate = logging.sample_rate;
        config.logging.payload_capture.enabled = logging.payload_capture_enabled;
        config.logging.payload_capture.max_bytes =
            logging.payload_capture_max_bytes.max(0) as usize;
        config.logging.payload_capture.redact_fields = logging.payload_capture_redact_fields;
        config.logging.payload_capture.models = logging.payload_capture_models;
        config.logging.payload_capture.virtual_key_ids = logging.payload_capture_virtual_key_ids;
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
        config.client = rolter_core::ClientConfig {
            public_base_url: client_settings.public_base_url,
            forwarded_headers: client_settings
                .forwarded_headers
                .iter()
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .collect(),
            // a non-object column would mean a hand-edited row; treat it as
            // "no injected headers" rather than failing every snapshot poll
            injected_headers: client_settings
                .injected_headers
                .as_object()
                .map(|map| {
                    map.iter()
                        .filter_map(|(name, value)| {
                            let value = value.as_str()?;
                            Some((name.trim().to_ascii_lowercase(), value.to_string()))
                        })
                        .filter(|(name, _)| !name.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            request_id_header: client_settings.request_id_header.to_ascii_lowercase(),
        };
        config.security = rolter_core::SecurityPolicyConfig {
            virtual_key_required: security.virtual_key_required,
            // same treatment as injected_headers: a hand-edited non-object row
            // must not take the fleet's config propagation down with it. an
            // unreadable rule is dropped, never silently turned into a
            // different rule
            required_headers: security
                .required_headers
                .as_object()
                .map(|map| {
                    map.iter()
                        .filter_map(|(name, value)| {
                            let value = value.as_str()?;
                            Some((name.trim().to_ascii_lowercase(), value.to_string()))
                        })
                        .filter(|(name, value)| !name.is_empty() && !value.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database_url() -> Option<String> {
        std::env::var("ROLTER_TEST_DATABASE_URL").ok()
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

    #[test]
    fn an_unknown_strategy_is_still_an_error() {
        let err = parse_strategy("teleport").expect_err("unknown strategy must not parse");
        assert!(
            err.to_string().contains("teleport"),
            "the message should name the offending value, got: {err}"
        );
    }

    async fn fresh_pool() -> PgPool {
        let url = database_url().expect("ROLTER_TEST_DATABASE_URL not set; skipping");
        super::test_support::fresh_scoped_pool(&url).await
    }

    #[tokio::test]
    async fn triggers_bump_version_atomically_with_writes() {
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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
        // bumps the version even when no key rows exist
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 3);
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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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

    #[tokio::test]
    async fn snapshot_carries_the_key_owners_access_policy() {
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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

    #[tokio::test]
    async fn prompt_template_writes_bump_version() {
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;

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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;

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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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

    #[tokio::test]
    async fn runtime_policy_projects_into_snapshot_runtime_controls() {
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;

        sqlx::query(
            "insert into model_prices (model, input_per_mtok, output_per_mtok, cached_input_per_mtok, currency)
             values ('gpt-4o', 3, 15, 1.5, 'USD'), ('gpt-4o-mini', 0.15, 0.6, null, 'USD')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let store = PostgresConfigStore::new(pool);
        let config = store.load().await.unwrap();

        assert_eq!(config.model_prices.len(), 2);
        assert_eq!(config.model_prices[0].model, "gpt-4o");
        assert_eq!(config.model_prices[0].input_per_mtok, 3.0);
        assert_eq!(config.model_prices[0].output_per_mtok, 15.0);
        assert_eq!(config.model_prices[0].cached_input_per_mtok, Some(1.5));
        assert_eq!(config.model_prices[1].model, "gpt-4o-mini");
        assert_eq!(config.model_prices[1].input_per_mtok, 0.15);
        assert_eq!(config.model_prices[1].cached_input_per_mtok, None);
    }

    #[tokio::test]
    async fn loads_published_prompt_templates_from_db() {
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;

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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;

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
        assert_eq!(config.budgets[0].limit_usd, 100.5);
    }

    // governance dimensions carry their own caps, so a business-unit budget and
    // a customer rate limit must survive the snapshot projection (#539)
    #[tokio::test]
    async fn loads_governance_scoped_budgets_and_limits() {
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;

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
        assert_eq!(budget.limit_usd, 250.0);

        let limit = config
            .rate_limits
            .iter()
            .find(|l| l.scope == BudgetScope::Customer)
            .expect("customer rate limit in snapshot");
        assert_eq!(limit.id, customer_id.to_string());
        assert_eq!(limit.rpm, Some(60));
    }

    // the adaptive-routing kill switch and blend weights are control-plane
    // owned, so an operator change must reach the polling gateway (#544)
    #[tokio::test]
    async fn adaptive_routing_policy_projects_into_snapshot() {
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;

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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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
        assert!(config.guardrail_webhook.enabled);
        assert_eq!(
            config.guardrail_webhook.url,
            "https://guardrails.internal/evaluate"
        );
        assert!(matches!(
            config.guardrail_webhook.failure_mode,
            FailureMode::FailClosed
        ));
        assert!(matches!(
            config.guardrail_webhook.auth,
            Some(WebhookAuth::Bearer { ref token_env }) if token_env == "ROLTER_GUARDRAIL_TOKEN"
        ));
    }

    // #567 shipped the registry without gateway consumption; #509 wires the
    // dispatch runtime, so an enabled instance must now reach the snapshot
    #[tokio::test]
    async fn plugin_registry_projects_enabled_instances_into_snapshot() {
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;

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
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
        let store = PostgresConfigStore::new(pool);
        assert!(store.save(GatewayConfig::default()).await.is_err());
    }
}
