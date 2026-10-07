//! `rolter kek` — verify and rotate the key-encryption key (#923).
//!
//! `rolter check` answers "is a KEK configured". This answers the question a
//! restore actually turns on: **is it the KEK that sealed this database**. The
//! two are unrelated, and only the second one fails when a `pg_dump` is
//! restored onto a deployment whose KEK did not travel with it.
//!
//! Both subcommands read their secrets from the environment rather than from
//! flags. A KEK on a command line lands in shell history, in `ps` output and in
//! whatever collects a container's args — which is a poor place for the one
//! value that decrypts every provider credential in the deployment.

use clap::{Args, Subcommand};

use rolter_store::postgres::crypto::Kek;
use rolter_store::postgres::kek_audit;

/// Environment variable holding the KEK the store is (or will be) sealed with.
const KEK_ENV: &str = "ROLTER_KEK";

/// Environment variable holding the KEK a rotation is moving *away* from.
const OLD_KEK_ENV: &str = "ROLTER_KEK_OLD";

#[derive(Args, Debug)]
pub struct KekArgs {
    #[command(subcommand)]
    command: KekCommand,

    /// postgres connection string for the control-plane store
    #[arg(long, env = "ROLTER_DATABASE_URL", hide_env_values = true)]
    database_url: String,
}

#[derive(Subcommand, Debug)]
enum KekCommand {
    /// check that ROLTER_KEK opens the secrets this store already holds
    Verify,
    /// reseal every stored secret from ROLTER_KEK_OLD to ROLTER_KEK
    Rotate(RotateArgs),
}

#[derive(Args, Debug)]
struct RotateArgs {
    /// actually write. Without it the rotation is a dry run that reports what
    /// it would reseal, because a rotation is not reversible without the old
    /// KEK and an operator should get to see the scope first
    #[arg(long)]
    apply: bool,
}

/// Seconds a pre-boot command waits for the database.
const CONNECT_TIMEOUT_SECS: u64 = 10;

fn kek_from(var: &str) -> anyhow::Result<Kek> {
    match std::env::var(var) {
        Ok(secret) if !secret.trim().is_empty() => Ok(Kek::from_secret(&secret)),
        _ => anyhow::bail!("{var} is unset or empty"),
    }
}

pub async fn run(args: KekArgs) -> anyhow::Result<()> {
    let timeout = std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS);
    match args.command {
        KekCommand::Verify => {
            let kek = kek_from(KEK_ENV)?;
            let audit = kek_audit::audit_kek_at(&args.database_url, &kek, timeout).await?;
            if audit.sampled() == 0 {
                println!(
                    "this store holds no sealed secrets, so nothing here confirms the KEK. \
                     A restore is only verified once it holds something."
                );
                return Ok(());
            }
            if audit.is_intact() {
                println!(
                    "{KEK_ENV} opens all {} sampled secrets across {} columns",
                    audit.sampled(),
                    audit.columns.len()
                );
                return Ok(());
            }
            for line in audit.damage_report() {
                println!("  {line}");
            }
            anyhow::bail!(
                "{KEK_ENV} is not the key this store was sealed with. The ciphertext cannot be \
                 recovered without the original KEK — restore it before starting the control \
                 plane."
            )
        }
        KekCommand::Rotate(rotate) => {
            let old = kek_from(OLD_KEK_ENV)?;
            let new = kek_from(KEK_ENV)?;
            if !rotate.apply {
                let audit = kek_audit::audit_kek_at(&args.database_url, &old, timeout).await?;
                if !audit.is_intact() {
                    for line in audit.damage_report() {
                        println!("  {line}");
                    }
                    anyhow::bail!(
                        "{OLD_KEK_ENV} does not open this store, so a rotation from it would \
                         abort. Nothing was changed."
                    );
                }
                if audit.sampled() == 0 {
                    println!("this store holds no sealed secrets; there is nothing to rotate");
                    return Ok(());
                }
                println!(
                    "dry run: {} sealed rows across {} columns would be resealed from \
                     {OLD_KEK_ENV} to {KEK_ENV}. Re-run with --apply.",
                    audit.sampled(),
                    audit.columns.len()
                );
                return Ok(());
            }
            let pool = rolter_store::postgres::connect(&args.database_url).await?;
            let rotation = kek_audit::rotate_kek(&pool, &old, &new).await;
            pool.close().await;
            let rotation = rotation?;
            println!("resealed {} secrets:", rotation.total());
            for (table, column, rows) in &rotation.resealed {
                println!("  {table}.{column}: {rows}");
            }
            println!(
                "every process in the fleet must now hold the new {KEK_ENV}; one still holding \
                 the old value can no longer read a stored credential."
            );
            Ok(())
        }
    }
}
