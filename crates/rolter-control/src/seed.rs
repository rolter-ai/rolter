//! idempotent database bootstrap shared by the `rolter-seed` binary and the
//! unified launcher's `easy-up` subcommand.
//!
//! [`seed`] creates an org (+ a `default`/`default` team/project), an optional
//! admin user, and imports providers, provider groups, routes, model prices
//! and prompt templates from a bootstrap `rolter.toml`. The org, team, project
//! and admin steps check for an existing row first, so re-running creates
//! nothing twice.
//!
//! What this consumes is what [`crate::config_export::render`] emits: the two
//! are the read and write halves of one round trip, so a section added to one
//! belongs in the other (#1082).
//!
//! The bootstrap-toml import is an **upsert**: the file is the desired state,
//! so a re-import of an edited file applies the edits rather than skipping the
//! rows it already created (#927). "Idempotent" used to mean only that
//! re-running was safe — an operator who added a missing `api_key_env` and
//! re-imported got `imported provider` in the log and no change in the
//! database, and found out through 401s from the upstream that read as a bad
//! key rather than as config that was never applied.
//!
//! Two things are deliberately left alone by a re-import:
//!
//! - the **sealed credential** in `provider_keys`. A key rotated through the
//!   dashboard outranks `api_key_env` in the snapshot's precedence, and a
//!   stale value in a file must not clobber it.
//! - a provider's **slug**, its stable identity once assigned.
//!
//! Rows present in the database but absent from the file are not deleted: the
//! file is the desired state for what it names, not an exhaustive inventory.
//!
//! That last rule is why `[logging.payload_capture]` is imported on *presence*
//! rather than on value (#954). Every other section of the file is a list, so
//! "absent" and "empty" look the same and mean the same. `logging` is a single
//! row with `serde` defaults, so a parsed config always carries a
//! `payload_capture` block whether or not the file wrote one — importing it
//! unconditionally would turn every re-import into a silent "capture off",
//! undoing a decision an operator made in the dashboard. The raw TOML is
//! therefore re-parsed to ask whether the key was actually written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHasher, SaltString};
use argon2::Argon2;
use rolter_core::{BalancingStrategy, GatewayConfig, PromptTemplate, ProviderKind};
use rolter_store::postgres::repo::{
    LoggingSettingsRepo, ModelPriceRepo, OrgRepo, ProjectRepo, ProviderGroupRepo, ProviderRepo,
    RouteRepo, RouteTargetRepo, TeamRepo,
};
use sqlx::PgPool;
use uuid::Uuid;

/// Options controlling what [`seed`] creates. `org` defaults to `default` when
/// left empty; `org_slug` defaults to a slugified `org`.
#[derive(Debug, Clone, Default)]
pub struct SeedOptions {
    pub org: String,
    pub org_slug: Option<String>,
    pub admin_email: Option<String>,
    pub admin_password: Option<String>,
    pub import: Option<PathBuf>,
}

/// What [`seed`] found or created — enough for a caller to print a summary
/// without re-querying (never carries the admin password).
#[derive(Debug, Clone)]
pub struct SeedSummary {
    pub org_name: String,
    pub org_slug: String,
    pub admin_email: Option<String>,
    pub admin_created: bool,
}

/// Idempotently bootstrap `pool`: org, default team/project, optional admin,
/// optional bootstrap-toml import. Assumes migrations have already run.
pub async fn seed(pool: &PgPool, opts: &SeedOptions) -> anyhow::Result<SeedSummary> {
    let org_name = if opts.org.trim().is_empty() {
        "default"
    } else {
        opts.org.trim()
    };
    let org_slug = opts.org_slug.clone().unwrap_or_else(|| slugify(org_name));

    let orgs = OrgRepo(pool);
    let org = match orgs.list().await?.into_iter().find(|o| o.slug == org_slug) {
        Some(existing) => {
            tracing::info!(org = %existing.name, "org already exists, reusing");
            existing
        }
        None => {
            let created = orgs.create(org_name, &org_slug).await?;
            tracing::info!(org = %created.name, "created org");
            created
        }
    };

    let teams = TeamRepo(pool);
    let team = match teams
        .list(org.id)
        .await?
        .into_iter()
        .find(|t| t.name == "default")
    {
        Some(t) => t,
        None => teams.create(org.id, "default").await?,
    };

    let projects = ProjectRepo(pool);
    let project = match projects
        .list(team.id)
        .await?
        .into_iter()
        .find(|p| p.name == "default")
    {
        Some(p) => p,
        None => projects.create(team.id, "default").await?,
    };

    let mut admin_created = false;
    if let (Some(email), Some(password)) = (&opts.admin_email, &opts.admin_password) {
        admin_created = create_admin(pool, email, password).await?;
    }

    if let Some(path) = &opts.import {
        import_bootstrap_toml(pool, org.id, project.id, path).await?;
    }

    Ok(SeedSummary {
        org_name: org.name,
        org_slug: org.slug,
        admin_email: opts.admin_email.clone(),
        admin_created,
    })
}

fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .fold(String::new(), |mut acc, s| {
            if !acc.is_empty() {
                acc.push('-');
            }
            acc.push_str(s);
            acc
        })
}

/// The `providers.kind` column value for a parsed [`ProviderKind`]. Shared by
/// the provider import and anything else that has to name the stored enum.
fn provider_kind_column(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Openai => "openai",
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenaiCompatible => "openai_compatible",
        ProviderKind::Ollama => "ollama",
        ProviderKind::OllamaCloud => "ollama_cloud",
        ProviderKind::LlamaCpp => "llama_cpp",
        ProviderKind::Openrouter => "openrouter",
        ProviderKind::Tei => "tei",
        ProviderKind::AzureOpenai => "azure_openai",
        ProviderKind::Bedrock => "bedrock",
        ProviderKind::Vertex => "vertex",
        ProviderKind::Gemini => "gemini",
        ProviderKind::GeminiNative => "gemini_native",
        ProviderKind::GeminiInteractions => "gemini_interactions",
        ProviderKind::Mistral => "mistral",
        ProviderKind::Groq => "groq",
        ProviderKind::Xai => "xai",
        ProviderKind::MetaLlamaApi => "meta_llama_api",
        ProviderKind::Cohere => "cohere",
        ProviderKind::Perplexity => "perplexity",
        ProviderKind::Together => "together",
        ProviderKind::Fireworks => "fireworks",
        ProviderKind::Databricks => "databricks",
        ProviderKind::AlephAlpha => "aleph_alpha",
        ProviderKind::Nebius => "nebius",
        ProviderKind::Ovhcloud => "ovhcloud",
        ProviderKind::Scaleway => "scaleway",
        ProviderKind::Deepseek => "deepseek",
        ProviderKind::Qwen => "qwen",
        ProviderKind::Zhipu => "zhipu",
        ProviderKind::Kimi => "kimi",
        ProviderKind::Ernie => "ernie",
        ProviderKind::Doubao => "doubao",
        ProviderKind::Hunyuan => "hunyuan",
        ProviderKind::Yi => "yi",
        ProviderKind::Minimax => "minimax",
        ProviderKind::Baichuan => "baichuan",
        ProviderKind::Gigachat => "gigachat",
        ProviderKind::YandexGpt => "yandex_gpt",
        ProviderKind::CloudRu => "cloud_ru",
        ProviderKind::MtsAi => "mts_ai",
        ProviderKind::Naver => "naver",
        ProviderKind::Upstage => "upstage",
        ProviderKind::Rinna => "rinna",
        ProviderKind::Rakuten => "rakuten",
        ProviderKind::Sarvam => "sarvam",
        ProviderKind::Krutrim => "krutrim",
        ProviderKind::Falcon => "falcon",
    }
}

/// The stored strategy value for a parsed [`BalancingStrategy`]. Routes and
/// provider groups share the column vocabulary, so they share this.
fn strategy_column(strategy: BalancingStrategy) -> &'static str {
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

/// Create a superadmin user; returns `true` when a new row was inserted and
/// `false` when one already existed for `email`.
async fn create_admin(pool: &PgPool, email: &str, password: &str) -> anyhow::Result<bool> {
    let existing: Option<Uuid> = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    if existing.is_some() {
        tracing::info!(%email, "admin user already exists, skipping");
        return Ok(false);
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hash admin password: {e}"))?
        .to_string();

    sqlx::query("insert into users (email, password_hash, is_superadmin) values ($1, $2, true)")
        .bind(email)
        .bind(hash)
        .execute(pool)
        .await?;
    tracing::info!(%email, "created admin user");
    Ok(true)
}

pub(crate) async fn import_bootstrap_toml(
    pool: &PgPool,
    org_id: Uuid,
    project_id: Uuid,
    path: &Path,
) -> anyhow::Result<()> {
    let config = GatewayConfig::load(path)?;
    import_config(pool, org_id, project_id, &config).await?;
    if declares_payload_capture(path)? {
        import_payload_capture(pool, &config.logging.payload_capture).await?;
    }
    Ok(())
}

/// Whether the file at `path` writes a `[logging.payload_capture]` section of
/// its own. Parsed from the raw text rather than from [`GatewayConfig`],
/// which cannot tell a written default from an unwritten one.
fn declares_payload_capture(path: &Path) -> anyhow::Result<bool> {
    let text = std::fs::read_to_string(path)?;
    let value: toml::Value = toml::from_str(&text)?;
    Ok(value
        .get("logging")
        .and_then(|logging| logging.get("payload_capture"))
        .is_some())
}

/// Apply the file's payload-capture policy to the single `logging_settings`
/// row, leaving sampling and retention as they are — those are operational
/// dials the file has no opinion about.
async fn import_payload_capture(
    pool: &PgPool,
    capture: &rolter_core::PayloadCaptureConfig,
) -> anyhow::Result<()> {
    let repo = LoggingSettingsRepo(pool);
    let current = repo.get().await?;
    let max_bytes = i32::try_from(capture.max_bytes).unwrap_or(i32::MAX);
    repo.update(
        current.sample_rate,
        capture.enabled,
        max_bytes,
        &capture.redact_fields,
        &capture.models,
        &capture.virtual_key_ids,
        current.retention_days,
        current.payload_retention_hours,
    )
    .await?;
    tracing::info!(
        enabled = capture.enabled,
        max_bytes,
        "imported payload capture policy"
    );
    Ok(())
}

/// Upsert an already-parsed bootstrap config. Split out from
/// [`import_bootstrap_toml`] so the desired-state behaviour can be tested
/// without going through a file on disk.
async fn import_config(
    pool: &PgPool,
    org_id: Uuid,
    project_id: Uuid,
    config: &GatewayConfig,
) -> anyhow::Result<()> {
    let providers = ProviderRepo(pool);
    let routes = RouteRepo(pool);
    let targets = RouteTargetRepo(pool);

    let mut provider_ids = HashMap::new();
    for p in &config.providers {
        let kind = provider_kind_column(p.kind);
        let existing = providers
            .list(org_id)
            .await?
            .into_iter()
            .find(|row| row.name == p.name);
        let row = match existing {
            // the file is the desired state, so an existing row is brought in
            // line with it rather than left alone (#927). the sealed credential
            // in `provider_keys` is never touched: a key rotated through the
            // dashboard outranks `api_key_env` in the snapshot's precedence and
            // must not be clobbered by a stale value from a file
            Some(row) => {
                let unchanged = row.kind == kind
                    && row.api_base == p.api_base
                    && row.api_key_env == p.api_key_env
                    && row.egress_proxy == p.egress_proxy
                    && row.egress_proxies.0 == p.egress_proxies;
                if unchanged {
                    tracing::info!(provider = %p.name, "provider unchanged");
                    row
                } else {
                    let updated = providers
                        .update(
                            row.id,
                            // slug is immutable once assigned; renaming the
                            // stable identity of a provider is not something a
                            // re-import should do behind the operator's back
                            None,
                            Some(kind),
                            Some(&p.api_base),
                            Some(p.api_key_env.as_deref()),
                            Some(p.egress_proxy.as_deref()),
                            Some(&p.egress_proxies),
                        )
                        .await?;
                    tracing::info!(provider = %p.name, "updated provider from file");
                    updated
                }
            }
            None => {
                let slug = p.slug.clone().unwrap_or_else(|| slugify(&p.name));
                let created = providers
                    .create(
                        org_id,
                        &p.name,
                        &slug,
                        kind,
                        &p.api_base,
                        p.api_key_env.as_deref(),
                        p.egress_proxy.as_deref(),
                        &p.egress_proxies,
                    )
                    .await?;
                tracing::info!(provider = %p.name, "created provider");
                created
            }
        };
        provider_ids.insert(p.name.clone(), row.id);
    }

    for r in &config.routes {
        let strategy = strategy_column(r.strategy);
        let existing = routes
            .list(project_id)
            .await?
            .into_iter()
            .find(|row| row.model == r.model);
        let route_row = match existing {
            Some(row) if row.strategy == strategy => {
                tracing::info!(model = %r.model, "route unchanged");
                row
            }
            Some(row) => {
                let updated = routes.set_strategy(row.id, strategy).await?;
                tracing::info!(
                    model = %r.model,
                    from = %row.strategy,
                    to = strategy,
                    "updated route strategy from file"
                );
                updated
            }
            None => {
                let created = routes.create(project_id, &r.model, strategy).await?;
                tracing::info!(model = %r.model, "created route");
                created
            }
        };

        // round-trip the admin param defaults + override policy (idempotent:
        // re-importing overwrites with the toml's current values)
        if !r.params.is_empty()
            || r.param_policy.mode != rolter_core::OverrideMode::Allow
            || !r.param_policy.allow.is_empty()
            || !r.param_policy.deny.is_empty()
        {
            let params = serde_json::to_value(&r.params)?;
            let param_policy = serde_json::to_value(&r.param_policy)?;
            routes
                .set_params(route_row.id, &params, &param_policy)
                .await?;
        }

        let existing_targets = targets.list(route_row.id).await?;
        for t in &r.targets {
            let Some(&provider_id) = provider_ids.get(&t.provider) else {
                tracing::warn!(
                    target_provider = %t.provider,
                    model = %r.model,
                    "skipping target: provider not imported"
                );
                continue;
            };
            // provider + upstream model is the natural key; the weight is the
            // part an operator edits and re-imports, so a match updates it
            // rather than counting as "already there" (#927)
            let existing_target = existing_targets.iter().find(|row| {
                row.provider_id == provider_id
                    && row.upstream_model.as_deref() == t.model.as_deref()
            });
            match existing_target {
                Some(row) if row.weight == t.weight as i32 => continue,
                Some(row) => {
                    targets.set_weight(row.id, t.weight as i32).await?;
                    tracing::info!(
                        model = %r.model,
                        target_provider = %t.provider,
                        from = row.weight,
                        to = t.weight,
                        "updated target weight from file"
                    );
                }
                None => {
                    targets
                        .create(
                            route_row.id,
                            provider_id,
                            t.model.as_deref(),
                            t.weight as i32,
                        )
                        .await?;
                    tracing::info!(
                        model = %r.model,
                        target_provider = %t.provider,
                        "created route target"
                    );
                }
            }
        }
    }
    import_provider_groups(pool, org_id, config, &provider_ids).await?;
    import_model_prices(pool, config).await?;
    import_prompt_templates(pool, org_id, project_id, config).await?;

    Ok(())
}

/// Upsert `[[provider_groups]]` keyed by slug, then replace each group's
/// membership with what the file names — the same desired-state rule the rest
/// of the import follows (#1082, so a configuration export re-imports whole).
///
/// A group whose members do not all resolve to providers the file also
/// declares is left entirely alone. Applying a partial membership would delete
/// the members the file could not name, which is the one thing an import is
/// never allowed to do.
async fn import_provider_groups(
    pool: &PgPool,
    org_id: Uuid,
    config: &GatewayConfig,
    provider_ids: &HashMap<String, Uuid>,
) -> anyhow::Result<()> {
    if config.provider_groups.is_empty() {
        return Ok(());
    }
    let groups = ProviderGroupRepo(pool);
    for g in &config.provider_groups {
        let slug = g.slug.clone().unwrap_or_else(|| slugify(&g.name));
        let strategy = strategy_column(g.strategy);
        let existing = groups
            .list(org_id)
            .await?
            .into_iter()
            .find(|row| row.slug == slug);
        let row = match existing {
            Some(row) if row.name == g.name && row.strategy == strategy => row,
            // the slug is the stable identity, so it is never rewritten — the
            // same rule providers follow
            Some(row) => {
                let updated = groups
                    .update(row.id, Some(&g.name), None, Some(strategy))
                    .await?;
                tracing::info!(group = %g.name, "updated provider group from file");
                updated
            }
            None => {
                let created = groups.create(org_id, &g.name, &slug, strategy).await?;
                tracing::info!(group = %g.name, "created provider group");
                created
            }
        };

        let mut members = Vec::with_capacity(g.members.len());
        let mut unresolved = false;
        for member in &g.members {
            match provider_ids.get(&member.provider) {
                Some(&provider_id) => {
                    members.push((provider_id, member.model.clone(), member.weight as i32))
                }
                None => {
                    tracing::warn!(
                        group = %g.name,
                        member_provider = %member.provider,
                        "leaving group membership alone: provider not imported"
                    );
                    unresolved = true;
                }
            }
        }
        if !unresolved {
            groups.set_members(row.id, &members).await?;
        }
    }
    Ok(())
}

/// Upsert `[[model_prices]]` keyed by public model name. Prices are what a
/// budget counts against, so an export that carried routes without them would
/// promote a deployment that silently charges nothing (#1082).
async fn import_model_prices(pool: &PgPool, config: &GatewayConfig) -> anyhow::Result<()> {
    if config.model_prices.is_empty() {
        return Ok(());
    }
    let prices = ModelPriceRepo(pool);
    for price in &config.model_prices {
        // the column is `numeric`, so the rate crosses as text rather than
        // through a float bind that would round it on the way in
        prices
            .upsert(
                &price.model,
                &price.input_per_mtok.to_string(),
                &price.output_per_mtok.to_string(),
                price
                    .cached_input_per_mtok
                    .map(|v| v.to_string())
                    .as_deref(),
                &price.currency,
            )
            .await?;
        tracing::info!(model = %price.model, "imported model price");
    }
    Ok(())
}

async fn import_prompt_templates(
    pool: &PgPool,
    org_id: Uuid,
    project_id: Uuid,
    config: &GatewayConfig,
) -> anyhow::Result<()> {
    if config.prompt_templates.templates.is_empty() {
        return Ok(());
    }

    let routes = RouteRepo(pool);
    let route_rows = routes.list(project_id).await?;
    let mut route_ids_by_model: HashMap<String, Vec<Uuid>> = HashMap::new();
    for route in route_rows {
        route_ids_by_model
            .entry(route.model.clone())
            .or_default()
            .push(route.id);
    }

    for template in &config.prompt_templates.templates {
        import_prompt_template(pool, org_id, template, &route_ids_by_model).await?;
    }
    Ok(())
}

async fn import_prompt_template(
    pool: &PgPool,
    org_id: Uuid,
    template: &PromptTemplate,
    route_ids_by_model: &HashMap<String, Vec<Uuid>>,
) -> anyhow::Result<()> {
    let variables = serde_json::to_value(&template.variables)?;
    let decorators = serde_json::to_value(&template.decorators)?;
    let version = i32::try_from(template.version)
        .map_err(|_| anyhow::anyhow!("prompt template version out of range"))?;

    let mut tx = pool.begin().await?;
    sqlx::query(
        "insert into prompt_templates (org_id, name, slug, description)
         values ($1, $2, $3, '')
         on conflict (org_id, slug) do nothing",
    )
    .bind(org_id)
    .bind(&template.id)
    .bind(&template.id)
    .execute(&mut *tx)
    .await?;
    let template_id: Uuid =
        sqlx::query_scalar("select id from prompt_templates where org_id = $1 and slug = $2")
            .bind(org_id)
            .bind(&template.id)
            .fetch_one(&mut *tx)
            .await?;

    let existing_version: Option<(serde_json::Value, serde_json::Value)> = sqlx::query_as(
        "select variables, decorators
         from prompt_template_versions
         where template_id = $1 and version = $2",
    )
    .bind(template_id)
    .bind(version)
    .fetch_optional(&mut *tx)
    .await?;
    match existing_version {
        Some((stored_variables, stored_decorators))
            if stored_variables != variables || stored_decorators != decorators =>
        {
            anyhow::bail!(
                "prompt template '{}' version {} differs from the immutable stored version",
                template.id,
                template.version
            );
        }
        Some(_) => {}
        None => {
            sqlx::query(
                "insert into prompt_template_versions (
                     template_id, version, variables, decorators
                 )
                 values ($1, $2, $3, $4)",
            )
            .bind(template_id)
            .bind(version)
            .bind(&variables)
            .bind(&decorators)
            .execute(&mut *tx)
            .await?;
        }
    }

    let published_version: Option<i32> =
        sqlx::query_scalar("select published_version from prompt_templates where id = $1")
            .bind(template_id)
            .fetch_one(&mut *tx)
            .await?;
    if published_version.is_none() {
        if template.routes.is_empty() {
            sqlx::query(
                "insert into prompt_template_scopes (
                     template_id, version, scope_type, scope_id, org_id
                 )
                 values ($1, $2, 'org', $3, $3)
                 on conflict (template_id, version, scope_type, scope_id) do nothing",
            )
            .bind(template_id)
            .bind(version)
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        } else {
            for model in &template.routes {
                let Some(route_ids) = route_ids_by_model.get(model) else {
                    tracing::warn!(template = %template.id, model, "skipping prompt-template route scope: route model not imported");
                    continue;
                };
                for route_id in route_ids {
                    sqlx::query(
                        "insert into prompt_template_scopes (
                             template_id, version, scope_type, scope_id, route_id
                         )
                         values ($1, $2, 'route', $3, $3)
                         on conflict (template_id, version, scope_type, scope_id) do nothing",
                    )
                    .bind(template_id)
                    .bind(version)
                    .bind(route_id)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }

        sqlx::query(
            "update prompt_templates
             set published_version = $2
             where id = $1",
        )
        .bind(template_id)
        .bind(version)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    tracing::info!(template = %template.id, version = template.version, "imported prompt template");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{declares_payload_capture, import_bootstrap_toml, import_prompt_template, slugify};
    use rolter_core::{
        Decorator, DecoratorPosition, DecoratorRole, PromptTemplate, TemplateVariable,
    };
    use rolter_store::postgres::repo::{ProviderRepo, RouteRepo, RouteTargetRepo};
    use std::collections::HashMap;
    use uuid::Uuid;

    #[test]
    fn slugify_normalizes() {
        assert_eq!(slugify("Default"), "default");
        assert_eq!(slugify("Acme Corp!"), "acme-corp");
        assert_eq!(slugify("  multi  space "), "multi-space");
    }

    /// #927: a re-import of an edited file used to log `imported provider` and
    /// change nothing. The operator's next signal was a 401 from the upstream,
    /// which reads as a bad key rather than as config that was never applied.
    #[tokio::test]
    async fn reimporting_an_edited_file_applies_the_edits() {
        let Some(pool) = scratch_db("reimport").await else {
            return;
        };
        let (org_id, project_id) = bootstrap_org(&pool).await;

        let dir = tempdir("reimport");
        let path = dir.join("rolter.toml");
        std::fs::write(
            &path,
            r#"
[[providers]]
name = "tei-embed-02"
kind = "tei"
api_base = "http://tei:80"

[[routes]]
model = "embed"
strategy = "round_robin"
[[routes.targets]]
provider = "tei-embed-02"
weight = 1
"#,
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        // the exact edit from the issue: an api_key_env that was missing, plus
        // a corrected base, a different strategy and a re-weighted target
        std::fs::write(
            &path,
            r#"
[[providers]]
name = "tei-embed-02"
kind = "tei"
api_base = "http://tei:8080"
api_key_env = "TEI_KEY"

[[routes]]
model = "embed"
strategy = "power_of_two"
[[routes.targets]]
provider = "tei-embed-02"
weight = 7
"#,
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        let provider = ProviderRepo(&pool)
            .list(org_id)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.name == "tei-embed-02")
            .expect("the provider must still be there");
        assert_eq!(
            provider.api_key_env.as_deref(),
            Some("TEI_KEY"),
            "the added api_key_env was not applied"
        );
        assert_eq!(provider.api_base, "http://tei:8080");

        let route = RouteRepo(&pool)
            .list(project_id)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.model == "embed")
            .expect("the route must still be there");
        assert_eq!(route.strategy, "power_of_two");

        let targets = RouteTargetRepo(&pool).list(route.id).await.unwrap();
        assert_eq!(targets.len(), 1, "the target was duplicated, not updated");
        assert_eq!(targets[0].weight, 7);

        // and nothing was duplicated along the way
        let providers = ProviderRepo(&pool).list(org_id).await.unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(RouteRepo(&pool).list(project_id).await.unwrap().len(), 1);
    }

    /// #954: presence, not value, is what makes the import touch capture. A
    /// parsed [`GatewayConfig`] always carries a `payload_capture` block, so
    /// only the raw text can say whether the operator wrote one.
    #[test]
    fn only_a_written_capture_section_counts_as_declared() {
        let dir = tempdir("declares");
        let path = dir.join("rolter.toml");

        std::fs::write(&path, "[logging]\nclickhouse_url = \"http://ch:8123\"\n").unwrap();
        assert!(
            !declares_payload_capture(&path).unwrap(),
            "a logging section without capture must not count as declaring one"
        );

        std::fs::write(&path, "[[providers]]\nname = \"a\"\nkind = \"openai\"\n").unwrap();
        assert!(
            !declares_payload_capture(&path).unwrap(),
            "a file with no logging section at all must not count"
        );

        // written and off is still written: turning capture *off* through the
        // file has to be applicable too
        std::fs::write(&path, "[logging.payload_capture]\nenabled = false\n").unwrap();
        assert!(declares_payload_capture(&path).unwrap());

        std::fs::write(&path, "[logging.payload_capture]\nenabled = true\n").unwrap();
        assert!(declares_payload_capture(&path).unwrap());
    }

    /// The dogfood profile turns capture on through the import file, and the
    /// gateway only sees that once it reaches `logging_settings` — the row the
    /// snapshot is built from. A file that says nothing must leave the row
    /// exactly as the dashboard left it.
    #[tokio::test]
    async fn a_declared_capture_policy_lands_in_logging_settings() {
        let Some(pool) = scratch_db("capture").await else {
            return;
        };
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let settings = rolter_store::postgres::repo::LoggingSettingsRepo(&pool);

        let dir = tempdir("capture");
        let path = dir.join("rolter.toml");
        std::fs::write(
            &path,
            r#"
[logging.payload_capture]
enabled = true
max_bytes = 32768
redact_fields = ["authorization", "api_key"]
models = ["gpt-4o"]
"#,
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        let row = settings.get().await.unwrap();
        assert!(row.payload_capture_enabled, "capture never reached the row");
        assert_eq!(row.payload_capture_max_bytes, 32768);
        assert_eq!(
            row.payload_capture_redact_fields,
            vec!["authorization".to_string(), "api_key".to_string()]
        );
        assert_eq!(row.payload_capture_models, vec!["gpt-4o".to_string()]);
        // sampling and retention are operational dials the file has no opinion
        // about, so the import leaves them where they were
        let sample_rate = row.sample_rate;
        let retention_days = row.retention_days;

        // a file that never mentions capture must not silently switch it off
        std::fs::write(
            &path,
            "[[providers]]\nname = \"openai-primary\"\nkind = \"openai\"\napi_base = \"https://api.openai.com\"\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        let row = settings.get().await.unwrap();
        assert!(
            row.payload_capture_enabled,
            "a file silent about capture turned it off"
        );
        assert_eq!(row.sample_rate, sample_rate);
        assert_eq!(row.retention_days, retention_days);
    }

    /// The upsert must not reach into `provider_keys`: a key rotated through
    /// the dashboard outranks `api_key_env`, and a stale file must not
    /// clobber it.
    #[tokio::test]
    async fn a_reimport_leaves_a_dashboard_sealed_credential_alone() {
        let Some(pool) = scratch_db("sealed").await else {
            return;
        };
        let (org_id, project_id) = bootstrap_org(&pool).await;

        let dir = tempdir("sealed");
        let path = dir.join("rolter.toml");
        let without_env = r#"
[[providers]]
name = "openai-primary"
kind = "openai"
api_base = "https://api.openai.com"
"#;
        std::fs::write(&path, without_env).unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        let provider_id = ProviderRepo(&pool).list(org_id).await.unwrap()[0].id;
        // stand in for a rotation through the dashboard
        sqlx::query(
            "insert into provider_keys (provider_id, ciphertext, nonce) values ($1, $2, $3)",
        )
        .bind(provider_id)
        .bind(b"sealed-by-the-dashboard".to_vec())
        .bind(b"nonce".to_vec())
        .execute(&pool)
        .await
        .unwrap();

        std::fs::write(
            &path,
            r#"
[[providers]]
name = "openai-primary"
kind = "openai"
api_base = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
"#,
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        let sealed: Vec<u8> =
            sqlx::query_scalar("select ciphertext from provider_keys where provider_id = $1")
                .bind(provider_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            sealed, b"sealed-by-the-dashboard",
            "the re-import overwrote a rotated credential"
        );
    }

    /// A scratch directory for the bootstrap toml a test writes.
    fn tempdir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rolter-seed-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A schema of its own per test: the coverage job runs plain `cargo test`
    /// against a shared database and would otherwise race.
    async fn scratch_db(label: &str) -> Option<sqlx::PgPool> {
        let url = std::env::var("ROLTER_TEST_DATABASE_URL").ok().or_else(|| {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            None
        })?;
        let schema = format!("seed_{label}_{}", Uuid::new_v4().simple());
        let admin = rolter_store::postgres::connect(&url).await.unwrap();
        sqlx::query(&format!("create schema {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let separator = if url.contains('?') { '&' } else { '?' };
        let scoped = format!("{url}{separator}options=-c%20search_path%3D{schema}");
        let pool = rolter_store::postgres::connect(&scoped).await.unwrap();
        rolter_store::postgres::run_migrations(&pool).await.unwrap();
        Some(pool)
    }

    async fn bootstrap_org(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let team = super::TeamRepo(pool)
            .create(org_id, "default")
            .await
            .unwrap();
        let project = super::ProjectRepo(pool)
            .create(team.id, "default")
            .await
            .unwrap();
        (org_id, project.id)
    }

    #[tokio::test]
    async fn prompt_template_seed_is_idempotent_and_rejects_version_drift() {
        let Ok(url) = std::env::var("ROLTER_TEST_DATABASE_URL") else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let schema = format!("seed_test_{}", Uuid::new_v4().simple());
        let admin = rolter_store::postgres::connect(&url).await.unwrap();
        sqlx::query(&format!("create schema {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let separator = if url.contains('?') { '&' } else { '?' };
        let scoped_url = format!("{url}{separator}options=-c%20search_path%3D{schema}");
        let pool = rolter_store::postgres::connect(&scoped_url).await.unwrap();
        rolter_store::postgres::run_migrations(&pool).await.unwrap();
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let template = PromptTemplate {
            id: "support-baseline".to_string(),
            version: 1,
            routes: Vec::new(),
            scopes: Vec::new(),
            variables: vec![TemplateVariable {
                name: "tone".to_string(),
                required: false,
                default: Some("calm".to_string()),
            }],
            decorators: vec![Decorator {
                role: DecoratorRole::System,
                position: DecoratorPosition::Prepend,
                content: "tone={{ tone }}".to_string(),
            }],
        };
        let route_ids = HashMap::new();
        import_prompt_template(&pool, org_id, &template, &route_ids)
            .await
            .unwrap();
        import_prompt_template(&pool, org_id, &template, &route_ids)
            .await
            .unwrap();

        let template_count: i64 = sqlx::query_scalar("select count(*) from prompt_templates")
            .fetch_one(&pool)
            .await
            .unwrap();
        let version_count: i64 =
            sqlx::query_scalar("select count(*) from prompt_template_versions")
                .fetch_one(&pool)
                .await
                .unwrap();
        let scope_count: i64 = sqlx::query_scalar("select count(*) from prompt_template_scopes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((template_count, version_count, scope_count), (1, 1, 1));

        let mut drifted = template;
        drifted.decorators[0].content = "different".to_string();
        assert!(import_prompt_template(&pool, org_id, &drifted, &route_ids)
            .await
            .is_err());
    }
}
