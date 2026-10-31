//! rolter-seed: bootstrap a rolter database from a clean state.
//!
//! Idempotently creates an org (+ a `default`/`default` team/project), an
//! optional admin user, and imports providers/routes from a bootstrap
//! `rolter.toml` — the import being an upsert, so the database ends up
//! matching the file. Thin CLI wrapper over [`rolter_control::seed::run`],
//! which the unified launcher's `seed` subcommand shares.

use clap::Parser;

use rolter_control::seed::{run, SeedArgs};

#[derive(Parser, Debug)]
#[command(name = "rolter-seed", version, about = "bootstrap a rolter database")]
struct Args {
    #[command(flatten)]
    seed: SeedArgs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = rolter_core::telemetry::init();
    run(Args::parse().seed).await
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
