//! Budgets, rate limits and model prices.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool, Row};
use uuid::Uuid;

use rolter_core::{Error, Result};

use super::super::models::{Budget, ModelPrice, RateLimit};
use super::keys::{keeps_a_cap, Edited};
use super::labels::{AutoLabelInput, LabelRepo, PRICED_LABEL_KEY};
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// Every column of `rate_limits` that its row type reads.
const RATE_LIMIT_COLUMNS: &str =
    "id, scope_type, scope_id, rpm, tpm, created_at, within_org_id, within_project_id";

/// The `where` clause (a literal, so it can sit inside `concat!`) shared by the org-wide budget and rate-limit lists: a row
/// belongs to org `$1` when its scope is the org, one of its teams, projects or
/// virtual keys, or one of its business units or customers.
macro_rules! in_org {
    () => {
        "(scope_type = 'org' and scope_id = $1)
    or (scope_type = 'team' and scope_id in (select id from teams where org_id = $1))
    or (scope_type = 'project' and scope_id in (
        select p.id from projects p join teams t on t.id = p.team_id where t.org_id = $1))
    or (scope_type = 'virtual_key' and scope_id in (
        select v.id from virtual_keys v
        join projects p on p.id = v.project_id
        join teams t on t.id = p.team_id where t.org_id = $1))
    or (scope_type = 'business_unit' and scope_id in (
        select id from business_units where org_id = $1))
    or (scope_type = 'customer' and scope_id in (
        select id from customers where org_id = $1))
    or (scope_type = 'user' and (within_org_id = $1 or within_project_id in (
        select p.id from projects p join teams t on t.id = p.team_id where t.org_id = $1)))"
    };
}

/// Where a `user`-scoped budget or rate limit applies (#1830): the keys the
/// person minted in one org, in one project, or (both `None`) anywhere. At most
/// one is set, and neither is meaningful on any other scope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Within {
    pub org: Option<Uuid>,
    pub project: Option<Uuid>,
}

impl Within {
    /// Narrowed to the keys minted in this org.
    pub fn org(org: Uuid) -> Self {
        Self {
            org: Some(org),
            project: None,
        }
    }

    /// Narrowed to the keys minted in this project.
    pub fn project(project: Uuid) -> Self {
        Self {
            org: None,
            project: Some(project),
        }
    }
}

/// The checks the table's own constraints make, as a message a caller can show
/// rather than a database error that surfaces as a 500.
fn require_user_scope_for_within(scope_type: &str, within: Within) -> Result<()> {
    if within.org.is_some() && within.project.is_some() {
        return Err(Error::Config(
            "a cap is narrowed to an org or to a project, not both".into(),
        ));
    }
    if scope_type != "user" && (within.org.is_some() || within.project.is_some()) {
        return Err(Error::Config(
            "only a user cap can be narrowed to an org or a project".into(),
        ));
    }
    Ok(())
}

/// Budgets, attachable at any scope (org/team/project/virtual_key, or a user).
pub struct BudgetRepo<'a>(pub &'a PgPool);

impl BudgetRepo<'_> {
    /// Every budget set at the org or at anything inside it, oldest first.
    pub async fn list_in_org(&self, org_id: Uuid) -> Result<Vec<Budget>> {
        sqlx::query_as(concat!(
            "select id, scope_type, scope_id, limit_usd::text as limit_usd, period, unpriced_policy, created_at, within_org_id, within_project_id
             from budgets where ",
            in_org!(),
            " order by created_at"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list_for_scope(&self, scope_type: &str, scope_id: Uuid) -> Result<Vec<Budget>> {
        sqlx::query_as(
            "select id, scope_type, scope_id, limit_usd::text as limit_usd, period, unpriced_policy, created_at, within_org_id, within_project_id
             from budgets where scope_type = $1 and scope_id = $2 order by created_at",
        )
        .bind(scope_type)
        .bind(scope_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// The budgets on a person narrowed to exactly `within` (#1830): `Within`
    /// as returned by a row, so a caller authorized for one org's caps never
    /// sees the same person's caps for another.
    pub async fn list_for_user(&self, user_id: Uuid, within: Within) -> Result<Vec<Budget>> {
        sqlx::query_as(
            "select id, scope_type, scope_id, limit_usd::text as limit_usd, period, unpriced_policy, created_at, within_org_id, within_project_id
             from budgets
             where scope_type = 'user' and scope_id = $1
               and within_org_id is not distinct from $2
               and within_project_id is not distinct from $3
             order by created_at",
        )
        .bind(user_id)
        .bind(within.org)
        .bind(within.project)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Every budget on a person, whatever it is narrowed to: what the person
    /// is shown about themselves (#1830).
    pub async fn list_all_for_user(&self, user_id: Uuid) -> Result<Vec<Budget>> {
        self.list_for_scope("user", user_id).await
    }

    pub async fn get(&self, id: Uuid) -> Result<Budget> {
        fetch_optional_or_not_found(
            sqlx::query_as(
                "select id, scope_type, scope_id, limit_usd::text as limit_usd, period, \
                 unpriced_policy, created_at, within_org_id, within_project_id \
                 from budgets where id = $1",
            )
            .bind(id),
            self.0,
            || format!("budget {id}"),
        )
        .await
    }

    /// `unpriced_policy` is `None` for a budget that inherits the
    /// deployment-wide setting, which is every budget created before #996.
    /// `within` narrows a `user` budget to one org or project (#1830) and is
    /// refused on any other scope, where the id already names a place.
    pub async fn create(
        &self,
        scope_type: &str,
        scope_id: Uuid,
        limit_usd: &str,
        period: &str,
        unpriced_policy: Option<&str>,
        within: Within,
    ) -> Result<Budget> {
        require_user_scope_for_within(scope_type, within)?;
        sqlx::query_as(
            "insert into budgets (scope_type, scope_id, limit_usd, period, unpriced_policy,
                                  within_org_id, within_project_id)
             values ($1, $2, $3::numeric, $4, $5, $6, $7)
             returning id, scope_type, scope_id, limit_usd::text as limit_usd, period,
                       unpriced_policy, created_at, within_org_id, within_project_id",
        )
        .bind(scope_type)
        .bind(scope_id)
        .bind(limit_usd)
        .bind(period)
        .bind(unpriced_policy)
        .bind(within.org)
        .bind(within.project)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Change a budget in place (#1285).
    ///
    /// `None` leaves a field as it is. `unpriced_policy` is doubly optional
    /// because clearing an override is an edit of its own: `Some(None)` sends
    /// the budget back to inheriting the deployment-wide setting, while `None`
    /// does not touch it.
    ///
    /// The row is locked and compared before anything is written, in one
    /// transaction. An edit that moves a field is one `update` statement, so
    /// the statement-level `budgets_bump_config_version` trigger fires once
    /// and a polling gateway sees either the old budget or the new one, never
    /// a scope with no cap in between. An edit whose every field already
    /// holds the value sent issues no `update` at all and comes back with
    /// [`Edited::changed`] unset: a statement trigger fires on an `update`
    /// that matches no row too, so filtering the no-op out in the `where`
    /// clause would still bump. `limit_usd` is compared in the column's own
    /// `numeric(12,4)`, so `"500.0"` against a stored `500.0000` is no change.
    /// The row keeps its id and `created_at`.
    pub async fn update(
        &self,
        id: Uuid,
        limit_usd: Option<&str>,
        period: Option<&str>,
        unpriced_policy: Option<Option<&str>>,
    ) -> Result<Edited<Budget>> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let current = sqlx::query(
            "select id, scope_type, scope_id, limit_usd::text as limit_usd, period,
                    unpriced_policy, created_at, within_org_id, within_project_id,
                    (limit_usd, period, unpriced_policy) is distinct from
                    (coalesce($2::numeric(12, 4), limit_usd), coalesce($3, period),
                     case when $4 then $5 else unpriced_policy end) as moves
             from budgets where id = $1
             for update",
        )
        .bind(id)
        .bind(limit_usd)
        .bind(period)
        .bind(unpriced_policy.is_some())
        .bind(unpriced_policy.flatten())
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("budget {id}")))?;
        let before = Budget::from_row(&current).map_err(store_err)?;
        let moves: bool = current.try_get("moves").map_err(store_err)?;
        if !moves {
            tx.commit().await.map_err(store_err)?;
            return Ok(Edited::unchanged(before));
        }
        let after = sqlx::query_as(
            "update budgets set
                 limit_usd = coalesce($2::numeric, limit_usd),
                 period = coalesce($3, period),
                 unpriced_policy = case when $4 then $5 else unpriced_policy end
             where id = $1
             returning id, scope_type, scope_id, limit_usd::text as limit_usd, period,
                       unpriced_policy, created_at, within_org_id, within_project_id",
        )
        .bind(id)
        .bind(limit_usd)
        .bind(period)
        .bind(unpriced_policy.is_some())
        .bind(unpriced_policy.flatten())
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(Edited {
            before,
            after,
            changed: true,
        })
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from budgets where id = $1").bind(id),
            self.0,
            || format!("budget {id}"),
        )
        .await?;
        Ok(())
    }
}

/// Rate limits, attachable at any scope (org/team/project/virtual_key).
pub struct RateLimitRepo<'a>(pub &'a PgPool);

impl RateLimitRepo<'_> {
    /// Every rate limit set at the org or at anything inside it, oldest first.
    pub async fn list_in_org(&self, org_id: Uuid) -> Result<Vec<RateLimit>> {
        sqlx::query_as(concat!(
            "select id, scope_type, scope_id, rpm, tpm, created_at, within_org_id, within_project_id
             from rate_limits where ",
            in_org!(),
            " order by created_at"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list_for_scope(&self, scope_type: &str, scope_id: Uuid) -> Result<Vec<RateLimit>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {RATE_LIMIT_COLUMNS}
             from rate_limits where scope_type = $1 and scope_id = $2 order by created_at"
        )))
        .bind(scope_type)
        .bind(scope_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// The rate limits on a person narrowed to exactly `within`; see
    /// [`BudgetRepo::list_for_user`].
    pub async fn list_for_user(&self, user_id: Uuid, within: Within) -> Result<Vec<RateLimit>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {RATE_LIMIT_COLUMNS}
             from rate_limits
             where scope_type = 'user' and scope_id = $1
               and within_org_id is not distinct from $2
               and within_project_id is not distinct from $3
             order by created_at"
        )))
        .bind(user_id)
        .bind(within.org)
        .bind(within.project)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Every rate limit on a person, whatever it is narrowed to.
    pub async fn list_all_for_user(&self, user_id: Uuid) -> Result<Vec<RateLimit>> {
        self.list_for_scope("user", user_id).await
    }

    pub async fn get(&self, id: Uuid) -> Result<RateLimit> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {RATE_LIMIT_COLUMNS}
             from rate_limits where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("rate limit {id}"),
        )
        .await
    }

    /// Store a new rate limit.
    ///
    /// Refused with [`Error::Config`] unless at least one cap is positive: the
    /// snapshot loader reads a cap of zero or below as none, so a limit
    /// without a positive one admits every request while looking like a hard
    /// stop (#1903). The same rule [`update`](Self::update) applies to what an
    /// edit leaves behind.
    pub async fn create(
        &self,
        scope_type: &str,
        scope_id: Uuid,
        rpm: Option<i32>,
        tpm: Option<i32>,
        within: Within,
    ) -> Result<RateLimit> {
        if !keeps_a_cap(rpm, tpm) {
            return Err(Error::Config(
                "a rate limit needs an rpm cap, a tpm cap or both, each at least 1".into(),
            ));
        }
        require_user_scope_for_within(scope_type, within)?;
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into rate_limits (scope_type, scope_id, rpm, tpm,
                                      within_org_id, within_project_id)
             values ($1, $2, $3, $4, $5, $6)
             returning {RATE_LIMIT_COLUMNS}"
        )))
        .bind(scope_type)
        .bind(scope_id)
        .bind(rpm)
        .bind(tpm)
        .bind(within.org)
        .bind(within.project)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Change a rate limit's caps in place (#1285).
    ///
    /// Each cap is doubly optional: `None` leaves it alone, `Some(None)`
    /// lifts it, `Some(Some(n))` sets it. The row is locked first, in one
    /// transaction, and the caps the edit would leave are checked against
    /// that locked row: a rate limit left with no positive cap admits every
    /// request, so such an edit is an [`Error::Config`] and writes nothing.
    /// Checked against a read taken before the lock, two edits that each lift
    /// a different cap would both pass and together store exactly that.
    ///
    /// An edit that moves a cap is one `update` statement, so the
    /// `rate_limits_bump_config_version` trigger bumps `config_version` once.
    /// One that leaves both caps as they were issues no statement and comes
    /// back with [`Edited::changed`] unset, for the reason
    /// [`BudgetRepo::update`] gives.
    pub async fn update(
        &self,
        id: Uuid,
        rpm: Option<Option<i32>>,
        tpm: Option<Option<i32>>,
    ) -> Result<Edited<RateLimit>> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let before: RateLimit = fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {RATE_LIMIT_COLUMNS}
             from rate_limits where id = $1
             for update"
            )))
            .bind(id),
            &mut *tx,
            || format!("rate limit {id}"),
        )
        .await?;
        let next_rpm = rpm.unwrap_or(before.rpm);
        let next_tpm = tpm.unwrap_or(before.tpm);
        if !keeps_a_cap(next_rpm, next_tpm) {
            return Err(Error::Config(
                "a rate limit must keep an rpm cap, a tpm cap or both; delete it to lift every cap"
                    .into(),
            ));
        }
        if (next_rpm, next_tpm) == (before.rpm, before.tpm) {
            tx.commit().await.map_err(store_err)?;
            return Ok(Edited::unchanged(before));
        }
        let after = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "update rate_limits set rpm = $2, tpm = $3
             where id = $1
             returning {RATE_LIMIT_COLUMNS}"
        )))
        .bind(id)
        .bind(next_rpm)
        .bind(next_tpm)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(Edited {
            before,
            after,
            changed: true,
        })
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from rate_limits where id = $1").bind(id),
            self.0,
            || format!("rate limit {id}"),
        )
        .await?;
        Ok(())
    }
}

/// The pricing catalog (usd per million tokens), keyed by public model name.
pub struct ModelPriceRepo<'a>(pub &'a PgPool);

/// The rates of a model price beyond the text token ones: the unit rates (#2937)
/// and the audio token rates (#1882). Each is tri-state like the cache write
/// rates: `None` keeps the stored rate, `Some(None)` clears it and
/// `Some(Some(rate))` sets it. The default keeps all of them, which is what a
/// client that predates them means by a body without them.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnitRates<'a> {
    pub image_per_unit: Option<Option<&'a str>>,
    pub audio_input_per_minute: Option<Option<&'a str>>,
    pub speech_per_mchar: Option<Option<&'a str>>,
    pub audio_input_per_mtok: Option<Option<&'a str>>,
    pub audio_output_per_mtok: Option<Option<&'a str>>,
    pub cached_audio_input_per_mtok: Option<Option<&'a str>>,
}

/// A model price being created or replaced. The cache-write rates and the
/// [`UnitRates`] are tri-state, as on [`ModelPriceRepo::upsert`]; `Default`
/// keeps every stored rate, so a caller names the model, the two token rates and
/// the currency.
#[derive(Debug, Clone, Copy, Default)]
pub struct NewModelPrice<'a> {
    pub model: &'a str,
    pub input_per_mtok: &'a str,
    pub output_per_mtok: &'a str,
    pub cached_input_per_mtok: Option<&'a str>,
    pub cache_write_per_mtok: Option<Option<&'a str>>,
    pub cache_write_1h_per_mtok: Option<Option<&'a str>>,
    pub units: UnitRates<'a>,
    pub currency: &'a str,
}

/// One price a catalogue import offers (#966). Rates are per million tokens,
/// already in `currency`, which the adapter states rather than assumes.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CataloguePrice {
    pub model: String,
    pub input_per_mtok: String,
    pub output_per_mtok: String,
    pub cached_input_per_mtok: Option<String>,
    pub cache_write_per_mtok: Option<String>,
    pub currency: String,
}

/// What an import did with one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogueImport {
    /// no price existed, one was created from the catalogue
    Created,
    /// the model already had a catalogue price, which was refreshed
    Updated,
    /// an operator set this price, so the catalogue left it alone
    KeptOverride,
}

impl ModelPriceRepo<'_> {
    /// Import catalogue prices without replacing an operator's own (#966).
    ///
    /// A row with no `source` is one an operator wrote, and stays exactly as it
    /// is; a row that names a source (this catalogue's or another's) is
    /// replaced by the offered values and re-stamped. Tokens-only: the unit
    /// rates (images, audio) of an existing row are left alone. Each model is
    /// one statement inside a single transaction, so a refused row rolls back
    /// the whole import and the config version is bumped once per statement the
    /// trigger sees.
    pub async fn import_catalogue(
        &self,
        source: &str,
        imported_at: DateTime<Utc>,
        prices: &[CataloguePrice],
    ) -> Result<Vec<(String, CatalogueImport)>> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let mut outcomes = Vec::with_capacity(prices.len());
        for price in prices {
            // `xmax = 0` is true for a row this statement inserted; no row back
            // at all means the conflict target was an operator's, which the
            // `where` clause refuses to touch
            let written: Option<bool> = sqlx::query_scalar(
                "insert into model_prices
                        (model, input_per_mtok, output_per_mtok, cached_input_per_mtok,
                         cache_write_per_mtok, currency, source, source_imported_at)
                 values ($1, $2::numeric, $3::numeric, $4::numeric, $5::numeric, $6, $7, $8)
                 on conflict (model) do update
                    set input_per_mtok = excluded.input_per_mtok,
                        output_per_mtok = excluded.output_per_mtok,
                        cached_input_per_mtok = excluded.cached_input_per_mtok,
                        cache_write_per_mtok = excluded.cache_write_per_mtok,
                        currency = excluded.currency,
                        source = excluded.source,
                        source_imported_at = excluded.source_imported_at
                    where model_prices.source is not null
                 returning (xmax = 0)",
            )
            .bind(&price.model)
            .bind(&price.input_per_mtok)
            .bind(&price.output_per_mtok)
            .bind(price.cached_input_per_mtok.as_deref())
            .bind(price.cache_write_per_mtok.as_deref())
            .bind(&price.currency)
            .bind(source)
            .bind(imported_at)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_err)?;
            let outcome = match written {
                Some(true) => CatalogueImport::Created,
                Some(false) => CatalogueImport::Updated,
                None => CatalogueImport::KeptOverride,
            };
            if outcome != CatalogueImport::KeptOverride {
                LabelRepo::upsert_auto(
                    &mut *tx,
                    AutoLabelInput {
                        subject_type: "model",
                        subject_id: &price.model,
                        key: PRICED_LABEL_KEY,
                        value: Some(&price.currency),
                        observation: "model_prices",
                        observed_at: Utc::now(),
                    },
                )
                .await?;
            }
            outcomes.push((price.model.clone(), outcome));
        }
        tx.commit().await.map_err(store_err)?;
        Ok(outcomes)
    }

    pub async fn list(&self) -> Result<Vec<ModelPrice>> {
        sqlx::query_as(
            "select id, model,
                    input_per_mtok::text as input_per_mtok,
                    output_per_mtok::text as output_per_mtok,
                    cached_input_per_mtok::text as cached_input_per_mtok,
                    cache_write_per_mtok::text as cache_write_per_mtok,
                    cache_write_1h_per_mtok::text as cache_write_1h_per_mtok,
                    image_per_unit::text as image_per_unit,
                    audio_input_per_minute::text as audio_input_per_minute,
                    speech_per_mchar::text as speech_per_mchar,
                    audio_input_per_mtok::text as audio_input_per_mtok,
                    audio_output_per_mtok::text as audio_output_per_mtok,
                    cached_audio_input_per_mtok::text as cached_audio_input_per_mtok,
                    currency, created_at, source, source_imported_at
             from model_prices order by model",
        )
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Create or replace a model's price.
    ///
    /// The two cache-write rates are tri-state, unlike their siblings (#2876,
    /// #2891): `None` keeps the stored rate, `Some(None)` clears it back to its
    /// fallback and `Some(Some(rate))` sets it. A caller that predates a column
    /// sends a body without it, and replacing the row must not wipe a rate an
    /// operator set since, which "omitted means null" would do on the next
    /// save.
    ///
    /// The non-token rates (#2937) keep their stored value unless `units` names
    /// them.
    pub async fn upsert(&self, values: NewModelPrice<'_>) -> Result<ModelPrice> {
        let NewModelPrice {
            model,
            input_per_mtok,
            output_per_mtok,
            cached_input_per_mtok,
            cache_write_per_mtok,
            cache_write_1h_per_mtok,
            units,
            currency,
        } = values;
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let row = sqlx::query_as(
            "insert into model_prices
                    (model, input_per_mtok, output_per_mtok, cached_input_per_mtok,
                     cache_write_per_mtok, cache_write_1h_per_mtok,
                     image_per_unit, audio_input_per_minute, speech_per_mchar,
                     audio_input_per_mtok, audio_output_per_mtok,
                     cached_audio_input_per_mtok, currency)
             values ($1, $2::numeric, $3::numeric, $4::numeric, $5::numeric, $6::numeric,
                     $7::numeric, $8::numeric, $9::numeric, $10::numeric, $11::numeric,
                     $12::numeric, $13)
             on conflict (model) do update
                set input_per_mtok = excluded.input_per_mtok,
                    output_per_mtok = excluded.output_per_mtok,
                    cached_input_per_mtok = excluded.cached_input_per_mtok,
                    cache_write_per_mtok = case when $14::boolean
                        then excluded.cache_write_per_mtok
                        else model_prices.cache_write_per_mtok end,
                    cache_write_1h_per_mtok = case when $15::boolean
                        then excluded.cache_write_1h_per_mtok
                        else model_prices.cache_write_1h_per_mtok end,
                    image_per_unit = case when $16::boolean
                        then excluded.image_per_unit
                        else model_prices.image_per_unit end,
                    audio_input_per_minute = case when $17::boolean
                        then excluded.audio_input_per_minute
                        else model_prices.audio_input_per_minute end,
                    speech_per_mchar = case when $18::boolean
                        then excluded.speech_per_mchar
                        else model_prices.speech_per_mchar end,
                    audio_input_per_mtok = case when $19::boolean
                        then excluded.audio_input_per_mtok
                        else model_prices.audio_input_per_mtok end,
                    audio_output_per_mtok = case when $20::boolean
                        then excluded.audio_output_per_mtok
                        else model_prices.audio_output_per_mtok end,
                    cached_audio_input_per_mtok = case when $21::boolean
                        then excluded.cached_audio_input_per_mtok
                        else model_prices.cached_audio_input_per_mtok end,
                    currency = excluded.currency,
                    source = null,
                    source_imported_at = null
             returning id, model,
                       input_per_mtok::text as input_per_mtok,
                       output_per_mtok::text as output_per_mtok,
                       cached_input_per_mtok::text as cached_input_per_mtok,
                       cache_write_per_mtok::text as cache_write_per_mtok,
                       cache_write_1h_per_mtok::text as cache_write_1h_per_mtok,
                       image_per_unit::text as image_per_unit,
                       audio_input_per_minute::text as audio_input_per_minute,
                       speech_per_mchar::text as speech_per_mchar,
                       audio_input_per_mtok::text as audio_input_per_mtok,
                       audio_output_per_mtok::text as audio_output_per_mtok,
                       cached_audio_input_per_mtok::text as cached_audio_input_per_mtok,
                       currency, created_at, source, source_imported_at",
        )
        .bind(model)
        .bind(input_per_mtok)
        .bind(output_per_mtok)
        .bind(cached_input_per_mtok)
        .bind(cache_write_per_mtok.flatten())
        .bind(cache_write_1h_per_mtok.flatten())
        .bind(units.image_per_unit.flatten())
        .bind(units.audio_input_per_minute.flatten())
        .bind(units.speech_per_mchar.flatten())
        .bind(units.audio_input_per_mtok.flatten())
        .bind(units.audio_output_per_mtok.flatten())
        .bind(units.cached_audio_input_per_mtok.flatten())
        .bind(currency)
        .bind(cache_write_per_mtok.is_some())
        .bind(cache_write_1h_per_mtok.is_some())
        .bind(units.image_per_unit.is_some())
        .bind(units.audio_input_per_minute.is_some())
        .bind(units.speech_per_mchar.is_some())
        .bind(units.audio_input_per_mtok.is_some())
        .bind(units.audio_output_per_mtok.is_some())
        .bind(units.cached_audio_input_per_mtok.is_some())
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        // the first producer against the label primitive (#985): "this model
        // has a price" is a fact rolter establishes, and the moment it becomes
        // true is this statement, so it is recorded here rather than by a job
        // that would have to guess how stale it is allowed to be
        LabelRepo::upsert_auto(
            &mut *tx,
            AutoLabelInput {
                subject_type: "model",
                subject_id: model,
                key: PRICED_LABEL_KEY,
                value: Some(currency),
                observation: "model_prices",
                observed_at: Utc::now(),
            },
        )
        .await?;
        tx.commit().await.map_err(store_err)?;
        Ok(row)
    }

    pub async fn delete(&self, model: &str) -> Result<()> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        require_affected(
            sqlx::query("delete from model_prices where model = $1").bind(model),
            &mut *tx,
            || format!("model price '{model}'"),
        )
        .await?;
        // the fact stops holding with the row, and an auto label that outlived
        // its observation would be worse than none
        LabelRepo::clear_auto(&mut *tx, "model", model, PRICED_LABEL_KEY).await?;
        tx.commit().await.map_err(store_err)?;
        Ok(())
    }
}
