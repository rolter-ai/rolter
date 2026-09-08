//! Shared plumbing for the repository modules under [`super`].
//!
//! Every repository in this tree repeats the same three shapes: map a `sqlx`
//! failure onto [`Error::Store`], turn a missing row from `fetch_optional`
//! into [`Error::NotFound`], and turn a write that matched nothing into the
//! same. Keeping those here means a domain module states the SQL and the
//! resource name and nothing else.

use sqlx::postgres::{PgArguments, PgRow};
use sqlx::query::{Query, QueryAs};
use sqlx::{Executor, FromRow, Postgres};

use rolter_core::{Error, Result};

/// Map a `sqlx` failure onto the store's error type.
pub(super) fn store_err(err: sqlx::Error) -> Error {
    Error::Store(err.to_string())
}

/// Run a single-row query that is expected to match, reporting
/// [`Error::NotFound`] with `resource` when it does not.
///
/// `resource` is a closure rather than a `Display` so the message is only
/// built on the miss, the way the hand-written `ok_or_else` sites it replaces
/// behaved. `format_args!` is deliberately not accepted: `fmt::Arguments` is
/// `!Send`, and holding one across the await would make every axum handler
/// that reaches this repository a non-`Send` future.
pub(super) async fn fetch_optional_or_not_found<'q, 'e, 'c: 'e, T, E, R>(
    query: QueryAs<'q, Postgres, T, PgArguments>,
    executor: E,
    resource: R,
) -> Result<T>
where
    'q: 'e,
    E: 'e + Executor<'c, Database = Postgres>,
    T: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'e,
    R: FnOnce() -> String,
{
    query
        .fetch_optional(executor)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(resource()))
}

/// Run a write that is expected to touch at least one row, reporting
/// [`Error::NotFound`] with `resource` when it touched none.
pub(super) async fn require_affected<'q, 'e, 'c: 'e, E, R>(
    query: Query<'q, Postgres, PgArguments>,
    executor: E,
    resource: R,
) -> Result<()>
where
    'q: 'e,
    E: 'e + Executor<'c, Database = Postgres>,
    R: FnOnce() -> String,
{
    let result = query.execute(executor).await.map_err(store_err)?;
    if result.rows_affected() == 0 {
        return Err(Error::NotFound(resource()));
    }
    Ok(())
}
