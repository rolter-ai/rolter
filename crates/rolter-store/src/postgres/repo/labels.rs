//! Labels attached to providers, provider groups, routes and models (#985).
//!
//! One table backs both kinds of label, because they are rendered and
//! filtered together, but the two are written through different doors:
//! [`LabelRepo::create_custom`] and its siblings only ever touch
//! `source = 'custom'`, and [`LabelRepo::upsert_auto`] only ever touches
//! `source = 'auto'`. Nothing in this module can turn one into the other, so
//! an operator cannot hand-write a label that impersonates an observation and
//! a subsystem cannot clobber an operator's.

use chrono::{DateTime, Utc};
use sqlx::{Executor, PgPool, Postgres};
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::Label;
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

const LABEL_COLUMNS: &str = "id, subject_type, subject_id, key, value, source, observed_at, \
    observation, created_at, updated_at";

/// the kinds of thing a label can be attached to, as stored in `subject_type`
pub const LABEL_SUBJECT_TYPES: [&str; 4] = ["provider", "provider_group", "route", "model"];

/// the auto label recording that a model has an entry in the pricing catalog;
/// its value is the currency that price is quoted in
pub const PRICED_LABEL_KEY: &str = "priced";

/// A custom label an operator is writing.
#[derive(Debug, Clone, Copy)]
pub struct CustomLabelInput<'a> {
    pub subject_type: &'a str,
    pub subject_id: &'a str,
    pub key: &'a str,
    pub value: Option<&'a str>,
}

/// An automatic label a subsystem is recording, with the provenance that
/// distinguishes it from an operator's assertion.
#[derive(Debug, Clone, Copy)]
pub struct AutoLabelInput<'a> {
    pub subject_type: &'a str,
    pub subject_id: &'a str,
    pub key: &'a str,
    pub value: Option<&'a str>,
    /// what established the fact, e.g. `"model_prices"` — shown to operators
    /// so an auto label is never an unattributed claim
    pub observation: &'a str,
    pub observed_at: DateTime<Utc>,
}

/// What a list request narrows on. Every field is optional; `None` means "any".
#[derive(Debug, Clone, Copy, Default)]
pub struct LabelFilter<'a> {
    pub subject_type: Option<&'a str>,
    pub subject_id: Option<&'a str>,
    pub key: Option<&'a str>,
    pub value: Option<&'a str>,
    pub source: Option<&'a str>,
}

pub struct LabelRepo<'a>(pub &'a PgPool);

impl LabelRepo<'_> {
    /// Every label matching `filter`, ordered so a subject's labels arrive
    /// together and auto labels lead within one subject.
    pub async fn list(&self, filter: LabelFilter<'_>) -> Result<Vec<Label>> {
        sqlx::query_as(&format!(
            "select {LABEL_COLUMNS} from labels
              where ($1::text is null or subject_type = $1)
                and ($2::text is null or subject_id = $2)
                and ($3::text is null or key = $3)
                and ($4::text is null or value = $4)
                and ($5::text is null or source = $5)
              order by subject_type, subject_id, source, key"
        ))
        .bind(filter.subject_type)
        .bind(filter.subject_id)
        .bind(filter.key)
        .bind(filter.value)
        .bind(filter.source)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// The labels on one subject.
    pub async fn list_for_subject(
        &self,
        subject_type: &str,
        subject_id: &str,
    ) -> Result<Vec<Label>> {
        self.list(LabelFilter {
            subject_type: Some(subject_type),
            subject_id: Some(subject_id),
            ..LabelFilter::default()
        })
        .await
    }

    /// Every label on every provider, provider group and route the org owns.
    ///
    /// Tenancy is enforced here rather than by the caller: the table has no
    /// `org_id` of its own (a label's subject decides who owns it), so a
    /// listing that did not join back to the subject would hand one org's
    /// operators another's label set.
    pub async fn list_in_org(&self, org_id: Uuid, filter: LabelFilter<'_>) -> Result<Vec<Label>> {
        sqlx::query_as(&format!(
            "select {LABEL_COLUMNS} from labels
              where subject_id in (
                        select id::text from providers where org_id = $1
                        union all
                        select id::text from provider_groups where org_id = $1
                        union all
                        select r.id::text from routes r
                          join projects p on p.id = r.project_id
                          join teams t on t.id = p.team_id
                         where t.org_id = $1)
                and subject_type <> 'model'
                and ($2::text is null or subject_type = $2)
                and ($3::text is null or subject_id = $3)
                and ($4::text is null or key = $4)
                and ($5::text is null or value = $5)
                and ($6::text is null or source = $6)
              order by subject_type, subject_id, source, key"
        ))
        .bind(org_id)
        .bind(filter.subject_type)
        .bind(filter.subject_id)
        .bind(filter.key)
        .bind(filter.value)
        .bind(filter.source)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Whether `org_id` owns the subject a label is being attached to. The
    /// subject types are matched exactly, so an unknown one answers `false`
    /// rather than falling through to a permissive branch.
    pub async fn subject_in_org(
        &self,
        subject_type: &str,
        subject_id: Uuid,
        org_id: Uuid,
    ) -> Result<bool> {
        let sql = match subject_type {
            "provider" => "select exists(select 1 from providers where id = $1 and org_id = $2)",
            "provider_group" => {
                "select exists(select 1 from provider_groups where id = $1 and org_id = $2)"
            }
            "route" => {
                "select exists(select 1 from routes r
                                 join projects p on p.id = r.project_id
                                 join teams t on t.id = p.team_id
                                where r.id = $1 and t.org_id = $2)"
            }
            _ => return Ok(false),
        };
        let (found,): (bool,) = sqlx::query_as(sql)
            .bind(subject_id)
            .bind(org_id)
            .fetch_one(self.0)
            .await
            .map_err(store_err)?;
        Ok(found)
    }

    pub async fn get(&self, id: Uuid) -> Result<Label> {
        fetch_optional_or_not_found(
            sqlx::query_as(&format!("select {LABEL_COLUMNS} from labels where id = $1")).bind(id),
            self.0,
            || format!("label {id}"),
        )
        .await
    }

    /// Attach a custom label, or report that the subject already carries one
    /// under that key.
    ///
    /// `Ok(None)` is the conflict: `on conflict do nothing` lets the database
    /// settle the race between two callers adding the same key, rather than a
    /// read-then-insert that both would pass.
    pub async fn create_custom(&self, input: CustomLabelInput<'_>) -> Result<Option<Label>> {
        sqlx::query_as(&format!(
            "insert into labels (subject_type, subject_id, key, value, source)
             values ($1, $2, $3, $4, 'custom')
             on conflict (subject_type, subject_id, source, key) do nothing
             returning {LABEL_COLUMNS}"
        ))
        .bind(input.subject_type)
        .bind(input.subject_id)
        .bind(input.key)
        .bind(input.value)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Change a custom label's value. The `source` clause is what makes an
    /// auto label read-only: the row exists but does not match, so the caller
    /// gets the same not-found it would get for a label that never existed —
    /// there is no code path here that can edit an observation.
    pub async fn update_custom(&self, id: Uuid, value: Option<&str>) -> Result<Label> {
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "update labels set value = $2, updated_at = now()
                  where id = $1 and source = 'custom' returning {LABEL_COLUMNS}"
            ))
            .bind(id)
            .bind(value),
            self.0,
            || format!("custom label {id}"),
        )
        .await
    }

    /// Remove a custom label; auto labels are out of reach for the same reason
    /// as in [`Self::update_custom`].
    pub async fn delete_custom(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from labels where id = $1 and source = 'custom'").bind(id),
            self.0,
            || format!("custom label {id}"),
        )
        .await
    }

    /// Record an automatic label, replacing the previous observation of the
    /// same fact. Runs on any executor so a producer can write it inside the
    /// transaction that changed the thing being observed.
    pub async fn upsert_auto<'e, E>(executor: E, input: AutoLabelInput<'_>) -> Result<Label>
    where
        E: Executor<'e, Database = Postgres>,
    {
        sqlx::query_as(&format!(
            "insert into labels (subject_type, subject_id, key, value, source, observation, observed_at)
             values ($1, $2, $3, $4, 'auto', $5, $6)
             on conflict (subject_type, subject_id, source, key) do update
                set value = excluded.value,
                    observation = excluded.observation,
                    observed_at = excluded.observed_at,
                    updated_at = now()
             returning {LABEL_COLUMNS}"
        ))
        .bind(input.subject_type)
        .bind(input.subject_id)
        .bind(input.key)
        .bind(input.value)
        .bind(input.observation)
        .bind(input.observed_at)
        .fetch_one(executor)
        .await
        .map_err(store_err)
    }

    /// Withdraw an automatic label because the fact no longer holds. Silent
    /// when there was none: a producer should not have to read before it
    /// retracts.
    pub async fn clear_auto<'e, E>(
        executor: E,
        subject_type: &str,
        subject_id: &str,
        key: &str,
    ) -> Result<()>
    where
        E: Executor<'e, Database = Postgres>,
    {
        sqlx::query(
            "delete from labels
              where subject_type = $1 and subject_id = $2 and key = $3 and source = 'auto'",
        )
        .bind(subject_type)
        .bind(subject_id)
        .bind(key)
        .execute(executor)
        .await
        .map_err(store_err)?;
        Ok(())
    }
}
