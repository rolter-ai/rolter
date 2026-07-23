//! idempotent database bootstrap shared by the `rolter-seed` binary and the
//! unified launcher's `easy-up` subcommand.
//!
//! [`seed`] creates an org (+ a `default`/`default` team/project), an optional
//! admin user, and imports providers/routes from a bootstrap `rolter.toml`.
//! Every step checks for an existing row first, so re-running is a no-op.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHasher, SaltString};
use argon2::Argon2;
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{BalancingStrategy, GatewayConfig, ProviderKind};
use rolter_store::postgres::repo::{
    OrgRepo, ProjectRepo, ProviderRepo, RouteRepo, RouteTargetRepo, TeamRepo,
};

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
        .collect::<Vec<_>>()
        .join("-")
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

async fn import_bootstrap_toml(
    pool: &PgPool,
    org_id: Uuid,
    project_id: Uuid,
    path: &Path,
) -> anyhow::Result<()> {
    let config = GatewayConfig::load(path)?;
    let providers = ProviderRepo(pool);
    let routes = RouteRepo(pool);
    let targets = RouteTargetRepo(pool);

    let mut provider_ids = HashMap::new();
    for p in &config.providers {
        let kind = match p.kind {
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
        };
        let existing = providers
            .list(org_id)
            .await?
            .into_iter()
            .find(|row| row.name == p.name);
        let row = match existing {
            Some(row) => row,
            None => {
                let slug = p.slug.clone().unwrap_or_else(|| slugify(&p.name));
                providers
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
                    .await?
            }
        };
        provider_ids.insert(p.name.clone(), row.id);
        tracing::info!(provider = %p.name, "imported provider");
    }

    for r in &config.routes {
        let strategy = match r.strategy {
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
        };
        let existing = routes
            .list(project_id)
            .await?
            .into_iter()
            .find(|row| row.model == r.model);
        let route_row = match existing {
            Some(row) => row,
            None => routes.create(project_id, &r.model, strategy).await?,
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
            let already_imported = existing_targets.iter().any(|row| {
                row.provider_id == provider_id
                    && row.upstream_model.as_deref() == t.model.as_deref()
            });
            if already_imported {
                continue;
            }
            targets
                .create(
                    route_row.id,
                    provider_id,
                    t.model.as_deref(),
                    t.weight as i32,
                )
                .await?;
        }
        tracing::info!(model = %r.model, "imported route");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::slugify;

    #[test]
    fn slugify_normalizes() {
        assert_eq!(slugify("Default"), "default");
        assert_eq!(slugify("Acme Corp!"), "acme-corp");
        assert_eq!(slugify("  multi  space "), "multi-space");
    }
}
