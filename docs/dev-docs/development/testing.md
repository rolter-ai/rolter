# Testing

## Run

Tests run under [nextest](https://nexte.st/) (the same runner CI uses), plus a
separate doc-test pass since nextest does not run doc tests:

```bash
cargo nextest run --workspace   # unit + integration tests
cargo test --doc --workspace    # doc tests
cd ui && bun run lint           # ui typecheck: src, scripts, .storybook, e2e
```

Install the runner once with `cargo install cargo-nextest` (or see the
[nextest install docs](https://nexte.st/docs/installation/)). `just test` runs
both Rust passes for you. Plain `cargo test --workspace` still works if you
haven't installed nextest, but CI runs nextest so prefer it locally.

The Ollama Cloud live smoke sends a billed request and is ignored by default:

```bash
OLLAMA_API_KEY=... ROLTER_OLLAMA_LIVE_MODEL=gpt-oss:20b \
  cargo test -p rolter-gateway --test ollama_cloud live_smoke -- --ignored
```

The Gemini Interactions smoke is gated the same way. It exists because Google
publishes no full JSON schema for the interactions wire format, so parts of the
adapter — the multimodal part field names and some `step.delta` variants —
were inferred from prose docs and only a real request confirms them (#764):

```bash
GEMINI_API_KEY=... ROLTER_GEMINI_LIVE_MODEL=gemini-3.6-flash \
  cargo test -p rolter-gateway --test gemini_interactions_live -- --ignored
```

Both run in CI only from dispatch-gated workflows, never the per-PR gate:
`quality.yml` takes no secrets by design (#734) so dependabot and fork PRs pass
exactly the same checks. Assertions in the live suites carry the upstream
response body in their failure message — with an inferred field name, the
provider's complaint _is_ the finding, and a bare status-code assertion would
throw it away.

### Configuring the Gemini smoke

`gemini-interactions-smoke.yml` needs one secret before it can verify anything:

1. Create a `live-providers` repository environment (Settings → Environments).
   Keeping the key there rather than at repository scope means a run against a
   billed provider is reviewable, not something any workflow can reach for.
2. Add `GEMINI_API_KEY` to that environment.
3. Dispatch the workflow (`gh workflow run gemini-interactions-smoke.yml`),
   optionally with `-f model=<id>`.

Until the secret exists the workflow **skips** every live step and says so: the
job summary reads "skipped: GEMINI_API_KEY not configured" and a notice
annotation repeats it on the run. The run is not red (a weekly failure for a
secret nobody has added teaches people to ignore scheduled failures, #2033), but
it is also not evidence: a skipped run made no request and confirms nothing
about the wire format. Check the summary, not just the tick, before treating the
sweep as having run. Once the secret is present the steps run as before and a
failure is a real finding.

Each run records the wire shapes it observed into the job summary and uploads
the full log as an artifact. A billable run should leave evidence behind: the
next question about a field name is then answered by reading the last run
rather than by spending another call.

What the suite covers, and why each probe exists:

| Probe                           | Confirms                                                                                 |
| ------------------------------- | ---------------------------------------------------------------------------------------- |
| text turn                       | turn mapping, `system_instruction`, `generation_config`, usage — all documented          |
| inline image part               | the inferred `mime_type`/`data` inline part shape                                        |
| remote image part               | the inferred `file_uri` shape — the _other_ branch, which the inline probe never reaches |
| tool call round trip            | `function_call` out, `function_result` back, and `call_id` correlation                   |
| interaction threading           | the id rolter surfaces as the response `id` is the one Google accepts back               |
| every client dialect            | Chat Completions, Messages and Responses have separate response translators              |
| every client dialect, streaming | the inferred `step.delta` variants, through all three separate SSE emitters              |

A content part the dialect cannot carry is rejected at the gateway with
`400 unsupported_content_part` rather than being dropped (#882), so an
unconfirmed part shape fails loudly instead of producing a shortened body.

Test grouping is configured in [`.config/nextest.toml`](../../../.config/nextest.toml):
the Postgres-backed `rolter-store`/`rolter-control` suites share one database and
reset the schema per test, so they run in a single-threaded group to avoid
clobbering each other.

## The Postgres test database

The Postgres-backed tests self-skip unless `ROLTER_TEST_DATABASE_URL` points at
a Postgres they may write to. A machine needs one such server however many
worktrees it has, and `just test-pg` is the way to get it:

```bash
eval "$(just test-pg)"   # starts rolter-test-pg on 127.0.0.1:55433 if needed, exports the url
cargo nextest run -p rolter-store -p rolter-control --features postgres
```

The recipe is idempotent: it creates the `rolter-test-pg` container the first
time, starts it again after a reboot, and otherwise only prints the export line,
so every session on the machine runs the same command and lands on the same
server. `just test-pg-status` shows the connections in use against
`max_connections` and which worktree each test database belongs to;
`just test-pg-down` removes the container together with its data.
`ROLTER_TEST_PG_PORT` and `ROLTER_TEST_PG_MAX_CONNECTIONS` pick a different port
or limit, and only take effect when the container is created, so change them
with a `just test-pg-down` first.

Do not start a Postgres per worktree. Each worktree already gets a database of
its own on the shared server (below), so a second container isolates nothing,
and nothing ever takes it down: it outlives the worktree that started it, holding
a port, shared memory and disk, and the next session has to step over it to find
a free port (#1736). Containers left over from that habit (`rolter-test-pg-<n>`,
`rolter-<n>-pg` and similar) can go once nothing uses them.

Any other Postgres works too, as long as the role may create databases
(`CREATEDB`), since each worktree gets one of its own; without it the tests still
run, they just share the database the url names.

### One database per worktree

`ROLTER_TEST_DATABASE_URL` names the **server**, not the database the tests end
up writing to. The database is derived from the workspace the test binary was
compiled in — `rolter_test_wt_<worktree>_<digest>` — and created on first use by
[`rolter_store::postgres::test_database`](../../../crates/rolter-store/src/postgres/test_database.rs).
A new worktree is therefore isolated without exporting anything, which is the
point: this repository expects several agents working several worktrees at once
(see [worktrees.md](worktrees.md)), so concurrent suites against one database is
the normal case rather than an edge case, and an isolation scheme that has to be
opted into is forgotten exactly when it is needed (#1430).

Two things that used to be true stop being true:

- a suite in another worktree no longer creates and drops `test_*` schemas
  underneath the one you are measuring — the 396 foreign schemas that appeared
  mid-session while #1364 was being verified
- a worktree on an older commit no longer applies its migration set to the
  database another worktree is reading

The worktree's path is stored as the database's comment, and both ways a
database is reclaimed go through it. Removing the worktree with `wt remove` runs
the `pre-remove` hook in [`.config/wt.toml`](../../../.config/wt.toml), which drops
the databases carrying that worktree's path on the `just test-pg` server
straight away. Independently of any hook, the next test run in any worktree drops
every `rolter_test_wt_*` database whose recorded directory no longer exists, so a
worktree removed some other way, or one whose tests ran against a different
server, still takes its database with it. List them with `just test-pg-status`,
`\l rolter_test_wt*`, or:

```sql
select datname, shobj_description(oid, 'pg_database')
from pg_database where datname like 'rolter_test_wt%';
```

Set `ROLTER_TEST_PER_WORKTREE_DATABASE=0` to use `ROLTER_TEST_DATABASE_URL`
exactly as given — a throwaway database that is already private, or a deliberate
reproduction of the shared-database behaviour. The derivation never falls back
silently (#1898): a failure to create the database is retried with a bounded
backoff, then `test_database::url()` panics naming the cause, because a quiet
fallback would put one worktree's migrations in a database other worktrees are
reading. A role without `CREATEDB` should set the opt-out above.

### The connection budget

A database per worktree isolates schemas and migrations, but not the server's
`max_connections`: every suite on the machine, from every worktree, draws from
that one budget (#1735). When it runs out, Postgres refuses new connections with
`sorry, too many clients already` and the refusal lands on whichever test
happened to be connecting.

Measured with the `rolter-store` and `rolter-control` postgres suites run the way
plain `cargo test` runs them (every test in a binary at once, one thread per
logical CPU by default), each simulated worktree on a database of its own, and
client backends sampled every 100 ms:

| Worktrees × test threads | Pool per test | `max_connections` | Peak client backends | Outcome                                         |
| ------------------------ | ------------- | ----------------- | -------------------- | ----------------------------------------------- |
| 1 × 8                    | 10            | 100               | 14                   | green                                           |
| 6 × 8                    | 10            | 100               | 78                   | green                                           |
| 4 × 24                   | 10            | 100               | 100, the limit       | 978 refused connections, 79 of 560 tests failed |
| 4 × 24                   | 10            | 300               | 120                  | green but one flake of the sweep test (#1910)   |
| 4 × 24                   | 4             | 300               | 114                  | green                                           |
| 1 × 24                   | 1             | 300               | 27                   | 3 tests fail on the pool: they need 2           |

A test holds about one connection at a time, so a worktree costs roughly one
connection per test thread plus a handful for the harness. The pool size barely
moves the peak; the number of tests running at once does. That is why the stock
limit of 100 holds six worktrees on an 8-thread laptop and fails four on a
24-thread workstation. Under `cargo nextest` the `serial-db` group in
[`.config/nextest.toml`](../../../.config/nextest.toml) runs one postgres test at a
time per worktree, so a worktree holds only a few connections there; the numbers
above are the case for `cargo test`, and for nextest too if that group goes
(#1429).

So the budget is kept in two places:

- `just test-pg` starts the server with `max_connections = 300`: room for about
  ten worktrees at 24 threads, and twenty at 8. A slot nothing is connected to
  costs a little shared memory, not a backend, and the larger lock table that
  comes with it (`max_locks_per_transaction` is per slot) also gives the schema
  drops more room. On a server of your own, size `max_connections` the same way:
  worktrees × (test threads + 5)
- `TestSchema` builds each test's pool with at most 4 connections instead of the
  control plane's 10: twice what the hungriest tests use (a KEK rotation holding
  a transaction while it queries, the readiness probe), and a test that wants
  more waits for a free connection instead of opening one. It is a ceiling on
  the worst case rather than the fix. `ROLTER_TEST_POOL_MAX_CONNECTIONS`
  overrides it

A test that cannot connect says which budget ran out, since the pool on its
own does not: it retries `too many clients` until its 30-second acquire timeout
and then reports a bare `pool timed out while waiting for an open connection`,
which reads like a slow query or a regression. `TestSchema` connects directly
for its own setup. It waits out a server at its limit for the same 30 seconds,
with backoff, since a slot usually frees within milliseconds as another test's
guard finishes its cleanup, and only then panics with the server's answer (`has
no connection slots left (SQLSTATE 53300 ...)`). A server that is down is
reported at once instead, as `nothing is accepting connections`. A test that fails while the server is at or within a tenth of its limit
gets the same note printed after its panic, and one that fails with its own pool
fully checked out is told that instead. `just test-pg-status` shows how many
connections are in use; `cargo test -- --test-threads=<n>` lowers a worktree's
share when the server cannot be changed.

### One schema per test

Each test gets a schema of its own, named `test_<pid>_<seq>` and pinned through
`search_path`, because plain `cargo test` — which the coverage job runs — puts
every test in one process as a thread, and a shared `public` schema would race
on DDL. Build it through
[`rolter_store::postgres::test_schema::TestSchema`](../../../crates/rolter-store/src/postgres/test_schema.rs)
rather than by hand; other crates reach it through the store's `test-support`
feature, which `rolter-control` already carries as a dev-dependency.

**Hold the guard for the whole test.** The schema is dropped when `TestSchema`
is, so a binding let go early takes the tables with it:

```rust
let db = TestSchema::migrated(&url).await;   // guard lives to the end of the test
let pool = db.pool().clone();
```

Cleanup runs from `Drop`, which also runs while a panicking test unwinds, so a
failing test reclaims its schema too. Two cases still leave residue, and both
are handled by a sweep the first `TestSchema` in a process performs:

- a process killed hard — SIGKILL, a `nextest` timeout, a laptop losing power —
  never runs `Drop` at all
- runs predating this mechanism (before #1364) never dropped anything

The sweep drops every `test_<pid>_<seq>` schema whose pid is not a live
process, in batches: each schema carries the full migration set, and dropping
thousands in one transaction runs the lock table out of shared memory. It is
deliberately one-sided — a schema whose pid _is_ live is always kept, so a suite
running concurrently in another process can never lose its schema, and a pid the
operating system has recycled only defers a drop to a later run.

So the database needs occasional attention rather than none: a crash-heavy
afternoon can leave schemas behind until the next run reclaims them, and a
database that has not been used for tests since #1364 landed still carries
whatever earlier runs orphaned. Check what is there with:

```sql
select count(*) from information_schema.schemata where schema_name like 'test\_%';
```

Schemas from the older helpers used `seed_*` and `export_*` names with no pid in
them, so the sweep cannot prove they are dead and leaves them alone. Nothing
creates those names any more, so drop them once by hand — in batches, for the
same lock-table reason:

```sql
do $$
declare victim text;
begin
  loop
    select schema_name::text into victim
    from information_schema.schemata
    where schema_name like 'seed\_%' or schema_name like 'export\_%'
    limit 1;
    exit when victim is null;
    execute 'drop schema ' || quote_ident(victim) || ' cascade';
    commit;
  end loop;
end $$;
```

One schema per statement is deliberate. A migrated schema holds around 210
relations and `cascade` locks every one of them, so even ten schemas in a
single transaction exhausts the lock table — the `out of shared memory` the
issue describes is reachable at far fewer schemas than it sounds.

When a failing test's rows _are_ the evidence, set `ROLTER_TEST_KEEP_SCHEMA=1`:
the guard then keeps every schema it creates (printing each name) and skips the
sweep, so nothing is reclaimed until you drop it yourself.

### What keeps all of this honest

Three rules hold the isolation together, and
[`crates/rolter-store/tests/db_test_isolation.rs`](../../../crates/rolter-store/tests/db_test_isolation.rs)
fails the build when a new test breaks one — the same shape of source-level
drift guard as the gateway's `lock_discipline.rs`:

- the database comes from `test_database::url()`, never from
  `ROLTER_TEST_DATABASE_URL` read directly
- the schema comes from `TestSchema`, never from a hand-rolled `create schema`
- no test in these suites installs a process-wide environment value only it
  wants — the one exception is the shared `TEST_KEK` every call site in
  `control_integration.rs` installs, which exists precisely so the race has
  nothing to observe (#1351)

That last rule is what `cargo test -p rolter-control -p rolter-store --features
postgres` used to break (#1444): the environment is process-wide and the
coverage job runs every test in a binary as a thread, so a value one test wanted
was read by another test's in-flight request. Pass it into the app under test
instead — `test_app_with_public_url` is the model.

## The Redis test server

The tests that exercise the gateway's Redis connections — budgets, rate limits,
the response cache and the reconnecting connection they share — self-skip
unless `ROLTER_TEST_REDIS_URL` names a Redis they may use. A throwaway container
on a free port is enough:

```bash
docker run -d --rm --name rolter-test-redis -p 127.0.0.1:6390:6379 redis:8-alpine
ROLTER_TEST_REDIS_URL=redis://127.0.0.1:6390 cargo nextest run -p rolter-gateway
docker rm -f rolter-test-redis
```

The url names the **server**; any database path on it is ignored. Each test
selects a logical database of its own (the `db` constants in
`crates/rolter-gateway/src/redis_conn.rs`), because the reconnect tests close
connections with `CLIENT KILL` and must only close their own: the kill targets
the clients that have that test's database selected, so parallel tests — and
the nextest process-per-test model — never cut each other's connections. Keys
carry a per-run suffix and are deleted afterwards, and nothing is flushed, so a
shared development Redis is safe to point at. A new Redis-backed test picks the
next unused number there; there are 16 databases by default.

The outage-and-restart tests do not touch the server at all: they put a TCP
forwarder (`testing::Outage`) in front of it and take that down and back up.

The `nextest / doctests` and coverage jobs in `quality.yml` run a Redis service
and set the variable, so these tests run in CI rather than passing as silent
no-ops.

## Layout

- **Unit tests** live next to the code in `#[cfg(test)] mod tests`. Current coverage: balancer strategies (round-robin cycling, consistent-hash stability, cache-aware affinity, empty targets), the prefix trie, config parsing, model rewrite, auth checks, and the in-memory store.
- Keep the pure crates (`rolter-core`, `rolter-balancer`, `rolter-auth`) fully unit-testable without I/O.

## Strategy as the project grows

- **Integration tests** for the gateway: spin up the Axum app with a mock upstream (`wiremock`/`httpmock`) and assert routing, auth, model rewrite, error mapping and streaming passthrough.
- **Property tests** (`proptest`) for the balancer: distribution fairness, affinity invariants.
- **DB tests** for `rolter-store` Postgres backend behind a feature, using a disposable container.
- **Load tests** (`oha`/`k6`) against a mock upstream to track added latency and max RPS (see [performance.md](../architecture/performance.md)).

## Chaos & resilience contracts

Resilience is asserted in two places, split by what each harness can drive
deterministically.

The **e2e chaos suite** (`integration/e2e/tests/test_chaos.py`, compose `chaos`
profile) drives a static-config gateway against mock upstreams whose failure mode
is fixed by an env var. It covers what is observable purely over the wire: retry
and failover on 5xx/429, a clean 5xx when every target is down, the request
timeout bound on a slow upstream, the circuit breaker's OPEN transition (asserted
via `rolter_breaker_opened_total`) and flap degrade/recovery.

The **gateway chaos tests** (`crates/rolter-gateway/tests/chaos.rs`) cover the two
contracts that need a request pinned at a known point inside the gateway, which a
black-box harness can only approximate with sleeps:

- **bounded-queue backpressure** — with `queue.capacity = 1`, `queue.workers = 1`
  and `backpressure = "error"`, one request pins the sole worker, exactly one
  surplus request takes the queue slot, and the rest must come back as
  `429` with `error.code = "queue_full"` while
  `rolter_provider_queue_rejections_total` advances. Memory is bounded by the
  queue, not by client burst size.
- **graceful SIGTERM drain** — a real `rolter-gateway` child process is sent
  `SIGTERM` while a request is pinned upstream. The in-flight request must still
  return `200`, new connections must be refused, and the process must exit `0`.
- **sink flush on SIGTERM** — with `flush_ms` set to an hour, a finished request's
  request-log and health-event rows must still reach a ClickHouse stand-in
  before the child exits, proving the shutdown sink drain (#1924).

All three use a mock upstream that blocks on a semaphore the test owns, so every step
is driven by a signal rather than by elapsed time — there are no sleeps to race.
Run them with:

```bash
cargo test -p rolter-gateway --test chaos
```

## Benchmarks

Hot-path micro-benchmarks run under [criterion](https://github.com/criterion-rs/criterion.rs). They live in `crates/<crate>/benches/` with a `[[bench]] harness = false` entry per file, and cover the per-request cost that shows up as pure gateway overhead:

```bash
just bench                       # cargo bench --workspace
cargo bench -p rolter-balancer   # just the balancer benches
cargo bench -p rolter-balancer --bench pick   # one bench target
```

Current coverage:

`rolter-balancer`

- `pick` — `LoadBalancer::pick` for every built-in strategy over a ~24-target pool with a populated `RouteContext`.
- `trie` — prefix-trie `insert` (bounded/unbounded, so LRU eviction is measured) and `longest_prefix` on a warm trie.

`rolter-core`

- `snapshot` — the CPU side of config-snapshot generation at 10/100/1000 routes: `sanitize_for_snapshot`, `validate`, and the JSON encode. `/internal/snapshot` is polled by every gateway in the fleet, so this cost is paid fleet-wide on every poll. The encode dominates — ~2.8 ms at 1000 routes against ~150 µs for sanitize — which is why payload size is its own metric (#845).

`rolter-gateway`

- `admission` — the two registries every upstream attempt consults before anything else: `Breaker::allows` and `Cooldowns::is_parked`, plus the outcome-recording calls beside them. Covers the healthy steady state (no entries recorded), a warm fleet, a tripped/parked target and a 64-model fleet, so the cost is measured where a real gateway actually sits rather than only in the worst case (#1050).

criterion writes HTML reports to `target/criterion/`. Benches are **not** run in CI (timings are noisy on shared runners), but `cargo clippy --workspace --all-targets -- -D warnings` compiles them on every PR, so they cannot silently bit-rot. Use `just bench-check` (`cargo bench --workspace --no-run`) to compile them locally without running.

## Coverage

Workspace line coverage is measured with
[`cargo llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov):

```bash
cargo install cargo-llvm-cov
cargo llvm-cov --workspace --all-features --summary-only   # quick %
cargo llvm-cov --workspace --all-features --html           # browsable report
```

### The coverage job runs a different runner

Everything else runs under nextest, which gives each test **its own process**.
`cargo llvm-cov` shells out to plain `cargo test`, so the coverage job runs the
whole suite as **threads in one process** sharing one environment. Two rules
follow, and both have bitten:

When the coverage job goes red, each failing test is named in an `::error`
annotation on the check run ("coverage test failed"), with its panic location
and message (`.github/scripts/annotate-test-failures.sh`, #2753). Read those
rather than the raw job log, which not every triage path can download.

- **Never set a process-wide environment variable to a value only your test
  wants.** `Kek::from_env()` is read at request time, so a test that installs
  its own `ROLTER_KEK` is read by another test's in-flight request, and a value
  sealed under one key then fails to open under the next. The symptom lands on
  whichever unrelated seal-then-open test was mid-flight, never on the test that
  caused it. `control_integration.rs` installs one shared `TEST_KEK` for exactly
  this reason (#1351); a test needing a non-matching key builds it with
  `Kek::from_secret` rather than through the environment.
- **Postgres tests must use a per-test schema** (`search_path`), since this job
  shares one database across concurrently running tests.

A comment saying "runs in its own process (nextest)" is true of every job except
this one, which is what makes the trap easy to walk into.

CI runs coverage in the `coverage` job of `quality.yml` on every pull request
(and on no other event), and again nightly on `master` in
[`extended.yml`](#nightly-extended-checks), whose run also saves the Rust cache
the PR job restores under the shared key `coverage`. Both enforce a
**ratcheting baseline**: the committed baseline lives in
[`.github/coverage-baseline.txt`](../../../.github/coverage-baseline.txt), and
[`.github/scripts/coverage-ratchet.sh`](../../../.github/scripts/coverage-ratchet.sh)
fails the step if the current percentage drops more than
`COVERAGE_TOLERANCE` points (default `0.5`) below it. The job also uploads the
`lcov.info` report as a CI artifact.

Policy (ROL-246):

- New code must not push coverage below `baseline − tolerance`. If a PR
  legitimately lowers coverage, edit `.github/coverage-baseline.txt` in the same
  PR and explain why.
- When coverage climbs well above the baseline, raise the baseline to lock in
  the gain (the ratchet only goes up).
- The PR job is **informational** (`continue-on-error: true`) until the
  baseline is trusted; promote it to blocking by removing that flag on the
  `coverage` job in `quality.yml`. The nightly copy carries no such flag, so a
  ratchet failure on `master` opens the tracking issue described below.

## CI

`.github/workflows/ci.yml` delegates to the shared `quality.yml` gate, which runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo nextest run --workspace --all-features` plus a `cargo test --doc` pass, the feature matrix (`cargo hack`), `cargo doc` (warnings as errors), the publish verify build, the gateway smoke, cargo-deny, gitleaks, the zizmor workflow audit, and the UI lint/build on every push and PR. `ci.yml`'s `ci-ok` job then checks the pull request itself: the title is one valid Conventional Commit line, and neither the body nor, on a dispatched or merge-queue run, the commit range carries a coding-agent session url (see [the `ci-ok` gate](ci-gating.md#what-runs-inside-ci-ok)).

### The static checks job

The checks that read the tree and build nothing run as steps of one job,
`static checks` (`static` in `quality.yml`): gitleaks over the working tree and
the branch history, the session-url check over the PR's commits, migrations
append-only, the dev-docs link check, typos, taplo, cargo-deny, unused deps, actionlint, zizmor, the
release handoff checker, its self-test and the release gate scripts' fixture
test, the board automation retry policy, and the helm chart's appVersion check,
lint and its renders (`scripts/check-helm-chart.sh`, shared with the `helm-render` prek hook). Until #2025 each was a job of its own. They did 0-15 s
of work apiece and then waited a median 86-200 s for a runner, since every job
a push starts draws on the same 20 concurrent slots. The decision and its
trade-offs are in
[the CI runner budget ADR](../adr/2026-09-29-ci-runner-budget.md).

The merge changed how a failure looks, not what can fail:

- Every step runs under `!cancelled()`, so a typo and a taplo drift in the same
  push fail as two steps, each named after the job it used to be. A step keeps
  its old job's event guard: the branch-history gitleaks pass runs on pull
  requests and merge-queue runs, the session-url commit check on pull requests
  only.
- A step that reads a tool the job installs (taplo, cargo-deny and cargo-machete
  from `taiki-e/install-action`, uv, helm) is guarded on that install as well,
  so a failed download shows up as a skipped check rather than as
  `command not found` under the check's name.
- The gitleaks steps run before any setup step, so the working-tree scan never
  sees a tool unpacked into the workspace.
- The last step, `report`, writes every step's outcome to the job summary and
  emits one error annotation per failed step, titled with the former job's name
  and carrying the command that reproduces it locally, usually the prek hook of
  the same name (`prek run typos --all-files`). It also fails the job when a
  step that must run on the event was skipped. A skipped step reads as a pass,
  and that is how `ci-ok` once went green without reading a commit message
  (#1562).
- The job times out after 20 minutes. It does about 90 s of work, and a hung
  step would otherwise hold every other check's verdict for GitHub's 360-minute
  default.

The report is the one place that knows which step must run on which event. A
check added to the job therefore needs three things: its step, an `OUTCOME_*`
line in the report's `env`, and a `row` call in the report's script. A step
without a row runs unreported, and a row whose step id is misspelled reads an
empty outcome, which the report counts as a failure.

The `dev-docs links` step runs `scripts/check-dev-docs-links.py` (also the
`dev-docs-links` prek hook). It fails on any relative link in a `.md` file under
`docs/dev-docs/` that does not resolve to an existing file or directory, with
the `#anchor` stripped. mdBook only validates links inside the book, so a link to
a repository file written with one `../` too few used to point at nothing. From
`docs/dev-docs/<section>/` the repository root is `../../../`; from
`docs/dev-docs/` itself it is `../../`.

### The rust lint and rust build jobs

The Rust checks outside nextest run as steps of two jobs, split by whether they
link. `rust lint` holds fmt, clippy (default features and `postgres`),
`cargo doc` with warnings as errors, `cargo hack` over each feature and the
cross-crate feature combination. None of them invokes the linker, so the job
skips the wild linker. `rust build` holds the publish verify build
(`cargo package` plus `maturin sdist`), the gateway smoke build and probe, and
last the three advisory `semver-checks` steps. Until #2025 these were six jobs:
`fmt / clippy`, `feature matrix`, `cargo doc (warnings = errors)`,
`package (publish verify)`, `gateway smoke (fake-llm)` and
`semver-checks (advisory)`.

They follow the rules of the static checks job above: every check step runs
under `!cancelled()` and is guarded on the setup it reads, and a `report` step
titles each failure with the former job's name and the command that reproduces
it. Every blocking step must run on every event, so a skip fails the job. Two
things differ:

- The flags that used to be job env are step env now. `RUSTDOCFLAGS` sits on
  the `cargo doc` step. `RUSTFLAGS=-D warnings` sits on the two `cargo hack`
  steps, which build into `target/hack`, because a different `RUSTFLAGS` would
  otherwise rebuild the clippy steps' dependency tree on every run.
  `rust build` sets no `RUSTFLAGS` at all: the variable replaces every
  `rustflags` entry in cargo's config, the wild linker's `--ld-path` included.
- The semver steps are `continue-on-error`, the install and the baseline
  lookup included, and the semver check has a 20-minute step limit. The report
  turns their failure into a warning. `ci-ok` never goes red over the semver
  check, as [ADR-0032](../adr/2026-09-09-one-point-oh-compatibility-guarantees.md)
  requires; see [API stability](api-stability.md).

`rust lint` times out after 30 minutes and `rust build` after 45, so a hung
build fails the gate well before GitHub's 360-minute default.

### The Rust cache is saved from `master` only

Every `Swatinem/rust-cache` step in `quality.yml`, `ci.yml`, `extended.yml` and
`engine-integration.yml` sets `save-if: ${{ github.ref == 'refs/heads/master' }}`. A pull request run
restores the cache its job last saved on `master` and writes nothing back, and
so do a release-PR dispatch, a merge-queue run and a `workflow_dispatch` on a
branch. Only the `master` push run and the nightly `extended.yml` run, which
GitHub starts on `master`, save.

A cache belongs to the ref that saved it. One saved on `master` is visible to
every pull request; one saved on `refs/pull/<n>/merge` is visible to that pull
request alone. Every PR and the release-PR branch used to save its own copy of
each Rust job's target directory, roughly 0.1-0.9 GB apiece. On 09-29 the cache
held 40 entries and about 14.4 GB against the repository's 10 GB limit, two
thirds of it on PR and release-PR refs, and GitHub evicts the least recently
used entries until the total fits, `master`'s included
([ADR-0034](../adr/2026-09-29-ci-runner-budget.md)).

The cost lands on the PR that changes `Cargo.lock` or the toolchain. A lockfile
change misses `master`'s exact key, so rust-cache restores the entry saved for
the old lockfile and the job rebuilds what changed, on every push until the PR
merges. A toolchain change alters the key's environment hash and starts cold.
The first `master` push after the merge saves the new cache. The
`nextest / doctests` job carries a 30-minute timeout with that in mind: warm
runs take 6-10 minutes, so a cold build fits inside the limit, and a hung test
no longer holds the gate for GitHub's 360-minute default.

A new `rust-cache` step takes the same `save-if` line. Only
`gemini-interactions-smoke.yml` goes without it: it runs on its weekly schedule,
which already runs on `master`.

`engine-integration.yml` runs only on path-filtered pull requests and on
dispatch, never on a `master` push, so a cache of its own would never warm. Its
smoke builds the same `cargo build -p rolter-gateway` as `rust build`, so it
restores that job's cache instead: both steps set `shared-key: rust-build`, and
the smoke installs the wild linker too, because the linker's rustflags are part
of every unit's fingerprint and a build without them would reuse nothing it
restored (#2203). Keep the two jobs' toolchain, linker and key in step. To see
what the cache holds:

```bash
gh api repos/rolter-ai/rolter/actions/cache/usage
gh cache list -R rolter-ai/rolter --sort size_in_bytes --limit 50
```

### The rustdoc gate is the one CI check nothing local reproduces

`cargo doc (warnings = errors)`, a step of `rust lint`, is the gate that most often turns a
locally-clean branch red, because `cargo fmt`, `cargo clippy` and
`cargo nextest` are all silent about it. Run it before pushing:

```bash
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
```

Keep `--all-features`: it is what CI runs, and the `postgres` modules are
feature-gated, so without it a broken link inside them (a doc comment still
naming a renamed function, say) passes locally and fails in CI.

The usual failure is `rustdoc::private_intra_doc_links`: a public item whose
doc comment links `[`Something`]` that is private. It is easy to write, because
explaining why a public type exists usually means naming the internals it
wraps — and it is invisible until CI says so. Either unlink it (a plain code
span reads the same) or make the target public if it deserves to be.

### UI dependencies and the lockfile

The `ui, storybook, docs` job installs with `bun install --frozen-lockfile`,
for everyone — dependabot included. That was not always true, and the reason it
is now is worth recording.

`ui/` is bun-managed, but dependabot could not speak bun, so the repository kept
a `package-lock.json` purely for dependabot's benefit and ran the `npm`
ecosystem against it. A bump moved `package.json` and `package-lock.json` and
left `bun.lock` untouched, which fails a frozen install — so the install
self-healed with `--no-frozen-lockfile` whenever the actor was dependabot.

That kept dependabot's own PR green without making its **merge** safe. The
moment such a bump landed, every other open PR installed frozen against the new
`package.json` and failed in eight seconds with `lockfile had changes, but
lockfile is frozen` — a red check on work that had touched no dependency, which
is exactly the kind of failure that trains people to re-run without reading
(#1137, reconciled by hand in #1086 and again in #1136).

Dependabot has spoken bun since bun 1.1.39, so `/ui` now runs as
`package-ecosystem: bun` and `package-lock.json` is gone. Dependabot writes
`bun.lock` itself, so a stale lockfile fails on the PR that caused it and never
reaches `master`. The trade is that the bun ecosystem does version updates but
not security updates, and GitHub's dependency graph reads `ui/package.json` but
not `bun.lock`. Alerts still fire for the dependencies `ui/package.json`
declares, and the pull request that fixes them comes from the workflow below
instead of from Dependabot. A package that only arrives transitively through
`bun.lock` raises no alert at all: every transitive `ui` alert was marked
`fixed` the moment `package-lock.json` was deleted, with no version having
changed. Nothing monitors that class until #1930 audits the lockfile itself, and
#1931 tracks the vulnerable transitive packages `bun audit` reports today.

### UI security updates

`.github/workflows/ui-security-updates.yml` stands in for the Dependabot
security updates the `bun` ecosystem lacks (#1148). GitHub's
[supported-ecosystems table](https://docs.github.com/en/code-security/dependabot/ecosystems-supported-by-dependabot/supported-ecosystems-and-repositories)
still marks bun as version updates only; once that changes, delete the workflow,
its script and the `.github/actionlint.yaml` entry.

It runs daily at 06:30 UTC and on demand. Each run:

1. lists the open Dependabot alerts with the repository `GITHUB_TOKEN`, under
   the `vulnerability-alerts: read` scope. A failed listing fails the run rather
   than reading as "no alerts";
2. hands them to `ui/scripts/security-updates.ts`, which raises each vulnerable
   direct dependency in `ui/package.json` to its first patched release and keeps
   the range operator (`^1.2.0` becomes `^1.2.7`);
3. refreshes `bun.lock` with `bun install --lockfile-only`;
4. rebuilds the `security/ui-dependencies` branch from `master` when the planned
   change differs from what the branch already carries, and opens or updates
   one pull request, titled
   `build(deps): raise vulnerable ui dependencies to patched releases`;
5. dispatches `ci.yml` against that branch after every push, since a pull
   request opened with the repository token triggers no `pull_request` run.
   This is the same path the release PR takes (see [ci-gating](ci-gating.md)).

The script only raises what it can raise safely. It judges each alert on its
own, raises a package to the highest first patched release among the alerts a
semver-compatible bump clears, and leaves the rest for a hand bump. Each one is
listed, with its reason, in the run summary and the pull request body:

- no patched release has been published yet;
- the package is not a direct dependency (the fix would be an `overrides` entry,
  which changes what every other dependent resolves);
- the range is not a plain `^`, `~` or exact version;
- the first patched release is a breaking bump from the current floor (a new
  major, or a new minor below 1.0). One such advisory does not hold back a
  compatible fix for another advisory on the same package;
- the range already starts at the patched release, so the alert is stale and
  closes on its own once GitHub re-reads the manifest.

A pull request
the repository token opens raises no event that `project-automation.yml` fires
on, so the workflow dispatches it with the new pull request's number and
`area=ui` right after `gh pr create`, which puts it on the board as
`In Review` (#2245). A failed dispatch only warns and names the command to run
by hand.

Each proposal body ends with a hidden marker naming the ranges it raises and
their targets. Closing the pull request unmerged declines that exact set of
bumps: while a closed, unmerged pull request for the branch carries the marker
of today's plan, the workflow does not open another. The decision reads pull
request state, never the branch, so an unrelated dependency change on `master`
does not reopen a declined bump, while a new alert or a different target does.

When nothing is left to raise, the workflow withdraws its own pull request. It
first rewrites the body to say why, with the table of anything still left for a
person and without the marker, so a withdrawal never counts as a person
declining, then closes the pull request and deletes the branch.

The run heals itself when a step fails half way:

- a push that landed before `gh pr create` failed leaves a branch with no pull
  request and no declined one, and the next run opens the pull request for it;
- when the branch already carries the planned change and `ci.yml` has no run for
  its head that is in progress or ended in success or failure, the workflow
  dispatches one. A cancelled or never-dispatched gate is re-run the next day;
  a failed one is left for a person to read and re-run.

To try a change to the workflow before it merges, dispatch it with `dry-run`
from the branch. A dry run writes the plan to the run summary and pushes
nothing. A run that publishes refuses to start anywhere but `master`.

```bash
gh workflow run ui-security-updates.yml --ref <branch> -f dry-run=true
```

The publish path only runs from `master`, and with no open alert it has nothing
to publish. The `synthetic-alert` input exercises it on purpose: it adds one
made-up alert (number `#0`, advisory `SYNTHETIC`, linking to the run) on a direct
dependency, patched at the version you name, which must be a real release above
the current floor and semver-compatible with it. After a change to the publish or dispatch steps
merges, run it once from `master`:

```bash
gh workflow run ui-security-updates.yml -f synthetic-alert=@opentelemetry/api@1.9.1
```

Then check that the pull request opened, that the dispatched
`ci.yml` run reported `ci-ok` on its head, and that the next run without the
input withdrew it and deleted the branch. Merge nothing from a synthetic run.

The planner runs locally against a saved alert listing:

```bash
gh api "repos/rolter-ai/rolter/dependabot/alerts?state=open&ecosystem=npm" > alerts.json
cd ui && bun scripts/security-updates.ts --alerts ../alerts.json   # plan only
```

actionlint has no entry for `vulnerability-alerts` yet
([rhysd/actionlint#713](https://github.com/rhysd/actionlint/issues/713)), so
`.github/actionlint.yaml` ignores that one message in that one file. Any other
permission typo in the workflow still fails the check.

CI pins actionlint to **1.7.12**: the `actionlint` step in `quality.yml` downloads that release's
tarball and checks it against a pinned sha256 before running it, so a new release that adds or
tightens a rule cannot turn every open PR red on its own. (`taiki-e/install-action` has no
actionlint manifest, which is why the step fetches it by hand.) Raising the version is a
deliberate PR that changes `ACTIONLINT_VERSION` and `ACTIONLINT_SHA256` together, takes the digest
from the release's `actionlint_<version>_checksums.txt`, and fixes whatever the newer rules
report; it is also the moment to drop the `vulnerability-alerts` ignore above if the new release
knows that scope. The `prek` hook runs whichever `actionlint` is on your `PATH`, so install the
pinned version locally when the two disagree.

### Secret scanning

The two gitleaks steps of the `static checks` job run the gitleaks **CLI** from
a digest-pinned container, not `gitleaks-action`. The action gates org-owned
repositories behind a license key, and license secrets are invisible to both
dependabot runs (a separate secret store) and fork PRs (no secrets at all), so
every such PR failed the check and with it `ci-ok`. The CLI is free and
unrestricted, so `quality.yml` now takes no secrets and behaves identically for
forks, dependabot and direct pushes.

Two passes run with the shared `.github/config/gitleaks.toml` policy: `gitleaks
dir` over the working tree (everything the commit ships) and, on PRs and
merge-queue runs, `gitleaks git --log-opts base..head` over the branch history.
That second pass catches a secret added and then removed inside the same PR; on
a queue run the range is every commit the queue is about to write to `master`
(see [ci-gating](ci-gating.md#what-runs-and-what-is-allowed-to-skip)). Both
passes run before any setup step, so the working-tree scan sees the checkout and
nothing a later step unpacked into it. The pinned digest is v8.30.1 — the
version `prek.toml` already uses for the staged-content hook, so local and CI
scans agree.

Reproduce a CI run locally:

```bash
docker run --rm -v "$PWD:/repo" -w /repo \
  ghcr.io/gitleaks/gitleaks@sha256:c00b6bd0aeb3071cbcb79009cb16a60dd9e0a7c60e2be9ab65d25e6bc8abbb7f \
  dir . --config .github/config/gitleaks.toml --redact --exit-code 1
```

### Workflow security (zizmor)

The `zizmor` step of the `static checks` job audits `.github/workflows/` and
`.github/actions/` for workflow security smells — unpinned or impostor action
refs, template injection into `run:`, over-broad `GITHUB_TOKEN` scopes, cache
poisoning, dangerous triggers.
It is a **merge gate** (#1456): a finding fails `quality`, which fails `ci-ok`.

It ran as informational (`continue-on-error: true`) until the baseline was
clean. That state is what the promotion is a reaction to: a regression to 34
findings went unnoticed precisely because nothing failed on it (#1325), and an
action pinned to an unreachable commit (#1227) was waved through as noise on
every PR for weeks. A check nobody has to fix is a check nobody reads.

The gate runs at `--min-severity=medium --persona=regular`, the setting the
baseline was proven clean against. Reproduce a CI run locally:

```bash
uvx zizmor@1.26.1 --min-severity=medium --persona=regular \
  .github/workflows/ .github/actions/
```

Some audits query the GitHub API (`impostor-commit`, `stale-action-refs`,
`known-vulnerable-actions`), so export a `GH_TOKEN` — or pass `--offline` to
skip them, which is enough for a quick check but is **not** what CI runs.

#### Retrying an API hiccup, but never a finding

Those online audits are also the gate's one source of flake. A single 401, 5xx
or rate-limit answer from the GitHub API aborts the **whole** run with `fatal:
no audit was performed`, so a PR that touched no workflow goes red for a reason
that has nothing to do with it. While the job was informational that was
invisible; now that it blocks, it is indistinguishable from a real finding. So
the step retries, up to three attempts with a short backoff (#1509).

What makes the retry safe is that zizmor's exit code already separates the two
cases, so the retry never has to infer which one it is:

| Exit      | Meaning                                         | Gate behaviour                                             |
| --------- | ----------------------------------------------- | ---------------------------------------------------------- |
| `0`       | audit completed, nothing to report              | pass, first attempt                                        |
| `11`–`14` | audit completed, findings at informational…high | **fail, first attempt**                                    |
| `1`       | no audit was produced                           | retry, but only when the output names a GitHub API failure |
| `2`, `3`  | bad arguments / no inputs collected             | fail, first attempt                                        |

A completed run is a verdict, and a verdict is final immediately — a genuine
finding can never be retried away because it never reaches the retryable branch.
Only exit `1` is a candidate, and then only when the output matches `no audit
was performed`, `request error while accessing GitHub API` or `couldn't list
tags`; an internal zizmor bug fails fast instead of costing three runs.
Exhausting the three attempts is a failure, never a silent pass — the same rule
`ci-ok` applies to its own run listing: no answer is not an answer.

#### Why `known-vulnerable-actions` still gates every PR

`known-vulnerable-actions` checks pinned actions against zizmor's advisory
database, so alone among the audits its verdict can change while the repository
does not: a newly published advisory against an action we pin turns every open
PR red at once, for something none of those PRs did. That is the same shape as a
fresh RUSTSEC advisory, which is exactly why `cargo audit` runs on a schedule
from `audit.yml` rather than on each PR.

It stays in the blocking PR gate anyway, and deliberately. zizmor has no flag
that disables a single audit — the only granularity is `--no-online-audits`,
which would also drop `impostor-commit` and `stale-action-refs`, and those two
are precisely the audits that catch a bad action ref _introduced by the diff in
front of you_. Trading away the audits that read the current diff to pre-empt an
advisory that has not fired yet costs more than it saves, and the remedy in the
noisy case is to bump the pin — the change we would want to make regardless, on
whichever PR notices first. Revisit this if zizmor gains per-audit selection or
if the advisory case starts actually costing PR time; a scheduled `zizmor
--min-severity=medium` job in `audit.yml`, beside `cargo audit`, is the shape
that split would take.

Fix a finding rather than silencing it. Where a finding is genuinely a
false positive for this repository, suppress that one rule on that one step with
`# zizmor: ignore[<rule>]` and put the reason in a comment directly above it —
a bare suppression is indistinguishable from the noise this gate exists to stop.
The four suppressions in the tree today are:

| Where                                             | Rule                 | Why                                                                                                                                                                                                |
| ------------------------------------------------- | -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `engine-integration.yml` — `Swatinem/rust-cache`  | `cache-poisoning`    | nothing this workflow builds is published, so the cache cannot poison a release                                                                                                                    |
| `release-plz.yml` — both `actions/checkout` steps | `artipacked`         | release-plz pushes the release branch and the tags with the persisted token, so `persist-credentials` must stay on                                                                                 |
| `project-automation.yml` — `pull_request_target`  | `dangerous-triggers` | required so fork PRs can read the org PAT; the workflow never checks out PR head and passes only the project id, the item's node id and url, and literal field names to `run:`, all through `env:` |

### Storybook play tests

The `ui, storybook, docs` job builds the static Storybook, serves it, and runs the
interaction (play) tests with `@storybook/test-runner` against a headless
chromium. It is a **merge gate** (#753): a failing play test fails `quality`,
which fails `ci-ok`. Locally:

```bash
cd ui
bunx playwright install --with-deps chromium chromium-headless-shell
bun run test:stories                            # every story file
bun run test:stories src/pages/Keys.stories.tsx # or just these
```

#### Using a pre-installed chromium

Both browser runners — the story tests above and the e2e journeys in `ui/e2e/`
(`ui/playwright.config.ts`) — launch the chromium revision the pinned Playwright
downloads. In a sandbox where that download is blocked but a chromium is already
installed, set `ROLTER_CHROMIUM_PATH` to the binary and both launch it through
`launchOptions.executablePath` instead (#2678):

```bash
export ROLTER_CHROMIUM_PATH=/opt/pw-browsers/chromium_headless_shell-1194/chrome-linux/headless_shell
bun run test:stories src/pages/Keys.stories.tsx
```

Playwright has no variable of its own for this: `PLAYWRIGHT_BROWSERS_PATH` only
moves the cache, and still looks for the exact revision the pinned version wants.
The story runner reads the variable in `ui/test-runner-jest.config.js`, which wraps
the test-runner's stock jest config. Unset, nothing changes. The path must name a
chromium Playwright can drive; the headless shell and the full build both work.

#### Why `test:stories` rather than the two commands by hand

`storybook dev -p <port>` **does not fail when the port is taken.** It logs
`Starting...`, exits, and leaves whatever was already listening in place —
another worktree's Storybook, or a stale `python3 -m http.server --directory
storybook-static` from an earlier session. `test-storybook --url
http://localhost:<port>` then runs against _that_ server and reports a green
suite for a build that never contained the stories under test. Nothing in the
output says so. This has happened twice in one day (#1648, #1684), and both
times the only thing that caught it was fetching `/index.json` by hand.

`bun run test:stories` (`ui/scripts/run-story-tests.ts`) closes that hole:

- it picks a free port itself, or **fails** when the one passed to `--port` is
  taken. The probe _connects_ rather than binding — `python3 -m http.server` and
  `Bun.serve` both set `SO_REUSEADDR`, so a second bind on a squatted port
  succeeds and a bind-only probe calls it free
- it starts `storybook dev --ci` and waits for `/index.json`
- **it checks that index against the story files it was asked to run**: the file
  must be indexed under its own import path, and every `export const … : Story`
  in it must be present. A Storybook that is not this project fails here, and so
  does a stale build of a file whose newest story is missing
- **it identifies the process holding the port.** The index check above compares
  _content_, so another worktree of this same repository sails through it — its
  build indexes the same story ids under the same import paths. That is the case
  that actually happens here, and it did, on port 6032 (#1693). So the guard
  reads the listening pid with `lsof -ti :<port>` and asks for its working
  directory (`lsof -a -p <pid> -d cwd -Fn`): `storybook dev` is spawned with its
  cwd in this worktree's `ui/`, so a listener rooted anywhere else — a sibling
  worktree, or a process whose cwd lsof will not disclose — fails the run. On a
  machine with no `lsof` the check is skipped with a warning rather than failing;
  the index check still applies
- only then does it run the tests, one file per invocation — the positional
  pattern is passed through `/bin/sh`, so a pattern containing `(`, `|` or `)`
  dies with a shell syntax error

Running the two commands by hand still works and is sometimes what you want
(driving the same server through several runs, say). If you do, check
`/index.json` for your story ids first; that is the whole difference between a
green run and a meaningless one.

Under the hood it is still:

```bash
bun run build-storybook
python3 -m http.server 6006 --directory storybook-static &
bun run test-storybook --url http://127.0.0.1:6006
```

`ui/package.json` pins `playwright` and `playwright-core` through `overrides`.
The test-runner declares its own loose `playwright` range, so without the pin it
resolves a different version from `@playwright/test` and launches a browser
revision `playwright install` never downloaded — the test-runner then fails at
launch and the play tests silently stop running (#737). Keep both on one version,
and install `chromium-headless-shell` alongside `chromium`, since the test-runner
launches the shell rather than the full build.

The static build is the one that matters. `storybook dev` serves modules
unbundled and answers from a warm cache, so it is consistently faster than the
build this job runs — fast enough to hide a story that is racing something.
**A focus assertion is the usual victim**, which is what #1675 was:

```ts
await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());
await expect(canvas.getByRole("button", { name: "open" })).toHaveFocus();  // ✗
```

A dialog, drawer or sheet takes focus from an effect a step after it enters the
document, and gives it back from that effect's cleanup a step after it leaves.
The second line above has no retry, so it passes only while the handover lands
inside the incidental gap between the two statements. Deferring the restoration
by 600ms makes it fail outright, with `Received element with focus: <body>`.

Wrapping it is the whole fix:

```ts
await waitFor(() => expect(canvas.getByRole("button", { name: "open" })).toHaveFocus());
```

`bun run check:focus` (`ui/scripts/check-story-focus.ts`) fails on the unwrapped
shape and runs in the `ui, storybook, docs` job, so this cannot reach a PR again. An
assertion straight after `.focus()`, a `userEvent` call or another `expect(…)`
needs no waiter — those have already settled where focus is, and the check
allows them.

Note that [#1672](https://github.com/rolter-ai/rolter/pull/1672)'s
`configure({ asyncUtilTimeout: 5000 })` does **not** cover this. That budget is
how long `waitFor` and `findBy*` are willing to _wait_; an assertion that never
polls waits zero milliseconds however high it is set. The two are orthogonal.

The job carries no `continue-on-error`, so it blocks: a failing story fails
`quality`, which fails `ci-ok`. (This paragraph used to say the opposite —
`continue-on-error: true` was removed when the job was promoted in #753 and the
line was left behind.)

#### What `userEvent.tab()` cannot see (#1998)

While a dialog, sheet or the nav drawer is open, `useModalA11y` makes
everything outside it `inert`. `user-event` does not know that attribute: it
works out where Tab goes from a selector of its own, which skips disabled
controls and `tabindex="-1"` but not inert ones, and then calls `.focus()` on
the result, which the browser refuses. A story that presses Tab from `<body>`
over an inert page therefore watches focus stay on `<body>`, which proves
neither the trap nor the inert page.

So a modal story asserts the two halves apart. It waits for focus to be inside
the panel before it tabs, since from there the panel's own trap answers the
key, and it checks the page behind directly:

```ts
await waitFor(() => expect(dialog).toHaveFocus());
await userEvent.tab();
await expect(dialog).toContainElement(document.activeElement as HTMLElement);
await expect(trigger.closest("[inert]")).not.toBeNull();
```

A pointer has the same blind spot. A synthetic click is dispatched straight to
its target, so it still "works" on an inert element, while a real one passes
through it as if it had `pointer-events: none`. Where a story needs to know that
a real pointer lands on a scrim, it asks `document.elementFromPoint(x, y)`,
which does honour inert. `ui/src/lib/modal-a11y.stories.tsx` has both shapes.

#### A fixed overlay is measured by its box (#2003)

`expectNoHorizontalOverflow` compares the document's scroll width with the
window's, which is the right question for a screen and the wrong one for a
dialog or a sheet. Both are `position: fixed`, so they add nothing to the
document's scroll width, and the page behind them has its scrolling locked. A
Save button past the edge of a full-screen sheet measures clean while nobody can
press it.

So an overlay story measures the control itself with `expectInViewport(el)`
from `ui/src/lib/story-viewport.ts`: the whole box has to be inside the window.
The helper first waits for every animation that ends, because a sheet slides in
from the right edge for 240 ms and a box read the moment the panel appears is
wherever the slide has got to. Looping animations such as spinners are skipped.

A box inside a scroll container is measured against that container, not the
window: a table in a card scrolls sideways inside a frame narrower than the
window by the page gutters, so a title the card's edge clips is still inside the
window (#2420). `expectInFrame(el, frame)` compares the box, or a `Range` for the
width of a line of text, with the frame's padding box less its scrollbar.

On the screen is not always the same as on the sheet. A footer that overflows
toward the right can still end a few pixels inside a 375 px window while it
sits in the sheet's gutter. `ModelSheet`'s phone stories therefore also compare
the primary action's right edge with the header's close button.

The widths and heights these stories run at come from the same module, spread
at story level: `atMobile` (375×812), `atTablet` (768×1024), `atWide`
(1440×900, for a story that needs more room than the runner's 1280×800 default)
and `atShort` (640×360, a 1280×720 screen at 200 % zoom, the size WCAG 1.4.10
asks content to reflow at). A story that also needs the Russian catalog merges the two globals:
`globals: { ...atMobile.globals, locale: "ru" }`.

#### Stories are drawn in the fonts the app ships (#2051)

Geist and Geist Mono are vendored through fontsource, and the one place that
imports the packages is `ui/src/lib/fonts.ts`. Both `ui/src/main.tsx` and
`.storybook/preview.ts` import that module. Until #2051 the preview imported
only `index.css`, so every story fell through to the browser's fallbacks: a
Courier-like mono, and in `ru` a serif for the "мс" unit in mono cells, because
that fallback has no Cyrillic. Overflow and truncation stories measured those
metrics, and screen reviews judged type the product never ships.

The faces are `font-display: swap`, so a face is fetched only when text first
asks for it and the fallback is drawn until it arrives. The preview's
`beforeAll` therefore loads every face of `--font-sans` and `--font-mono`
before the first story renders, and a story that measures text sees Geist's
metrics from its first paint.

Storybook builds its story store only after that hook resolves, so the test
runner's `preVisit` in `.storybook/test-runner.ts` awaits
`__STORYBOOK_PREVIEW__.ready()` before it reads a story's context. Without the
wait, a slow font load made `getStoryContext` read the store too early, and
every story in that suite failed with `SB_PREVIEW_API_0011` (#2275). A new
async step in the preview gets the same protection as long as it runs inside
`beforeAll`.

Two checks keep it that way. `Behaviour/Fonts` (`ui/src/lib/fonts.stories.tsx`)
asserts in `en` and `ru` that each character of a sans sentence and a mono
latency has a loaded Geist face covering it and is measured differently from
the fallback, which fails when the preview loses the import. `ui/src/lib/fonts.test.ts`
fails when either entry stops importing `lib/fonts.ts`, when any other file
imports a fontsource package directly, or when a package's family is not the
first one the tokens name. A new face goes into `lib/fonts.ts`, never into
`main.tsx`.

#### How long a story waits (#1279)

`.storybook/preview.ts` calls `configure({ asyncUtilTimeout: 5000 })`, which
raises testing-library's default from one second for every `findBy*` and
`waitFor` in every story.

The default is a unit-test budget: it assumes the thing being awaited is a
render. A screen story is not that — it mounts a page that resolves org, then
team, then project, then its own endpoints, each one a fetch through the stub
and a react-query transition. On an idle machine that chain lands in a couple
of hundred milliseconds, which is why every one of these stories passes when it
is the only file running. Under the full parallel run it does not always:
#1279 caught `Screens/Rbac` timing out on a branch that touched no RBAC code,
with the failure dump showing the screen still on its tab header — the
assertion was right and the data was still in flight.

The budget is a ceiling on how long a _failing_ assertion waits, never a delay a
passing one pays, so the suite does not get slower. Prefer it over a
per-assertion `{ timeout }`: a timeout written at one first-paint assertion is a
timeout the next story will not have. The exceptions are the few places that
genuinely need longer than the shared budget, such as `expectLoadError` in
`ui/src/pages/story-harness.tsx`, which has to outlast a screen's own retry
policy and says so beside the number.

To check whether a story is racing rather than broken, add a delay to `scoped()`
in the harness, rebuild, and re-run the file — and restart the static server
after every rebuild, since a server left running over a replaced
`storybook-static` keeps serving the build it started with.

A raised budget only helps an assertion that _retries_. `getByRole` and a bare
`expect` do not: they read the DOM once, so they wait zero milliseconds at any
budget and pass only while the stub answers inside the same tick. That is what
#1689 was — three plays acting on a control whose data is a request behind the
surface it sits on:

- a sheet fires its own query as it opens, so its rows are not there the instant
  the dialog is. `await form.findByRole(…)`, never `form.getByRole(…)` on the
  line after the sheet opened
- a gated control renders **enabled** until `/api/v1/rbac/effective` answers —
  `undefined` is "not known yet" and only an explicit `false` disables — so
  `expect(button).toBeDisabled()` asserted once is reading the gate before it
  spoke. Use `expectRefused`, which waits for the disabled flag and the `title`
  together
- and `toBeDisabled()` on its own passes for the wrong reason whenever the
  control is also disabled by its own form state (an empty key, an invalid
  draft). The `title` is what tells a refusal apart from a draft

And the mirror image is worse rather than equal ([#1707](https://github.com/rolter-ai/rolter/issues/1707)).
A gated control renders **enabled** before the answer lands, so `toBeEnabled()`
on it is true from the first paint: it is satisfied on the first poll, a
`waitFor` around it changes nothing, and it cannot fail at any latency —
including against a control plane that 404s the endpoint and leaves every
capability unknown. There is no state change to wait for, which is why the fix
is not a waiter but `expectAllowed`.

`expectAllowed(canvasElement, name, role?)` waits on the harness's own gate
probe first: `Harness` renders a hidden `data-gate` span inside the
`CapabilityProvider` whenever a story carries a `role`, reading `answered` only
once the effective-permissions query has settled _with a payload_. Only then is
the control read — enabled, and carrying none of the refusal sentences, so a
control disabled by its own form state cannot pass for a permitted one. The
probe is the harness's and never the dashboard's: no production component
learns a test-only attribute.

`bun run check:waits` (`ui/scripts/check-story-waits.ts`) enforces all three
shapes, in the same `ui, storybook, docs` job as `check:focus` and on the same
grep-level terms:

- a `toBeDisabled()` outside a waiter, in a story whose harness carries a
  `role` — the only case where a gate is in flight at all
- a `toBeEnabled()` in such a story, waiter or not. It is the one rule here a
  `waitFor` does not satisfy, because the state it asserts is the state the
  control starts in
- a `getByRole("checkbox" | "radio" | "row" | "cell" | "option" | …)` on the
  statement right after `sheet()` or a `findByRole("dialog")`

The second rule looks only at roles a screen renders one of per row of fetched
data. A `heading` or a confirm `button` is part of the sheet's own markup and
paints with it, and flagging those would mean a waiver on every editor story —
which is how a guard gets switched off. `getByLabelText` is out for the same
reason: a sheet's fields are in its first paint.

A case the rule is genuinely not about carries `// story-wait-allow: <reason>`
in the comment block above the line, the way `check:primitives` takes its
waiver. There are three in the tree today, and every run prints them:

```ts
// story-wait-allow: disabled by its own prop from the first paint, so there
// is no gate answer to wait for - that it stays disabled is the point
await expect(within(canvasElement).getByRole("button")).toBeDisabled();
```

One thing the check deliberately does not see, and which a reviewer still has
to: a data query more than one statement after the sheet opened.

#### Text inside a `CodeBlock` is a container assertion

`CodeBlock` paints its value as one text node and swaps it for token and line
spans once the lazy highlight chunk resolves
([#2644](https://github.com/rolter-ai/rolter/issues/2644)). So a
`getByText(/"model": "gpt-4o"/)` or `findByText("curl https://…")` aimed at code
passes only while it wins the race against that chunk: afterwards the key, the
colon and the value sit in separate spans and no single element carries the
string. The same story is green locally, where the chunk is cached, and red in
CI. Only `language="text"` and values past `HIGHLIGHT_CHAR_LIMIT` never
highlight.

Assert on the container's text instead, and let it retry:

```ts
const body = within(drawer).getByRole("region", { name: /^Request — / });
await waitFor(() => expect(body).toHaveTextContent(/"model": "gpt-4o"/));
```

`CodeBlock` names its scroll region from `label`, so the region is the handle.
`toHaveTextContent` reads `textContent`, which is the same before and after
highlighting; the `waitFor` is for the data behind the block, not the chunk. A
matcher function over `textContent` is the alternative when a query is wanted.
`check:waits` cannot see this: whether a string lives inside a `CodeBlock`
depends on the page, not on the line, and a heuristic over the argument would
flag prose as often as code. A reviewer still has to.

#### Every story is also an axe test

`postVisit` in `ui/.storybook/test-runner.ts` runs `axe-playwright` over the
whole document once the play function has finished, and fails the story on
**any violation at any impact** — minor and moderate included. The rule set is
`wcag2a` + `wcag2aa` + `best-practice`, and every disabled rule is named in
`DISABLED_RULES` with the reason beside it. The whole document rather than
`#storybook-root`, because dialogs, sheets and toasts portal to `<body>` and
those are the ones worth checking.

##### The moderate/minor band, measured

The gate shipped as serious+critical only (#1181); #1244 measured what the
other half contained before turning it on. Over all 695 stories:

| rule                   | impact   | nodes | stories | decision                    |
| ---------------------- | -------- | ----: | ------: | --------------------------- |
| `region`               | moderate |  3177 |     488 | off by default — page-level |
| `landmark-one-main`    | moderate |   593 |     593 | off by default — page-level |
| `page-has-heading-one` | moderate |   570 |     570 | off by default — page-level |
| `empty-table-header`   | minor    |    13 |      13 | fixed                       |
| `heading-order`        | moderate |    11 |      11 | fixed                       |
| `landmark-unique`      | moderate |     9 |       9 | fixed                       |

The three page-level rules all describe a _page_. A story normally mounts one
component, or one screen body, into a bare iframe with no app shell around it:
the landmarks, the `<main>` and the `<h1>` those rules ask for live in `App.tsx`
and `components/ScreenHeader.tsx`. Asserting them on a component story would
only ever fail, and satisfying them would mean every story grew a fake shell
that ships nowhere — so `DISABLED_RULES` turns them off for the default case.

They are off by default, not unchecked (#1353). Two story files mount a whole
page and turn them back on by name:

| Story file                       | What it mounts                                                  | Widths         |
| -------------------------------- | --------------------------------------------------------------- | -------------- |
| `ui/src/App.stories.tsx`         | the assembled shell — rail + header + screen, signed in (#1239) | 1280, 768, 375 |
| `ui/src/pages/Login.stories.tsx` | the signed-out login page, which has no shell around it         | desktop        |

Both spread `withPageA11y` from `ui/src/lib/story-a11y.ts` into their meta
`parameters`; `postVisit` merges `parameters.a11y.rules` over `DISABLED_RULES`,
so an override is per-story and additive and cannot loosen the gate for
anything else. Spread it into `parameters`, never as a bare story field — the
JSDoc docgen transform appends a `parameters: { docs: … }` of its own to every
meta and would replace a whole-object spread silently, leaving the story green
and unchecked.

##### Two guards, because prose did not hold (#1373)

The first version of the override above shipped as `...withPageA11y` at meta
level and ran green asserting nothing; it was caught by printing the merged
rule map by hand. A gate that can be switched off without a word is the one
failure worth spending code on, so the placement rule is now enforced twice:

| Guard                         | Where                                                             | What it catches                                                                                                                                                                                                                                                                                                                                                                              |
| ----------------------------- | ----------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `parameters.a11y.expectRules` | `.storybook/test-runner.ts` `postVisit`                           | the fixture did not arrive. `withPageA11y` carries the rule ids it claims to enable; the runner fails the story if any of them is not enabled in the map it actually merged. A story whose id matches `PAGE_A11Y_STORY_ID` (`shell-app--*`, `screens-login--*`) is held to the three rules whether or not it carries the claim, so losing the fixture entirely — claim and all — still fails |
| `bun run check:stories`       | `ui/scripts/check-story-parameters.ts`, run by `bun test scripts` | the spread is in the wrong object, before Storybook is even built. It reads `src/lib/story-*.ts` and sorts each exported fixture by shape: one with its own `parameters` key (`atMobile`, `atTablet`) must be spread at story level, one without (`withPageA11y`) must be spread inside `parameters`                                                                                         |

The shapes are read from the fixture modules rather than listed in the checker,
so a fixture added later is covered the day it is written. A third story file
that mounts a whole page needs its title added to `PAGE_A11Y_STORY_ID` in
`ui/src/lib/story-a11y.ts`; `story-a11y.test.ts` fails if the pattern and the
files that spread the fixture disagree.

Moving the spread up one level fails like this:

```
screens-login--wrong-password: axe rules region, landmark-one-main,
page-has-heading-one should be enabled for this story but are not. spread
`withPageA11y` from src/lib/story-a11y.ts *inside* the meta's `parameters`
object (`parameters: { ...withPageA11y }`) — spread as a bare story or meta
field it is replaced by the docgen transform and the story passes asserting
nothing (#1373).
```

`parameters: { a11y: { disable: true } }` still opts a story out of the axe
gate entirely, the `expectRules` check included — that is one explicit,
reviewable line, which is the opposite of the silent drop these guards exist
for.

That is what a separate `@axe-core/playwright` pass in `ui/e2e/` would have
bought, for a fraction of the cost: no new dependency, and it runs on every PR
with the rest of the story gate rather than only where Playwright does. The one
fix it asked for was `Login.tsx`, whose card is now a `<main>`.

The three fixed ones were small and real: an empty `<th>` over the actions
column in Cluster and User provisioning (now an `sr-only` `common.rowActions`),
an `<h4>` under an `<h2>` in Single sign-on, and the two unnamed `<aside>`
landmarks on Prompt repository and Skills repository.

Re-measure any time — set `ROLTER_AXE_TALLY` to a file path and the runner
appends one JSON line per story listing every violation at every impact,
including the excluded rules, without failing anything:

```
ROLTER_AXE_TALLY=/tmp/axe.jsonl bun run test-storybook --url http://127.0.0.1:6011
```

The failure prints the story id, then two tables: the rule and its impact, then
the CSS selector and the HTML of each offending node. `color-contrast` also
names the measured foreground, background and ratio, which is usually enough to
pick the right token straight from
[Dashboard theme](dashboard-theme.md) without opening a browser. Narrow a run
to one screen by naming its file: `bun run test:stories src/pages/Keys.stories.tsx`.

A story can opt out with `parameters: { a11y: { disable: true } }` and a comment
saying why. None currently does — treat needing one as a signal that the screen,
not the checker, is wrong. The inverse, `parameters: { a11y: { rules: { … } } }`,
re-enables a rule `DISABLED_RULES` turns off; it is for stories that mount a
whole page, and the two that do are listed above.

#### The screen-story harness

A screen story renders the real page component against a stubbed `fetch`, so it
exercises the same query wiring, empty/error branches and editor sheets that
ship. There is no shared mock module: each screen's fixtures live in its own
`.stories.tsx`, built from these stubs, and a browser test that needs real rows
seeds a running control plane through `ui/e2e/seed.ts` instead. `ui/src/pages/story-harness.tsx` holds the shared pieces — it is not a
`.stories.tsx` file, so Storybook never tries to render it as a screen:

| Helper                                         | What it is for                                                                                                |
| ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `Harness`                                      | swaps `globalThis.fetch`, clears the persisted scope, renders under a fresh `QueryClient` with `retry: false` |
| `scoped(handler)`                              | answers the org → team → project chain every scoped screen resolves first, then defers to `handler`           |
| `routes([...])`                                | fragment-matched routing table, matched in order so a longer path can precede the prefix it shares            |
| `pending`                                      | a stub that never settles, for the loading state                                                              |
| `json(body, status)`                           | a JSON `Response`, with no body for 204/205/304 so a success stub cannot throw                                |
| `recording(handler)`                           | wraps a stub and keeps every call, so a story can assert the method, URL and body that actually left          |
| `clickWhenEnabled`                             | waits for a button to be _enabled_, not merely present                                                        |
| `sheet()` / `expectSheetClosed()`              | the editor sheet, which portals to `document.body` rather than into the canvas                                |
| `withConfirm` / `expectClosesWithoutPrompting` | the discard guard from #868, asserted in both answers                                                         |

Two traps this encodes. Scope endpoints are matched on the whole pathname: a
screen's own endpoint often _contains_ one of them (`/api/v1/projects/{id}/virtual-keys`),
and a substring match would answer it with the project list. And most screens
disable their primary action until the three-request scope chain resolves, so
`findByRole` followed by a click races and throws `pointer-events: none` —
`clickWhenEnabled` is the fix.

Each screen should carry `Loaded`, `Loading`, `Empty` and an error/forbidden
story, one interaction story that opens the primary editor and saves, and at
least one story exercising the discard guard. Where a sheet opens pre-filled
(budgets seed `100` / monthly), assert the seed too: its dirty flag means "differs
from the seed", not "is non-empty", and getting that backwards makes an
untouched form prompt on every close.

#### Components carry stories on the same terms

Not only screens: every component under `ui/src/components/` has a sibling
`.stories.tsx`, and the states it can be in are what the stories enumerate — a
sheet gets add/edit, saving, rejected and both answers to the discard guard; a
primitive gets its variants plus whatever contract it promises (that `Field`
labels its control, that `Table` keeps its column headers over an empty body,
that a scroll container is reachable from the keyboard).

Two habits that the run enforces:

- **Wait for the first read.** A component that seeds itself in an effect
  (`ParamsEditor`, `ModelSheet`, `ProviderGroupSheet`) has not seeded yet when
  the play function starts — Storybook does not render inside `act`. Open with
  `findBy*`/`waitFor`, or the assertion races the mount, and an edit typed
  before the seed lands is overwritten by it.
- **A stub the story reads back must be created once.** Building a `recording()`
  in a `useMemo` keyed on a prop whose identity changes per render hands every
  render a fresh recorder, and the requests the last one saw are thrown away.

The axe pass over the new stories found three real defects rather than story
bugs, which is the argument for the gate: `ModelSheet`'s collapsible section
headers were a `div role="button"` wrapping the info hint's own button
(`nested-interactive`), `ProviderGroupSheet`'s member weights were labelled by
`title` alone (`label-title-only`), and `StatusRow`'s `colorText` painted its
label with the solid `--status-*` signal colour, which is below AA as text — the
`--status-*-text` pair exists for exactly that.

#### A StrictMode story mounts its subject in a later commit (#1744)

The obvious way to test `React.StrictMode` behaviour in a story is to wrap the
subject in `<React.StrictMode>` inside `render`. That story renders the subject
twice, but it mounts each effect once and never runs a cleanup, so an assertion
about the simulated unmount and remount passes against broken code. The first
`StrictMode*` story in `ui/src/components/EditorSheet.stories.tsx` did exactly
that in #1743: it stayed green against the naive fix it was written to refuse.

React decides what to double after each commit. It walks down from the root to
every newly placed fiber and runs the simulated unmount and remount on it only
if the walk passed a `StrictMode` element on the way, or the placed fiber is one
(`recursivelyTraverseAndDoubleInvokeEffectsInDEV` in
`react-dom-client.development.js`, React 19.3). Storybook mounts every story as
`<ErrorBoundary key={storyId}><Story /></ErrorBoundary>` (`renderToCanvas` in
`@storybook/react`), so the placed fiber is that boundary. It sits above the
story's own `StrictMode`, the walk stops there, and nothing below it is doubled.
Double rendering follows a different rule, the fiber's mode, which is why the
renders still come in pairs and the story looks strict.

So mount the `StrictMode` first and the subject into it later. The host renders
the subject only when its own state says so, `render` starts it without one, and
the play function mounts it with a click:

```tsx
export const StrictModeDoesNotInventAnAbandon: Story = {
  render: () => (
    <React.StrictMode>
      <Unmountable mounted={false} />
    </React.StrictMode>
  ),
  play: async () => {
    // mounts the sheet in a later commit, under a StrictMode that is already there
    await userEvent.click(screen().getByRole("button", { name: "open the editor" }));
    // the double-invoke has happened by the time the sheet can be used
  },
};
```

This is also the app's own shape. `ui/src/main.tsx` makes the root strict long
before anyone opens a sheet, so a story built this way runs the same lifecycle a
browser on `bun run dev` does.

Two more habits keep such a story from passing for the wrong reason:

- **Anchor an absence on something that happened.** "No `form_abandon`" is also
  true before the sheet has finished mounting, so `expectNoUxEvent(…)` on the
  line after the click proves nothing. `StrictModeDoesNotInventAnAbandon` first
  presses the sheet's save button and waits for its `form_submit`: the button
  cannot be pressed until the sheet has mounted, been remounted and settled, so
  the absence asserted after that point covers the whole double-invoke.
- **Watch it fail once.** Break the code the story guards and run the file with
  `bun run test:stories`; for #1739 that meant emitting the abandon straight
  from the effect cleanup. A StrictMode story that stays green against the
  broken code is asserting against a lifecycle that never ran. When the reason
  is unclear, a probe settles it: a child whose effect counts its mounts and
  cleanups should read two mounts and one cleanup under a working `StrictMode`,
  and reads one and zero in the same-commit shape.

`framework.options.strictMode` in `ui/.storybook/main.ts` would put a
`StrictMode` above that boundary for every story. It is off, and turning it on
changes the lifecycle of every story in the tree, which is a larger decision
than one assertion needs.

### Nightly extended checks

[`.github/workflows/extended.yml`](../../../.github/workflows/extended.yml) holds
the informational checks that need a full build and gate nothing. It runs nightly
at 01:41 UTC and on `workflow_dispatch`, rather than on every push, so none of
them takes a slot from the 20-job runner pool while PRs wait
([ADR-0034](../adr/2026-09-29-ci-runner-budget.md)):

| Job             | What it checks                                                                                                                      |
| --------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `macos check`   | `cargo check --workspace --all-features` on `macos-latest`                                                                          |
| `compose smoke` | the full Docker Compose topology, described below                                                                                   |
| `msrv build`    | `cargo +<rust-version> check --workspace --all-features`, with the version read from the root `Cargo.toml`                          |
| `coverage`      | the PR coverage job on `master`, same services, toolchain and cache key, so it gives a daily number and seeds the cache PRs restore |

The msrv job runs `cargo +<version>` because `rust-toolchain.toml` pins `stable`
and outranks the default a toolchain action sets, so a plain `cargo check` would
test stable and never the declared version; the step prints `rustc --version`
for that toolchain first, so the log shows which compiler ran. The declared
version is 1.91 (#2026): the lockfile alone needs 1.88 (redis, tonic, icu and
`time` declare it), and our own code calls `str::floor_char_boundary`, stable
since 1.91. To find the floor again after a dependency bump or a newer std API,
install the candidate with `rustup toolchain install <ver> --profile minimal`
and run `cargo +<ver> check --workspace --all-features`; the version below it
must fail.

None of these jobs is `continue-on-error`: nothing gates on `extended.yml`, and a
failure has to reach the `report failure` job as `failure`. On `master` that
job opens an issue titled `extended.yml: nightly checks failing`, labelled `ci`,
the first time a run fails, and comments on it with the run link and each job's
result while it stays open. Close it once the fix lands; the next failure opens
a new one. It is the only job in the workflow with write scopes, `issues: write`
and `actions: write` at job level with no checkout.

An issue opened with the workflow's own token raises no `issues` event, so
`project-automation` never sees it. The job triages a new issue itself instead:
it sets the `Maintenance, CI & DX` milestone, then dispatches
`project-automation.yml` with the issue number, `area=ci` and `effort=XS`, which
puts it on the board with `Todo` and `Priority: Medium` as well (#2201). Either
half only warns when it fails, since a renamed milestone must not cost the issue
itself; the warning names the command to run by hand. Breakage in these checks
therefore shows up up to a day late, on that issue, rather than on the PR that
caused it. To check a branch before merging, dispatch the workflow on it; a
failure there shows in that run and leaves the issue alone:

```bash
gh workflow run extended.yml --ref <branch>
```

### Full-stack compose smoke

The `compose smoke` job in `extended.yml` boots the production-shaped Docker
Compose topology (Postgres, Redis, ClickHouse, gateway, control) and exercises
it end-to-end. Run it locally with the same script CI uses:

```bash
bash docker/smoke/smoke.sh
```

It layers [`docker/docker-compose.ci.yml`](../../../docker/docker-compose.ci.yml)
over the base compose file: the overlay mounts
[`docker/smoke/rolter.smoke.toml`](../../../docker/smoke/rolter.smoke.toml) (a
keyless open config, `require_auth = false`) into the gateway and the control
plane, so the built-in `fake-llm` model answers without any provider secret. The
control plane gets it too because the gateway follows the control plane's
snapshot, which carries the control plane's own bootstrap config: the example
baked into the image would bring its virtual key back. The script waits for both
`/healthz` endpoints, checks `/v1/models` and `fake-llm` chat (non-streaming +
SSE) on the gateway and the postgres-backed `/internal/snapshot` on the control
plane, then creates an org, team, project, provider and route through the
control plane's open API and waits for the gateway to list the new model, which
is the check that the two planes are wired together. It then always dumps
compose logs and runs `down -v`. It runs nightly rather than on every push,
because its cold Docker release build costs about five minutes of a runner
(ROL-245, ADR-0034).

### Nightly dashboard journeys

[`.github/workflows/ui-e2e.yml`](../../../.github/workflows/ui-e2e.yml) runs the
Playwright journeys in `ui/e2e/` against the fake-vLLM compose stack
(`integration/e2e/docker-compose.e2e.yml`), nightly at 03:17 UTC and on
`workflow_dispatch`. Like `extended.yml` it gates nothing, and for the same
reason: one run holds a runner for about ten minutes, and most pull requests touch
`ui/`, so a path-filtered PR trigger would take a slot from the 20-job pool on
nearly every push ([ADR-0034](../adr/2026-09-29-ci-runner-budget.md)). Why it
stays out of `ci-ok` is in
[ci-gating.md](ci-gating.md#suites-that-stay-out-of-ci-ok).

A failing `master` run used to sit unread in the Actions tab; it was red for a
week before anyone noticed (#2677). The workflow now ends in a `report failure`
job, a copy of `extended.yml`'s: on `master` it opens an issue titled
`ui-e2e.yml: dashboard journeys failing`, labelled `ci`, the first time a run
fails, and comments on it with the run link while it stays open. The run's
`playwright-report` artifact holds the trace and screenshots. A new issue gets
the `Maintenance, CI & DX` milestone and a `project-automation.yml` dispatch
with `area=ui` and `effort=S`, both best-effort as in `extended.yml`. The two
workflows use different titles, so they never share an issue. Close it once
the fix lands; the next failure opens a new one.

A pull request that changes a screen a journey walks through, or the control
plane API under it, should dispatch the suite on its branch before merging. A
failure there shows in that run and leaves the issue alone:

```bash
gh workflow run ui-e2e.yml --ref <branch>
```

### Published-port image smoke

The `image-smoke` job builds the single image from `docker/Dockerfile` and runs
it the way the quickstart does: default command (`rolter easy-up`), ports
published with `-p`, curled from the host. It checks three states: with no
`ROLTER_ADMIN_TOKEN` and no `ROLTER_ALLOW_OPEN_MODE` the container exits with the
refusal; acknowledged open, the gateway answers `fake-llm` and the control plane
serves the dashboard; closed by a throwaway token, `/internal/snapshot` is 401
without it and 200 with it. A bind on the container's loopback passes every
check made from inside the container and answers nothing through a published
port, which is how #1891 shipped. Run it locally against any tag:

```bash
docker build -f docker/Dockerfile --target runtime -t rolter:dev .
bash docker/smoke/image-smoke.sh rolter:dev
```

It needs no secrets and no compose stack, so unlike the compose smoke it runs
on every push and is blocking. The release workflow's `smoke image` job runs the
same script against each architecture's pushed digest before anything is
published.
