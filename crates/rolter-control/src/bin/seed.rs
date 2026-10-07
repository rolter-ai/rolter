//! rolter-seed: bootstrap a rolter database from a clean state.
//!
//! Idempotently creates an org (+ a `default`/`default` team/project), an
//! optional admin user, and imports providers/routes from a bootstrap
//! `rolter.toml` — the import being an upsert, so the database ends up
//! matching the file. Thin CLI wrapper over [`rolter_control::seed::seed`],
//! which the unified launcher's `easy-up` subcommand reuses.

use std::path::PathBuf;

use clap::Parser;

use rolter_control::seed::{seed, SeedOptions};
use rolter_store::postgres::{connect, run_migrations};

#[derive(Parser, Debug)]
#[command(name = "rolter-seed", version, about = "bootstrap a rolter database")]
struct Args {
    #[arg(long, env = "ROLTER_DATABASE_URL", hide_env_values = true)]
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
    /// bootstrap rolter.toml to import providers/routes from. The file is the
    /// desired state: re-importing an edited file updates the rows it already
    /// created. Never overwrites a credential sealed through the dashboard,
    /// and never deletes rows the file no longer mentions
    #[arg(long)]
    import: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = rolter_core::telemetry::init();
    let args = Args::parse();

    let pool = connect(&args.database_url).await?;
    run_migrations(&pool).await?;

    let summary = seed(
        &pool,
        &SeedOptions {
            org: args.org,
            org_slug: args.org_slug,
            admin_email: args.admin_email,
            admin_password: args.admin_password,
            import: args.import,
        },
    )
    .await?;

    println!(
        "seeded org '{}' (slug '{}')",
        summary.org_name, summary.org_slug
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    /// `--help` prints `[env: NAME=value]` for every flag backed by a variable,
    /// so the database url, password and all, must opt out (#2793)
    #[test]
    fn secret_env_values_stay_out_of_help() {
        let checked = rolter_core::cli_guard::assert_secret_envs_hidden(&super::Args::command());
        assert!(checked > 0, "the walk found no secret-looking env flags");
    }
}
