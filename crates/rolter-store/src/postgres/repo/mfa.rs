//! TOTP second factor, recovery codes, and the short-lived challenges the
//! login exchange hands out (#1078, `migrations/0067_totp_second_factor.sql`).
//!
//! The shared secret is sealed with the deployment KEK
//! ([`super::super::crypto::Kek`]) and is never returned by any read this
//! module exposes to the API layer: [`MfaRepo::status`] describes the factor
//! without it, and [`MfaRepo::open_secret`] -- the one path that unseals -- is
//! only reachable from verification. There is deliberately no model type that
//! serialises the secret at all.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{Error, Result};

use super::super::crypto::Kek;
use super::super::models::{MfaChallenge, TotpFactorStatus};
use super::support::store_err;

pub struct MfaRepo<'a>(pub &'a PgPool);

/// The factor row as it sits on disk. Private to this module and never
/// `Serialize`: the sealed secret has no route out of here except through
/// [`MfaRepo::open_secret`], which hands back an [`OpenFactor`] instead.
#[derive(sqlx::FromRow)]
struct SealedFactor {
    secret_ciphertext: Vec<u8>,
    secret_nonce: Vec<u8>,
    confirmed_at: Option<DateTime<Utc>>,
    last_used_step: Option<i64>,
}

/// The KEK seals a string, and a TOTP secret is bytes, so it is hex-encoded on
/// the way in. Hex rather than base32: this is the storage encoding, invisible
/// to users, and it round-trips arbitrary bytes with no padding question.
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    for &b in bytes {
        out.push(HEX_CHARS[(b >> 4) as usize] as char);
        out.push(HEX_CHARS[(b & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// An unsealed factor, as verification needs it. Not `Serialize`, and not a
/// row type: it exists only between the unseal and the code comparison.
pub struct OpenFactor {
    /// the raw shared secret, already base32-decoded
    pub secret: Vec<u8>,
    pub confirmed: bool,
    /// the highest step this factor has accepted, if any
    pub last_used_step: Option<i64>,
}

impl MfaRepo<'_> {
    /// Start (or restart) enrolment: seal a fresh secret as the user's
    /// unconfirmed factor.
    ///
    /// Restarting replaces the secret outright and clears `confirmed_at`,
    /// which means a user who begins a second enrolment while one factor is
    /// already armed **loses the armed factor**. That is the safe direction:
    /// the alternative is two live secrets, only one of which the user can
    /// still generate codes from. Callers that must not disarm an existing
    /// factor check [`Self::status`] first.
    pub async fn begin_enrolment(&self, user_id: Uuid, secret: &[u8], kek: &Kek) -> Result<()> {
        let (ciphertext, nonce) = kek.encrypt(&hex_encode(secret))?;
        sqlx::query(
            "insert into user_totp_factors (user_id, secret_ciphertext, secret_nonce)
             values ($1, $2, $3)
             on conflict (user_id) do update
                 set secret_ciphertext = excluded.secret_ciphertext,
                     secret_nonce = excluded.secret_nonce,
                     confirmed_at = null,
                     last_used_step = null,
                     updated_at = now()",
        )
        .bind(user_id)
        .bind(ciphertext)
        .bind(nonce)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(())
    }

    /// Unseal the factor for verification. `None` when the user has none.
    pub async fn open_secret(&self, user_id: Uuid, kek: &Kek) -> Result<Option<OpenFactor>> {
        let row: Option<SealedFactor> = sqlx::query_as(
            "select secret_ciphertext, secret_nonce, confirmed_at, last_used_step
             from user_totp_factors where user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let hex = kek.decrypt(&row.secret_ciphertext, &row.secret_nonce)?;
        let secret = hex_decode(&hex)
            .ok_or_else(|| Error::Config("stored totp secret is not valid hex".into()))?;
        Ok(Some(OpenFactor {
            secret,
            confirmed: row.confirmed_at.is_some(),
            last_used_step: row.last_used_step,
        }))
    }

    /// The factor's public shape, or `None` when the user has none.
    pub async fn status(&self, user_id: Uuid) -> Result<Option<TotpFactorStatus>> {
        sqlx::query_as(
            "select user_id, confirmed_at, created_at, updated_at
             from user_totp_factors where user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Whether the user has an **armed** (confirmed) factor. This, not the
    /// existence of a row, is what login enforcement branches on.
    pub async fn has_armed_factor(&self, user_id: Uuid) -> Result<bool> {
        let found: Option<(Uuid,)> = sqlx::query_as(
            "select user_id from user_totp_factors
             where user_id = $1 and confirmed_at is not null",
        )
        .bind(user_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        Ok(found.is_some())
    }

    /// Arm the factor, recording the step the confirming code came from.
    ///
    /// Returns `false` when there is no factor to confirm. Confirming an
    /// already-confirmed factor is allowed and simply re-stamps it -- a
    /// duplicate submit of the enrolment form must not be an error.
    pub async fn confirm(&self, user_id: Uuid, step: i64) -> Result<bool> {
        let result = sqlx::query(
            "update user_totp_factors
             set confirmed_at = coalesce(confirmed_at, now()),
                 last_used_step = $2,
                 updated_at = now()
             where user_id = $1",
        )
        .bind(user_id)
        .bind(step)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(result.rows_affected() > 0)
    }

    /// Spend a TOTP step, refusing one that is not strictly newer than the
    /// last accepted.
    ///
    /// The comparison is in the `where` clause rather than in Rust on purpose:
    /// two logins racing with the same stolen code would both read the same
    /// `last_used_step` and both decide it was fresh. As a conditional update
    /// exactly one of them affects a row, so exactly one of them proceeds.
    pub async fn spend_step(&self, user_id: Uuid, step: i64) -> Result<bool> {
        let result = sqlx::query(
            "update user_totp_factors set last_used_step = $2, updated_at = now()
             where user_id = $1 and (last_used_step is null or last_used_step < $2)",
        )
        .bind(user_id)
        .bind(step)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(result.rows_affected() > 0)
    }

    /// Remove the factor and every recovery code with it.
    ///
    /// Codes go too because they exist only to get past this factor; leaving
    /// them behind would arm a second enrolment with a stranger's old codes.
    /// Returns `false` when there was nothing to remove.
    pub async fn disable(&self, user_id: Uuid) -> Result<bool> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query("delete from user_recovery_codes where user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        let result = sqlx::query("delete from user_totp_factors where user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(result.rows_affected() > 0)
    }

    /// Replace the user's whole recovery-code batch.
    ///
    /// Replacing rather than appending is what makes "you have N codes left" a
    /// single answerable number, and it means a user who suspects a printed
    /// sheet is compromised invalidates it by regenerating.
    pub async fn replace_recovery_codes(&self, user_id: Uuid, hashes: &[String]) -> Result<()> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query("delete from user_recovery_codes where user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        for hash in hashes {
            sqlx::query("insert into user_recovery_codes (user_id, code_hash) values ($1, $2)")
                .bind(user_id)
                .bind(hash)
                .execute(&mut *tx)
                .await
                .map_err(store_err)?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok(())
    }

    /// Spend a recovery code, returning whether it was live.
    ///
    /// Single use is enforced by `used_at is null` in the `where` clause, so
    /// two requests presenting the same code cannot both succeed.
    pub async fn consume_recovery_code(&self, user_id: Uuid, code_hash: &str) -> Result<bool> {
        let result = sqlx::query(
            "update user_recovery_codes set used_at = now()
             where user_id = $1 and code_hash = $2 and used_at is null",
        )
        .bind(user_id)
        .bind(code_hash)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(result.rows_affected() > 0)
    }

    /// How many of the user's recovery codes are still unspent.
    pub async fn remaining_recovery_codes(&self, user_id: Uuid) -> Result<i64> {
        let (count,): (i64,) = sqlx::query_as(
            "select count(*) from user_recovery_codes where user_id = $1 and used_at is null",
        )
        .bind(user_id)
        .fetch_one(self.0)
        .await
        .map_err(store_err)?;
        Ok(count)
    }

    /// Issue a challenge for a half-completed login.
    pub async fn create_challenge(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<MfaChallenge> {
        sqlx::query_as(
            "insert into mfa_challenges (user_id, token_hash, expires_at)
             values ($1, $2, $3)
             returning id, user_id, attempts, expires_at",
        )
        .bind(user_id)
        .bind(token_hash)
        .bind(expires_at)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Charge one attempt against a live challenge, returning it when the
    /// budget held.
    ///
    /// The charge happens before the code is checked, and in the same
    /// statement that reads the challenge, so a caller cannot spend more than
    /// `max_attempts` guesses however many requests it makes in parallel. A
    /// challenge whose budget is exhausted is left in place until it expires
    /// rather than deleted, so a further guess still costs a round trip and
    /// learns nothing new.
    pub async fn charge_challenge_attempt(
        &self,
        token_hash: &str,
        max_attempts: i32,
    ) -> Result<Option<MfaChallenge>> {
        sqlx::query_as(
            "update mfa_challenges set attempts = attempts + 1
             where token_hash = $1 and expires_at > now() and attempts < $2
             returning id, user_id, attempts, expires_at",
        )
        .bind(token_hash)
        .bind(max_attempts)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Delete a challenge once it has been redeemed.
    pub async fn delete_challenge(&self, token_hash: &str) -> Result<()> {
        sqlx::query("delete from mfa_challenges where token_hash = $1")
            .bind(token_hash)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    /// Drop expired challenges. Cheap enough to run on the login path: the
    /// table only ever holds logins in flight.
    pub async fn purge_expired_challenges(&self) -> Result<u64> {
        let result = sqlx::query("delete from mfa_challenges where expires_at <= now()")
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(result.rows_affected())
    }
}
