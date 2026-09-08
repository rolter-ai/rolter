//! `rolter mfa reset` — the break-glass path back into an account whose second
//! factor is gone (#1078).
//!
//! Every mandatory second factor needs one, or a lost phone is an unrecoverable
//! account, and for a superadmin that means an unrecoverable deployment. It is
//! deliberately **not** an API endpoint: an endpoint that clears a second
//! factor is a second factor an attacker with a session can clear. This runs on
//! the host, against the database directly, which is the same privilege level
//! as reading the rows it deletes.
//!
//! It is audited like any other change (`auth.mfa_break_glass_reset`, carrying
//! the operator's stated reason), and it revokes every live session for the
//! account as well: the reason to run it is that the account is in unknown
//! hands, so leaving a week-long session alive would leave the factor bypassed
//! anyway.

use clap::{Args, Subcommand};

use rolter_store::postgres::repo::UserRepo;

#[derive(Args, Debug)]
pub struct MfaArgs {
    #[command(subcommand)]
    command: MfaCommand,
}

#[derive(Subcommand, Debug)]
enum MfaCommand {
    /// clear an account's second factor and recovery codes, and revoke its
    /// sessions, so it can sign in with its password and enrol again
    Reset(ResetArgs),
}

#[derive(Args, Debug)]
struct ResetArgs {
    /// postgres connection string for the control-plane store
    #[arg(long, env = "ROLTER_DATABASE_URL")]
    database_url: String,
    /// the account to clear
    #[arg(long)]
    email: String,
    /// recorded verbatim on the audit entry. Required, not optional: a
    /// break-glass with no stated reason is indistinguishable from an attack
    /// when it is read back six months later
    #[arg(long)]
    reason: String,
}

pub async fn run(args: MfaArgs) -> anyhow::Result<()> {
    match args.command {
        MfaCommand::Reset(reset) => {
            let pool = rolter_store::postgres::connect(&reset.database_url).await?;
            let user = UserRepo(&pool)
                .find_by_email(reset.email.trim())
                .await?
                .ok_or_else(|| anyhow::anyhow!("no account with email {}", reset.email))?;
            let had_factor =
                rolter_control::mfa::break_glass_reset(&pool, user.id, &reset.reason).await?;
            pool.close().await;
            if had_factor {
                eprintln!(
                    "cleared the second factor for {} and revoked its sessions",
                    user.email
                );
            } else {
                // still a success: the sessions were revoked and the reset was
                // audited, and the operator's goal (this account can sign in
                // with its password) now holds either way
                eprintln!(
                    "{} had no second factor enrolled; revoked its sessions anyway",
                    user.email
                );
            }
            Ok(())
        }
    }
}
