//! Per-test Postgres schema isolation, with the schema actually reclaimed.
//!
//! Every postgres-backed test runs in a schema of its own pinned through
//! `search_path`, because plain `cargo test` (the coverage job) runs tests as
//! threads in one process against a shared database and would otherwise race
//! on DDL. Each schema carries the full migration set, so a schema that is
//! never dropped costs a few megabytes and one run leaks dozens of them; local
//! databases grew to thousands of schemas and gigabytes until Postgres
//! degraded and the whole suite failed in a way that reads like a code
//! regression (#1364).
//!
//! [`TestSchema`](crate::postgres::test_schema::TestSchema) fixes that at the
//! source: it drops the schema in `Drop`, so
//! cleanup also happens when a test panics. A hard-killed process — SIGKILL,
//! or a `cargo nextest` timeout — never runs `Drop`, and neither did any run
//! predating this module, so the first call in a process also sweeps schemas
//! left behind by processes that are no longer alive.
//!
//! Set `ROLTER_TEST_KEEP_SCHEMA=1` to keep a failing test's schema (and skip
//! the sweep) when the rows themselves are the evidence.

use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use sqlx::{Connection, Executor, PgConnection, PgPool};

use super::{connect, run_migrations};

/// Prefix every isolated test schema carries. The sweep only ever considers
/// names shaped exactly `test_<pid>_<seq>`, so a schema a human created is
/// never a candidate.
const SCHEMA_PREFIX: &str = "test";

/// Schemas dropped per statement by the sweep. `drop schema ... cascade` takes
/// a lock per object it removes, and a migrated schema holds around 210 of
/// them, so a batch that looks modest still overruns the lock table
/// (`max_locks_per_transaction` defaults to 64, shared across backends) and
/// fails with `out of shared memory` — measured here at ten schemas, fine at
/// four. [`drop_schemas`] falls back to one at a time when a batch does fail,
/// since the budget is shared and a busy database can refuse a size that
/// usually fits.
const SWEEP_BATCH: usize = 4;

/// Advisory-lock key serialising the sweep between concurrently starting test
/// processes. Arbitrary, only has to be stable.
const SWEEP_LOCK_KEY: i64 = 0x0101_3641_5745_4550;

/// Ceiling on one guard's cleanup. `Drop` blocks the test thread while this
/// runs, so it has to end whatever happens; a schema left behind is a nuisance
/// the next run's sweep clears, a wedged suite is not.
const CLEANUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

static SCHEMA_SEQ: AtomicU32 = AtomicU32::new(0);
static SWEPT: AtomicBool = AtomicBool::new(false);

/// Whether the caller asked for schemas to be preserved for inspection.
fn keep_schemas() -> bool {
    std::env::var("ROLTER_TEST_KEEP_SCHEMA").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Isolated schema name unique to this process and call, safe to interpolate
/// (only ascii lowercase, digits and underscores).
fn unique_schema() -> String {
    let n = SCHEMA_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{SCHEMA_PREFIX}_{}_{}", std::process::id(), n)
}

/// Return `url` with the connection pinned to `schema` via `search_path`, so
/// migrations and queries land in the isolated schema rather than `public`.
pub fn with_search_path(url: &str, schema: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    // percent-encode the space and `=` inside the libpq options string
    format!("{url}{sep}options=-c%20search_path%3D{schema}")
}

/// An isolated schema plus a pool pinned to it, dropped when the guard is.
///
/// Hold it for the whole test: the schema disappears with the guard, so a
/// binding dropped early takes the tables with it. It derefs to the pool, so
/// `&guard` works wherever a `&PgPool` is expected.
pub struct TestSchema {
    schema: String,
    admin_url: String,
    /// taken in `Drop` so the pool can be closed before the schema goes
    pool: Option<PgPool>,
}

impl TestSchema {
    /// Create an isolated schema and return a pool scoped to it. Migrations
    /// are *not* applied — use [`TestSchema::migrated`] for that, or apply
    /// them through whatever the test is exercising.
    pub async fn create(url: &str) -> Self {
        sweep_once(url).await;

        // (re)create the isolated schema over a default-search_path connection.
        // the defensive drop matters because the operating system recycles
        // pids: a previous run holding our pid may have left this exact name
        let schema = unique_schema();
        let admin = connect(url).await.expect("connect");
        sqlx::query(&format!("drop schema if exists {schema} cascade"))
            .execute(&admin)
            .await
            .expect("reset schema");
        sqlx::query(&format!("create schema {schema}"))
            .execute(&admin)
            .await
            .expect("create schema");
        admin.close().await;

        let pool = connect(&with_search_path(url, &schema))
            .await
            .expect("connect scoped");
        Self {
            schema,
            admin_url: url.to_string(),
            pool: Some(pool),
        }
    }

    /// As [`TestSchema::create`], with every migration applied.
    pub async fn migrated(url: &str) -> Self {
        let guard = Self::create(url).await;
        run_migrations(guard.pool()).await.expect("run migrations");
        guard
    }

    /// The pool pinned to this schema.
    pub fn pool(&self) -> &PgPool {
        self.pool.as_ref().expect("pool taken only in Drop")
    }

    /// The schema name, for the rare test that has to name it in SQL or hand
    /// it to an external tool such as `pg_dump`.
    pub fn schema(&self) -> &str {
        &self.schema
    }
}

impl Deref for TestSchema {
    type Target = PgPool;

    fn deref(&self) -> &Self::Target {
        self.pool()
    }
}

impl Drop for TestSchema {
    fn drop(&mut self) {
        let pool = self.pool.take();
        if keep_schemas() {
            eprintln!(
                "ROLTER_TEST_KEEP_SCHEMA set: keeping schema {} for inspection",
                self.schema
            );
            return;
        }
        let schema = std::mem::take(&mut self.schema);
        let url = std::mem::take(&mut self.admin_url);

        // the pool's sockets are registered with the reactor of the runtime the
        // test built them on, so awaiting anything on them from the cleanup
        // runtime below never completes — `close()` in particular hangs the
        // whole suite. dropping the handle is enough: the connections go with
        // the test's runtime, and `drop_schema` below opens its own
        drop(pool);

        // `Drop` cannot await, and neither of the in-place options works here:
        // `block_in_place` panics on the current-thread runtime `#[tokio::test]`
        // builds by default, and a task spawned onto that runtime never
        // finishes because the runtime shuts down as the test returns. A
        // short-lived thread with a runtime of its own is independent of both,
        // and behaves the same under `cargo test` (tests as threads) and
        // `cargo nextest` (a process per test)
        let cleanup = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("cleanup runtime");
            runtime.block_on(async move {
                match tokio::time::timeout(CLEANUP_TIMEOUT, drop_schema(&url, &schema)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => eprintln!("failed to drop test schema {schema}: {err}"),
                    // cleanup must never be the thing that hangs a test run; the
                    // next run's sweep takes the schema instead
                    Err(_) => eprintln!("timed out dropping test schema {schema}"),
                }
            });
        });
        // never panic from `Drop`: a test that is already unwinding would abort
        // the process and hide the real failure
        if cleanup.join().is_err() {
            eprintln!("test schema cleanup thread panicked");
        }
    }
}

/// Drop the schema one finished test owned, over a connection of this
/// runtime's own.
///
/// A test that ended with a connection still inside a transaction leaves locks
/// on its own tables behind, and `drop schema` would queue behind them until
/// that connection goes — which cannot happen before this returns, since the
/// test's runtime only shuts down once `Drop` does. Every such connection is
/// this test's, because nothing else ever touched this schema, so terminating
/// the lock holders is safe and turns a deadlock into a clean drop.
async fn drop_schema(url: &str, schema: &str) -> Result<(), sqlx::Error> {
    let mut conn = PgConnection::connect(url).await?;
    conn.execute("set lock_timeout = '10s'").await?;
    sqlx::query(
        "select pg_terminate_backend(l.pid)
           from pg_locks l
           join pg_class c on c.oid = l.relation
           join pg_namespace n on n.oid = c.relnamespace
          where n.nspname = $1 and l.pid <> pg_backend_pid()",
    )
    .bind(schema)
    .execute(&mut conn)
    .await?;
    conn.execute(format!("drop schema if exists {schema} cascade").as_str())
        .await?;
    conn.close().await
}

/// Drop `schemas` in lock-table-sized batches over one short-lived connection.
async fn drop_schemas(url: &str, schemas: &[String]) -> Result<(), sqlx::Error> {
    if schemas.is_empty() {
        return Ok(());
    }
    let mut conn = PgConnection::connect(url).await?;
    // the sweep only ever takes schemas whose process is gone, so nothing
    // should hold a lock on one; the timeout is there so a surprise cannot
    // wedge a test run
    conn.execute("set lock_timeout = '10s'").await?;
    for batch in schemas.chunks(SWEEP_BATCH) {
        let sql = format!("drop schema if exists {} cascade", batch.join(", "));
        if let Err(err) = conn.execute(sql.as_str()).await {
            // another sweeper may have taken a schema between the listing and
            // the drop; retry one at a time so one loser cannot strand a batch
            eprintln!("batched schema drop failed ({err}); retrying individually");
            for schema in batch {
                let sql = format!("drop schema if exists {schema} cascade");
                if let Err(err) = conn.execute(sql.as_str()).await {
                    eprintln!("failed to drop test schema {schema}: {err}");
                }
            }
        }
    }
    conn.close().await
}

/// Sweep once per process, before the first schema is created.
async fn sweep_once(url: &str) {
    if keep_schemas() || SWEPT.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Err(err) = sweep_stale_schemas(url).await {
        // a sweep is housekeeping; failing it must not fail the test
        eprintln!("test schema sweep failed: {err}");
    }
}

/// Drop every `test_<pid>_<seq>` schema whose pid is not a live process, and
/// report how many went.
///
/// This is the migration path for databases that already carry thousands of
/// schemas from runs predating the guard, plus the backstop for a process
/// killed hard enough that `Drop` never ran. It is deliberately conservative:
/// a schema is only dropped when its pid is provably gone, so a suite running
/// concurrently in another process can never lose its schema. Pids are
/// recycled, so a schema may occasionally be judged live when its creator is
/// long dead — that only defers the drop to a later run, which is the harmless
/// direction of the error.
pub async fn sweep_stale_schemas(url: &str) -> Result<usize, sqlx::Error> {
    let mut conn = PgConnection::connect(url).await?;

    // one sweeper at a time: concurrent processes starting together would
    // otherwise race each other's drops for no benefit
    let acquired: bool = sqlx::query_scalar("select pg_try_advisory_lock($1)")
        .bind(SWEEP_LOCK_KEY)
        .fetch_one(&mut conn)
        .await?;
    if !acquired {
        conn.close().await?;
        return Ok(0);
    }

    let names: Vec<String> = sqlx::query_scalar(
        "select nspname::text from pg_namespace where nspname::text like $1 order by nspname",
    )
    .bind(format!("{SCHEMA_PREFIX}\\_%"))
    .fetch_all(&mut conn)
    .await?;

    let stale: Vec<String> = names
        .into_iter()
        .filter(|name| embedded_pid(name).is_some_and(|pid| !pid_is_live(pid)))
        .collect();
    let count = stale.len();
    if count > 0 {
        eprintln!("sweeping {count} test schemas left by processes that are gone");
        drop_schemas(url, &stale).await?;
    }
    // the lock lives on `conn`, so it is only released here — after the drops,
    // not after the listing
    conn.close().await?;
    Ok(count)
}

/// The pid embedded in a schema this module created, or `None` for any other
/// name. The shape is checked strictly — `test_<digits>_<digits>` — so a
/// hand-made schema that merely starts with `test_` is never a candidate.
fn embedded_pid(schema: &str) -> Option<u32> {
    let parts: Vec<&str> = schema.split('_').collect();
    let [prefix, pid, seq] = parts[..] else {
        return None;
    };
    if prefix != SCHEMA_PREFIX || seq.is_empty() || !seq.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    pid.parse().ok()
}

/// Whether `pid` names a live process. Errs towards reporting live: the caller
/// deletes data, so an inconclusive probe must never read as dead.
fn pid_is_live(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        // only trust procfs when it is actually mounted, or every pid would
        // look dead and the sweep would eat a running suite's schema
        if std::path::Path::new("/proc/self").is_dir() {
            return std::path::Path::new(&format!("/proc/{pid}")).exists();
        }
    }
    // `kill -0` probes without signalling; a probe we could not run at all
    // counts as live
    match std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Comfortably above any `kernel.pid_max`, so it can never be live.
    const DEAD_PID: u32 = 4_000_000_000;

    fn database_url() -> Option<String> {
        std::env::var("ROLTER_TEST_DATABASE_URL").ok()
    }

    /// The database url, unless the run asked for schemas to be kept: every
    /// test below asserts that something was reclaimed, which is exactly what
    /// `ROLTER_TEST_KEEP_SCHEMA` switches off, and a developer debugging with
    /// it set should not be handed four failures of their own making.
    fn database_url_with_cleanup() -> Option<String> {
        if keep_schemas() {
            eprintln!("skipping: ROLTER_TEST_KEEP_SCHEMA keeps schemas by design");
            return None;
        }
        database_url()
    }

    async fn schema_exists(url: &str, schema: &str) -> bool {
        let pool = connect(url).await.expect("connect");
        let exists: bool =
            sqlx::query_scalar("select exists (select 1 from pg_namespace where nspname = $1)")
                .bind(schema)
                .fetch_one(&pool)
                .await
                .expect("query");
        pool.close().await;
        exists
    }

    #[test]
    fn only_schemas_this_module_named_are_sweep_candidates() {
        assert_eq!(embedded_pid("test_1234_7"), Some(1234));
        assert_eq!(embedded_pid("test_1234_7_extra"), None);
        assert_eq!(embedded_pid("test_fixtures"), None);
        assert_eq!(embedded_pid("test_1234_seven"), None);
        assert_eq!(embedded_pid("testing_1234_7"), None);
        assert_eq!(embedded_pid("public"), None);
    }

    #[test]
    fn liveness_errs_towards_keeping_the_schema() {
        assert!(pid_is_live(std::process::id()));
        assert!(!pid_is_live(DEAD_PID));
    }

    #[tokio::test]
    async fn the_schema_goes_when_the_guard_does() {
        let Some(url) = database_url_with_cleanup() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let schema = {
            let guard = TestSchema::create(&url).await;
            let name = guard.schema().to_string();
            assert!(schema_exists(&url, &name).await);
            name
        };
        assert!(
            !schema_exists(&url, &schema).await,
            "the guard left {schema} behind"
        );
    }

    /// Cleanup must not be able to wedge the suite it is cleaning up after.
    ///
    /// The first cut of this guard closed the pool from the cleanup runtime and
    /// hung every postgres test in the workspace: a pool's sockets are
    /// registered with the reactor of the runtime that opened them, so awaiting
    /// them anywhere else waits forever, and a transaction the test never
    /// finished holds locks that `drop schema` would otherwise queue behind
    /// until that runtime shuts down — which cannot happen until `Drop`
    /// returns. Both conditions are reproduced here; the assertion is that the
    /// drop finishes at all.
    #[tokio::test]
    async fn an_unfinished_transaction_cannot_wedge_the_cleanup() {
        let Some(url) = database_url_with_cleanup() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let guard = TestSchema::migrated(&url).await;
        let schema = guard.schema().to_string();

        // a checked-out connection sitting in a transaction, holding a lock on
        // one of the schema's tables and never committed
        let mut tx = guard.pool().begin().await.expect("begin");
        let _: i64 = sqlx::query_scalar("select count(*) from orgs")
            .fetch_one(&mut *tx)
            .await
            .expect("read inside the transaction");

        let started = std::time::Instant::now();
        drop(guard);
        let took = started.elapsed();

        assert!(
            took < CLEANUP_TIMEOUT,
            "cleanup took {took:?}, so it waited on the open transaction"
        );
        assert!(
            !schema_exists(&url, &schema).await,
            "the guard left {schema} behind"
        );
        drop(tx);
    }

    /// The leak this module exists to stop was reintroduced by any failing
    /// test, so cleanup has to survive the unwind rather than only the happy
    /// path.
    #[tokio::test]
    async fn a_panicking_test_still_drops_its_schema() {
        let Some(url) = database_url_with_cleanup() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let guard = TestSchema::create(&url).await;
        let schema = guard.schema().to_string();

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = guard;
            panic!("as a failing test would");
        }));

        assert!(panicked.is_err());
        assert!(
            !schema_exists(&url, &schema).await,
            "a panicking test left {schema} behind"
        );
    }

    #[tokio::test]
    async fn the_sweep_takes_dead_schemas_and_spares_live_ones() {
        let Some(url) = database_url_with_cleanup() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let dead = format!("{SCHEMA_PREFIX}_{DEAD_PID}_0");
        let live = format!("{SCHEMA_PREFIX}_{}_999999", std::process::id());
        let pool = connect(&url).await.expect("connect");
        for schema in [&dead, &live] {
            sqlx::query(&format!("create schema if not exists {schema}"))
                .execute(&pool)
                .await
                .expect("create schema");
        }
        pool.close().await;

        // the sweep steps aside when another process holds the advisory lock,
        // so retry rather than assert on a single attempt — a suite running
        // beside this one would otherwise fail the test for behaving correctly
        for _ in 0..10 {
            sweep_stale_schemas(&url).await.expect("sweep");
            if !schema_exists(&url, &dead).await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        assert!(
            !schema_exists(&url, &dead).await,
            "{dead} survived the sweep"
        );
        assert!(
            schema_exists(&url, &live).await,
            "the sweep dropped {live}, whose process is running"
        );

        drop_schemas(&url, &[live]).await.expect("clean up");
    }
}
