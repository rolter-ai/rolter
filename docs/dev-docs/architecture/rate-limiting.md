# Rate limiting

`[[rate_limits]]` caps a scope (org, team, project, key, business unit or
customer) to `rpm` requests and/or `tpm` tokens per minute. The gateway enforces
the caps in `crates/rolter-gateway/src/rate_limits.rs`, using counters in Redis
that every replica shares. This page covers two separate things:

- the **approximation**: how the counters estimate "the last 60 seconds"
- the **concurrency guarantee**: why concurrent requests can't all take the same
  free slot

They are independent. The estimate can be off at the edges of a bucket while
admission is still exact under concurrency.

## The window: a sliding-window counter

Each limit keeps one counter per fixed one-minute bucket (`unix_seconds / 60`)
and per kind:

```
rolter:rl:<scope>:<id>:<req|tok>:<bucket>
```

A check reads the current bucket and the previous one, and weights the previous
one by how much of it still lies inside the trailing 60 seconds:

```
estimate = current + previous × (60 − seconds_into_current_bucket) / 60
```

On a bucket boundary the previous bucket counts in full. Half way through it
counts for half. This is an approximation: it assumes the previous bucket's
requests were spread evenly across its minute.

- **Traffic bunched at the end of the previous minute** is under-weighted. For
  example, suppose 60 requests arrived in its last second. Thirty seconds into
  the next minute they count as 30, although all 60 are still inside the
  trailing window.
- **Traffic bunched at the start** is over-weighted by the same logic.

The error is bounded by one bucket's traffic and disappears as traffic evens
out. In exchange, a check costs two reads per limit and no per-request data.
The smoothing is still better than a plain fixed window, which would allow
2 × `rpm` across a bucket edge.

`Retry-After` on a 429 is the number of seconds left in the current bucket. That
is when the weighting next changes in the caller's favour.

Buckets expire two minutes after their last write, so idle limits clean
themselves up.

## Admission is one atomic step

A request is admitted only if **every** applicable limit passes:

- `estimate + 1 ≤ rpm`
- `estimate < tpm` (explained [below](#tokens-are-reactive))

The limits are checked in scope-chain order and the first refusal wins
(most-restrictive-wins). An admitted request is charged to every applicable
`rpm` bucket. A refused one is charged to none, so a request that the key's
limit rejects costs the org nothing.

The whole decision runs as one Lua script (`ADMIT` in `rate_limits.rs`) on the
Redis server. The script reads the buckets, evaluates every limit and increments
the request buckets. Redis runs a script to completion before it serves any
other command, so no request can read a counter between another request's read
and its increment. Every replica talks to the same Redis, so this holds across
replicas too.

Before #1484 the gateway ran the same logic as three client-side steps: `MGET`,
decide, then an `INCR` pipeline. Requests arriving together all read the same
counts before any of them wrote, so 32 concurrent requests against `rpm = 1`
were all admitted. Multiplexing and extra replicas made that more likely, not
less.

The script is loaded once per server with `EVALSHA`. After a Redis restart the
script cache is empty, so the first call gets `NOSCRIPT`, loads the script and
retries. The redis crate's `Script::invoke_async` handles that.

The script touches keys belonging to several scopes, so it needs a single Redis
node (or a primary with replicas). It cannot run on Redis Cluster, where those
keys would hash to different slots. The gateway connects with a single-node
client in any case.

## Tokens are reactive

Token usage is only known once the response has finished. So `tpm` is read in
the same atomic step (the decision sees one consistent snapshot), but it is only
**charged afterwards**, by `record_tokens`.

A request is refused once the trailing window has already reached the cap.
Requests admitted together before that point can overshoot the cap by their own
usage. This is the documented reactive behaviour of `tpm`, not a race: the
tokens do not exist yet at admission time, so no atomic step can charge them.
Reserving an estimate up front is a separate feature (#1464).

## When the Redis connection drops

Admission is the one write that opts out of the no-replay rule in
[Redis connections](redis-connections.md#which-commands-are-replayed). If the
script call meets a dead connection, it is replayed once on a fresh one, so the
first request after a drop is still enforced.

The replay is exact when the first attempt never reached Redis, which is the
usual case: the connection was already dead. If the script did run and only its
reply was lost, the replay charges one extra request to a bucket that expires
within two minutes, and may refuse a request that the lost reply had admitted.
That errs by one towards _stricter_, for one window. The alternative is to admit
the request unchecked and uncounted, and for a throughput control that is the
worse failure.

Spend recording stays on the no-replay side. A double-applied `INCRBYFLOAT`
would corrupt a budget for its whole period.

While Redis is unreachable, admission fails open, like every other Redis-backed
control.

## Tests

Tests against a real Redis live in `rate_limits.rs`. They self-skip without
`ROLTER_TEST_REDIS_URL`; see
[Testing](../development/testing.md#the-redis-test-server).

- 64 concurrent requests against `rpm = 1` admit exactly one.
- Three limiters with separate connections, standing in for three replicas,
  race 96 requests against `rpm = 5`. Exactly 5 are admitted and exactly 5 are
  counted.
- The org/key scope chain under concurrency: the key's cap wins, and the org is
  charged only for what the key admitted.
- Window boundaries, with the clock pinned through `check_at`: the previous
  bucket counts in full on the boundary and for half at mid-minute.
- A full `tpm` window refuses and charges no request slot.
- Recovery after `CLIENT KILL`: the first check after the drop is enforced
  through the replayed script.
