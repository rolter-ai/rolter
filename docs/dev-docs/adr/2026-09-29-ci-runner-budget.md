# CI is sized to the 20-slot runner pool

**Status:** Accepted (rolling out as the PRs under #2025) · **Date:** 29 Sep 2026 · **Issues:** [#2025](https://github.com/rolter-ai/rolter/issues/2025)
**Relates:** ADR-0033 (merge queue), ADR-0032 (1.0 guarantees, which keep `semver-checks` advisory), [the `ci-ok` gate](../development/ci-gating.md), [merge protection](../development/merge-protection.md), [packaging and releases](../development/packaging.md)

## Context

The rolter-ai organisation is on GitHub's Free plan and rolter is its only
repository, so every workflow run draws on one pool of 20 concurrent standard
jobs, 5 of them macOS
([Actions limits](https://docs.github.com/en/actions/reference/limits)). CI had
grown to ask for far more than that. One push to a pull request started 36 jobs
in the same second, 28 from `quality.yml` and 8 from `ci.yml`, so 16 of them
queued even when the organisation was otherwise idle.

The pool was saturated. The audit behind #2025 measured job concurrency across
474 runs from the Actions API, second by second from each job's real start and
end times. Org-wide running jobs peaked at exactly 20, and while anything was
queued, 17 to 20 jobs were running in 92% of seconds on 2026-09-28 and in 98%
of seconds during the burst on the night of 09-26/27.

Queueing cost `master` far more than it cost pull requests. Over the 88
successful full-gate `ci.yml` runs created between 2026-09-26 20:34Z and
2026-09-28 15:55Z:

| Event                            | Runs | Median run | p90      | Median time lost to queueing |
| -------------------------------- | ---- | ---------- | -------- | ---------------------------- |
| `pull_request`                   | 55   | 9.4 min    | 39.8 min | 1.5 min                      |
| push to `master`                 | 26   | 29.7 min   | 43.8 min | 21.5 min (max 53.0)          |
| `workflow_dispatch` (release PR) | 7    | 11.8 min   | 20.9 min | 5.0 min                      |

A typical PR gate was already close to the no-queue floor of about 8.1 min,
which `codeql (rust)` sets at 488 s. Pull requests suffered in the p90 tail on
busy days. The master push gate lost a median 21.5 min, and it is the run a
release waits on.

Master pays because every merge fans out. An isolated merge started 33 jobs for
the `ci.yml` push run and 30 for release-plz, 27 of which were `verify`, a full
re-run of `quality.yml` on the sha the push run was already checking. Then,
2 to 44 min later, release-plz dispatched a 35-job `ci.yml` run on the release
PR. That is a ceiling of about 98 jobs per merge. The measured average over 27
master pushes (09-26 20:34Z to 09-28 16:10Z) was about 57, because release-plz's
concurrency group cancelled 15 of its 27 runs while they were still pending, and
over those 27 pushes only 11 release-PR dispatches ran. Even so, post-merge work
took 51% of the runner time on 09-28, against 43% for PR gating. `verify` alone
was 13% and the release-PR dispatches 16.7%, and 226 of the 355 slot-minutes
those dispatches used went to runs that were later cancelled.

The job list itself was wasteful:

- 17 jobs did under 20 s of work. They were 45% of all jobs and 4.7% of
  slot-seconds, and their checks took 0-15 s. The 13 of them in `quality.yml`
  had median waits for a runner between 86 and 200 s depending on the job, with
  one wait of 553 s. The four in `ci.yml` usually got a runner within 20 s.
- The `helm chart` job spent about 200 s building the project's maturin wheel by
  accident: `uv run --with pyyaml` in the repository root installs the project.
  That one job cost more runner time than the 17 small jobs together.
- Informational jobs paid for full builds on every push. Coverage took 257 s,
  compose smoke 326 s (280 s of it a cold Docker release build), and the macOS
  check held one of the 5 macOS slots.
- The msrv job never tested 1.82 at all, because `rust-toolchain.toml` pins
  `stable` and outranks the rustup default the job sets (#2026).
- #1950's `image smoke (published ports)` added a second cold Docker build,
  308 s in the one run measured, to every `quality.yml` call (#2037).
- The Actions cache held 11.53 GB in 18 entries against its 10 GB limit, so
  master's Rust caches were evicted and jobs built cold.

The merge queue that ADR-0033 decided on is also not enabled today:
`mergeQueue(branch: "master")` is null and `ci.yml` has never run on
`merge_group` (#2029). Until it is, the master push run is the only check on the
tree that lands, which makes its 21.5 min of queueing more expensive.

## Decision

Run fewer, larger jobs, keep `ci-ok` exactly as branch protection and the
release tooling see it, move informational work off the push path, and stop
re-running the gate after every merge. A PR push goes from 36 jobs to 11. An
isolated merge to `master` goes from a ceiling of about 98 to 13, or 23 while
the release PR carries `release:ready`.

### The layout

| Job                             | Workflow      | Runs when                   | Blocking           | Contains                                                                                                                                                                                                                                                                                                          |
| ------------------------------- | ------------- | --------------------------- | ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `static checks`                 | `quality.yml` | every call                  | yes                | gitleaks (working tree first, then branch history), agent session urls in commits, migrations append-only, typos, taplo fmt, cargo-deny, unused deps, actionlint, zizmor, release handoff wired, board automation retry policy, helm chart (the checker runs under `uv run --script`), and a report step. 65-90 s |
| `rust lint`                     | `quality.yml` | every call                  | yes                | fmt, clippy (default and `postgres`), cargo doc, `cargo hack check --each-feature` and the cross-crate feature combination. About 220 s warm                                                                                                                                                                      |
| `rust build`                    | `quality.yml` | every call                  | yes, except semver | package (publish verify, plus a `maturin sdist` check of the PyPI packaging) and the gateway smoke build and probe, then three `semver-checks` steps (install, baseline, check), placed last and each `continue-on-error`                                                                                         |
| `nextest / doctests`            | `quality.yml` | every call                  | yes                | unchanged, about 388 s                                                                                                                                                                                                                                                                                            |
| `image smoke (published ports)` | `quality.yml` | every call                  | yes                | unchanged from #1950, still its own job                                                                                                                                                                                                                                                                           |
| `ui, storybook, docs`           | `quality.yml` | every call                  | yes                | one pinned bun and one `bun install`, then every former ui, storybook, docs-formatting and `llms.txt` check as its own step. About 320 s, 25 min job timeout                                                                                                                                                      |
| `coverage (informational)`      | `quality.yml` | `pull_request` only         | no                 | unchanged llvm-cov, ratchet and lcov artifact                                                                                                                                                                                                                                                                     |
| `codeql (rust)`                 | `ci.yml`      | unchanged                   | yes                | same name; on a PR with no Rust change it skips extract and analyze inside the job                                                                                                                                                                                                                                |
| `codeql (actions-js-python)`    | `ci.yml`      | unchanged                   | yes                | one leg for actions, javascript-typescript and python, which were three                                                                                                                                                                                                                                           |
| `gate-ok`                       | `ci.yml`      | unchanged                   | yes                | unchanged; the job the title-edit fast path looks up by name                                                                                                                                                                                                                                                      |
| `ci-ok`                         | `ci.yml`      | every event, `if: always()` | required check     | needs `quality` and `codeql`; runs pr-title and both agent-session-url checks as steps, then the verdict                                                                                                                                                                                                          |

The informational jobs that needed a full build move to a new `extended.yml`
that runs nightly and on `workflow_dispatch`. Coverage runs there on `master`
with the same `shared-key: coverage` as the PR job, so it seeds the cache that
PR coverage restores and gives a daily number for `master`. msrv runs as
`cargo +1.82.0 check`, so the toolchain file can no longer override it. The
macOS check and compose smoke move unchanged. None of these jobs carries
`continue-on-error`, since nothing gates on `extended.yml`. A `report failure`
job opens or comments on one tracking issue when any of them fails. It holds
`issues: write` at job level with no checkout, because zizmor 1.26.1 rates the
same permission at workflow level as high and would fail the gate.

Coverage stays on pull requests because `testing.md` asks an author to edit the
coverage baseline in the same PR that moves it, and that rule needs a per-PR
number.

### The release gate

release-plz's `verify` job, the 27-job re-run of `quality.yml`, is deleted. A
`release-gate` job replaces it:

1. It lists every package `cargo metadata` marks publishable and looks each
   version up on crates.io. Any lookup error counts as pending. If nothing is
   pending it exits in about 30 s without setting its `verified` output.
2. If something is pending, `scripts/wait-for-ci-gate.sh` waits for the
   `ci.yml` push run on the same sha and requires that run's `ci-ok` job to have
   concluded `success`. It fails closed on API errors, on any other conclusion,
   and after 90 min.
3. `release-plz release` needs `release-gate` and runs only when `verified` is
   `'true'`.

The gate binds to the `ci.yml` push run because a check-run name proves little.
Any workflow can post a check-run called `ci-ok` on a `master` sha. `ci.yml`'s
`pull_request` trigger has no `branches:` filter, so a PR from `master` into a
branch with an edited `ci.yml` reports its own `ci-ok` on master's head sha, and
outside workflows already write there (128c647 carries 44 `add to project board`
check-runs). A `ci.yml` push run on sha X can only come from `ci.yml` at X.
`release.yml` still matches check-run names, which #2034 tracks.

A detector that wrongly reports nothing pending delays a release. It cannot
publish an unverified one, because the publish depends on `verified` rather
than on the detector's success. `check-release-handoff.py` asserts that binding,
with self-test fixtures for a dropped `needs`, a dropped `if`, and a wait
replaced by a name lookup. The job holding release-plz's `contents: write`
token never sits on a runner through the wait. The deadline is 90 min because
push-run time to `ci-ok` measured a median of about 29 min and a maximum of
61.1 min over 26 runs; `release.yml`'s 45 min works only because it starts after
tagging.

release-plz also stops dispatching `ci.yml` on the release PR by default. The
dispatch runs only while the PR carries the `release:ready` label. Of the 33
release-PR dispatches from 09-21 to 09-28, a longer window than the 27 pushes
above, 11 were cancelled mid-flight.

### Smaller changes

- Every `rust-cache` step in `quality.yml` and `ci.yml` saves only on
  `refs/heads/master`. PR runs restore master's caches and write none, which
  keeps the cache under its 10 GB limit. `engine-integration.yml` is exempt: it
  never runs on a master push, so a master-only save would leave it cold
  forever.
- pr-title and both agent-session-url checks move into `ci-ok` as steps. The
  PR-body check reads the live body from the API on `pull_request` instead of
  the frozen event payload, which closes the gap in #2035 where a body edited
  mid-run could still merge.
- The codeql matrix becomes two legs. `release.yml` waits on the check names in
  `RELEASE_REQUIRED_CHECKS`, whose default lists `ci-ok` and the four current
  codeql legs, so that default changes in the same PR as the matrix. The new
  leg's name has no comma because `release.yml` splits the list on commas. An
  admin deletes the `RELEASE_REQUIRED_CHECKS` repository variable first; it
  equals the default today, so deleting it changes nothing.
- The `helm chart` renders run the checker with `uv run --script`, which
  installs only the pyyaml pinned in the script's inline metadata (#1901), so
  the job stops building the wheel (#2038). That accidental build was the gate's
  only pass over `pyproject.toml`, so the package step gains a `maturin sdist`
  that checks the PyPI packaging in seconds without compiling.

Each item ships as its own PR under #2025. The helm fix goes first as the
cheapest, then the release gate as the largest single cut (about 98 jobs per
merge down to 71).

### What stays fixed

- `ci-ok` is the only required check, with the same name and job id. Branch
  protection requires the `ci-ok` context from the GitHub Actions app,
  `check-release-handoff.py` asserts it, and the release gate finds the job by
  that name.
- `gate-ok` keeps its name, its `needs: [quality, codeql]` and its guard,
  because the title-edit fast path finds it by name. That fast path lets `ci-ok`
  go green on a PR title or body edit without re-running the gate, once it has
  found a successful `gate-ok` from an earlier run on the same head sha. The job
  stays in `ci.yml`; in `quality.yml` the API would call it `quality / gate-ok`.
- A merged job keeps one named step per former check. Every check step runs
  under `!cancelled()`, so one failure never hides another. `always()` is
  reserved for the `ci-ok` verdict step, so a cancelled run's `ci-ok` fails
  closed instead of going green with every step skipped. Check steps avoid it
  because superseded PR runs are cancelled, and `always()` would keep them on
  their runners.
- Each merged job ends with a report step. It writes a table of checks and
  outcomes to the step summary and one `::error` annotation per failure, titled
  with the old job name and the local command that reproduces it. It exits 1
  when a step that must run on this event shows `skipped`. A skipped step reads
  as success to its job, the way a job skipped inside `quality.yml` read as
  success to its caller in #1562, where `ci-ok` went green without reading a
  single commit message.
- `quality.yml` stays secret-free (#734). The only token in it is
  `github.token`, passed to zizmor as step env.
- Advisory steps inside a blocking job are `continue-on-error`, install and
  setup steps included, and never share an install with a blocking tool.
  `semver-checks` stays advisory as ADR-0032 requires; a failure there becomes a
  `::warning::`, never an `::error::`.
- No blocking check is skipped based on the diff. The in-job `codeql (rust)`
  skip on a PR with no Rust change is the one diff-based skip in the gate, and
  it cannot change a verdict: the codeql job never fails on alerts.

## Consequences

### Jobs per run

Jobs that take a runner, today (master at 51fbf06, with image smoke) and after
the whole decision:

| Run                                  | Today                          | After                           |
| ------------------------------------ | ------------------------------ | ------------------------------- |
| PR push                              | 36                             | 11                              |
| PR title or body edit                | 3                              | 1                               |
| master push, `ci.yml`                | 33                             | 10                              |
| master push, release-plz             | 30                             | 3 (5 on a release commit)       |
| master push, release-PR dispatch     | 35                             | 10, only while `release:ready`  |
| isolated merge to `master` (ceiling) | about 98                       | 13, or 23 while `release:ready` |
| master push, measured average        | about 57 (about 59 with #1950) | about 11                        |
| `merge_group` run (queue off today)  | 35                             | 10                              |
| `publish-bootstrap` verify           | 27                             | 6                               |

The measured-average row applies the coalescing rates seen over the 27 pushes to
the new layout. GitHub Code Quality, a repository setting, adds 2 jobs per push
on top of these unless an admin turns it off.

### Latency

The latency estimates come from a 20-slot first-in-first-out replay of the real
09-26 to 09-28 workload, with each merged job's duration set to the sum of its
members minus 5 s per removed job. They are simulations. The simulator
over-predicts today's queueing by 20-50%, so the fair comparison is the "after"
column against the measured one, with the simulated baseline shown for scale.
The measured column and the simulated baseline both predate #1950; the
"after" column includes one image smoke per `quality.yml` call.

| Gate                                                | Measured today (before #1950) | Simulated, today's layout (before #1950) | Simulated, after |
| --------------------------------------------------- | ----------------------------- | ---------------------------------------- | ---------------- |
| PR median, 09-28 13:30-16:12Z (n=13)                | 10.7 min (p90 12.0)           | 14.5 (p90 18.2)                          | 8.3 (p90 10.5)   |
| master push median, 09-28 (n=7)                     | 16.9 (p90 31.5)               | 19.8 (p90 25.1)                          | 9.1 (p90 13.0)   |
| PR median, burst 09-26 23:00 to 09-27 01:00Z (n=12) | 32.0 (p90 47.4)               | 38.3 (p90 45.5)                          | 14.8 (p90 26.0)  |
| master push median, burst (n=7)                     | 27.8 (p90 43.3)               | 30.5 (p90 41.2)                          | 11.6 (p90 15.8)  |

On a moderate day a PR gate gains little: against the measured values, 10.7 to
8.3 min at the median and 12.0 to 10.5 at p90. The large gains are the master
push gate and PR gates during a burst.
Over the 09-28 window, runner time falls from about 101k slot-seconds (the
measured 92.1k plus one image smoke per `quality.yml` call) to a simulated
54.5k, and jobs from 1144 to 345. The no-queue floor stays at about 8.1 min, set
by `codeql (rust)`; PRs with no Rust change (39 of the last 80) drop to the
nextest floor of about 6.5 min.

### What gets harder

A failing check now shows up as a step inside a job. The PR checks list shows
11 names where it showed 36, and a reviewer has to open the red job to see which
check failed. The report step carries the per-check signal, through the step
summary table and the `::error` annotations titled with the old job names.
Re-running one failed check re-runs every check in its job.

One failing tool no longer has a runner to itself. `!cancelled()` keeps the
other steps running after a failure, but a hung step or a lost runner takes its
siblings' verdicts down with it until the job is re-run. Of the merged jobs
only `ui, storybook, docs` carries a job timeout (25 min), and the semver check
has a step timeout. `static checks`, `rust lint` and `rust build` fall back to
GitHub's 360-minute default.

The critical path can move. `rust lint` takes about 450-560 s cold against 488 s
for `codeql (rust)`, so on a `Cargo.lock` bump it can become the longest job.
The same bump builds cold on every PR that carries it, because PR runs no
longer save caches; the next master push saves one.

PR title feedback arrives when `ci-ok` runs, about 8 min after the push. The
separate `pr-title` job used to answer within about half a minute on a quiet
pool: over 55 PR runs its median wait for a runner was 19 s, and its p75 was
237 s.

The msrv job now checks 1.82 for the first time, and it will go red. The
lockfile already resolves redis 1.7.0, tonic 0.14.6 and icu 2.3, which all
declare `rust-version = "1.88"`, while `Cargo.toml` still says 1.82. #2026
decides between raising the declared version and holding dependencies back.
Breakage in msrv, macOS or compose smoke now surfaces up to a day late, as a
tracking issue, rather than on the PR that caused it. Coverage on `master`
becomes a daily number.

Releases gain a manual step: someone labels the release PR `release:ready`
before its CI runs. On a release push, `release-gate` holds one runner and
release-plz's concurrency group for as long as the push gate takes. If it times
out it fails red, and `gh run rerun <id> --failed` on that run is the recovery.
A later push that reaches release-plz first gates its own sha and publishes from
that tree, which is also verified.

The merge queue stays as ADR-0033 describes it, and this decision makes it
affordable: a `merge_group` run drops from 35 jobs to 10. It is still off
(#2029). When it is turned on, its build concurrency should be 1 or 2, not the 5
that `merge-protection.md` recommends, because five group builds at 10 jobs
each would ask for 50 slots out of 20 and starve every PR gate.

The audit's other findings are filed as #2026 through #2035 and #2037; the ones
this decision depends on are linked above.

## Rejected

- A `paths:` filter on `ci.yml`: a workflow skipped by a path filter leaves the
  required `ci-ok` pending forever, so a docs-only PR could never merge.
- Diff-based skipping of blocking jobs: a skipped job inside `quality.yml` reads
  as success to its caller, the mechanism behind #1562. With the queue off, one
  classifier miss lands on `master` unchecked.
- A check-run name lookup for the release gate: anything can post a check-run
  named `ci-ok` on a `master` sha, as the release gate section explains.
- Self-hosted runners for PR jobs, the owner's Pi and workstation included:
  rolter is public, and a fork PR needs approval only when its author is a
  first-time contributor, so a returning contributor's code would run on that
  host unreviewed.
- ARM or `ubuntu-slim` runners: they draw on the same plan pool, so they change
  the hardware without freeing a slot.
- `parallel:` or `background:` steps: actionlint cannot parse them yet
  (rhysd/actionlint#693, open as of v1.7.12), and actionlint is a blocking step.
- Coverage inside nextest: the blocking test verdict would then come from an
  instrumented build.
- Caching bun and Playwright downloads: it would save about 2 s and 26 s per
  run, into a cache that is already over its limit.
- Merging the docs build and deploy jobs: the build tools would then run with
  `pages: write`.
- Folding `image smoke` into `static checks`: the simulation, run before the
  `release:ready` change was added to the layout, gives the same latency either
  way (PR median 8.3 min in both, burst 15.1 min folded against 15.4 separate)
  and it would save one slot per call, but static failures would then show about
  5 min later. It stays separate until #2037 stops the cold build.
