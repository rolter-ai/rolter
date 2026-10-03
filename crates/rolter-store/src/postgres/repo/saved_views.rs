//! A user's saved filter presets (#1825).
//!
//! Every query here is keyed by the owner's `user_id`, including the ones that
//! also take a preset id. There is deliberately no way to read or write a
//! preset by id alone: a preset that belongs to someone else is
//! indistinguishable from one that does not exist, so ids cannot be probed.

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::SavedView;
use super::store_err;

const COLUMNS: &str = "id, user_id, surface, name, filters, created_at, updated_at";

/// What a create or update did. Collisions are outcomes rather than errors so
/// the caller can say which rule the request broke.
#[derive(Debug)]
pub enum SavedViewWrite {
    Saved(SavedView),
    /// the owner already has a preset of that name on that surface
    DuplicateName,
    /// the owner is at the per-surface cap (create only)
    LimitReached,
    /// no preset with that id belongs to the owner (update only)
    NotFound,
}

pub struct SavedViewRepo<'a>(pub &'a PgPool);

impl SavedViewRepo<'_> {
    /// the owner's presets, oldest first; `surface` narrows to one screen
    pub async fn list(&self, user_id: Uuid, surface: Option<&str>) -> Result<Vec<SavedView>> {
        sqlx::query_as(&format!(
            "select {COLUMNS} from saved_views
             where user_id = $1 and ($2::text is null or surface = $2)
             order by created_at, id"
        ))
        .bind(user_id)
        .bind(surface)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, user_id: Uuid, id: Uuid) -> Result<Option<SavedView>> {
        sqlx::query_as(&format!(
            "select {COLUMNS} from saved_views where user_id = $1 and id = $2"
        ))
        .bind(user_id)
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// add a preset unless the owner is at `max_per_surface` or already has
    /// that name. The owner's `users` row is locked for the transaction, so two
    /// concurrent creates cannot both squeeze under the cap
    pub async fn create(
        &self,
        user_id: Uuid,
        surface: &str,
        name: &str,
        filters: &serde_json::Value,
        max_per_surface: i64,
    ) -> Result<SavedViewWrite> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query("select id from users where id = $1 for update")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_err)?;
        let held: i64 = sqlx::query_scalar(
            "select count(*) from saved_views where user_id = $1 and surface = $2",
        )
        .bind(user_id)
        .bind(surface)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        if held >= max_per_surface {
            return Ok(SavedViewWrite::LimitReached);
        }
        let row: Option<SavedView> = sqlx::query_as(&format!(
            "insert into saved_views (user_id, surface, name, filters)
             values ($1, $2, $3, $4)
             on conflict (user_id, surface, lower(name)) do nothing
             returning {COLUMNS}"
        ))
        .bind(user_id)
        .bind(surface)
        .bind(name)
        .bind(filters)
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(row.map_or(SavedViewWrite::DuplicateName, SavedViewWrite::Saved))
    }

    /// rename and/or replace the filters of the owner's preset. `None` leaves
    /// that column as it was
    pub async fn update(
        &self,
        user_id: Uuid,
        id: Uuid,
        name: Option<&str>,
        filters: Option<&serde_json::Value>,
    ) -> Result<SavedViewWrite> {
        let res = sqlx::query_as(&format!(
            "update saved_views set
                 name = coalesce($3, name),
                 filters = coalesce($4, filters),
                 updated_at = now()
             where user_id = $1 and id = $2
             returning {COLUMNS}"
        ))
        .bind(user_id)
        .bind(id)
        .bind(name)
        .bind(filters)
        .fetch_optional(self.0)
        .await;
        match res {
            Ok(Some(row)) => Ok(SavedViewWrite::Saved(row)),
            Ok(None) => Ok(SavedViewWrite::NotFound),
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                Ok(SavedViewWrite::DuplicateName)
            }
            Err(err) => Err(store_err(err)),
        }
    }

    /// whether a preset of the owner's was deleted
    pub async fn delete(&self, user_id: Uuid, id: Uuid) -> Result<bool> {
        let res = sqlx::query("delete from saved_views where user_id = $1 and id = $2")
            .bind(user_id)
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(res.rows_affected() > 0)
    }
}
