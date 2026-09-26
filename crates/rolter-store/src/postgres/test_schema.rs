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
//!
//! Every suite on a machine, from every worktree, draws its connections from
//! one Postgres server, so a test pool is sized for what a test uses rather
//! than for what the control plane serves, and a test that cannot connect says
//! whether the server ran out of slots instead of reporting a bare pool
//! timeout (#1735).

use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, Executor, PgConnection, PgPool};

use super::{connect_with, run_migrations, PoolConfig};

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

/// Connections one test's pool may open.
///
/// The production default of ten is sized for the control plane's fan-out; a
/// test that shares a Postgres server with every other suite on the machine
/// has no use for it. A pool opens connections lazily, so the ceiling only
/// bites for a test that runs more than this many queries at once, and such a
/// test waits for a free connection rather than failing.
const TEST_POOL_MAX_CONNECTIONS: u32 = 4;

/// Overrides [`TEST_POOL_MAX_CONNECTIONS`], to re-run the connection-budget
/// measurement in `docs/dev-docs/development/testing.md` or to give one test
/// more room while investigating it.
const POOL_SIZE_ENV: &str = "ROLTER_TEST_POOL_MAX_CONNECTIONS";

/// SQLSTATE `too_many_connections`: the server has no connection slot left.
const TOO_MANY_CONNECTIONS: &str = "53300";

/// SQLSTATE `cannot_connect_now`: the server is still starting up.
const CANNOT_CONNECT_NOW: &str = "57P03";

/// First and longest wait between two attempts of [`connect_riding_out`].
/// The first is short because a slot usually frees within milliseconds, as
/// another test's guard finishes its cleanup; the cap keeps the last attempts
/// close together so a slot that frees late is still taken promptly.
const RETRY_BACKOFF_START: std::time::Duration = std::time::Duration::from_millis(10);
const RETRY_BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(1);

static SCHEMA_SEQ: AtomicU32 = AtomicU32::new(0);
static SWEPT: AtomicBool = AtomicBool::new(false);

/// Whether the caller asked for schemas to be preserved for inspection.
fn keep_schemas() -> bool {
    std::env::var("ROLTER_TEST_KEEP_SCHEMA").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// The ceiling for a test pool: [`POOL_SIZE_ENV`] when it holds a positive
/// number, [`TEST_POOL_MAX_CONNECTIONS`] otherwise.
fn pool_max_connections(configured: Option<&str>) -> u32 {
    configured
        .and_then(|value| value.trim().parse().ok())
        .filter(|&n: &u32| n > 0)
        .unwrap_or(TEST_POOL_MAX_CONNECTIONS)
}

/// The budget every [`TestSchema`] pool is built with.
fn test_pool_config() -> PoolConfig {
    PoolConfig {
        max_connections: pool_max_connections(std::env::var(POOL_SIZE_ENV).ok().as_deref()),
        ..PoolConfig::default()
    }
}

/// `host:port` of the server `url` names, with the credentials left out, for
/// messages a test prints.
fn server_label(url: &str) -> String {
    url.parse::<PgConnectOptions>().map_or_else(
        |_| "the configured server".to_string(),
        |options| format!("{}:{}", options.get_host(), options.get_port()),
    )
}

fn is_too_many_connections(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db) if db.code().as_deref() == Some(TOO_MANY_CONNECTIONS))
}

/// Whether a failed connect clears up on its own: the server has no slot free
/// right now, or is still starting. The same two codes sqlx's pool retries
/// while it connects, so a direct connection is no less patient than a pool.
fn is_transient_connect_error(err: &sqlx::Error) -> bool {
    matches!(
        err,
        sqlx::Error::Database(db)
            if matches!(db.code().as_deref(), Some(TOO_MANY_CONNECTIONS | CANNOT_CONNECT_NOW))
    )
}

/// Open one direct connection, riding out a transient refusal for up to
/// `window` with capped exponential backoff, as a pool would during its
/// acquire timeout. A server at `max_connections` for a moment, which is what
/// several suites on one server produce, then costs a short wait rather than
/// a failed test. Any other error, a refused TCP connection included, is
/// returned at once: retrying a server that is not there only delays the
/// message that says so.
async fn connect_riding_out(
    options: &PgConnectOptions,
    window: std::time::Duration,
) -> Result<PgConnection, sqlx::Error> {
    let deadline = tokio::time::Instant::now() + window;
    let mut backoff = RETRY_BACKOFF_START;
    loop {
        match PgConnection::connect_with(options).await {
            Err(err) if is_transient_connect_error(&err) => {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    return Err(err);
                }
                tokio::time::sleep(backoff.min(left)).await;
                backoff = (backoff * 2).min(RETRY_BACKOFF_MAX);
            }
            result => return result,
        }
    }
}

/// What a test prints when the server has no connection slot left.
fn exhaustion_message(server: &str) -> String {
    format!(
        "the test Postgres at {server} has no connection slots left (SQLSTATE \
         {TOO_MANY_CONNECTIONS}, `sorry, too many clients already`). max_connections is one \
         budget shared by every suite running against that server, from every worktree, so \
         this is connection exhaustion rather than a failure of this test: give the server \
         more headroom (`just test-pg` starts one with enough) or run fewer suites at once. \
         See \"The connection budget\" in docs/dev-docs/development/testing.md"
    )
}

/// Client connections in use on the server and its `max_connections`.
async fn connection_usage(conn: &mut PgConnection) -> Option<(i64, i64)> {
    sqlx::query_as(
        "select count(*) filter (where backend_type = 'client backend'),
                current_setting('max_connections')::int8
           from pg_stat_activity",
    )
    .fetch_one(conn)
    .await
    .ok()
}

/// Why a direct connection to `url` failed, phrased for whoever reads the test
/// output. `pool_error` is what the pool reported first, when there was one.
fn describe_direct_failure(url: &str, err: &sqlx::Error, pool_error: Option<&str>) -> String {
    let server = server_label(url);
    if is_too_many_connections(err) {
        return exhaustion_message(&server);
    }
    if matches!(err, sqlx::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused) {
        return format!(
            "nothing is accepting connections at {server}, the server \
             ROLTER_TEST_DATABASE_URL names: start the test Postgres with `just test-pg`, or \
             unset the variable to skip the postgres tests ({err})"
        );
    }
    match pool_error {
        Some(pool_error) => format!(
            "could not connect to the test Postgres at {server}: {pool_error} (a direct \
             connection failed too: {err})"
        ),
        None => format!("could not connect to the test Postgres at {server}: {err}"),
    }
}

/// Turn a pool that failed to connect into a message that says why.
///
/// A pool treats `too many clients` as transient and retries it until its
/// acquire timeout, then reports a bare `PoolTimedOut` that reads like a slow
/// query. A single direct connection is not retried, so it surfaces the
/// server's own answer, which is what this reports.
async fn explain_pool_failure(url: &str, pool_error: &str) -> String {
    let server = server_label(url);
    match PgConnection::connect(url).await {
        Err(err) => describe_direct_failure(url, &err, Some(pool_error)),
        Ok(mut conn) => {
            let usage = connection_usage(&mut conn)
                .await
                .map_or_else(String::new, |(used, max)| {
                    format!(" and has {used} of max_connections {max} in use")
                });
            let _ = conn.close().await;
            format!(
                "could not connect to the test Postgres at {server}: {pool_error}. The server \
                 accepts a direct connection now{usage}, so it was out of connection slots or \
                 unreachable for the pool's whole acquire timeout; see \"The connection \
                 budget\" in docs/dev-docs/development/testing.md"
            )
        }
    }
}

/// A note for a test that failed while the server was out of connections, or
/// close to it, since the failure the test reports is then most likely a pool
/// timeout rather than a regression. `None` when the server has room.
async fn exhaustion_note(url: &str) -> Option<String> {
    match PgConnection::connect(url).await {
        Err(err) if is_too_many_connections(&err) => Some(exhaustion_message(&server_label(url))),
        Err(_) => None,
        Ok(mut conn) => {
            let usage = connection_usage(&mut conn).await;
            let _ = conn.close().await;
            let (used, max) = usage?;
            // within a tenth of the ceiling: close enough that the pool's
            // retries were most likely what ran the clock out
            (used * 10 >= max * 9).then(|| {
                format!(
                    "the test Postgres at {} had {used} of max_connections {max} in use when \
                     this test failed; if the failure is a pool timeout, it is connection \
                     exhaustion rather than a regression. See \"The connection budget\" in \
                     docs/dev-docs/development/testing.md",
                    server_label(url)
                )
            })
        }
    }
}

/// A direct connection for the few statements a guard runs outside the test's
/// schema, with the failure explained.
///
/// It waits out a server with no free slot for the test pool's acquire
/// timeout, the time the pool it replaces gave it, and only then reports the
/// exhaustion.
async fn admin_connection(url: &str) -> PgConnection {
    let window = test_pool_config().acquire_timeout;
    let connected = match url.parse::<PgConnectOptions>() {
        Ok(options) => connect_riding_out(&options, window).await,
        Err(err) => Err(err),
    };
    match connected {
        Ok(conn) => conn,
        Err(err) if is_transient_connect_error(&err) => panic!(
            "{} (still refused after retrying for {window:?})",
            describe_direct_failure(url, &err, None)
        ),
        Err(err) => panic!("{}", describe_direct_failure(url, &err, None)),
    }
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
        // connect before the sweep, so a server that cannot be reached is
        // reported as such and the once-per-process sweep is not spent on it
        let mut admin = admin_connection(url).await;
        sweep_once(url).await;

        // (re)create the isolated schema over a default-search_path connection.
        // the defensive drop matters because the operating system recycles
        // pids: a previous run holding our pid may have left this exact name
        let schema = unique_schema();
        admin
            .execute(format!("drop schema if exists {schema} cascade").as_str())
            .await
            .expect("reset schema");
        admin
            .execute(format!("create schema {schema}").as_str())
            .await
            .expect("create schema");
        let _ = admin.close().await;

        let pool = match connect_with(&with_search_path(url, &schema), test_pool_config()).await {
            Ok(pool) => pool,
            Err(err) => panic!("{}", explain_pool_failure(url, &err.to_string()).await),
        };
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

        // a failing test on a busy server most often failed on a pool timeout,
        // which names neither the pool nor the server, so say which one ran out
        let panicking = std::thread::panicking();
        if panicking {
            if let Some(pool) = pool.as_ref() {
                let max = pool.options().get_max_connections();
                if pool.size() >= max && pool.num_idle() == 0 {
                    eprintln!(
                        "this test failed with its pool at its ceiling ({POOL_SIZE_ENV}={max}) \
                         and no connection idle; if the failure is a pool timeout, the test \
                         needs a bigger pool or holds a connection while it waits for another"
                    );
                }
            }
        }

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
                let work = async {
                    if panicking {
                        if let Some(note) = exhaustion_note(&url).await {
                            eprintln!("{note}");
                        }
                    }
                    drop_schema(&url, &schema).await
                };
                match tokio::time::timeout(CLEANUP_TIMEOUT, work).await {
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
    use crate::postgres::connect;

    /// Comfortably above any `kernel.pid_max`, so it can never be live.
    const DEAD_PID: u32 = 4_000_000_000;

    async fn database_url() -> Option<String> {
        crate::postgres::test_database::url().await
    }

    /// The database url, unless the run asked for schemas to be kept: every
    /// test below asserts that something was reclaimed, which is exactly what
    /// `ROLTER_TEST_KEEP_SCHEMA` switches off, and a developer debugging with
    /// it set should not be handed four failures of their own making.
    async fn database_url_with_cleanup() -> Option<String> {
        if keep_schemas() {
            eprintln!("skipping: ROLTER_TEST_KEEP_SCHEMA keeps schemas by design");
            return None;
        }
        database_url().await
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
    fn the_test_pool_budget_falls_back_to_the_default_on_anything_but_a_positive_number() {
        assert_eq!(pool_max_connections(None), TEST_POOL_MAX_CONNECTIONS);
        assert_eq!(pool_max_connections(Some("10")), 10);
        assert_eq!(pool_max_connections(Some(" 2 ")), 2);
        assert_eq!(pool_max_connections(Some("0")), TEST_POOL_MAX_CONNECTIONS);
        assert_eq!(pool_max_connections(Some("-1")), TEST_POOL_MAX_CONNECTIONS);
        assert_eq!(
            pool_max_connections(Some("lots")),
            TEST_POOL_MAX_CONNECTIONS
        );
    }

    /// The label ends up in panic messages and CI logs, so the password in the
    /// url must never reach it.
    #[test]
    fn the_server_label_names_host_and_port_and_never_the_password() {
        let label = server_label("postgres://u:p@db.internal:55432/rolter_test");
        assert_eq!(label, "db.internal:55432");
        assert_eq!(server_label("not a url"), "the configured server");
    }

    #[test]
    fn the_exhaustion_message_names_the_limit_and_where_to_read_about_it() {
        let message = exhaustion_message("127.0.0.1:55432");
        for needle in [
            "127.0.0.1:55432",
            "53300",
            "max_connections",
            "connection exhaustion",
            "The connection budget",
        ] {
            assert!(
                message.contains(needle),
                "{needle:?} missing from {message}"
            );
        }
    }

    /// A url naming a local port nothing listens on.
    fn url_with_no_server() -> (String, u16) {
        // bind and release a port so nothing is listening on it
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        (format!("postgres://u:p@127.0.0.1:{port}/rolter_test"), port)
    }

    /// The text a panic carried, for a test that asserts on what a failing
    /// test would print.
    fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
        match payload.downcast::<String>() {
            Ok(text) => *text,
            Err(payload) => payload
                .downcast_ref::<&str>()
                .map_or_else(|| "a non-text panic".to_string(), |s| s.to_string()),
        }
    }

    /// A server that is down used to cost the full acquire timeout and then a
    /// bare `PoolTimedOut`; a direct connection answers at once and says so.
    #[tokio::test]
    async fn a_server_that_is_not_listening_is_named_as_such() {
        let (url, port) = url_with_no_server();
        let err = PgConnection::connect(&url)
            .await
            .expect_err("nothing listens there");
        let message = describe_direct_failure(&url, &err, None);
        assert!(
            message.contains("nothing is accepting connections")
                && message.contains(&format!("127.0.0.1:{port}")),
            "{message}"
        );
    }

    /// The same, through the guard every postgres test builds: its setup
    /// fails at once with the explanation rather than after the acquire
    /// timeout with a bare `connect` expectation.
    #[tokio::test]
    async fn a_guard_for_a_server_that_is_not_listening_says_so_at_once() {
        let (url, port) = url_with_no_server();
        let started = std::time::Instant::now();
        let payload = tokio::spawn(async move {
            let _guard = TestSchema::create(&url).await;
        })
        .await
        .expect_err("a guard needs a server")
        .into_panic();
        let took = started.elapsed();
        let message = panic_text(payload);
        assert!(
            message.contains("nothing is accepting connections")
                && message.contains(&format!("127.0.0.1:{port}")),
            "{message}"
        );
        assert!(
            took < test_pool_config().acquire_timeout / 2,
            "a refused connection was retried for {took:?}"
        );
    }

    /// `url` with `role` and `password` in place of its credentials.
    fn url_as(url: &str, role: &str, password: &str) -> String {
        let options = url
            .parse::<PgConnectOptions>()
            .expect("the test url parses");
        let host = options.get_host();
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        format!(
            "postgres://{role}:{password}@{host}:{}/{}",
            options.get_port(),
            options.get_database().unwrap_or("postgres")
        )
    }

    /// Postgres refuses a role over its `connection limit` with the same
    /// SQLSTATE 53300 a server at `max_connections` sends, so a role limited to
    /// no connections at all is exhaustion on demand, without filling the
    /// server every other suite shares. Against it this checks that the
    /// classification matches what a real server sends, that a direct
    /// connection waits a refusal out for its whole window rather than failing
    /// on the first, and that a guard's setup connects once a slot frees.
    #[tokio::test]
    async fn a_server_out_of_slots_is_waited_out_and_then_named() {
        let Some(url) = database_url().await else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let role = format!("rolter_{}", unique_schema());
        // a fresh password per run, since the role lives, however briefly, on
        // a server every suite on the machine shares
        let password = uuid::Uuid::new_v4().simple().to_string();
        let mut admin = PgConnection::connect(&url).await.expect("connect");
        admin
            .execute(
                format!("create role {role} login password '{password}' connection limit 0")
                    .as_str(),
            )
            .await
            .expect("create a role with no connection slots");
        let slotless = url_as(&url, &role, &password);

        // assertions run in a task of their own so the role is dropped even
        // when one of them fails
        let checks = tokio::spawn({
            let url = url.clone();
            let role = role.clone();
            async move {
                let err = PgConnection::connect(&slotless)
                    .await
                    .expect_err("a role with no slots is refused");
                assert!(is_too_many_connections(&err), "not classified: {err}");
                let message = describe_direct_failure(&slotless, &err, None);
                assert!(message.contains("no connection slots left"), "{message}");

                let options = slotless.parse::<PgConnectOptions>().expect("parses");
                let window = std::time::Duration::from_millis(300);
                let started = std::time::Instant::now();
                let err = connect_riding_out(&options, window)
                    .await
                    .expect_err("the role still has no slots");
                assert!(is_too_many_connections(&err), "{err}");
                assert!(
                    started.elapsed() >= window,
                    "gave up after {:?} instead of retrying for {window:?}",
                    started.elapsed()
                );

                // the connection a guard opens for its setup, started while
                // the role has no slot and given one a moment later
                let waiting = tokio::spawn(async move { admin_connection(&slotless).await });
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let mut other = PgConnection::connect(&url).await.expect("connect");
                other
                    .execute(format!("alter role {role} connection limit -1").as_str())
                    .await
                    .expect("free a slot");
                let _ = other.close().await;
                let conn = waiting.await.expect("connects once a slot frees");
                let _ = conn.close().await;
            }
        })
        .await;

        let _ = admin
            .execute(format!("drop role if exists {role}").as_str())
            .await;
        let _ = admin.close().await;
        if let Err(err) = checks {
            std::panic::resume_unwind(err.into_panic());
        }
    }

    #[tokio::test]
    async fn a_test_pool_is_built_with_the_test_budget() {
        let Some(url) = database_url().await else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let guard = TestSchema::create(&url).await;
        assert_eq!(
            guard.pool().options().get_max_connections(),
            test_pool_config().max_connections
        );
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
        let Some(url) = database_url_with_cleanup().await else {
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
        let Some(url) = database_url_with_cleanup().await else {
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
        let Some(url) = database_url_with_cleanup().await else {
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
        let Some(url) = database_url_with_cleanup().await else {
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
