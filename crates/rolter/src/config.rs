//! `rolter config export` — the live configuration as an importable
//! `rolter.toml` (#1082).
//!
//! The counterpart to `rolter-seed --import`, and deliberately the same shape:
//! the importer treats a TOML file as desired state and converges the database
//! onto it, so a document that reproduces the database is a GitOps artifact, an
//! environment-promotion input and a config diff all at once.
//!
//! It reads the store directly rather than calling
//! `GET /api/v1/config/export`, the same way `rolter-seed` writes to it
//! directly: a bootstrap tool that needed a running control plane and an admin
//! token would be useless in exactly the situations it exists for. Both paths
//! render through [`rolter_control::config_export::render`], so the endpoint
//! and the CLI cannot drift.
//!
//! No credential is emitted. Provider keys leave only as the *name* of the
//! environment variable they are read from; a key sealed in the store is marked
//! with a comment where it would have been. `ROLTER_KEK` is therefore not
//! required to run this.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use rolter_store::postgres::PostgresConfigStore;
use rolter_store::ConfigStore;

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    /// write the deployment's live configuration as a rolter.toml that
    /// `rolter-seed --import` accepts
    Export(ExportArgs),
}

#[derive(Args, Debug)]
struct ExportArgs {
    /// postgres connection string for the control-plane store
    #[arg(long, env = "ROLTER_DATABASE_URL")]
    database_url: String,
    /// write to this file instead of stdout
    #[arg(long, short)]
    output: Option<PathBuf>,
}

pub async fn run(args: ConfigArgs) -> anyhow::Result<()> {
    match args.command {
        ConfigCommand::Export(export) => {
            let pool = rolter_store::postgres::connect(&export.database_url).await?;
            let store = PostgresConfigStore::new(pool.clone());
            let config = store.load().await;
            pool.close().await;
            let document = rolter_control::config_export::render(&config?);
            match export.output {
                // the document goes to the file and the confirmation to stderr,
                // so `rolter config export > rolter.toml` and `--output` both
                // produce a file with nothing but configuration in it
                Some(path) => {
                    std::fs::write(&path, document)?;
                    eprintln!("wrote {}", path.display());
                }
                None => print!("{document}"),
            }
            Ok(())
        }
    }
}
