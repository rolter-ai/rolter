//! Periodic cleanup of what an abandoned flow leaves behind (#2414, #2616):
//! mostly rows, and the sealed token of an invitation that expired (#3397).
//!
//! A login abandoned at the identity provider, a consent never finished at an
//! MCP authorization server, or a second-factor prompt never answered leaves a
//! row that only its own flow would ever delete. Each table is swept here in
//! bounded batches; every read path checks expiry itself, so a row waiting for
//! the next pass is inert.

use std::future::Future;

use chrono::{Duration, Utc};
use rolter_core::Result;
use rolter_store::postgres::repo::{
    InvitationRepo, McpOAuthRepo, MfaRepo, SsoRepo, MCP_SWEEP_BATCH, SSO_SWEEP_BATCH,
};

/// How often the background sweep runs.
const SWEEP_SECS: u64 = 300;
/// Batches one pass deletes per table before yielding to the next tick, so a
/// vast backlog drains over several passes instead of in one long burst.
const MAX_BATCHES: usize = 10;

/// Spawn the sweep over every table whose rows are otherwise removed only
/// when the flow that wrote them comes back.
pub(crate) fn start(pool: sqlx::PgPool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(SWEEP_SECS));
        loop {
            interval.tick().await;
            sweep_once(&pool).await;
        }
    });
}

/// One pass over every table, each in bounded batches. Failures are logged and
/// swallowed: the next tick tries again.
pub(crate) async fn sweep_once(pool: &sqlx::PgPool) {
    let sso = SsoRepo(pool);
    let sso_ttl = crate::sso::LOGIN_STATE_TTL_SECS;
    let states = drain("sso login states", SSO_SWEEP_BATCH, || {
        sso.sweep_login_states(sso_ttl, SSO_SWEEP_BATCH)
    })
    .await;
    let codes = drain("sso exchange codes", SSO_SWEEP_BATCH, || {
        sso.sweep_exchange_codes(SSO_SWEEP_BATCH)
    })
    .await;
    let mcp_ttl = Duration::seconds(crate::mcp_oauth_flow::LOGIN_STATE_TTL_SECS);
    let mcp = McpOAuthRepo(pool);
    let mcp_states = drain("mcp oauth login states", MCP_SWEEP_BATCH, || {
        mcp.sweep_login_states(Utc::now() - mcp_ttl, MCP_SWEEP_BATCH)
    })
    .await;
    let mfa = MfaRepo(pool);
    let challenges = drain("mfa challenges", SSO_SWEEP_BATCH, || {
        mfa.sweep_expired_challenges(SSO_SWEEP_BATCH)
    })
    .await;
    let invitations = InvitationRepo(pool);
    let sealed = drain("invitation sealed tokens", SSO_SWEEP_BATCH, || {
        invitations.sweep_expired_sealed_tokens(SSO_SWEEP_BATCH)
    })
    .await;
    if states + codes + mcp_states + challenges + sealed > 0 {
        tracing::debug!(
            states,
            codes,
            mcp_states,
            challenges,
            sealed,
            "swept expired flow state"
        );
    }
}

/// Run `batch` until a short batch says the table is drained or the per-pass
/// cap is reached, and return how many rows went.
async fn drain<F, Fut>(table: &str, size: i64, mut batch: F) -> u64
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<u64>>,
{
    let mut total = 0;
    for _ in 0..MAX_BATCHES {
        match batch().await {
            Ok(n) => {
                total += n;
                if n < size as u64 {
                    break;
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, table, "could not sweep expired rows");
                break;
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn drain_stops_on_a_short_batch() {
        let calls = AtomicUsize::new(0);
        let total = drain("t", 10, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(if n == 0 { 10 } else { 3 }) }
        })
        .await;
        assert_eq!(total, 13);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn drain_is_capped_per_pass() {
        let calls = AtomicUsize::new(0);
        let total = drain("t", 10, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(10) }
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), MAX_BATCHES);
        assert_eq!(total, 10 * MAX_BATCHES as u64);
    }

    #[tokio::test]
    async fn drain_gives_up_on_an_error_without_losing_the_count() {
        let calls = AtomicUsize::new(0);
        let total = drain("t", 10, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Ok(10)
                } else {
                    Err(rolter_core::Error::Store("db down".into()))
                }
            }
        })
        .await;
        assert_eq!(total, 10);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
