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
pub(crate) const EXAMPLE_CONFIG: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/rolter.example.toml"));

#[derive(Args, Debug)]
pub struct EasyUpArgs {
    /// bootstrap config file; created from the bundled example if missing
    #[arg(short, long, env = "ROLTER_CONFIG", default_value = "rolter.toml")]
    pub config: PathBuf,
    /// host both the gateway and the control plane bind to. Loopback by
    /// default: `easy-up` is the zero-credential local path, and the control
    /// plane it starts is unauthenticated unless `--admin-token` is set, which
    /// must not reach a public interface by omission (#970). Pass
    /// `--host 0.0.0.0` with an admin token to serve a network
    #[arg(long, default_value = "127.0.0.1")]
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
    /// acknowledge serving an unauthenticated management API on a non-loopback
    /// `--host`. Without it that combination refuses to start (#970). Accepts
    /// the truthy spellings an env var actually gets set to, `=1` included
    #[arg(
        long,
        env = "ROLTER_ALLOW_OPEN_MODE",
        action = clap::ArgAction::SetTrue,
        value_parser = clap::builder::FalseyValueParser::new(),
    )]
    pub allow_open_mode: bool,
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
        // easy-up co-hosts both planes in one process on loopback, so the
        // snapshot channel never crosses a network boundary and there is
        // nothing to separate it from (see docs/architecture/security.md)
        internal_token: None,
    }
}

/// Build the control-plane args for `easy-up`.
// `..Default::default()` rather than an exhaustive literal: this crate's
// `postgres` feature turns on `rolter-control/postgres`, but not the other way
// round, so `cargo check -p rolter --features rolter-control/postgres` compiles
// this function against a wider `Args` than the cfgs below can see (#1295).
// clippy sees the exhaustive half of that pair and calls the base redundant
#[allow(clippy::needless_update)]
fn control_args(args: &EasyUpArgs, database_url: Option<String>) -> rolter_control::Args {
    // `database_url` only backs a field under the postgres feature
    #[cfg(not(feature = "postgres"))]
    let _ = &database_url;
    rolter_control::Args {
        host: args.host.clone(),
        port: args.control_port,
        ui_dir: args.ui_dir.clone(),
        // these args are built by hand rather than parsed, so clap's `env =`
        // never runs for them. read the same variables here or `easy-up` would
        // silently ignore a browser-tracing endpoint that works verbatim under
        // `rolter control` — and local dev is exactly where it gets set (#805)
        ui_otel_endpoint: std::env::var("ROLTER_UI_OTEL_ENDPOINT").ok(),
        ui_otel_service_name: std::env::var("ROLTER_UI_OTEL_SERVICE_NAME").ok(),
        gateway_url: format!("http://127.0.0.1:{}", args.gateway_port),
        config: Some(args.config.clone()),
        #[cfg(feature = "postgres")]
        database_url,
        redis_url: args.redis_url.clone(),
        clickhouse_url: args.clickhouse_url.clone(),
        admin_token: args.admin_token.clone(),
        internal_token: None,
        internal_addr: None,
        allow_open_mode: args.allow_open_mode,
        // same reason as the tracing endpoint above: clap's `env =` never runs
        // for hand-built args, so the pool settings are read here too (#1052)
        #[cfg(feature = "postgres")]
        db_max_connections: env_or("ROLTER_DB_MAX_CONNECTIONS", 10),
        #[cfg(feature = "postgres")]
        db_min_connections: env_or("ROLTER_DB_MIN_CONNECTIONS", 0),
        #[cfg(feature = "postgres")]
        db_acquire_timeout_secs: env_or("ROLTER_DB_ACQUIRE_TIMEOUT_SECS", 30),
        #[cfg(feature = "postgres")]
        db_idle_timeout_secs: env_or("ROLTER_DB_IDLE_TIMEOUT_SECS", 600),
        #[cfg(feature = "postgres")]
        db_max_lifetime_secs: env_or("ROLTER_DB_MAX_LIFETIME_SECS", 1800),
        // and the same for the failed-login throttle (#1079): hand-built args
        // skip clap's `env =`, so an operator who tuned the budget under
        // `rolter control` would silently get the defaults under `easy-up`
        login_throttle_disabled: env_flag("ROLTER_LOGIN_THROTTLE_DISABLED"),
        login_max_failures: env_or_num("ROLTER_LOGIN_MAX_FAILURES", 5),
        login_ip_max_failures: env_or_num("ROLTER_LOGIN_IP_MAX_FAILURES", 50),
        login_failure_window_secs: env_or_num("ROLTER_LOGIN_FAILURE_WINDOW_SECS", 900),
        login_lock_secs: env_or_num("ROLTER_LOGIN_LOCK_SECS", 60),
        login_max_lock_secs: env_or_num("ROLTER_LOGIN_MAX_LOCK_SECS", 900),
        login_max_delay_ms: env_or_num("ROLTER_LOGIN_MAX_DELAY_MS", 2000),
        login_trust_forwarded_for: env_flag("ROLTER_LOGIN_TRUST_FORWARDED_FOR"),
        ..Default::default()
    }
}

/// Same as [`env_or`], but available in every feature build: the throttle
/// settings are not postgres-gated the way the pool settings are.
fn env_or_num<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(default)
}

/// A boolean switch read the way clap reads one: present and truthy is on.
fn env_flag(key: &str) -> bool {
    std::env::var(key)
        .map(|raw| matches!(raw.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false)
}

/// Read a numeric setting from the environment, falling back to the same
/// default `rolter control` declares. An unparsable value falls back rather
/// than failing: `easy-up` is the zero-config entry point, and a typo in an
/// exported variable should not stop it from booting.
#[cfg(feature = "postgres")]
fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(default)
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

    /// Serializes every test that touches an environment key [`control_args`]
    /// reads.
    ///
    /// `set_var` is process-wide. Under `cargo nextest` each test owns its
    /// process, but the coverage job runs plain `cargo test`, where the whole
    /// binary is one process and tests are threads — so the value one test
    /// installs under a real key like `ROLTER_DB_MAX_CONNECTIONS` is visible to
    /// any sibling reading it at the same moment (#1418). Poison is ignored:
    /// a test that panicked holding this must not brick every later one.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

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
            host: "127.0.0.1".to_string(),
            gateway_port: 4000,
            control_port: 4001,
            ui_dir: PathBuf::from("ui/dist"),
            redis_url: None,
            admin_token: None,
            allow_open_mode: false,
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

    #[test]
    fn easy_up_binds_loopback_by_default() {
        // easy-up runs with no admin token, so its default bind is the only
        // thing keeping an unauthenticated management api off the network
        // (#970). clap owns the default, so assert it through the parser
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            easy_up: EasyUpArgs,
        }

        let cli = Cli::parse_from(["rolter"]);
        assert_eq!(cli.easy_up.host, "127.0.0.1");
        assert!(!cli.easy_up.allow_open_mode);
        // and the flag still works as a flag, with no value
        let cli = Cli::parse_from(["rolter", "--allow-open-mode"]);
        assert!(cli.easy_up.allow_open_mode);
    }

    #[test]
    fn the_open_mode_acknowledgement_reaches_the_control_plane() {
        // control_args reads the pool keys out of the environment, so this must
        // not run beside the test that installs one of them
        let _guard = env_lock();
        // easy-up builds control args by hand, so a flag that is parsed but not
        // forwarded would silently refuse to start on --host 0.0.0.0
        let mut args = EasyUpArgs {
            config: PathBuf::from("rolter.toml"),
            host: "0.0.0.0".to_string(),
            gateway_port: 4000,
            control_port: 4001,
            ui_dir: PathBuf::from("ui/dist"),
            redis_url: None,
            admin_token: None,
            allow_open_mode: true,
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
        assert!(control_args(&args, None).allow_open_mode);
        args.allow_open_mode = false;
        assert!(!control_args(&args, None).allow_open_mode);
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn env_or_falls_back_on_absent_and_unparsable_values() {
        let _guard = env_lock();
        // keys unique to this test so nothing else in the binary can race it
        let absent = "ROLTER_TEST_EASY_UP_ABSENT";
        let bad = "ROLTER_TEST_EASY_UP_BAD";
        let good = "ROLTER_TEST_EASY_UP_GOOD";
        std::env::remove_var(absent);
        std::env::set_var(bad, "not-a-number");
        std::env::set_var(good, "  42  ");

        assert_eq!(env_or(absent, 10u32), 10);
        // a typo must not stop the zero-config entry point from booting
        assert_eq!(env_or(bad, 10u32), 10);
        assert_eq!(env_or(good, 10u32), 42, "surrounding whitespace is trimmed");

        std::env::remove_var(bad);
        std::env::remove_var(good);
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn control_args_reads_the_pool_settings_from_the_environment() {
        let _guard = env_lock();
        // `easy-up` hand-builds Args, so clap's `env =` never runs for them and
        // an unwired field is silently ignored rather than failing to compile
        // once a default exists. this is the #805 failure mode, for #1052.
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            easy_up: EasyUpArgs,
        }

        let key = "ROLTER_DB_MAX_CONNECTIONS";
        std::env::set_var(key, "37");
        let cli = Cli::parse_from(["rolter"]);
        let built = control_args(&cli.easy_up, None);
        std::env::remove_var(key);

        assert_eq!(built.db_max_connections, 37);
        // the untouched ones still carry the same defaults `rolter control` declares
        assert_eq!(built.db_min_connections, 0);
        assert_eq!(built.db_acquire_timeout_secs, 30);
        assert_eq!(built.db_idle_timeout_secs, 600);
        assert_eq!(built.db_max_lifetime_secs, 1800);
    }
}
