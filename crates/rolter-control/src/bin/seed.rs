//! rolter-seed: bootstrap a rolter database from a clean state.
//!
//! Idempotently creates an org (+ a `default`/`default` team/project), an
//! optional admin user, and imports providers/routes from a bootstrap
//! `rolter.toml`. This is the write-side counterpart to the read-only
//! control-plane API surfaced while the full CRUD API (ROL-23) is built out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHasher, SaltString};
use argon2::Argon2;
use clap::Parser;
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{BalancingStrategy, GatewayConfig, ProviderKind};
use rolter_store::postgres::repo::{
    OrgRepo, ProjectRepo, ProviderRepo, RouteRepo, RouteTargetRepo, TeamRepo,
};
use rolter_store::postgres::{connect, run_migrations};

#[derive(Parser, Debug)]
#[command(name = "rolter-seed", version, about = "bootstrap a rolter database")]
struct Args {
    #[arg(long, env = "ROLTER_DATABASE_URL")]
    database_url: String,
    /// org display name; created if no org with the derived slug exists yet
    #[arg(long, default_value = "default")]
    org: String,
    /// org slug; defaults to a slugified `--org`
    #[arg(long)]
    org_slug: Option<String>,
    /// admin user email; combined with --admin-password to create an admin user
    #[arg(long)]
    admin_email: Option<String>,
    #[arg(long)]
    admin_password: Option<String>,
    /// bootstrap rolter.toml to import providers/routes from
    #[arg(long)]
    import: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rolter_core::telemetry::init();
    let args = Args::parse();

    let pool = connect(&args.database_url).await?;
    run_migrations(&pool).await?;

    let org_slug = args.org_slug.clone().unwrap_or_else(|| slugify(&args.org));
    let orgs = OrgRepo(&pool);
    let org = match orgs.list().await?.into_iter().find(|o| o.slug == org_slug) {
        Some(existing) => {
            tracing::info!(org = %existing.name, "org already exists, reusing");
            existing
        }
        None => {
            let created = orgs.create(&args.org, &org_slug).await?;
            tracing::info!(org = %created.name, "created org");
            created
        }
    };

    let teams = TeamRepo(&pool);
    let team = match teams
        .list(org.id)
        .await?
        .into_iter()
        .find(|t| t.name == "default")
    {
        Some(t) => t,
        None => teams.create(org.id, "default").await?,
    };

    let projects = ProjectRepo(&pool);
    let project = match projects
        .list(team.id)
        .await?
        .into_iter()
        .find(|p| p.name == "default")
    {
        Some(p) => p,
        None => projects.create(team.id, "default").await?,
    };

    if let (Some(email), Some(password)) = (&args.admin_email, &args.admin_password) {
        create_admin(&pool, email, password).await?;
    }

    if let Some(path) = &args.import {
        import_bootstrap_toml(&pool, org.id, project.id, path).await?;
    }

    println!("seeded org '{}' (slug '{}')", org.name, org.slug);
    Ok(())
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

async fn create_admin(pool: &PgPool, email: &str, password: &str) -> anyhow::Result<()> {
    let existing: Option<Uuid> = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    if existing.is_some() {
        tracing::info!(%email, "admin user already exists, skipping");
        return Ok(());
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
    Ok(())
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
        };
        let existing = providers
            .list(org_id)
            .await?
            .into_iter()
            .find(|row| row.name == p.name);
        let row = match existing {
            Some(row) => row,
            None => {
                providers
                    .create(
                        org_id,
                        &p.name,
                        kind,
                        &p.api_base,
                        p.api_key_env.as_deref(),
                        p.egress_proxy.as_deref(),
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
