# Testing

## Run

Tests run under [nextest](https://nexte.st/) (the same runner CI uses), plus a
separate doc-test pass since nextest does not run doc tests:

```bash
cargo nextest run --workspace   # unit + integration tests
cargo test --doc --workspace    # doc tests
cd ui && bun run lint           # ui typecheck
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

`.github/workflows/ci.yml` delegates to the shared `quality.yml` gate, which runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo nextest run --workspace --all-features` plus a `cargo test --doc` pass, the feature matrix, `cargo doc` (warnings as errors), cargo-deny, gitleaks, the UI lint/build, and a Conventional Commit PR-title check on every push/PR.

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
### Storybook play tests

The `storybook` job builds the static Storybook, serves it, and runs the
interaction (play) tests with `@storybook/test-runner` against a headless
chromium. It is a **merge gate** (#753): a failing play test fails `quality`,
which fails `ci-ok`. Locally:

```bash
cd ui
bunx playwright install --with-deps chromium chromium-headless-shell
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

The job carries no `continue-on-error`, so it blocks: a failing story fails
`quality`, which fails `ci-ok`. (This paragraph used to say the opposite —
`continue-on-error: true` was removed when the job was promoted in #753 and the
line was left behind.)

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
| `region` | moderate | 3177 | 488 | excluded — page-level |
| `landmark-one-main` | moderate | 593 | 593 | excluded — page-level |
| `page-has-heading-one` | moderate | 570 | 570 | excluded — page-level |
| `empty-table-header` | minor | 13 | 13 | fixed |
| `heading-order` | moderate | 11 | 11 | fixed |
| `landmark-unique` | moderate | 9 | 9 | fixed |

The three excluded rules all describe a *page*. A story mounts one component,
or one screen body, into a bare iframe with no app shell around it: the
landmarks, the `<main>` and the `<h1>` those rules ask for live in `App.tsx`
and `components/screen.tsx`, which no story mounts. Asserting them per story
would only ever fail, and satisfying them would mean every story grew a fake
shell that ships nowhere. Nothing checks them today — the e2e suite drives the
real shell but has no axe pass; #1353 adds one there.

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
to one screen with `bun run test-storybook --url … -- -t "Keys"`.

A story can opt out with `parameters: { a11y: { disable: true } }` and a comment
saying why. None currently does — treat needing one as a signal that the screen,
not the checker, is wrong.

#### The screen-story harness

A screen story renders the real page component against a stubbed `fetch`, so it
exercises the same query wiring, empty/error branches and editor sheets that
ship. `ui/src/pages/story-harness.tsx` holds the shared pieces — it is not a
`.stories.tsx` file, so Storybook never tries to render it as a screen:

| Helper | What it is for |
|---|---|
| `Harness` | swaps `globalThis.fetch`, clears the persisted scope, renders under a fresh `QueryClient` with `retry: false` |
| `scoped(handler)` | answers the org → team → project chain every scoped screen resolves first, then defers to `handler` |
| `routes([...])` | fragment-matched routing table, matched in order so a longer path can precede the prefix it shares |
| `pending` | a stub that never settles, for the loading state |
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
