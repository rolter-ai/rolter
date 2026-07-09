//! rolter: unified command-line launcher.
//!
//! Dispatches to the data-plane gateway or the control plane so a single
//! `rolter` binary (and the `rolter` pypi wheel / crates.io crate) exposes the
//! whole system:
//!
//! ```text
//! rolter gateway --config rolter.toml
//! rolter control --database-url postgres://...
//! ```
//!
//! Each subcommand reuses the exact argument set of the standalone binary via
//! [`rolter_gateway::Args`] / [`rolter_control::Args`].

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rolter_core::telemetry::init();
    match Cli::parse().command {
        Command::Gateway(args) => rolter_gateway::run(args).await,
        Command::Control(args) => rolter_control::run(args).await,
    }
}
