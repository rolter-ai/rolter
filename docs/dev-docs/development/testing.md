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
provider's complaint *is* the finding, and a bare status-code assertion would
throw it away.

### Configuring the Gemini smoke

`gemini-interactions-smoke.yml` needs one secret before it can verify anything:

1. Create a `live-providers` repository environment (Settings → Environments).
   Keeping the key there rather than at repository scope means a run against a
   billed provider is reviewable, not something any workflow can reach for.
2. Add `GEMINI_API_KEY` to that environment.
3. Dispatch the workflow (`gh workflow run gemini-interactions-smoke.yml`),
   optionally with `-f model=<id>`.

Until the secret exists the workflow **fails** rather than skipping. A green
tick from a run that made no request reads as "the wire format is still
confirmed" when nothing was checked — worse than no sweep at all. Pass
`-f allow_unconfigured=true` for a deliberate dry run of the workflow itself.

Each run records the wire shapes it observed into the job summary and uploads
the full log as an artifact. A billable run should leave evidence behind: the
next question about a field name is then answered by reading the last run
rather than by spending another call.

What the suite covers, and why each probe exists:

| Probe | Confirms |
|---|---|
| text turn | turn mapping, `system_instruction`, `generation_config`, usage — all documented |
| inline image part | the inferred `mime_type`/`data` inline part shape |
| remote image part | the inferred `file_uri` shape — the *other* branch, which the inline probe never reaches |
| tool call round trip | `function_call` out, `function_result` back, and `call_id` correlation |
| interaction threading | the id rolter surfaces as the response `id` is the one Google accepts back |
| every client dialect | Chat Completions, Messages and Responses have separate response translators |
| every client dialect, streaming | the inferred `step.delta` variants, through all three separate SSE emitters |

A content part the dialect cannot carry is rejected at the gateway with
`400 unsupported_content_part` rather than being dropped (#882), so an
unconfirmed part shape fails loudly instead of producing a shortened body.

Test grouping is configured in [`.config/nextest.toml`](../../.config/nextest.toml):
the Postgres-backed `rolter-store`/`rolter-control` suites share one database and
reset the schema per test, so they run in a single-threaded group to avoid
clobbering each other.

## The Postgres test database

The Postgres-backed tests self-skip unless `ROLTER_TEST_DATABASE_URL` points at
a Postgres they may write to. The role wants `CREATEDB`, since each worktree
gets a database of its own (below); without it the tests still run, they just
share the database the url names:

```bash
ROLTER_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/rolter_test \
  cargo nextest run -p rolter-store -p rolter-control --features postgres
```

### One database per worktree

`ROLTER_TEST_DATABASE_URL` names the **server**, not the database the tests end
up writing to. The database is derived from the workspace the test binary was
compiled in — `rolter_test_wt_<worktree>_<digest>` — and created on first use by
[`rolter_store::postgres::test_database`](../../crates/rolter-store/src/postgres/test_database.rs).
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

The worktree's path is stored as the database's comment, which is the whole
cleanup story: the next run drops any `rolter_test_wt_*` database whose recorded
directory no longer exists, so a removed worktree takes its database with it and
no hook has to run. List them with `\l rolter_test_wt*`, or:

```sql
select datname, shobj_description(oid, 'pg_database')
from pg_database where datname like 'rolter_test_wt%';
```

Set `ROLTER_TEST_PER_WORKTREE_DATABASE=0` to use `ROLTER_TEST_DATABASE_URL`
exactly as given — a throwaway database that is already private, or a deliberate
reproduction of the shared-database behaviour. The derivation also steps aside
when it cannot create a database (a role without `CREATEDB`, for instance): it
prints why and falls back to the configured url, because losing isolation is
better than losing the suite.

### One schema per test

Each test gets a schema of its own, named `test_<pid>_<seq>` and pinned through
`search_path`, because plain `cargo test` — which the coverage job runs — puts
every test in one process as a thread, and a shared `public` schema would race
on DDL. Build it through
[`rolter_store::postgres::test_schema::TestSchema`](../../crates/rolter-store/src/postgres/test_schema.rs)
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
deliberately one-sided — a schema whose pid *is* live is always kept, so a suite
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

When a failing test's rows *are* the evidence, set `ROLTER_TEST_KEEP_SCHEMA=1`:
the guard then keeps every schema it creates (printing each name) and skips the
sweep, so nothing is reclaimed until you drop it yourself.

### What keeps all of this honest

Three rules hold the isolation together, and
[`crates/rolter-store/tests/db_test_isolation.rs`](../../crates/rolter-store/tests/db_test_isolation.rs)
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

Both use a mock upstream that blocks on a semaphore the test owns, so every step
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

CI runs coverage in the `coverage` job of `quality.yml` and enforces a
**ratcheting baseline**: the committed baseline lives in
[`.github/coverage-baseline.txt`](../../.github/coverage-baseline.txt), and
[`.github/scripts/coverage-ratchet.sh`](../../.github/scripts/coverage-ratchet.sh)
fails the step if the current percentage drops more than
`COVERAGE_TOLERANCE` points (default `0.5`) below it. The job also uploads the
`lcov.info` report as a CI artifact.

Policy (ROL-246):

- New code must not push coverage below `baseline − tolerance`. If a PR
  legitimately lowers coverage, edit `.github/coverage-baseline.txt` in the same
  PR and explain why.
- When coverage climbs well above the baseline, raise the baseline to lock in
  the gain (the ratchet only goes up).
- The job is **informational** (`continue-on-error: true`) until the baseline is
  trusted; promote it to blocking by removing that flag on the `coverage` job.

## CI

`.github/workflows/ci.yml` delegates to the shared `quality.yml` gate, which runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo nextest run --workspace --all-features` plus a `cargo test --doc` pass, the feature matrix, `cargo doc` (warnings as errors), cargo-deny, gitleaks, the zizmor workflow audit, the UI lint/build, and a Conventional Commit PR-title check on every push/PR.

### UI dependencies and the lockfile

The `ui` and `storybook` jobs both install with `bun install --frozen-lockfile`,
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
not security updates: alerts still fire on `ui` dependencies, but the security
bump has to be raised by hand (#1148).

### Secret scanning

The `gitleaks` job runs the gitleaks **CLI** from a digest-pinned container, not
`gitleaks-action`. The action gates org-owned repositories behind a license key,
and license secrets are invisible to both dependabot runs (a separate secret
store) and fork PRs (no secrets at all), so every such PR failed the job and with
it `ci-ok`. The CLI is free and unrestricted, so `quality.yml` now takes no
secrets and behaves identically for forks, dependabot and direct pushes.

Two passes run with the shared `.github/config/gitleaks.toml` policy: `gitleaks
dir` over the working tree (everything the commit ships) and, on PRs, `gitleaks
git --log-opts base..head` over the branch history (catches a secret added and
then removed inside the same PR). The pinned digest is v8.30.1 — the version
`prek.toml` already uses for the staged-content hook, so local and CI scans agree.

Reproduce a CI run locally:

```bash
docker run --rm -v "$PWD:/repo" -w /repo \
  ghcr.io/gitleaks/gitleaks@sha256:c00b6bd0aeb3071cbcb79009cb16a60dd9e0a7c60e2be9ab65d25e6bc8abbb7f \
  dir . --config .github/config/gitleaks.toml --redact --exit-code 1
```
### Workflow security (zizmor)

The `zizmor` job audits `.github/workflows/` and `.github/actions/` for workflow
security smells — unpinned or impostor action refs, template injection into
`run:`, over-broad `GITHUB_TOKEN` scopes, cache poisoning, dangerous triggers.
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

| Exit | Meaning | Gate behaviour |
|---|---|---|
| `0` | audit completed, nothing to report | pass, first attempt |
| `11`–`14` | audit completed, findings at informational…high | **fail, first attempt** |
| `1` | no audit was produced | retry, but only when the output names a GitHub API failure |
| `2`, `3` | bad arguments / no inputs collected | fail, first attempt |

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
are precisely the audits that catch a bad action ref *introduced by the diff in
front of you*. Trading away the audits that read the current diff to pre-empt an
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

| Where | Rule | Why |
|---|---|---|
| `engine-integration.yml` — `Swatinem/rust-cache` | `cache-poisoning` | nothing this workflow builds is published, so the cache cannot poison a release |
| `release-plz.yml` — both `actions/checkout` steps | `artipacked` | release-plz pushes the release branch and the tags with the persisted token, so `persist-credentials` must stay on |
| `project-automation.yml` — `pull_request_target` | `dangerous-triggers` | required so fork PRs can read the org PAT; the workflow never checks out PR head and passes only the project id and literal field names to `run:` |

### Storybook play tests

The `storybook` job builds the static Storybook, serves it, and runs the
interaction (play) tests with `@storybook/test-runner` against a headless
chromium. It is a **merge gate** (#753): a failing play test fails `quality`,
which fails `ci-ok`. Locally:

```bash
cd ui
bunx playwright install --with-deps chromium chromium-headless-shell
bun run test:stories                            # every story file
bun run test:stories src/pages/Keys.stories.tsx # or just these
```

#### Why `test:stories` rather than the two commands by hand

`storybook dev -p <port>` **does not fail when the port is taken.** It logs
`Starting...`, exits, and leaves whatever was already listening in place —
another worktree's Storybook, or a stale `python3 -m http.server --directory
storybook-static` from an earlier session. `test-storybook --url
http://localhost:<port>` then runs against *that* server and reports a green
suite for a build that never contained the stories under test. Nothing in the
output says so. This has happened twice in one day (#1648, #1684), and both
times the only thing that caught it was fetching `/index.json` by hand.

`bun run test:stories` (`ui/scripts/run-story-tests.ts`) closes that hole:

- it picks a free port itself, or **fails** when the one passed to `--port` is
  taken. The probe *connects* rather than binding — `python3 -m http.server` and
  `Bun.serve` both set `SO_REUSEADDR`, so a second bind on a squatted port
  succeeds and a bind-only probe calls it free
- it starts `storybook dev --ci` and waits for `/index.json`
- **it checks that index against the story files it was asked to run**: the file
  must be indexed under its own import path, and every `export const … : Story`
  in it must be present. A Storybook that is not this project fails here, and so
  does a stale build of a file whose newest story is missing
- **it identifies the process holding the port.** The index check above compares
  *content*, so another worktree of this same repository sails through it — its
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
shape and runs in the `ui lint / build` job, so this cannot reach a PR again. An
assertion straight after `.focus()`, a `userEvent` call or another `expect(…)`
needs no waiter — those have already settled where focus is, and the check
allows them.

Note that [#1672](https://github.com/rolter-ai/rolter/pull/1672)'s
`configure({ asyncUtilTimeout: 5000 })` does **not** cover this. That budget is
how long `waitFor` and `findBy*` are willing to *wait*; an assertion that never
polls waits zero milliseconds however high it is set. The two are orthogonal.

The job carries no `continue-on-error`, so it blocks: a failing story fails
`quality`, which fails `ci-ok`. (This paragraph used to say the opposite —
`continue-on-error: true` was removed when the job was promoted in #753 and the
line was left behind.)

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

The budget is a ceiling on how long a *failing* assertion waits, never a delay a
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

A raised budget only helps an assertion that *retries*. `getByRole` and a bare
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
once the effective-permissions query has settled *with a payload*. Only then is
the control read — enabled, and carrying none of the refusal sentences, so a
control disabled by its own form state cannot pass for a permitted one. The
probe is the harness's and never the dashboard's: no production component
learns a test-only attribute.

`bun run check:waits` (`ui/scripts/check-story-waits.ts`) enforces all three
shapes, in the same `ui lint / build` job as `check:focus` and on the same
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

| rule | impact | nodes | stories | decision |
|---|---|---:|---:|---|
| `region` | moderate | 3177 | 488 | off by default — page-level |
| `landmark-one-main` | moderate | 593 | 593 | off by default — page-level |
| `page-has-heading-one` | moderate | 570 | 570 | off by default — page-level |
| `empty-table-header` | minor | 13 | 13 | fixed |
| `heading-order` | moderate | 11 | 11 | fixed |
| `landmark-unique` | moderate | 9 | 9 | fixed |

The three page-level rules all describe a *page*. A story normally mounts one
component, or one screen body, into a bare iframe with no app shell around it:
the landmarks, the `<main>` and the `<h1>` those rules ask for live in `App.tsx`
and `components/ScreenHeader.tsx`. Asserting them on a component story would
only ever fail, and satisfying them would mean every story grew a fake shell
that ships nowhere — so `DISABLED_RULES` turns them off for the default case.

They are off by default, not unchecked (#1353). Two story files mount a whole
page and turn them back on by name:

| Story file | What it mounts | Widths |
|---|---|---|
| `ui/src/App.stories.tsx` | the assembled shell — rail + header + screen, signed in (#1239) | 1280, 768, 375 |
| `ui/src/pages/Login.stories.tsx` | the signed-out login page, which has no shell around it | desktop |

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

| Guard | Where | What it catches |
|---|---|---|
| `parameters.a11y.expectRules` | `.storybook/test-runner.ts` `postVisit` | the fixture did not arrive. `withPageA11y` carries the rule ids it claims to enable; the runner fails the story if any of them is not enabled in the map it actually merged. A story whose id matches `PAGE_A11Y_STORY_ID` (`shell-app--*`, `screens-login--*`) is held to the three rules whether or not it carries the claim, so losing the fixture entirely — claim and all — still fails |
| `bun run check:stories` | `ui/scripts/check-story-parameters.ts`, run by `bun test scripts` | the spread is in the wrong object, before Storybook is even built. It reads `src/lib/story-*.ts` and sorts each exported fixture by shape: one with its own `parameters` key (`atMobile`, `atTablet`) must be spread at story level, one without (`withPageA11y`) must be spread inside `parameters` |

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

| Helper | What it is for |
|---|---|
| `Harness` | swaps `globalThis.fetch`, clears the persisted scope, renders under a fresh `QueryClient` with `retry: false` |
| `scoped(handler)` | answers the org → team → project chain every scoped screen resolves first, then defers to `handler` |
| `routes([...])` | fragment-matched routing table, matched in order so a longer path can precede the prefix it shares |
| `pending` | a stub that never settles, for the loading state |
| `json(body, status)` | a JSON `Response`, with no body for 204/205/304 so a success stub cannot throw |
| `recording(handler)` | wraps a stub and keeps every call, so a story can assert the method, URL and body that actually left |
| `clickWhenEnabled` | waits for a button to be *enabled*, not merely present |
| `sheet()` / `expectSheetClosed()` | the editor sheet, which portals to `document.body` rather than into the canvas |
| `withConfirm` / `expectClosesWithoutPrompting` | the discard guard from #868, asserted in both answers |

Two traps this encodes. Scope endpoints are matched on the whole pathname: a
screen's own endpoint often *contains* one of them (`/api/v1/projects/{id}/virtual-keys`),
and a substring match would answer it with the project list. And most screens
disable their primary action until the three-request scope chain resolves, so
`findByRole` followed by a click races and throws `pointer-events: none` —
`clickWhenEnabled` is the fix.

Each screen should carry `Loaded`, `Loading`, `Empty` and an error/forbidden
story, one interaction story that opens the primary editor and saves, and at
least one story exercising the discard guard. Where a sheet opens pre-filled
(budgets seed `100` / `30d`), assert the seed too: its dirty flag means "differs
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


### Full-stack compose smoke

The `compose-smoke` job boots the production-shaped Docker Compose topology
(Postgres, Redis, ClickHouse, gateway, control) and exercises it end-to-end. Run
it locally with the same script CI uses:

```bash
bash docker/smoke/smoke.sh
```

It layers [`docker/docker-compose.ci.yml`](../../docker/docker-compose.ci.yml)
over the base compose file: the overlay mounts
[`docker/smoke/rolter.smoke.toml`](../../docker/smoke/rolter.smoke.toml) (a
keyless open gateway config) so the built-in `fake-llm` model answers without any
provider secret. The script waits for both `/healthz` endpoints, checks
`/v1/models` and `fake-llm` chat (non-streaming + SSE) on the gateway and the
postgres-backed `/internal/snapshot` on the control plane, then always dumps
compose logs and runs `down -v`. It is **informational** (`continue-on-error`)
until the image-build cost and flake profile are trusted (ROL-245).
