//! rolter-control binary: thin wrapper over [`rolter_control::run`].

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rolter_core::telemetry::init();
    rolter_control::run(rolter_control::Args::parse()).await
}
