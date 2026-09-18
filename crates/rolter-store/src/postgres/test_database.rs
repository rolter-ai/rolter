//! A Postgres database per worktree, derived rather than configured.
//!
//! [`test_schema`](crate::postgres::test_schema) already gives every *test* a
//! schema of its own, which is what makes plain `cargo test` — threads in one
//! process, as the coverage job runs — safe. It does nothing about the level
//! above: every worktree on a developer's machine points
//! `ROLTER_TEST_DATABASE_URL` at the same database, so suites running in
//! parallel worktrees share one. That is how 396 foreign `test_*` schemas
//! appeared mid-session while #1364 was being measured, and why a worktree on
//! an older commit can apply a different migration set to the database another
//! worktree is reading (#1430).
//!
//! So the url in the environment is treated as a *server* pointer, and the
//! database actually used is derived from the workspace the test was compiled
//! in: `rolter_test_wt_<worktree>_<digest>`, created on first use. A new
//! worktree is therefore isolated without anyone remembering to export
//! anything, which was the requirement — an isolation scheme that has to be
//! opted into is one that is forgotten exactly when parallel worktrees are busy.
//!
//! The worktree path is recorded as the database's comment, so
//! [`url()`](crate::postgres::test_database::url) can also drop the databases of
//! worktrees that no longer exist. That is the whole cleanup story: nothing has
//! to run when a worktree is removed.
//!
//! Set `ROLTER_TEST_PER_WORKTREE_DATABASE=0` to use `ROLTER_TEST_DATABASE_URL`
//! exactly as given — for a throwaway database that is already private, or to
//! reproduce the shared-database behaviour deliberately.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};
use sqlx::{Connection, Executor, PgConnection};

/// The env var naming the server, and the maintenance database on it.
pub const URL_ENV: &str = "ROLTER_TEST_DATABASE_URL";

/// Opt out of the derivation and use [`URL_ENV`] verbatim.
const OPT_OUT_ENV: &str = "ROLTER_TEST_PER_WORKTREE_DATABASE";

/// Prefix every derived database carries. The sweep only ever considers names
/// shaped `rolter_test_wt_*`, so a database a human created is never touched.
const DB_PREFIX: &str = "rolter_test_wt";

/// Advisory-lock key serialising create-and-sweep between worktrees starting
/// together. Arbitrary, only has to be stable, and distinct from the schema
/// sweep's.
const SETUP_LOCK_KEY: i64 = 0x0101_3641_5745_4443;

/// Resolved once per process: the derivation costs a connection and a couple of
/// round trips, and nothing about it changes while the process lives.
static RESOLVED: OnceLock<Option<String>> = OnceLock::new();

/// Whether a test database was configured at all, without connecting.
///
/// Test modules guard on this before doing any work, so it has to stay cheap
/// and synchronous; [`url()`](crate::postgres::test_database::url) is what
/// actually resolves the database.
pub fn is_configured() -> bool {
    std::env::var(URL_ENV).is_ok_and(|url| !url.is_empty())
}

/// The url the postgres tests should connect through, or `None` when
/// [`URL_ENV`] is unset and the caller should skip.
///
/// Falls back to the configured url whenever the derived database cannot be
/// created — a role without `CREATEDB` should lose isolation, not the ability
/// to run the suite at all.
pub async fn url() -> Option<String> {
    if let Some(resolved) = RESOLVED.get() {
        return resolved.clone();
    }
    let resolved = resolve().await;
    RESOLVED.get_or_init(|| resolved).clone()
}

/// The database name this worktree owns, exposed so a test can assert on the
/// derivation without connecting.
pub fn worktree_database_name() -> String {
    database_name_for(workspace_root())
}

async fn resolve() -> Option<String> {
    let configured = std::env::var(URL_ENV).ok().filter(|u| !u.is_empty())?;
    if opted_out() {
        return Some(configured);
    }
    // a url already pointing at a derived database is left alone: re-deriving
    // from one would nest a worktree database inside another's name
    if database_of(&configured).is_some_and(|db| db.starts_with(DB_PREFIX)) {
        return Some(configured);
    }

    let root = workspace_root();
    let name = database_name_for(root);
    match ensure_database(&configured, &name, root).await {
        Ok(()) => Some(with_database(&configured, &name)),
        Err(err) => {
            // isolation is a convenience; losing it must not lose the suite
            eprintln!(
                "could not create the per-worktree test database {name} ({err}); \
                 falling back to {URL_ENV} as configured"
            );
            Some(configured)
        }
    }
}

fn opted_out() -> bool {
    std::env::var(OPT_OUT_ENV).is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("false"))
}

/// The workspace root, taken from this crate's manifest directory at compile
/// time.
///
/// Every crate in the workspace links this one, so every test binary in a
/// worktree derives the same value — which a runtime `current_dir()` would not
/// guarantee, since it is the package directory under `cargo test` and anything
/// at all when a test binary is run by hand.
fn workspace_root() -> &'static str {
    // `<root>/crates/rolter-store`
    let manifest = env!("CARGO_MANIFEST_DIR");
    manifest
        .rsplit_once('/')
        .and_then(|(head, _)| head.rsplit_once('/'))
        .map_or(manifest, |(head, _)| head)
}

/// `rolter_test_wt_<slug>_<digest>`, stable for a path and safe to interpolate.
///
/// The slug is only there to make `\l` readable; the digest is what makes the
/// name unique, since two worktrees can share a directory name.
fn database_name_for(root: &str) -> String {
    let slug: String = root
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("workspace")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(24)
        .collect();
    let digest = Sha256::digest(root.as_bytes());
    let short: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{DB_PREFIX}_{slug}_{short}")
}

/// The database component of a postgres url, if it has one.
fn database_of(url: &str) -> Option<&str> {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    let name = base.rsplit('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// `url` with its database component replaced, query string preserved.
fn with_database(url: &str, database: &str) -> String {
    let (base, rest) = match url.find(['?', '#']) {
        Some(at) => (&url[..at], &url[at..]),
        None => (url, ""),
    };
    let head = base.rsplit_once('/').map_or(base, |(head, _)| head);
    format!("{head}/{database}{rest}")
}

/// Create the worktree's database if it is missing, record which worktree owns
/// it, and drop the databases of worktrees that are gone.
async fn ensure_database(maintenance_url: &str, name: &str, root: &str) -> Result<(), sqlx::Error> {
    let mut conn = PgConnection::connect(maintenance_url).await?;
    // one worktree at a time: `create database` takes a lock of its own and
    // concurrent creators would otherwise trade "already exists" errors
    let acquired: bool = sqlx::query_scalar("select pg_try_advisory_lock($1)")
        .bind(SETUP_LOCK_KEY)
        .fetch_one(&mut conn)
        .await?;
    if !acquired {
        // another worktree is mid-setup; wait for it rather than racing, since
        // it may be creating the very database this process needs
        sqlx::query("select pg_advisory_lock($1)")
            .bind(SETUP_LOCK_KEY)
            .execute(&mut conn)
            .await?;
    }

    let exists: bool =
        sqlx::query_scalar("select exists (select 1 from pg_database where datname = $1)")
            .bind(name)
            .fetch_one(&mut conn)
            .await?;
    if !exists {
        // `create database` cannot run inside a transaction, so it goes out as
        // a bare statement; the name is derived here and contains only ascii
        // lowercase, digits and underscores
        conn.execute(format!("create database {name}").as_str())
            .await?;
    }
    // the comment is the only record of which worktree owns the database, and
    // is what makes the sweep below possible
    conn.execute(format!("comment on database {name} is '{}'", escape(root)).as_str())
        .await?;

    if let Err(err) = sweep_abandoned(&mut conn, name).await {
        // housekeeping must never fail the suite
        eprintln!("test database sweep failed: {err}");
    }
    conn.close().await
}

/// Drop every `rolter_test_wt_*` database whose recorded worktree directory no
/// longer exists.
///
/// Deliberately conservative: a database with no comment, or one whose path is
/// still on disk, is left alone, and `drop database` is left to fail when
/// something is still connected — a worktree that is merely idle keeps its
/// database, and the next sweep takes it once the directory is removed.
async fn sweep_abandoned(conn: &mut PgConnection, keep: &str) -> Result<(), sqlx::Error> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "select datname::text, shobj_description(oid, 'pg_database')
           from pg_database
          where datname::text like $1",
    )
    .bind(format!("{DB_PREFIX}\\_%"))
    .fetch_all(&mut *conn)
    .await?;

    for (name, owner) in rows {
        if name == keep {
            continue;
        }
        let Some(root) = owner.filter(|root| !root.is_empty()) else {
            continue;
        };
        if std::path::Path::new(&root).is_dir() {
            continue;
        }
        eprintln!("dropping test database {name}: its worktree {root} is gone");
        if let Err(err) = conn
            .execute(format!("drop database if exists {name}").as_str())
            .await
        {
            eprintln!("failed to drop test database {name}: {err}");
        }
    }
    Ok(())
}

/// Escape a value for a single-quoted SQL literal. Only ever applied to a
/// filesystem path, which cannot be parameterised inside `comment on`.
fn escape(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_database_name_is_derived_from_the_worktree_and_is_a_safe_identifier() {
        let name = database_name_for("/home/dev/.worktrees/test-1430-isolate");
        assert!(
            name.starts_with("rolter_test_wt_test_1430_isolate_"),
            "{name}"
        );
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
            "{name} would have to be quoted"
        );
        assert!(name.len() <= 63, "{name} exceeds the identifier limit");
    }

    /// Two worktrees commonly share a directory name — a repository checked out
    /// twice, or `wt` reusing a branch slug — so the leaf alone cannot be the
    /// identity.
    #[test]
    fn worktrees_sharing_a_directory_name_still_get_different_databases() {
        assert_ne!(
            database_name_for("/a/rolter"),
            database_name_for("/b/rolter")
        );
        assert_eq!(
            database_name_for("/a/rolter"),
            database_name_for("/a/rolter")
        );
    }

    #[test]
    fn the_database_component_is_swapped_and_the_query_string_kept() {
        assert_eq!(
            with_database("postgres://u:p@localhost:5432/rolter_test", "wt_db"),
            "postgres://u:p@localhost:5432/wt_db"
        );
        assert_eq!(
            with_database(
                "postgres://u:p@localhost:5432/rolter_test?sslmode=disable",
                "wt_db"
            ),
            "postgres://u:p@localhost:5432/wt_db?sslmode=disable"
        );
    }

    #[test]
    fn the_configured_database_is_read_back_out_of_the_url() {
        assert_eq!(
            database_of("postgres://u:p@localhost:5432/rolter_test"),
            Some("rolter_test")
        );
        assert_eq!(
            database_of("postgres://u:p@localhost:5432/rolter_test?a=b"),
            Some("rolter_test")
        );
        assert_eq!(database_of("postgres://u:p@localhost:5432/"), None);
    }

    /// The manifest directory is `<root>/crates/rolter-store`, and the root is
    /// what every crate in the workspace has to agree on.
    #[test]
    fn the_workspace_root_is_two_levels_above_this_crate() {
        let root = workspace_root();
        assert!(
            std::path::Path::new(root)
                .join("crates/rolter-store")
                .is_dir(),
            "{root} does not look like the workspace root"
        );
    }
}
