# Redis connections on the data plane

Three gateway components keep state in Redis: the budget enforcer
(`crates/rolter-gateway/src/budgets.rs`), the rate limiter (`rate_limits.rs`) and
the response cache (`cache.rs`). Each holds one multiplexed connection through
`ReconnectingRedis` in `crates/rolter-gateway/src/redis_conn.rs`. This page
covers what happens to that connection when Redis goes away and comes back.

## The contract

- **During an outage, fail open.** Redis unreachable means requests pass
  unchecked, spend and tokens go unrecorded, and every cache lookup misses. A
  counter-store outage never takes the data plane down.
- **After an outage, recover without a restart.** Once Redis is reachable
  again, the enforcers and the cache that already exist resume on their own. A
  gateway restart or a snapshot update is not needed. Counters that Redis kept
  (it persists them, or it never went down and only dropped connections) are
  enforced again straight away.

Before #1483 only the first half held. The connection sat in a `OnceCell`, so
the first connection was the only one the process would ever make. After Redis
closed it (a restart, a failover, `CLIENT KILL`, an idle timeout on a proxy in
between), every later command failed against the dead handle. Budgets and rate
limits then stayed failed open, and recording stayed off, until the gateway
restarted. That was true even though Redis itself was healthy again.

## How a lost connection is replaced

The live connection sits in an `ArcSwapOption`. On the connected path, taking
it is a lock-free load plus a clone of the `MultiplexedConnection` handle,
which is the same cost the `OnceCell` had.

Commands go through a `Lease`, which implements `redis::aio::ConnectionLike`,
so call sites pass it to `query_async` and the `AsyncCommands` methods as they
did before. When a command fails with a connection-level error, the lease
evicts its connection from the slot. Connection-level errors are:

- I/O errors, a broken pipe, or a reset
- a response timeout
- an error the client marks unrecoverable
- `READONLY` from a primary that a failover demoted

A server rejecting one command, such as `WRONGTYPE`, leaves the connection in
place. Each connection carries a generation number, so a failure reported late
against an old connection can never evict its replacement.

The next caller that finds the slot empty reconnects straight away. Losing a
connection that worked does not count as a failed attempt, so no backoff
applies to it.

## Which commands are replayed

A command that fails because its connection died is replayed on a fresh
connection only if it is read-only. The allowlist is `GET`, `MGET`, `LRANGE`,
`EXISTS`, `TTL`, `PTTL` and `PING`. A pipeline is replayed only when every
command in it is on that list.

The budget and cache admission reads are on that list: the budget `MGET` and
the cache `GET`/`LRANGE`/`MGET`. So the first request after a drop is still
checked against the counters, rather than failed open.

Writes are not replayed by default. The client library reports "never sent" and "sent,
reply lost" as the same `BrokenPipe`, and replaying an `INCRBYFLOAT` in the
second case would charge one request's spend twice. A write that meets a dead
connection is lost, the same way any write is lost during an outage. The
write after it uses the new connection. In practice one request's spend or
token count per consumer can go unrecorded at the moment a connection drops.

A caller can opt a lease into replaying writes with `Lease::replay_writes`, but
only after it has worked out what a double application costs. Rate-limit
admission is the one caller that does. It is a single atomic script that both
checks and charges, and replaying it can at worst over-count one request in a
bucket that expires within two minutes. That is stricter, never looser. The
trade-off is argued in [Rate limiting](rate-limiting.md#when-the-redis-connection-drops).

The redis crate can report a closed socket before the next command is sent,
through a synthesized `Disconnection` push. Rolter does not use it, because the
push is only delivered on RESP3 connections. Rolter uses whatever protocol the
url asks for (RESP2 by default), so it keeps working behind proxies and servers
that do not speak `HELLO 3`.

## Bounded reconnects during a real outage

Connection attempts are **single-flight**. Callers that arrive while an attempt
is in progress wait on an async mutex for that attempt instead of opening their
own. The mutex is only taken when there is no live connection, so the connected
path never touches it.

Consecutive failed attempts back off exponentially: 100 ms after the first,
doubling each time, up to a ceiling of 5 s. While a backoff window is open, no
attempt is made at all. Callers get "unavailable" at once and fail open without
waiting. Each consumer therefore makes at most one connection attempt per
backoff window during an outage, however much traffic the gateway is serving,
and a replica makes three in total (budgets, rate limits, cache).

Two timeouts bound how long a caller can wait:

| Timeout          | Value  | What it bounds                                                                                                                    |
| ---------------- | ------ | --------------------------------------------------------------------------------------------------------------------------------- |
| connect timeout  | 1 s    | the callers that wait on one connection attempt, including one against a blackholed address                                       |
| response timeout | 500 ms | one command; a half-open socket fails at this point and its connection is evicted, instead of stalling every request that uses it |

These match the redis crate's own defaults. `ReconnectPolicy` sets them
explicitly so that a dependency bump cannot change them without anyone
noticing.

## What operators see

- On the first failure: `redis connection lost; reconnecting on next use`
  (warn), with the cause.
- During an outage: `redis unavailable; failing open until it is reachable again`
  (warn), with `retry_in_ms`. This is logged once per attempt, so at most once
  per backoff window for each consumer, never once per request.
- On recovery: `redis connection re-established; enforcement resumed` (info).

Each line carries `consumer` = `budgets`, `rate limits` or `response cache`.

## Tests

The contract is tested against a real Redis. The tests self-skip unless
`ROLTER_TEST_REDIS_URL` is set; see
[Testing](../development/testing.md#the-redis-test-server).

- `budgets`, `rate_limits` and `cache` each have a test that closes the
  component's own connection with `CLIENT KILL`. The test then asserts that the
  _same_ instance enforces against the counters that survived the kill, and
  that it records through the new connection.
- `redis_conn` puts a TCP forwarder in front of Redis and takes it down and
  back up. Those tests cover an outage followed by a restart, the fact that
  reads are replayed and writes are not, and single-flight connection attempts.
- Tests that point at a closed port cover backoff pacing and the fail-open
  latency.
