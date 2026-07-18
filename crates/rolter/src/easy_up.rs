//! `rolter easy-up`: one-command, production-ish bring-up.
//!
//! Unlike `just dev` (a source-checkout dev loop over `cargo`/`bun`), `easy-up`
//! runs from the installed `rolter` binary/image: it brings up the control
//! plane, the data-plane gateway, and the built UI in a single supervised
//! process, ready to serve immediately.
//!
//! - **no database**: auto-creates `rolter.toml` from the bundled example when
//!   missing and relies on the built-in `fake-llm` model, so it answers with
//!   zero provider keys and zero database.
//! - **with `--database-url`**: runs migrations, seeds idempotently (default
//!   org/team/project, optional admin, optional `--import`), starts the control
//!   plane DB-backed, and points the gateway at its snapshot endpoint.
//!
//! Safe to re-run; suitable as a container entrypoint (`CMD ["rolter",
//! "easy-up"]`).

use std::path::{Path, PathBuf};

use clap::Args;

/// bundled default config, written to disk on first run when none exists
///
/// kept as a copy inside the crate (rather than `include_str!`-ing the
/// workspace-root `rolter.example.toml`) because `cargo publish` packages and
/// verifies each crate in isolation, so paths outside the crate root aren't
/// available; a test (`bundled_example_config_matches_workspace_root`) guards
/// against the two copies drifting apart.
const EXAMPLE_CONFIG: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/rolter.example.toml"));

#[derive(Args, Debug)]
pub struct EasyUpArgs {
    /// bootstrap config file; created from the bundled example if missing
    #[arg(short, long, env = "ROLTER_CONFIG", default_value = "rolter.toml")]
    pub config: PathBuf,
    /// host both the gateway and the control plane bind to
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,
    /// gateway (data-plane) port
    #[arg(long, env = "ROLTER_PORT", default_value_t = 4000)]
    pub gateway_port: u16,
    /// control-plane / UI port
    #[arg(long, env = "ROLTER_CONTROL_PORT", default_value_t = 4001)]
    pub control_port: u16,
    /// directory holding the built UI (index.html + assets)
    #[arg(long, env = "ROLTER_UI_DIR", default_value = "ui/dist")]
    pub ui_dir: PathBuf,
    /// redis url; when set, control publishes config bumps and the gateway
    /// refetches immediately instead of waiting for its poll interval
    #[arg(long, env = "ROLTER_REDIS_URL")]
    pub redis_url: Option<String>,
    /// bearer token protecting the management API and snapshot endpoint;
    /// shared by the control plane (enforces) and gateway (sends)
    #[arg(long, env = "ROLTER_ADMIN_TOKEN")]
    pub admin_token: Option<String>,
    /// clickhouse http url; enables the dashboard usage/cost analytics
    #[arg(long, env = "CLICKHOUSE_URL")]
    pub clickhouse_url: Option<String>,
    /// postgres url; when set, runs migrations + seed and serves config from
    /// the database instead of the bootstrap toml
    #[cfg(feature = "postgres")]
    #[arg(long, env = "ROLTER_DATABASE_URL")]
    pub database_url: Option<String>,
    /// admin email to create on seed (with --admin-password); database mode only
    #[cfg(feature = "postgres")]
    #[arg(long)]
    pub admin_email: Option<String>,
    #[cfg(feature = "postgres")]
    #[arg(long)]
    pub admin_password: Option<String>,
    /// bootstrap rolter.toml to import providers/routes from on seed; database
    /// mode only (defaults to `--config`)
    #[cfg(feature = "postgres")]
    #[arg(long)]
    pub import: Option<PathBuf>,
}

/// Ensure a config file exists at `path`, writing the bundled example when it
/// does not. Returns `true` when a new file was created.
fn ensure_config(path: &Path) -> anyhow::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, EXAMPLE_CONFIG)?;
    Ok(true)
}

/// Build the gateway args for `easy-up`. In database mode the gateway polls the
/// control plane's snapshot endpoint; in file mode it runs from the local toml.
fn gateway_args(args: &EasyUpArgs, db_mode: bool) -> rolter_gateway::Args {
    rolter_gateway::Args {
        config: args.config.clone(),
        host: Some(args.host.clone()),
        port: Some(args.gateway_port),
        snapshot_url: db_mode
            .then(|| format!("http://127.0.0.1:{}/internal/snapshot", args.control_port)),
        snapshot_poll_secs: 5,
        redis_url: args.redis_url.clone(),
        // in db mode the gateway port doubles as the management surface:
        // /admin/* proxies to the co-hosted control plane
        admin_url: db_mode.then(|| format!("http://127.0.0.1:{}", args.control_port)),
        admin_token: args.admin_token.clone(),
    }
}

/// Build the control-plane args for `easy-up`.
fn control_args(args: &EasyUpArgs, database_url: Option<String>) -> rolter_control::Args {
    // `database_url` only backs a field under the postgres feature
    #[cfg(not(feature = "postgres"))]
    let _ = &database_url;
    rolter_control::Args {
        host: args.host.clone(),
        port: args.control_port,
        ui_dir: args.ui_dir.clone(),
        gateway_url: format!("http://127.0.0.1:{}", args.gateway_port),
        config: Some(args.config.clone()),
        #[cfg(feature = "postgres")]
        database_url,
        redis_url: args.redis_url.clone(),
        clickhouse_url: args.clickhouse_url.clone(),
        admin_token: args.admin_token.clone(),
    }
}

/// Run `easy-up` to completion: bootstrap config, optionally migrate+seed the
/// database, print a startup summary, then supervise the control plane and
/// gateway together.
pub async fn run(args: EasyUpArgs) -> anyhow::Result<()> {
    let created = ensure_config(&args.config)?;
    if created {
        tracing::info!(config = %args.config.display(), "created config from bundled example");
    }

    #[cfg(feature = "postgres")]
    let database_url: Option<String> = args.database_url.clone();
    #[cfg(not(feature = "postgres"))]
    let database_url: Option<String> = None;

    #[cfg(feature = "postgres")]
    let admin_note: Option<String> = if let Some(url) = database_url.clone() {
        use rolter_control::seed::{seed, SeedOptions};
        use rolter_store::postgres::{connect, run_migrations};

        tracing::info!("database mode: connecting, migrating and seeding");
        let pool = connect(&url).await?;
        run_migrations(&pool).await?;
        let summary = seed(
            &pool,
            &SeedOptions {
                org: "default".to_string(),
                org_slug: None,
                admin_email: args.admin_email.clone(),
                admin_password: args.admin_password.clone(),
                import: args.import.clone().or_else(|| Some(args.config.clone())),
            },
        )
        .await?;
        // release the bootstrap pool; control opens its own
        pool.close().await;
        summary
            .admin_created
            .then_some(summary.admin_email)
            .flatten()
    } else {
        None
    };
    #[cfg(not(feature = "postgres"))]
    let admin_note: Option<String> = None;

    let db_mode = database_url.is_some();
    print_summary(&args, db_mode, admin_note.as_deref());

    let control = rolter_control::run(control_args(&args, database_url));
    let gateway = rolter_gateway::run(gateway_args(&args, db_mode));

    // supervise both in one process; whichever exits (error or shutdown signal)
    // brings the command down
    tokio::try_join!(control, gateway)?;
    Ok(())
}

fn print_summary(args: &EasyUpArgs, db_mode: bool, admin_created_email: Option<&str>) {
    let mode = if db_mode { "database" } else { "file" };
    let display_host = match args.host.as_str() {
        "0.0.0.0" | "::" => "localhost",
        host => host,
    };
    eprintln!("\nrolter easy-up — {mode} mode");
    eprintln!(
        "  gateway (OpenAI/Anthropic):  http://{}:{}",
        display_host, args.gateway_port
    );
    eprintln!(
        "  control + UI:                http://{}:{}",
        display_host, args.control_port
    );
    eprintln!("  config:                      {}", args.config.display());
    if !db_mode {
        eprintln!("  built-in model:              fake-llm (no keys, no database)");
    }
    if let Some(email) = admin_created_email {
        eprintln!("  admin user created:          {email}");
    }
    eprintln!("\n  try it:");
    eprintln!(
        "  curl http://{display_host}:{}/v1/chat/completions -H 'Content-Type: application/json' -d '{{\"model\":\"fake-llm\",\"messages\":[{{\"role\":\"user\",\"content\":\"hello\"}}]}}'",
        args.gateway_port
    );
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_example_config_matches_workspace_root() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../rolter.example.toml");
        let root_contents = std::fs::read_to_string(root).unwrap();
        assert_eq!(
            EXAMPLE_CONFIG, root_contents,
            "crates/rolter/rolter.example.toml is out of sync with the workspace-root copy; \
             re-run `cp rolter.example.toml crates/rolter/rolter.example.toml`"
        );
    }

    #[test]
    fn ensure_config_writes_when_missing_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("rolter-easyup-{}", std::process::id()));
        let path = dir.join("rolter.toml");
        let _ = std::fs::remove_dir_all(&dir);

        // first call creates it from the bundled example
        assert!(ensure_config(&path).unwrap());
        assert!(path.exists());
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, EXAMPLE_CONFIG);

        // second call is a no-op (does not overwrite / re-report)
        assert!(!ensure_config(&path).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gateway_polls_snapshot_only_in_db_mode() {
        let args = EasyUpArgs {
            config: PathBuf::from("rolter.toml"),
            host: "0.0.0.0".to_string(),
            gateway_port: 4000,
            control_port: 4001,
            ui_dir: PathBuf::from("ui/dist"),
            redis_url: None,
            admin_token: None,
            clickhouse_url: None,
            #[cfg(feature = "postgres")]
            database_url: None,
            #[cfg(feature = "postgres")]
            admin_email: None,
            #[cfg(feature = "postgres")]
            admin_password: None,
            #[cfg(feature = "postgres")]
            import: None,
        };
        assert!(gateway_args(&args, false).snapshot_url.is_none());
        assert_eq!(
            gateway_args(&args, true).snapshot_url.as_deref(),
            Some("http://127.0.0.1:4001/internal/snapshot")
        );
    }
}
