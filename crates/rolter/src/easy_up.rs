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
    /// host the gateway binds to, and the control plane too unless
    /// `--control-host` is set. Loopback by default: `easy-up` is the
    /// zero-credential local path, and the control plane it starts is
    /// unauthenticated unless `--admin-token` is set, which must not reach a
    /// public interface by omission (#970). Pass `--host 0.0.0.0` with an
    /// admin token to serve a network.
    ///
    /// Read from `ROLTER_HOST` when the flag is absent, the same variable the
    /// gateway binds from. The published image's default command passes
    /// `--host 0.0.0.0`, because a container's loopback is unreachable through
    /// a published port (#1891); a non-loopback host with no admin token still
    /// refuses to start unless `ROLTER_ALLOW_OPEN_MODE` acknowledges it
    #[arg(long, env = "ROLTER_HOST", default_value = "127.0.0.1")]
    pub host: String,
    /// host the control plane binds to, when it should differ from `--host`.
    ///
    /// Read from `ROLTER_CONTROL_HOST`, the variable `rolter control` binds
    /// from, so an env file that keeps the management plane on loopback while
    /// the gateway serves a network (`rolter init --profile production` writes
    /// one) means the same thing under `easy-up`. Unset or blank falls back to
    /// `--host`
    #[arg(long, env = "ROLTER_CONTROL_HOST")]
    pub control_host: Option<String>,
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

impl EasyUpArgs {
    /// The host the control plane binds: `--control-host` when it is set and
    /// not blank, `--host` otherwise.
    fn control_host(&self) -> &str {
        self.control_host
            .as_deref()
            .filter(|host| !host.trim().is_empty())
            .unwrap_or(&self.host)
    }
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
        // nothing to separate it from (see docs/dev-docs/architecture/security.md)
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
        host: args.control_host().to_string(),
        port: args.control_port,
        ui_dir: args.ui_dir.clone(),
        // these args are built by hand rather than parsed, so clap's `env =`
        // never runs for them. read the same variables here or `easy-up` would
        // silently ignore a browser-tracing endpoint that works verbatim under
        // `rolter control` — and local dev is exactly where it gets set (#805)
        ui_otel_endpoint: std::env::var("ROLTER_UI_OTEL_ENDPOINT").ok(),
        ui_otel_service_name: std::env::var("ROLTER_UI_OTEL_SERVICE_NAME").ok(),
        ui_docs_base_url: std::env::var("ROLTER_UI_DOCS_BASE_URL").ok(),
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

/// Same as `env_or`, but available in every feature build: the throttle
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

/// Refuse the one exposure the control plane would refuse anyway: the
/// management API on a host that is not loopback, with no admin token and no
/// acknowledgement (#970).
///
/// The control plane enforces this itself for its own listener. Checking it
/// here as well stops `easy-up` before it writes a config, seeds a database or
/// prints a `try it` command that cannot work, and lets the refusal name the
/// container remedy. The published image binds `0.0.0.0` (#1891), and there
/// "bind loopback", the control plane's own advice, is what makes the
/// container unreachable.
///
/// In database mode the gateway serves the same API at `/admin/*` by proxying
/// to the co-hosted control plane, so the gateway's host counts too: a control
/// plane on loopback behind a gateway on `0.0.0.0` passes the control plane's
/// own check and is still reachable from the network.
fn refuse_unacknowledged_open_mode(args: &EasyUpArgs, db_mode: bool) -> anyhow::Result<()> {
    let token_set = args
        .admin_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty());
    if token_set || args.allow_open_mode {
        return Ok(());
    }
    let mut surfaces = vec![("the management API", args.control_host(), args.control_port)];
    if db_mode {
        surfaces.push((
            "the gateway's /admin/* proxy to the management API",
            args.host.as_str(),
            args.gateway_port,
        ));
    }
    // an ip literal, bracketed or not; anything else is left for the
    // listeners to reject with their own error, since they bind only literals
    let exposed = surfaces.into_iter().find(|(_, host, _)| {
        host.strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .unwrap_or(host)
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| !ip.is_loopback())
    });
    let Some((surface, host, port)) = exposed else {
        return Ok(());
    };
    anyhow::bail!(
        "refusing to start: easy-up is set to serve {surface} on {host}:{port}, which is not a \
         loopback address, and no ROLTER_ADMIN_TOKEN is set, so it would accept every request \
         from off this machine as superadmin. Set ROLTER_ADMIN_TOKEN to close it. For a \
         local-only container, acknowledge the open control plane with \
         `-e ROLTER_ALLOW_OPEN_MODE=1` and publish the ports on loopback only \
         (`-p 127.0.0.1:{gateway}:{gateway} -p 127.0.0.1:{control}:{control}`). Outside a \
         container, keep both planes on loopback: unset ROLTER_HOST and ROLTER_CONTROL_HOST, \
         or pass --host 127.0.0.1 --control-host 127.0.0.1",
        gateway = args.gateway_port,
        control = args.control_port,
    )
}

/// Run `easy-up` to completion: bootstrap config, optionally migrate+seed the
/// database, print a startup summary, then supervise the control plane and
/// gateway together.
pub async fn run(args: EasyUpArgs) -> anyhow::Result<()> {
    #[cfg(feature = "postgres")]
    let database_url: Option<String> = args.database_url.clone();
    #[cfg(not(feature = "postgres"))]
    let database_url: Option<String> = None;

    refuse_unacknowledged_open_mode(&args, database_url.is_some())?;
    let created = ensure_config(&args.config)?;
    if created {
        tracing::info!(config = %args.config.display(), "created config from bundled example");
    }

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
    // read the config back rather than assuming the bundled example's key: the
    // printed command has to work against whatever is actually on disk (#1615)
    let loaded = rolter_core::GatewayConfig::load(&args.config).ok();
    print_summary(
        &args,
        db_mode,
        admin_note.as_deref(),
        &hint_auth(loaded.as_ref(), db_mode),
    );

    let control = rolter_control::run(control_args(&args, database_url));
    let gateway = rolter_gateway::run(gateway_args(&args, db_mode));

    // supervise both in one process; whichever exits (error or shutdown signal)
    // brings the command down
    tokio::try_join!(control, gateway)?;
    Ok(())
}

/// The public model name the printed `try it` command asks for. Built in, so
/// it answers with no provider key and no database.
const HINT_MODEL: &str = "fake-llm";

/// What the printed command should present as its credential.
#[derive(Debug, PartialEq, Eq)]
enum HintAuth {
    /// the gateway's key set is empty and nothing requires one, so the command
    /// succeeds bare
    Open,
    /// a virtual key read back out of the config `easy-up` just wrote, so an
    /// edited config still prints a command that works
    Key(String),
    /// database mode: keys live in the store and none is seeded, so no literal
    /// can be printed that would authenticate
    Minted,
}

/// Pick the credential the `try it` command should carry.
///
/// Read back from the config rather than hardcoded: the bundled example ships
/// `sk-rolter-dev`, but an operator who edited the file, renamed the key or
/// deleted the section must still be handed a command that works (#1615).
///
/// In database mode the gateway authenticates against the store, the seed
/// mints no virtual key, and `managed_auth` makes an empty key set a locked
/// door rather than an open one — so there is nothing truthful to print but a
/// placeholder and the instruction to mint one.
fn hint_auth(config: Option<&rolter_core::GatewayConfig>, db_mode: bool) -> HintAuth {
    if db_mode {
        return HintAuth::Minted;
    }
    let Some(config) = config else {
        // the config did not parse; the gateway is about to say so much more
        // loudly than this line can, and a placeholder beats a wrong key
        return HintAuth::Minted;
    };
    let now = chrono::Utc::now();
    let usable = config.virtual_keys.iter().find(|key| {
        key.is_active(now)
            && !key.key.trim().is_empty()
            && (key.models.is_empty() || key.models.iter().any(|model| model == HINT_MODEL))
    });
    match usable {
        Some(key) => HintAuth::Key(key.key.clone()),
        // every key is revoked, expired or scoped away from the builtin: the
        // gateway still requires one, so a bare command would 401 just as the
        // old hint did
        None if !config.virtual_keys.is_empty() => HintAuth::Minted,
        None => HintAuth::Open,
    }
}

/// The `try it` block: an optional note, then a command that succeeds exactly
/// as printed wherever one can be.
fn try_it_lines(host: &str, gateway_port: u16, auth: &HintAuth) -> Vec<String> {
    let auth_header = match auth {
        HintAuth::Open => String::new(),
        HintAuth::Key(key) => format!(" -H 'Authorization: Bearer {key}'"),
        HintAuth::Minted => " -H \"Authorization: Bearer $ROLTER_API_KEY\"".to_string(),
    };
    let mut lines = Vec::new();
    if matches!(auth, HintAuth::Minted) {
        lines.push(
            "  try it (mint a virtual key on the dashboard's Virtual Keys screen, then \
             export it as ROLTER_API_KEY):"
                .to_string(),
        );
    } else {
        lines.push("  try it:".to_string());
    }
    lines.push(format!(
        "  curl http://{host}:{gateway_port}/v1/chat/completions{auth_header} \
-H 'Content-Type: application/json' \
-d '{{\"model\":\"{HINT_MODEL}\",\"messages\":[{{\"role\":\"user\",\"content\":\"hello\"}}]}}'"
    ));
    lines
}

/// The host to print in a url: a wildcard bind is reached as `localhost`.
fn display_host(host: &str) -> &str {
    match host {
        "0.0.0.0" | "::" => "localhost",
        host => host,
    }
}

fn print_summary(
    args: &EasyUpArgs,
    db_mode: bool,
    admin_created_email: Option<&str>,
    auth: &HintAuth,
) {
    let mode = if db_mode { "database" } else { "file" };
    let gateway_host = display_host(&args.host);
    eprintln!("\nrolter easy-up — {mode} mode");
    eprintln!(
        "  gateway (OpenAI/Anthropic):  http://{}:{}",
        gateway_host, args.gateway_port
    );
    eprintln!(
        "  control + UI:                http://{}:{}",
        display_host(args.control_host()),
        args.control_port
    );
    eprintln!("  config:                      {}", args.config.display());
    if !db_mode {
        eprintln!("  built-in model:              fake-llm (no keys, no database)");
    }
    if let Some(email) = admin_created_email {
        eprintln!("  admin user created:          {email}");
    }
    eprintln!();
    for line in try_it_lines(gateway_host, args.gateway_port, auth) {
        eprintln!("{line}");
    }
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
    fn the_bundled_example_config_is_lint_clean() {
        // the byte-equality test above plus rolter-core's
        // `the_shipped_example_config_is_clean` only prove this transitively,
        // and the copy exists precisely so it *may* diverge one day. lint the
        // bytes `easy-up` actually writes, so relaxing the equality check
        // cannot silently drop coverage on the file operators start from
        let findings =
            rolter_core::config_lint::unknown_keys(EXAMPLE_CONFIG).expect("bundled example parses");
        assert!(
            findings.is_empty(),
            "crates/rolter/rolter.example.toml has unrecognised keys: {:?}",
            findings
                .iter()
                .map(rolter_core::config_lint::describe)
                .collect::<Vec<_>>()
        );
    }

    /// The `try it` command as `easy-up` prints it for a given config.
    fn printed_command(toml: &str, db_mode: bool) -> String {
        let config = rolter_core::GatewayConfig::from_toml_str(toml).expect("config parses");
        let auth = hint_auth(Some(&config), db_mode);
        try_it_lines("127.0.0.1", 4000, &auth).join("\n")
    }

    #[test]
    fn the_printed_command_carries_the_key_from_the_bundled_config() {
        // this is the first command a new user runs, against the very file
        // easy-up just wrote; without the key it answers 401 (#1615)
        let command = printed_command(EXAMPLE_CONFIG, false);
        assert!(
            command.contains("-H 'Authorization: Bearer sk-rolter-dev'"),
            "the bundled example ships a virtual key, so the hint must present it: {command}"
        );
        assert!(command.contains("\"model\":\"fake-llm\""));
    }

    #[test]
    fn the_printed_command_follows_an_edited_key_rather_than_hardcoding_one() {
        let command = printed_command(
            r#"
[[virtual_keys]]
key = "sk-rolter-my-own"
name = "mine"
"#,
            false,
        );
        assert!(command.contains("Bearer sk-rolter-my-own"), "{command}");
        assert!(!command.contains("sk-rolter-dev"), "{command}");
    }

    #[test]
    fn a_config_with_no_keys_prints_a_bare_command() {
        // deleting the section is the documented way to run open locally, and
        // then an auth header is noise rather than help
        let command = printed_command("", false);
        assert!(!command.contains("Authorization"), "{command}");
        assert_eq!(
            hint_auth(None::<&rolter_core::GatewayConfig>, false),
            HintAuth::Minted
        );
    }

    #[test]
    fn a_key_that_cannot_reach_the_builtin_model_is_not_offered() {
        // a disabled, expired or scoped-away key would 401 exactly like no key
        for toml in [
            r#"
[[virtual_keys]]
key = "sk-rolter-dead"
disabled = true
"#,
            r#"
[[virtual_keys]]
key = "sk-rolter-old"
expires_at = "2020-01-01T00:00:00Z"
"#,
            r#"
[[virtual_keys]]
key = "sk-rolter-scoped"
models = ["gpt-4o"]
"#,
        ] {
            let command = printed_command(toml, false);
            assert!(
                command.contains("$ROLTER_API_KEY"),
                "a key that cannot answer for fake-llm must not be printed as if it could: \
                 {command}"
            );
            assert!(command.contains("Virtual Keys screen"), "{command}");
        }
        // a key scoped *to* the builtin is offered
        let command = printed_command(
            r#"
[[virtual_keys]]
key = "sk-rolter-fake"
models = ["fake-llm"]
"#,
            false,
        );
        assert!(command.contains("Bearer sk-rolter-fake"), "{command}");
    }

    #[test]
    fn database_mode_asks_for_a_minted_key_instead_of_a_literal() {
        // the seed mints no virtual key and a managed gateway treats an empty
        // key set as locked, so no literal would authenticate here
        let command = printed_command(EXAMPLE_CONFIG, true);
        assert!(!command.contains("sk-rolter-dev"), "{command}");
        assert!(command.contains("$ROLTER_API_KEY"), "{command}");
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
        let args = args_on("127.0.0.1");
        assert!(gateway_args(&args, false).snapshot_url.is_none());
        assert_eq!(
            gateway_args(&args, true).snapshot_url.as_deref(),
            Some("http://127.0.0.1:4001/internal/snapshot")
        );
    }

    /// Sets or clears one environment variable for the life of the guard and
    /// puts the previous value back on drop, panic included. Hold
    /// [`env_lock`] around it: the variable is process-wide.
    struct EnvVar {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVar {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let previous = std::env::var_os(key);
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVar {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[derive(clap::Parser)]
    struct Cli {
        #[command(flatten)]
        easy_up: EasyUpArgs,
    }

    /// Args as `easy-up` would parse them with no environment, bound to `host`.
    fn args_on(host: &str) -> EasyUpArgs {
        EasyUpArgs {
            config: PathBuf::from("rolter.toml"),
            host: host.to_string(),
            control_host: None,
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
        }
    }

    #[test]
    fn easy_up_binds_loopback_by_default() {
        // easy-up runs with no admin token, so its default bind is the only
        // thing keeping an unauthenticated management api off the network
        // (#970). clap owns the default, so assert it through the parser —
        // with the variables it falls back to cleared, since a developer who
        // sourced .env.example has ROLTER_HOST=0.0.0.0 exported
        use clap::Parser;
        let _guard = env_lock();
        let _host = EnvVar::set("ROLTER_HOST", None);
        let _control_host = EnvVar::set("ROLTER_CONTROL_HOST", None);
        let _open = EnvVar::set("ROLTER_ALLOW_OPEN_MODE", None);

        let cli = Cli::parse_from(["rolter"]);
        assert_eq!(cli.easy_up.host, "127.0.0.1");
        assert_eq!(control_args(&cli.easy_up, None).host, "127.0.0.1");
        assert!(!cli.easy_up.allow_open_mode);
        // and the flag still works as a flag, with no value
        let cli = Cli::parse_from(["rolter", "--allow-open-mode"]);
        assert!(cli.easy_up.allow_open_mode);
    }

    #[test]
    fn the_host_falls_back_to_rolter_host_and_the_flag_wins_over_it() {
        // outside the image, easy-up binds where the gateway would: the host
        // an env file exports as ROLTER_HOST. an explicit --host, which is how
        // the image's default command binds every interface (#1891), wins
        use clap::Parser;
        let _guard = env_lock();
        let _host = EnvVar::set("ROLTER_HOST", Some("0.0.0.0"));
        let _control_host = EnvVar::set("ROLTER_CONTROL_HOST", None);

        let cli = Cli::parse_from(["rolter"]);
        assert_eq!(cli.easy_up.host, "0.0.0.0");
        assert_eq!(control_args(&cli.easy_up, None).host, "0.0.0.0");
        let cli = Cli::parse_from(["rolter", "--host", "127.0.0.1"]);
        assert_eq!(cli.easy_up.host, "127.0.0.1");
    }

    #[test]
    fn the_control_plane_binds_rolter_control_host_when_one_is_set() {
        // `rolter init --profile production` writes ROLTER_HOST=0.0.0.0 beside
        // ROLTER_CONTROL_HOST=127.0.0.1: the gateway serves the network and
        // the management plane stays on loopback. easy-up must not widen the
        // second to match the first
        use clap::Parser;
        let _guard = env_lock();
        let _host = EnvVar::set("ROLTER_HOST", Some("0.0.0.0"));
        let _control_host = EnvVar::set("ROLTER_CONTROL_HOST", Some("127.0.0.1"));

        let cli = Cli::parse_from(["rolter"]);
        assert_eq!(
            gateway_args(&cli.easy_up, false).host.as_deref(),
            Some("0.0.0.0")
        );
        assert_eq!(control_args(&cli.easy_up, None).host, "127.0.0.1");
        // the flag still wins over the variable
        let cli = Cli::parse_from(["rolter", "--control-host", "0.0.0.0"]);
        assert_eq!(control_args(&cli.easy_up, None).host, "0.0.0.0");

        // a blank value is unset, as it is for the admin token
        let _control_host = EnvVar::set("ROLTER_CONTROL_HOST", Some(""));
        let cli = Cli::parse_from(["rolter"]);
        assert_eq!(control_args(&cli.easy_up, None).host, "0.0.0.0");
        let mut args = args_on("0.0.0.0");
        args.control_host = Some("  ".to_string());
        assert_eq!(args.control_host(), "0.0.0.0");
    }

    #[test]
    fn an_open_control_plane_off_loopback_is_refused_before_anything_starts() {
        // the image's default command binds 0.0.0.0, so this is what a bare
        // `docker run -p ...` meets: it must still refuse rather than serve a
        // superadmin api to the network (#970), and say how to proceed
        let err = refuse_unacknowledged_open_mode(&args_on("0.0.0.0"), false)
            .expect_err("no token and no acknowledgement on 0.0.0.0");
        let message = err.to_string();
        assert!(message.contains("ROLTER_ADMIN_TOKEN"), "{message}");
        assert!(message.contains("ROLTER_ALLOW_OPEN_MODE=1"), "{message}");
        assert!(message.contains("-p 127.0.0.1:4001:4001"), "{message}");

        for host in ["10.0.0.5", "::", "[::]"] {
            assert!(
                refuse_unacknowledged_open_mode(&args_on(host), false).is_err(),
                "{host}"
            );
        }
        // a blank token is no token, exactly as the control plane reads it
        let mut blank = args_on("0.0.0.0");
        blank.admin_token = Some("  ".to_string());
        assert!(refuse_unacknowledged_open_mode(&blank, false).is_err());

        // the control host decides, not the gateway's: a control plane
        // widened on its own is refused behind a loopback gateway
        let mut widened = args_on("127.0.0.1");
        widened.control_host = Some("0.0.0.0".to_string());
        assert!(refuse_unacknowledged_open_mode(&widened, false).is_err());
    }

    #[test]
    fn database_mode_counts_the_gateways_admin_proxy_as_the_management_api() {
        // in database mode the gateway serves /admin/* by proxying to the
        // co-hosted control plane, so a control plane on loopback behind a
        // gateway on 0.0.0.0 is the open api on the network all the same
        let mut split = args_on("0.0.0.0");
        split.control_host = Some("127.0.0.1".to_string());
        let err = refuse_unacknowledged_open_mode(&split, true)
            .expect_err("the admin proxy reaches the open control plane");
        let message = err.to_string();
        assert!(message.contains("/admin/*"), "{message}");
        assert!(message.contains("0.0.0.0:4000"), "{message}");

        // in file mode the gateway proxies nothing, so the same split is fine
        assert!(refuse_unacknowledged_open_mode(&split, false).is_ok());
        split.admin_token = Some("secret".to_string());
        assert!(refuse_unacknowledged_open_mode(&split, true).is_ok());
    }

    #[test]
    fn a_token_an_acknowledgement_or_loopback_lets_easy_up_start() {
        for host in ["127.0.0.1", "::1"] {
            for db_mode in [false, true] {
                assert!(
                    refuse_unacknowledged_open_mode(&args_on(host), db_mode).is_ok(),
                    "{host} db_mode={db_mode}"
                );
            }
        }
        let mut closed = args_on("0.0.0.0");
        closed.admin_token = Some("secret".to_string());
        assert!(refuse_unacknowledged_open_mode(&closed, true).is_ok());

        let mut acknowledged = args_on("0.0.0.0");
        acknowledged.allow_open_mode = true;
        assert!(refuse_unacknowledged_open_mode(&acknowledged, true).is_ok());
    }

    #[test]
    fn the_open_mode_acknowledgement_reaches_the_control_plane() {
        // control_args reads the pool keys out of the environment, so this must
        // not run beside the test that installs one of them
        let _guard = env_lock();
        // easy-up builds control args by hand, so a flag that is parsed but not
        // forwarded would silently refuse to start on --host 0.0.0.0
        let mut args = args_on("0.0.0.0");
        args.allow_open_mode = true;
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
