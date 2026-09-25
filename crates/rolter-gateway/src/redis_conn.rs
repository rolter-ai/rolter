//! A shared Redis connection that re-establishes itself after it is lost.
//!
//! The budget enforcer, the rate limiter and the response cache each keep one
//! multiplexed connection to Redis. They used to hold it in a `OnceCell`, which
//! meant the first connection was the only one: once Redis closed it — a
//! restart, a failover, `CLIENT KILL`, an idle timeout on a proxy in between —
//! every later command failed against the dead handle, the consumer failed open
//! for the rest of the process lifetime, and spend and token recording stopped,
//! even though Redis itself was healthy again (#1483).
//!
//! [`ReconnectingRedis`] keeps the connection in an `ArcSwapOption` instead, so
//! the connected path is a lock-free load. Every command goes through a
//! [`Lease`], and a connection-level failure (I/O, broken pipe, response
//! timeout) evicts the connection it happened on. The next caller to find the
//! slot empty reconnects straight away: losing a connection that worked is not
//! a failed attempt, so it waits for nothing.
//!
//! A read-only command (`GET`, `MGET`, `LRANGE`, …) that failed that way is
//! replayed once on a fresh connection, so the first admission check after a
//! drop is still enforced rather than failed open. A write is never replayed:
//! the client cannot tell "never sent" from "sent, reply lost", and replaying
//! an `INCRBYFLOAT` on the second would charge the same spend twice. A write
//! that meets a dead connection is dropped like any write during an outage, and
//! the next one uses the new connection. A caller whose write is safe to
//! repeat opts in with [`Lease::replay_writes`]; rate-limit admission does,
//! and documents why.
//!
//! Connection attempts are single-flight — callers arriving during one wait for
//! it rather than opening their own — and consecutive failures back off
//! exponentially up to [`ReconnectPolicy::max_backoff`]. While a backoff window
//! is open no attempt is made at all: callers get `None` at once and the
//! consumer fails open, which is the documented behaviour for a genuine Redis
//! outage. So an outage costs each consumer one connection attempt per backoff
//! window, never one per request.
//!
//! The library's own disconnect notification would let an idle drop be noticed
//! before the next command, but it is only delivered on RESP3 connections, and
//! rolter connects with the protocol the url asks for (RESP2 by default) so
//! that it keeps working behind proxies and servers that never learned `HELLO
//! 3`. Replaying reads covers the same ground without that requirement.
//!
//! Each instance keeps [`RedisConnStats`] for `/metrics` (#1772): whether it
//! holds a live connection, how often it reconnected and how often it failed
//! to, and — counted by the consumer — how many requests an admission check
//! let through unchecked. All of it is updated on the slow path only; a
//! command over a live connection touches none of it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use redis::aio::{ConnectionLike, MultiplexedConnection};
use redis::{
    AsyncConnectionConfig, Cmd, ErrorKind, Pipeline, RedisError, RedisFuture, RedisResult,
    ServerErrorKind, Value,
};

use crate::metrics::RedisConnStats;

/// How a [`ReconnectingRedis`] connects and how it backs off between failed
/// attempts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReconnectPolicy {
    /// Wait after the first failed attempt; doubles with each further failure.
    pub(crate) initial_backoff: Duration,
    /// Ceiling on the wait between attempts during a sustained outage.
    pub(crate) max_backoff: Duration,
    /// Upper bound on one connection attempt, so a blackholed address stalls
    /// the callers waiting on it for this long at most.
    pub(crate) connect_timeout: Duration,
    /// Upper bound on one command, so a half-open socket fails and is replaced
    /// instead of stalling every request that uses it.
    pub(crate) response_timeout: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(5),
            // both match the redis crate's own defaults; spelled out so a
            // dependency bump cannot silently change the bound on the hot path
            connect_timeout: Duration::from_secs(1),
            response_timeout: Duration::from_millis(500),
        }
    }
}

impl ReconnectPolicy {
    /// The wait before the next attempt after `failures` consecutive failures.
    fn backoff(&self, failures: u32) -> Duration {
        // capped shift: past 2^16 the product is far beyond any sane ceiling
        let shift = failures.saturating_sub(1).min(16);
        self.initial_backoff
            .saturating_mul(1u32 << shift)
            .min(self.max_backoff)
    }
}

/// A Redis connection shared by clones, re-established after it is lost.
/// Cheap to clone.
#[derive(Clone)]
pub(crate) struct ReconnectingRedis {
    shared: Arc<Shared>,
}

struct Shared {
    client: redis::Client,
    /// which consumer owns the connection, for logs
    consumer: &'static str,
    policy: ReconnectPolicy,
    live: ArcSwapOption<Live>,
    // an async mutex held across the connection attempt is fine here: it is
    // only reached when there is no live connection, never on the connected
    // path, and it is what makes the attempt single-flight
    connecting: tokio::sync::Mutex<()>,
    backoff: parking_lot::Mutex<Backoff>,
    /// incremented per attempt; tags each connection so a failure reported
    /// against an old one can never evict its replacement
    generation: AtomicU64,
    ever_connected: AtomicBool,
    attempts: AtomicU64,
    /// what `/metrics` reports for this consumer's connection
    stats: Arc<RedisConnStats>,
}

struct Live {
    conn: MultiplexedConnection,
    generation: u64,
}

#[derive(Default)]
struct Backoff {
    failures: u32,
    retry_at: Option<Instant>,
}

impl ReconnectingRedis {
    /// Parse `redis_url` without connecting; the first use connects.
    pub(crate) fn new(redis_url: &str, consumer: &'static str) -> RedisResult<Self> {
        Self::with_policy(redis_url, consumer, ReconnectPolicy::default())
    }

    /// [`new`](Self::new) with an explicit [`ReconnectPolicy`].
    pub(crate) fn with_policy(
        redis_url: &str,
        consumer: &'static str,
        policy: ReconnectPolicy,
    ) -> RedisResult<Self> {
        let client = redis::Client::open(redis_url)?;
        Ok(Self {
            shared: Arc::new(Shared {
                client,
                consumer,
                policy,
                live: ArcSwapOption::empty(),
                connecting: tokio::sync::Mutex::new(()),
                backoff: parking_lot::Mutex::new(Backoff::default()),
                generation: AtomicU64::new(0),
                ever_connected: AtomicBool::new(false),
                attempts: AtomicU64::new(0),
                stats: Arc::default(),
            }),
        })
    }

    /// The connection state this instance reports, for registering with
    /// [`Metrics::watch_redis`](crate::metrics::Metrics::watch_redis) and for
    /// counting the requests its consumer admits without it.
    pub(crate) fn stats(&self) -> &Arc<RedisConnStats> {
        &self.shared.stats
    }

    /// Connect in the background now instead of on first use, so the state
    /// reported for this consumer is known from startup rather than from its
    /// first request (#1772). A failed attempt opens a backoff window like any
    /// other, and the next use tries again. Does nothing outside a runtime.
    pub(crate) fn warm_up(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let redis = self.clone();
        runtime.spawn(async move {
            let _ = redis.get().await;
        });
    }

    /// A handle on the live connection, connecting first when there is none.
    ///
    /// `None` means Redis is unavailable right now — the attempt failed, or a
    /// backoff window after earlier failures is still open — and the caller
    /// should fail open.
    pub(crate) async fn get(&self) -> Option<Lease<'_>> {
        let (conn, generation) = self.shared.acquire().await?;
        Some(Lease {
            conn,
            generation,
            shared: &self.shared,
            replay_writes: false,
        })
    }

    /// Connection attempts made so far, successful or not.
    #[cfg(test)]
    pub(crate) fn attempts(&self) -> u64 {
        self.shared.attempts.load(Relaxed)
    }
}

impl Shared {
    fn current(&self) -> Option<(MultiplexedConnection, u64)> {
        self.live
            .load()
            .as_ref()
            .map(|live| (live.conn.clone(), live.generation))
    }

    async fn acquire(&self) -> Option<(MultiplexedConnection, u64)> {
        match self.current() {
            Some(current) => Some(current),
            None => self.connect().await,
        }
    }

    async fn connect(&self) -> Option<(MultiplexedConnection, u64)> {
        if self.backing_off() {
            return None;
        }
        let _flight = self.connecting.lock().await;
        // whoever held the lock before this caller may have connected, or
        // failed and opened a backoff window; either way it is settled
        if let Some(current) = self.current() {
            return Some(current);
        }
        if self.backing_off() {
            return None;
        }

        self.attempts.fetch_add(1, Relaxed);
        let generation = self.generation.fetch_add(1, Relaxed) + 1;
        let config = AsyncConnectionConfig::new()
            .set_connection_timeout(Some(self.policy.connect_timeout))
            .set_response_timeout(Some(self.policy.response_timeout));
        match self
            .client
            .get_multiplexed_async_connection_with_config(&config)
            .await
        {
            Ok(conn) => {
                *self.backoff.lock() = Backoff::default();
                self.live.store(Some(Arc::new(Live {
                    conn: conn.clone(),
                    generation,
                })));
                let reconnect = self.ever_connected.swap(true, Relaxed);
                self.stats.on_connect(reconnect);
                if reconnect {
                    tracing::info!(
                        consumer = self.consumer,
                        "redis connection re-established; enforcement resumed"
                    );
                }
                Some((conn, generation))
            }
            Err(error) => {
                self.stats.on_connect_failure();
                let retry_in = self.record_failure();
                tracing::warn!(
                    %error,
                    consumer = self.consumer,
                    retry_in_ms = retry_in.as_millis() as u64,
                    "redis unavailable; failing open until it is reachable again"
                );
                None
            }
        }
    }

    fn backing_off(&self) -> bool {
        self.backoff
            .lock()
            .retry_at
            .is_some_and(|at| Instant::now() < at)
    }

    /// Count a failed attempt and open the next backoff window.
    fn record_failure(&self) -> Duration {
        let mut backoff = self.backoff.lock();
        backoff.failures = backoff.failures.saturating_add(1);
        let wait = self.policy.backoff(backoff.failures);
        backoff.retry_at = Some(Instant::now() + wait);
        wait
    }

    /// Drop connection `generation` from the slot, if it is still the one
    /// there, so the next caller reconnects.
    fn invalidate(&self, generation: u64, cause: &RedisError) {
        let current = self.live.load();
        let Some(live) = current.as_ref() else {
            return;
        };
        if live.generation != generation {
            return;
        }
        let previous = self.live.compare_and_swap(&*current, None);
        let evicted = previous
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(previous, live));
        if evicted {
            self.stats.on_lost();
            tracing::warn!(
                cause = %cause,
                consumer = self.consumer,
                "redis connection lost; reconnecting on next use"
            );
        }
    }
}

/// Whether `err` means the connection itself is unusable, as opposed to Redis
/// rejecting one command (a wrong type, an out-of-memory refusal), after which
/// the connection is fine.
fn is_connection_error(err: &RedisError) -> bool {
    err.is_io_error()
        || err.is_connection_dropped()
        || err.is_timeout()
        || err.is_unrecoverable_error()
        // a primary demoted by a failover answers writes with READONLY for as
        // long as the connection lives; a fresh one follows the address to the
        // new primary
        || err.kind() == ErrorKind::Server(ServerErrorKind::ReadOnly)
}

/// Commands that change nothing, so running one twice is the same as once.
/// Deliberately an allowlist: a command missing from it is merely not
/// replayed, while a write wrongly on it would be applied twice.
const READ_ONLY: &[&[u8]] = &[
    b"GET", b"MGET", b"LRANGE", b"EXISTS", b"TTL", b"PTTL", b"PING",
];

fn is_read_only(cmd: &Cmd) -> bool {
    match cmd.args_iter().next() {
        Some(redis::Arg::Simple(name)) => {
            READ_ONLY.iter().any(|read| read.eq_ignore_ascii_case(name))
        }
        _ => false,
    }
}

/// One use of the live connection. Implements [`ConnectionLike`], so it is
/// passed to `query_async` and the `AsyncCommands` methods exactly like a
/// `MultiplexedConnection`; see the module docs for what it does on failure.
pub(crate) struct Lease<'a> {
    conn: MultiplexedConnection,
    generation: u64,
    shared: &'a Shared,
    replay_writes: bool,
}

impl Lease<'_> {
    /// Replay any command through this lease once on a fresh connection when
    /// the first attempt met a dead one, not only the read-only ones.
    ///
    /// Only for a caller that has worked out what a double application costs
    /// — a command that did run before its reply was lost runs again — and
    /// decided it is the lesser failure. Spend recording must never use it.
    pub(crate) fn replay_writes(&mut self) {
        self.replay_writes = true;
    }

    fn replayable(&self, cmd: &Cmd) -> bool {
        self.replay_writes || is_read_only(cmd)
    }

    /// Evict this lease's connection if `result` shows it dead; reports
    /// whether it did.
    fn evict_if_dead<T>(&self, result: &RedisResult<T>) -> bool {
        match result {
            Err(err) if is_connection_error(err) => {
                self.shared.invalidate(self.generation, err);
                true
            }
            _ => false,
        }
    }

    /// Move this lease onto a fresh connection; `false` when none can be had.
    async fn renew(&mut self) -> bool {
        match self.shared.acquire().await {
            Some((conn, generation)) => {
                self.conn = conn;
                self.generation = generation;
                true
            }
            None => false,
        }
    }
}

impl ConnectionLike for Lease<'_> {
    fn req_packed_command<'b>(&'b mut self, cmd: &'b Cmd) -> RedisFuture<'b, Value> {
        Box::pin(async move {
            let result = self.conn.req_packed_command(cmd).await;
            if !self.evict_if_dead(&result) || !self.replayable(cmd) || !self.renew().await {
                return result;
            }
            let replayed = self.conn.req_packed_command(cmd).await;
            self.evict_if_dead(&replayed);
            replayed
        })
    }

    fn req_packed_commands<'b>(
        &'b mut self,
        pipeline: &'b Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'b, Vec<Value>> {
        Box::pin(async move {
            let result = self.conn.req_packed_commands(pipeline, offset, count).await;
            if !self.evict_if_dead(&result)
                || !pipeline.cmd_iter().all(|cmd| self.replayable(cmd))
                || !self.renew().await
            {
                return result;
            }
            let replayed = self.conn.req_packed_commands(pipeline, offset, count).await;
            self.evict_if_dead(&replayed);
            replayed
        })
    }

    fn get_db(&self) -> i64 {
        self.conn.get_db()
    }
}

/// Helpers for the tests that need a real Redis.
///
/// They run only when `ROLTER_TEST_REDIS_URL` names a server (for example
/// `redis://127.0.0.1:6379`) and pass as a logged skip otherwise, like the
/// Postgres and ClickHouse suites. Each test picks its own logical database,
/// which is what lets [`kill_clients_on_db`] drop one test's connections
/// without touching another's while they run in parallel.
#[cfg(test)]
pub(crate) mod testing {
    use std::net::SocketAddr;

    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::{JoinHandle, JoinSet};

    /// The env var naming the test server.
    pub(crate) const URL_ENV: &str = "ROLTER_TEST_REDIS_URL";

    /// Logical databases, one per test, so connection kills stay local.
    pub(crate) mod db {
        pub(crate) const PROXY_OUTAGE: u8 = 10;
        pub(crate) const SINGLE_FLIGHT: u8 = 11;
        pub(crate) const BUDGETS: u8 = 12;
        pub(crate) const RATE_LIMITS: u8 = 13;
        pub(crate) const CACHE: u8 = 14;
        /// no test kills connections here, so racing admissions stay intact
        pub(crate) const RATE_ADMISSION: u8 = 15;
    }

    /// The server url with any database path removed, or `None` (and a skip
    /// notice) when no test server is configured.
    fn server_url() -> Option<String> {
        match std::env::var(URL_ENV) {
            Ok(url) if !url.trim().is_empty() => {
                let url = url.trim().trim_end_matches('/');
                // drop a trailing `/<db>` so each test can pick its own
                let scheme_end = url.find("://").map_or(0, |i| i + 3);
                let base = match url[scheme_end..].find('/') {
                    Some(slash) => &url[..scheme_end + slash],
                    None => url,
                };
                Some(base.to_string())
            }
            _ => {
                eprintln!("skipping: {URL_ENV} is unset");
                None
            }
        }
    }

    /// The test server url selecting logical database `db`.
    pub(crate) fn url(db: u8) -> Option<String> {
        server_url().map(|base| format!("{base}/{db}"))
    }

    /// A key no other run can share, so persisted counters from an earlier run
    /// never leak into this one.
    pub(crate) fn unique(prefix: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("{prefix}-{}-{nanos}", std::process::id())
    }

    /// An admin connection on database 0, which no test selects.
    pub(crate) async fn admin() -> redis::aio::MultiplexedConnection {
        let base = server_url().expect("only called once a test url is known");
        redis::Client::open(format!("{base}/0"))
            .expect("a valid test url")
            .get_multiplexed_async_connection()
            .await
            .expect("the test redis answers")
    }

    /// Close every client connection that has database `db` selected, the way
    /// a Redis restart or an idle-timeout on a proxy would. Returns how many
    /// were closed. No data is touched.
    pub(crate) async fn kill_clients_on_db(db: u8) -> usize {
        let mut admin = admin().await;
        let list: String = redis::cmd("CLIENT")
            .arg("LIST")
            .query_async(&mut admin)
            .await
            .expect("CLIENT LIST");
        let wanted = format!("db={db}");
        let mut killed = 0;
        for line in list.lines() {
            let mut fields = line.split_whitespace();
            if !line.split_whitespace().any(|f| f == wanted) {
                continue;
            }
            let Some(id) = fields.find_map(|f| f.strip_prefix("id=")) else {
                continue;
            };
            let n: i64 = redis::cmd("CLIENT")
                .arg("KILL")
                .arg("ID")
                .arg(id)
                .query_async(&mut admin)
                .await
                .expect("CLIENT KILL ID");
            killed += n as usize;
        }
        let pong: String = redis::cmd("PING")
            .query_async(&mut admin)
            .await
            .expect("redis stays up across the kill");
        assert_eq!(pong, "PONG");
        killed
    }

    /// A TCP forwarder in front of the test Redis that can be taken down and
    /// brought back on the same port — an outage and a restart, as far as the
    /// gateway can tell, without touching the shared server.
    pub(crate) struct Outage {
        addr: SocketAddr,
        upstream: SocketAddr,
        task: Option<JoinHandle<()>>,
    }

    impl Outage {
        /// Start forwarding; returns the forwarder and a url through it for
        /// database `db`, or `None` when no test server is configured.
        pub(crate) async fn start(db: u8) -> Option<(Self, String)> {
            let base = server_url()?;
            let authority = base.split_once("://").map_or(base.as_str(), |(_, a)| a);
            let (credentials, host_port) = match authority.rsplit_once('@') {
                Some((credentials, host_port)) => (format!("{credentials}@"), host_port),
                None => (String::new(), authority),
            };
            // `localhost` may resolve to `::1` first while the server only
            // listens on IPv4 (a CI service port), so forward to the first
            // address that actually accepts a connection
            let mut upstream = None;
            for candidate in tokio::net::lookup_host(host_port)
                .await
                .expect("the test redis host resolves")
            {
                if TcpStream::connect(candidate).await.is_ok() {
                    upstream = Some(candidate);
                    break;
                }
            }
            let upstream = upstream.expect("the test redis accepts a connection");
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("local addr");
            let mut outage = Self {
                addr,
                upstream,
                task: None,
            };
            outage.task = Some(tokio::spawn(forward(listener, upstream)));
            Some((outage, format!("redis://{credentials}{addr}/{db}")))
        }

        /// Stop listening and cut every forwarded connection.
        pub(crate) async fn down(&mut self) {
            if let Some(task) = self.task.take() {
                task.abort();
                let _ = task.await;
            }
        }

        /// Listen again on the same port.
        pub(crate) async fn up(&mut self) {
            let listener = TcpListener::bind(self.addr).await.expect("rebind");
            self.task = Some(tokio::spawn(forward(listener, self.upstream)));
        }
    }

    impl Drop for Outage {
        fn drop(&mut self) {
            if let Some(task) = self.task.take() {
                task.abort();
            }
        }
    }

    async fn forward(listener: TcpListener, upstream: SocketAddr) {
        // owning the connection tasks here means aborting this one drops them
        // all, so `down` closes the established sockets too
        let mut connections = JoinSet::new();
        while let Ok((mut inbound, _)) = listener.accept().await {
            connections.spawn(async move {
                if let Ok(mut outbound) = TcpStream::connect(upstream).await {
                    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{self, db, Outage};
    use super::*;
    use redis::AsyncCommands;

    fn fast() -> ReconnectPolicy {
        ReconnectPolicy {
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_millis(200),
            ..ReconnectPolicy::default()
        }
    }

    /// `(connected, reconnects, connect failures)` as `/metrics` renders them.
    fn reported(redis: &ReconnectingRedis) -> (bool, u64, u64) {
        let metrics = crate::metrics::Metrics::default();
        metrics.watch_redis(
            crate::metrics::RedisConsumer::Budgets,
            redis.stats().clone(),
        );
        let out = metrics.render();
        let sample = |name: &str| -> u64 {
            let prefix = format!("{name}{{consumer=\"budgets\"}} ");
            out.lines()
                .find_map(|line| line.strip_prefix(&prefix)?.parse().ok())
                .unwrap_or_else(|| panic!("no {name} in:\n{out}"))
        };
        (
            sample("rolter_redis_connected") == 1,
            sample("rolter_redis_reconnects_total"),
            sample("rolter_redis_connect_failures_total"),
        )
    }

    /// An address nothing listens on: bound once to learn a free port, then
    /// released, so a connection attempt is refused at once.
    async fn closed_url() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("redis://{addr}")
    }

    #[test]
    fn backoff_doubles_from_the_initial_wait_up_to_the_ceiling() {
        let policy = ReconnectPolicy::default();
        assert_eq!(policy.backoff(1), Duration::from_millis(100));
        assert_eq!(policy.backoff(2), Duration::from_millis(200));
        assert_eq!(policy.backoff(3), Duration::from_millis(400));
        assert_eq!(policy.backoff(7), Duration::from_secs(5));
        assert_eq!(policy.backoff(u32::MAX), Duration::from_secs(5));
    }

    #[test]
    fn only_connection_failures_evict_the_connection() {
        use std::io;
        for kind in [
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::TimedOut,
        ] {
            assert!(
                is_connection_error(&RedisError::from(io::Error::from(kind))),
                "{kind:?} means the socket is gone"
            );
        }
        // redis refusing one command leaves the connection perfectly usable
        let rejected = RedisError::from((
            ErrorKind::Server(ServerErrorKind::ResponseError),
            "WRONGTYPE",
        ));
        assert!(!is_connection_error(&rejected));
        let mistyped = RedisError::from((ErrorKind::UnexpectedReturnType, "not a string"));
        assert!(!is_connection_error(&mistyped));
        // a demoted primary is the one server answer that does need a new one
        let demoted = RedisError::from((ErrorKind::Server(ServerErrorKind::ReadOnly), "READONLY"));
        assert!(is_connection_error(&demoted));
    }

    /// The fail-open half of the contract: during a real outage a caller gets
    /// `None` straight away, and the backoff window turns a burst of requests
    /// into one connection attempt rather than one per request.
    #[tokio::test]
    async fn an_outage_fails_open_with_one_attempt_per_backoff_window() {
        let redis = ReconnectingRedis::with_policy(
            &closed_url().await,
            "test",
            ReconnectPolicy {
                initial_backoff: Duration::from_secs(60),
                ..ReconnectPolicy::default()
            },
        )
        .unwrap();
        let started = Instant::now();
        for _ in 0..500 {
            assert!(redis.get().await.is_none());
        }
        assert_eq!(redis.attempts(), 1, "the window must absorb the burst");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "failing open must not wait: {:?}",
            started.elapsed()
        );
        // and `/metrics` counts the attempt, not the burst (#1772)
        let (connected, _, failures) = reported(&redis);
        assert!(!connected);
        assert_eq!(failures, 1);
    }

    /// Once the window closes the next caller tries again, and each further
    /// failure widens the window.
    #[tokio::test]
    async fn a_closed_window_allows_exactly_one_more_attempt() {
        let redis = ReconnectingRedis::with_policy(&closed_url().await, "test", fast()).unwrap();
        assert!(redis.get().await.is_none());
        assert_eq!(redis.attempts(), 1);
        tokio::time::sleep(Duration::from_millis(60)).await;
        for _ in 0..50 {
            assert!(redis.get().await.is_none());
        }
        assert_eq!(redis.attempts(), 2);
        assert_eq!(redis.shared.backoff.lock().failures, 2);
    }

    /// A cold start under concurrent load opens one connection, not one per
    /// caller, and every caller that waited on it is served by it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_callers_share_a_single_connection_attempt() {
        let Some(url) = testing::url(db::SINGLE_FLIGHT) else {
            return;
        };
        let redis = ReconnectingRedis::new(&url, "test").unwrap();
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..64 {
            let redis = redis.clone();
            tasks.spawn(async move { redis.get().await.is_some() });
        }
        while let Some(served) = tasks.join_next().await {
            assert!(served.unwrap(), "a healthy redis serves every waiter");
        }
        assert_eq!(redis.attempts(), 1);
    }

    #[test]
    fn only_idempotent_commands_are_replayable() {
        for read in ["GET", "mget", "LRANGE", "PING"] {
            assert!(is_read_only(&redis::cmd(read)), "{read}");
        }
        for write in [
            "INCRBYFLOAT",
            "INCR",
            "SET",
            "SETEX",
            "EXPIRE",
            "LPUSH",
            "EVAL",
        ] {
            assert!(!is_read_only(&redis::cmd(write)), "{write}");
        }
        let mut reads = redis::pipe();
        reads.get("a").get("b");
        assert!(reads.cmd_iter().all(is_read_only));
        let mut mixed = redis::pipe();
        mixed.get("a").incr("b", 1);
        assert!(!mixed.cmd_iter().all(is_read_only));
    }

    /// A read that meets a dead connection is replayed on a fresh one and
    /// answers; a write is not replayed, so it is applied at most once.
    #[tokio::test]
    async fn a_dead_connection_replays_reads_but_never_writes() {
        let Some((mut outage, url)) = Outage::start(db::PROXY_OUTAGE).await else {
            return;
        };
        let redis = ReconnectingRedis::with_policy(&url, "test", fast()).unwrap();
        let key = testing::unique("rolter:test:replay");
        let mut conn = redis.get().await.expect("connected through the forwarder");
        let _: () = conn.set_ex(&key, "7", 60).await.unwrap();

        // cut the socket under the lease and let redis be reachable again
        outage.down().await;
        outage.up().await;
        let read: Option<String> = conn.get(&key).await.expect("the read is replayed");
        assert_eq!(read.as_deref(), Some("7"));
        assert_eq!(redis.attempts(), 2, "one reconnect, for the replay");

        outage.down().await;
        outage.up().await;
        let write: RedisResult<i64> = conn.incr(&key, 1).await;
        assert!(
            write.is_err(),
            "a write on a dead connection is not replayed"
        );
        let mut conn = redis.get().await.expect("reconnects on next use");
        let after: i64 = conn.incr(&key, 1).await.unwrap();
        assert_eq!(
            after, 8,
            "the failed write was applied zero times, not twice"
        );
        let _: () = conn.del(&key).await.unwrap();
    }

    /// The whole of #1483 at the connection level: an outage fails open, and
    /// when Redis is reachable again the same instance recovers and still sees
    /// the data written before the outage.
    #[tokio::test]
    async fn an_outage_and_a_restart_recover_the_same_instance() {
        let Some((mut outage, url)) = Outage::start(db::PROXY_OUTAGE).await else {
            return;
        };
        let redis = ReconnectingRedis::with_policy(&url, "test", fast()).unwrap();
        let key = testing::unique("rolter:test:outage");
        {
            let mut conn = redis.get().await.expect("connected through the forwarder");
            let _: () = conn.set_ex(&key, "before", 60).await.unwrap();
        }
        assert_eq!(reported(&redis), (true, 0, 0));

        outage.down().await;
        {
            // the first command finds the connection dead, and the replay
            // finds redis unreachable
            let mut conn = redis
                .get()
                .await
                .expect("the dead connection is still held");
            let during: RedisResult<Option<String>> = conn.get(&key).await;
            assert!(during.is_err());
        }
        let started = Instant::now();
        for _ in 0..100 {
            assert!(redis.get().await.is_none(), "an outage fails open");
        }
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "failing open must not wait: {:?}",
            started.elapsed()
        );
        let attempts_during_outage = redis.attempts();
        // the outage shows on `/metrics` while it lasts (#1772)
        let (connected, reconnects, failures) = reported(&redis);
        assert!(!connected, "the lost connection is reported");
        assert_eq!(reconnects, 0);
        assert!(failures >= 1, "the failed attempts are counted");

        outage.up().await;
        // at most one backoff window (200ms here) until the next attempt
        let mut conn = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(conn) = redis.get().await {
                    break conn;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("recovers once redis is reachable again");
        let value: Option<String> = conn.get(&key).await.unwrap();
        assert_eq!(value.as_deref(), Some("before"));
        assert!(
            redis.attempts() - attempts_during_outage <= 10,
            "reconnects are paced by the backoff, not by the retry loop"
        );
        let (connected, reconnects, _) = reported(&redis);
        assert!(connected, "and so is the recovery");
        assert_eq!(reconnects, 1);
        let _: () = conn.del(&key).await.unwrap();
    }
}
