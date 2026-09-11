//! rolter: unified command-line launcher.
//!
//! Dispatches to the data-plane gateway or the control plane so a single
//! `rolter` binary (and the `rolter` pypi wheel / crates.io crate) exposes the
//! whole system:
//!
//! ```text
//! rolter gateway --config rolter.toml
//! rolter control --database-url postgres://...
//! rolter easy-up            # gateway + control + UI in one supervised process
//! rolter init               # generate a production config and its secrets
//! rolter check              # pre-boot validation for a production deployment
//! rolter kek verify         # does ROLTER_KEK open what the store already holds
//! rolter config export      # the live config as an importable rolter.toml
//! ```
//!
//! The `gateway`/`control` subcommands reuse the exact argument set of the
//! standalone binaries via [`rolter_gateway::Args`] / [`rolter_control::Args`];
//! `easy-up` composes both for a zero-config one-command bring-up.

#[cfg(feature = "postgres")]
mod config;
mod easy_up;
mod init;
#[cfg(feature = "postgres")]
mod kek;
#[cfg(feature = "postgres")]
mod mfa;
mod preflight;
mod update_notice;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "rolter",
    version,
    about = "high-performance openai/anthropic-compatible llm gateway and load balancer"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// run the data-plane gateway (openai/anthropic-compatible proxy)
    Gateway(rolter_gateway::Args),
    /// run the control plane (management api + static ui host)
    Control(rolter_control::Args),
    /// bring up gateway + control + UI in one supervised process (zero-config
    /// with the built-in fake-llm model, or database-backed with --database-url)
    EasyUp(easy_up::EasyUpArgs),
    /// generate a production deployment's config and secrets, so an operator
    /// does not have to invent them (the counterpart to `check`)
    Init(init::InitArgs),
    /// validate a production deployment before starting it, so a
    /// misconfiguration fails loudly instead of starting degraded
    Check(preflight::CheckArgs),
    /// verify or rotate the key-encryption key against the control-plane store
    #[cfg(feature = "postgres")]
    Kek(kek::KekArgs),
    /// export the live configuration as an importable rolter.toml
    #[cfg(feature = "postgres")]
    Config(config::ConfigArgs),
    /// break-glass management of local accounts' second factor
    #[cfg(feature = "postgres")]
    Mfa(mfa::MfaArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = rolter_core::telemetry::init();
    let cli = Cli::parse();
    // beside the command, never ahead of it: a one-line stderr notice when a
    // newer release exists, cached for a day, silenced by
    // ROLTER_UPDATE_CHECK=false (#901). the command's exit status is its own
    update_notice::spawn();
    match cli.command {
        Command::Gateway(args) => rolter_gateway::run(args).await,
        Command::Control(args) => rolter_control::run(args).await,
        Command::EasyUp(args) => easy_up::run(args).await,
        Command::Init(args) => init::run(args).await,
        Command::Check(args) => preflight::run(args).await,
        #[cfg(feature = "postgres")]
        Command::Kek(args) => kek::run(args).await,
        #[cfg(feature = "postgres")]
        Command::Config(args) => config::run(args).await,
        #[cfg(feature = "postgres")]
        Command::Mfa(args) => mfa::run(args).await,
    }
}
