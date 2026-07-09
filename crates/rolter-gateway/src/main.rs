//! rolter-gateway binary: thin wrapper over [`rolter_gateway::run`].

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rolter_core::telemetry::init();
    rolter_gateway::run(rolter_gateway::Args::parse()).await
}
