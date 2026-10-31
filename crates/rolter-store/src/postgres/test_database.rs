//! A Postgres database per worktree, derived rather than configured.
//!
//! [`test_schema`] already gives every *test* a
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
//! database actually used is derived from the workspace the test is running
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
/// and synchronous; [`url()`] is what
/// actually resolves the database.
pub fn is_configured() -> bool {
    std::env::var(URL_ENV).is_ok_and(|url| !url.is_empty())
}

/// The url the postgres tests should connect through, or `None` when
/// [`URL_ENV`] is unset and the caller should skip.
///
/// Never falls back to the shared database: a failure to create the derived
/// one is retried with a bounded backoff and then panics with the cause, since
/// a silent fallback would put this worktree's migrations in a database other
/// worktrees are reading (#1898). Set `ROLTER_TEST_PER_WORKTREE_DATABASE=0` to
/// choose the shared database deliberately, for a role without `CREATEDB`.
///
/// # Panics
///
/// When the derived database cannot be created after every retry.
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
    database_name_for(&workspace_root())
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
    let name = database_name_for(&root);
    let result = retry(RETRY_DELAYS, |_| ensure_database(&configured, &name, &root)).await;
    match result {
        Ok(()) => Some(with_database(&configured, &name)),
        // a silent fallback would share one database between worktrees, which
        // is the isolation failure #1430 removed
        Err(err) => panic!(
            "could not create the per-worktree test database {name}: {err}; \
             refusing to fall back to the shared database. Fix the connection, \
             or set {OPT_OUT_ENV}=0 to use {URL_ENV} as given"
        ),
    }
}

/// Backoff between attempts; one more attempt than there are delays.
const RETRY_DELAYS: &[std::time::Duration] = &[
    std::time::Duration::from_millis(200),
    std::time::Duration::from_millis(500),
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(2),
];

/// Run `op` until it succeeds or the delays are spent, returning the last error.
async fn retry<T, E, F, Fut>(delays: &[std::time::Duration], mut op: F) -> Result<T, E>
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut attempt = 0;
    loop {
        match op(attempt).await {
            Ok(value) => return Ok(value),
            Err(err) => {
                let Some(delay) = delays.get(attempt) else {
                    return Err(err);
                };
                eprintln!("per-worktree test database setup failed ({err}); retrying in {delay:?}");
                tokio::time::sleep(*delay).await;
                attempt += 1;
            }
        }
    }
}

fn opted_out() -> bool {
    std::env::var(OPT_OUT_ENV).is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("false"))
}

/// The workspace root of the worktree the test is running in.
///
/// Cargo and nextest both export `CARGO_MANIFEST_DIR` to the test process at
/// *run* time, naming the package under test, so the root is found by walking
/// up from it. That is what keeps the answer right when worktrees share a
/// `CARGO_TARGET_DIR`: the compiled `rolter-store` artifact, and any path baked
/// into it with `env!`, can be reused from another worktree's build, and the
/// suite then connects to that worktree's database (#2770). Every package in
/// the workspace sits in the same worktree, so every test binary still agrees.
///
/// Falls back to the path this crate was compiled in when the variable is unset
/// (a test binary run by hand), which is the best a binary with no other
/// context can do.
fn workspace_root() -> String {
    let runtime = std::env::var_os("CARGO_MANIFEST_DIR")
        .and_then(|dir| workspace_root_from(std::path::Path::new(&dir)));
    match runtime {
        Some(root) => root.to_string_lossy().into_owned(),
        None => compiled_workspace_root().to_owned(),
    }
}

/// The nearest ancestor of `start` (itself included) whose `Cargo.toml` declares
/// a `[workspace]`.
fn workspace_root_from(start: &std::path::Path) -> Option<std::path::PathBuf> {
    start.ancestors().find_map(|dir| {
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
        manifest
            .lines()
            .any(|line| line.trim() == "[workspace]")
            .then(|| dir.to_path_buf())
    })
}

/// The workspace root as of compile time: `<root>/crates/rolter-store` with the
/// last two segments removed.
fn compiled_workspace_root() -> &'static str {
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
        conn.execute(sqlx::AssertSqlSafe(format!("create database {name}")))
            .await?;
    }
    // the comment is the only record of which worktree owns the database, and
    // is what makes the sweep below possible
    conn.execute(sqlx::AssertSqlSafe(format!(
        "comment on database {name} is '{}'",
        escape(root)
    )))
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
            .execute(sqlx::AssertSqlSafe(format!(
                "drop database if exists {name}"
            )))
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
            std::path::Path::new(&root)
                .join("crates/rolter-store")
                .is_dir(),
            "{root} does not look like the workspace root"
        );
        assert_eq!(root, compiled_workspace_root());
    }

    /// Every package in a worktree must resolve to the same root, whichever
    /// package's tests are running (#2770).
    #[test]
    fn any_package_in_a_workspace_resolves_to_the_same_root() {
        let base = std::env::temp_dir().join(format!("rolter-wt-root-{}", uuid::Uuid::new_v4()));
        let a = base.join("crates/a");
        let b = base.join("crates/b/nested");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(base.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        std::fs::write(a.join("Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();

        assert_eq!(workspace_root_from(&a).as_deref(), Some(base.as_path()));
        assert_eq!(workspace_root_from(&b).as_deref(), Some(base.as_path()));
        assert_eq!(workspace_root_from(&base).as_deref(), Some(base.as_path()));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_directory_outside_any_workspace_resolves_to_nothing() {
        let base = std::env::temp_dir().join(format!("rolter-wt-none-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let found = workspace_root_from(&base);
        std::fs::remove_dir_all(&base).unwrap();
        // a stray Cargo.toml in a parent of the temp dir would be a false hit
        assert!(found.is_none_or(|root| !root.starts_with(&base)));
    }

    #[tokio::test]
    async fn a_transient_failure_is_retried_until_it_succeeds() {
        let ms = std::time::Duration::from_millis(1);
        let result = retry(&[ms, ms, ms], |attempt| async move {
            if attempt < 2 {
                Err("connection refused")
            } else {
                Ok(attempt)
            }
        })
        .await;
        assert_eq!(result, Ok(2));
    }

    #[tokio::test]
    async fn a_persistent_failure_is_returned_after_the_bound_not_swallowed() {
        let ms = std::time::Duration::from_millis(1);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result: Result<(), &str> = retry(&[ms, ms], |_| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { Err("down") }
        })
        .await;
        assert_eq!(result, Err("down"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}
