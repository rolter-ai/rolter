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
//! ```
//!
//! The `gateway`/`control` subcommands reuse the exact argument set of the
//! standalone binaries via [`rolter_gateway::Args`] / [`rolter_control::Args`];
//! `easy-up` composes both for a zero-config one-command bring-up.

mod easy_up;

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = rolter_core::telemetry::init();
    match Cli::parse().command {
        Command::Gateway(args) => rolter_gateway::run(args).await,
        Command::Control(args) => rolter_control::run(args).await,
        Command::EasyUp(args) => easy_up::run(args).await,
    }
}
