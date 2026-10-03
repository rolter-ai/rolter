# The `ci-ok` gate and the title-edit fast path

`ci-ok` is the single required status check on `master`. Everything else is
aggregated into it, so adding or removing a job never means touching branch
protection settings. The two heavy jobs, `quality` and `codeql`, reach it
through `needs`; the cheap pull-request metadata checks (`pr-title` and the two
agent-session-url checks) run as steps inside it (see
[What runs inside `ci-ok`](#what-runs-inside-ci-ok)). This page records the one
place that aggregation is subtle: the fast path for a pull-request **title
edit**, and the rule that keeps it honest.

## What runs inside `ci-ok`

`ci-ok` needs `[quality, codeql]` and runs on every event (`if: always()`). Its
steps, in order:

| Step                                              | Runs on                                                                   |
| ------------------------------------------------- | ------------------------------------------------------------------------- |
| checkout                                          | `pull_request`, `workflow_dispatch`, `merge_group`                        |
| _assert the gate already ran for this commit_     | `pull_request` with action `edited` and no `changes.base` (the fast path) |
| `pr-title`                                        | `pull_request`                                                            |
| _no agent session urls (pr body)_                 | `pull_request`, `workflow_dispatch`, `merge_group`                        |
| _no agent session urls (commits, dispatch/queue)_ | `workflow_dispatch`, `merge_group`                                        |
| _assert every required check succeeded_ (verdict) | every event, under `always()`                                             |

The checkout takes `fetch-depth: 1` on a pull request, where only the script
is read, and full history on a dispatch or queue run, where the commit-range
step walks the range locally. The expression is
`github.event_name == 'pull_request' && 1 || 0`. That form is safe because its
middle operand, `1`, is truthy. The reverse, `cond && 0 || 1`, would always
yield `1`, since `0` is falsy.

Every check step is guarded by `!cancelled() && <its event guard>`, never by
`always()`. One failing check does not hide the ones after it, and a run that the
concurrency group cancelled spends nothing on checks nobody will read. Only the
verdict step runs under `always()`. In a cancelled run every other step skips,
and a verdict that skipped with them would leave the job, which is the one
required check, green over nothing.

The verdict reads every job result and step outcome through `env:` and reports
every broken rule before it exits, not just the first one:

| Result                                            | Must be                                                                                                                      |
| ------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `quality`, `codeql`                               | `success`; on a metadata-only `edited` run, `success` or `skipped`                                                           |
| _assert the gate already ran_                     | `success` on a metadata-only `edited` run                                                                                    |
| `pr-title`                                        | never `failure`; `success` on any `pull_request` run                                                                         |
| _no agent session urls (pr body)_                 | `success` on every event except `push`, where it is `skipped`                                                                |
| _no agent session urls (commits, dispatch/queue)_ | never `failure`; `success` on `workflow_dispatch` and `merge_group`                                                          |
| `pr-title` skipped on a `workflow_dispatch` run   | allowed, with a `::warning::` that the title went unvalidated (see [below](#the-workflow_dispatch-path-checks-the-body-too)) |

These three checks used to be jobs of their own, `pr-title`, `session-urls` and
`dispatch-commit-urls`, each holding a runner for a few seconds of work and
often queueing minutes to get one (#2025). As steps they cost no extra runner,
and a title or body edit starts one job instead of three. The price is when
title feedback arrives. On a run a commit started (`opened`, `synchronize`,
`reopened`), `pr-title` now runs only after `quality` and `codeql` finish,
about eight minutes in, rather than within seconds. An `edited` run skips
both, so a title fix made after the gate is still checked within seconds.
The action fetches the title live rather than reading it from the payload, so
a title fixed while the gate is still running is the one that step validates.
The body step does the same on a pull request (see
[The `pull_request` path reads the live body](#the-pull_request-path-reads-the-live-body)).

## Why the fast path exists

`ci.yml` triggers on `pull_request: [opened, synchronize, reopened, edited]`.
The `edited` type is there for a single reason: PR titles are validated as
Conventional Commits by `ci-ok`'s `pr-title` step, so fixing a bad title has to
be able to turn `ci-ok` green **without a new commit**. Without an `edited`
trigger, the only way to re-run the title check would be to push an empty
commit, which invalidates every review and re-runs a twenty-minute gate for a
typo.

The heavy jobs are skipped on a title or body edit: a title lives in GitHub's
database, not in the tree, so no test result can change because of it. The tree
that was gated is the same tree. That is only true of a _metadata-only_ edit,
which is why the guard on `quality`, `codeql` and `gate-ok` reads
`changes.base` as well as the action (see the next section).

## A retarget is not a metadata edit (#2031)

GitHub sends `pull_request` `edited` for three different things: a title edit, a
body edit and a **base-branch change**. Only the first two leave the gated tree
alone. A retarget changes it twice over: the checkout `quality` builds is the
merge of the head into the base, and `quality.yml`'s commit-range checks scan
`base.sha..head.sha`. A gate run made against the old base says nothing about
the new one, yet it sits on the same head sha, which is all the fast path used
to look at.

Every stacked merge hits this. When a parent merges, GitHub retargets its child
from the parent's branch onto `master` with an `edited` event, and the fast path
reported `ci-ok` green off the gate run made against the parent branch. #1610
(head `eacb837c`) was retargeted at 2026-09-17T21:13:22Z; run 35275442183
skipped the gate, went green off run 35273857488, and #1610 merged three
minutes later without ever having been gated against `master`. #1609 (runs
35275326391, 35273843569) and #1863 (run 36280523944) went the same way.

GitHub marks a retarget in the payload: an `edited` event that changed the base
carries `github.event.changes.base` (the old `ref` and `sha`), and one that only
changed the title or body does not. So a metadata-only run is defined once, as

```
github.event_name == 'pull_request' && github.event.action == 'edited' && !github.event.changes.base
```

and every guard around the fast path uses exactly that expression:

- `quality`, `codeql` and `gate-ok` skip on it, and on nothing else, so a
  retarget runs the full gate against the new base and records its own
  `gate-ok` verdict.
- `ci-ok`'s _assert the gate already ran_ step runs on it, and on nothing else.
- `ci-ok`'s verdict reads it through `env:` as `METADATA_ONLY` and accepts a
  skipped `quality` or `codeql` only when it is `true`. A retarget therefore
  needs both to succeed outright, like a run a push started.
- The concurrency group gives only a metadata-only run a per-run group. A
  retarget joins the shared `gate` group like any other gate run, so it
  supersedes a gate still running against the old base rather than racing it
  (see [Concurrency](#concurrency)).

`scripts/test-assert-gate-ran.sh` checks this wiring: every `${{ }}` expression
in `ci.yml` that mentions `edited` must also exclude `changes.base`, the three
gate jobs must skip on exactly the metadata-only expression, and the verdict
must branch on `METADATA_ONLY`.

### A pass against an old base does not count (#2649)

Running the gate on a retarget closed only half of it. The retarget's gate run
sits on the same head sha as the one made against the old base, so a head sha
can carry a pass and a failure for two different bases. The fast path used to
accept any passing `gate-ok` on the sha, so this sequence went green:

1. The PR is gated green against base A.
2. It is retargeted to base B, and the gate fails against B.
3. Somebody edits the title or body.
4. The fast path finds the pass against A and reports `ci-ok` green.

So `gate-ok` records the base it gated, and the fast path accepts only a pass
against the base the PR targets **now**. The record is the name of `gate-ok`'s
one step:

```yaml
- name: gated against base ${{ github.event.pull_request.base.sha || 'none' }}
```

The runner renders a step name's expression when the step starts, so the jobs
listing the fast path already fetches for the verdict carries the base too: no
extra API call, no extra permission, and nothing written that a fork PR's
read-only token could not write. The fast path takes the current base from its
own payload (`BASE_SHA: ${{ github.event.pull_request.base.sha }}`) and counts a
run only when its `gate-ok` concluded `success` _and_ its step reads
`gated against base <BASE_SHA>`.

The run object's own `pull_requests[].base.sha` looks like the obvious source
and is not one: the API fills `pull_requests` in from the pull request as it is
when the run is read, not as it was when the run started. Run 37069049215 was
started on #2587's head `8727bdb8`, and read two hours after `4712bdf2` was
pushed it reported `pull_requests[0].head.sha` as `4712bdf2`. An old run reads
back the current base the same way, so comparing it to the current base would
always match. It is also empty once the PR is closed, and the listing the fast
path uses passes `exclude_pull_requests=true`.

What counts as "no pass against this base", and is red:

- A pass whose step recorded a different base sha — the case above.
- A pass that recorded `none`. A run with no pull request in its payload
  (`workflow_dispatch`, which is how the release PR is gated, and `push`)
  gated the head on its own, not merged into any base. An `edited` run on such
  a PR needs a `pull_request` gate run, or a new commit.
- A pass from a `gate-ok` without the step at all, which is every run made
  before #2649 landed. A PR gated before then and only retitled afterwards
  goes red, and needs a push or a re-run of its gate run followed by a re-run
  of the `edited` run.
- A step name that is not a full sha, such as an expression left unrendered.

A gate run whose `gate-ok` has not concluded has no rendered step yet, so its
base is unknown while it runs. The fast path waits for it as before and judges
it by its base once it has concluded.

## Why that was a hole

Every run of `ci.yml` writes a check-run named `ci-ok` against the pull
request's **head sha**. Branch protection, and anything reading
`check-runs?check_name=ci-ok`, resolves the _newest_ check-run with that name.
That holds for a green check-run landing before any other `ci-ok` exists on the
sha, which is the hole below. It does not let a green one clear a red one:
once both are there, the commit's status rollup lists both and reads `FAILURE`
until the red run is re-run (#2391).

An `edited` run finishes in under a minute, because it skips the gate and runs
only the metadata checks. So retitling a PR while the real gate run on the same
sha is still in progress produced a newer, successful `ci-ok` on that sha, and
the commit became mergeable before a single test had run. That is not
theoretical: on 2026-09-08, #1315 (head `67586128`) and #1267 (head `cae26a31`)
were each rebased, pushed and retitled inside a minute, ended up with two `ci`
runs per sha — one `edited` run green at 17:50:08Z, one real `pull_request` run
still `in_progress` — and #1315 was squash-merged on the hollow green. The gate
went green afterwards, so `master` survived on luck (#1328).

An earlier round of this (#767) had already made the `edited` run harmless in
one direction, by giving it a per-run concurrency group so it cannot _cancel_
the gate run it is racing. What was left was the other direction: it could still
_outrank_ it.

## The rule

The `edited` fast path may report green only when the gate has demonstrably
finished, successfully, on this exact head sha. `ci-ok` asserts that before it
accepts a skipped `quality`, by listing the other runs of `ci.yml` on the same
head sha (the _assert the gate already ran for this commit_ step of
`.github/workflows/ci.yml`, which runs `scripts/assert-gate-ran.sh`):

- **Another `ci.yml` run on this sha that is not `completed` and whose
  `gate-ok` job has no conclusion yet** is a gate still in flight. The step
  **waits** for it, polling once a minute for up to 90 minutes, and then reports
  the verdict that run delivered. It never reports green while such a run is
  pending, and a gate that has still not concluded at the deadline fails the
  step. See [Waiting for an in-flight gate](#waiting-for-an-in-flight-gate-2391)
  for why it waits rather than declining.
- **No run on this sha whose `gate-ok` job succeeded against the PR's current
  base** fails the step. This closes the same hole in its other shape: a
  retitle over a gate run that _failed_ also used to write a newer green
  `ci-ok`. A pass against a base the PR has since been retargeted away from
  does not count (see
  [A pass against an old base does not count](#a-pass-against-an-old-base-does-not-count-2649)).
- **A `cancelled` run does not count as a pass.** It is `completed`, so it does
  not block as in-flight, but it carries no verdict — it is treated exactly like
  a missing run, which is to say the fast path stays red until a real gate run
  succeeds.
- **A failed API query is never green.** Both the run listing and each run's job
  listing fail the step when they cannot be read. No answer is not an answer.

### `gate-ok`: what the guard actually asks

The question the fast path needs answered is narrow — _did the heavy gate run on
this sha, and did it pass_ — and for a long time it was asked in a way that
answered something broader: _is there a run on this sha whose **overall
conclusion** is `success`_.

Those differ whenever a run fails on something that is not the gate. A run can
pass `quality` and `codeql` and still end `failure` because the PR-body check
rejected the body. Under the old question that run counted as **no gate at
all**, and since the `opened` run is the only one that runs the gate and
`edited` runs never re-run it, nothing could ever make that sha green again —
the failure was permanent and unfixable from the PR side (#1522).

So `ci.yml` has a `gate-ok` job that records the gate's verdict by itself:

```yaml
gate-ok:
  if: ${{ !(github.event_name == 'pull_request' && github.event.action == 'edited' && !github.event.changes.base) }}
  needs: [quality, codeql]
  steps:
    - name: gated against base ${{ github.event.pull_request.base.sha || 'none' }}
      run: echo "quality and codeql both succeeded on this run"
```

It is deliberately trivial, and two things about it are load-bearing:

- **No `if: always()`.** With the default condition the job runs only when every
  job it needs succeeded, so a failed gate leaves `gate-ok` `skipped` rather
  than adding a second red check beside `ci-ok`. `skipped` is not `success`, so
  the guard reads it correctly either way.
- **It never re-derives `quality`'s verdict.** `needs.quality.result` is
  computed by Actions, which already accounts for what `continue-on-error`
  covers inside `quality.yml` (the `coverage` job, the `semver-checks` steps of
  `rust build`) and for `coverage` being
  skipped on every event but `pull_request`. Any hand-rolled scan of job
  conclusions would get those wrong.

The guard then walks the other `ci.yml` runs on the sha and asks each one
whether it has a `gate-ok` job that concluded `success`. A cancelled run has no
such job, so it still counts as no run at all — `#1328` stays closed. A run
whose `gate-ok` has concluded counts even while that run's own `ci-ok` is still
going: what is left of it is the title and body steps, which say nothing about
the gate.

Details that matter if you touch this code:

- The listing is scoped to `ci.yml` by filename. A run of `docs.yml` or
  `ui-e2e.yml` on the same sha says nothing about this gate.
- It is keyed on `github.event.pull_request.head.sha`, **not** `github.sha`. On
  a `pull_request` run `github.sha` is the throwaway merge commit; check-runs and
  workflow runs hang off the PR head.
- The current run appears in its own listing and is excluded **by run id**
  (`github.run_id`), never by name — every run of this workflow shares a name.
- The wait is on `gate-ok`, not on the run. An `edited` run's `gate-ok` is
  `skipped` within seconds of it starting, so two edits that race each other
  never wait on one another, and neither waits on the other's title and body
  steps.
- `ci-ok` carries `timeout-minutes: 110`, above the 90-minute wait, so the
  script's own deadline message is what fires rather than a bare job timeout.
  Every other path through the job takes seconds.
- `scripts/test-assert-gate-ran.sh` runs the script against a fake `gh` and a
  fake clock: a gate that passes or fails while the edit waits, one that never
  finishes, concurrent edits, a pass against an old base beside a failure
  against the current one, and a flaking or malformed API. It also checks
  the step's wiring and that the job timeout sits above the wait. It runs as the
  _ci-ok fast path against a fake gh_ step of the `static checks` job and as the
  `assert-gate-ran` prek hook.
- The job carries `actions: read` for the listing, and the query writes to a
  file that `jq` then reads. The step runs under `set -o pipefail`, where
  piping into `head` or `grep -q` fails the step for the wrong reason when the
  reader short-circuits; this repo has fixed that bug twice already (#1291,
  #1306).

`ci-ok` remains the single required status check. No branch-protection change
is needed for any of this.

### Waiting for an in-flight gate (#2391)

Editing a PR body during a long gate run is an ordinary two-step — write the
body, then add the follow-up issue numbers once those issues exist — so an
`edited` run that starts while the gate is still running is common.

That run used to decline at once: `ci-ok` red with `gate still running on
<sha>`, on the theory that the gate run would write its own, newer `ci-ok` when
it finished and that the newer one would win. GitHub does not work that way.
The commit's status rollup lists **both** `ci-ok` check-runs, the red one and
the green one, the rollup is `FAILURE`, and the pull request stays `BLOCKED`
until somebody re-runs the red `edited` run by hand. Every body edit during a
gate cost a manual re-run.

So the step now waits. While another run on the sha owes a gate verdict, it
logs a `::notice::` saying so and polls once a minute; once every such run's
`gate-ok` has concluded, it reports what they said. The `edited` run ends green
when the gate passed and red when it failed — the same verdict, on both
`ci-ok` check-runs. The rule it enforces is unchanged: it still never reports
green over a gate that failed or is still running. The cost is a runner held
for the rest of the gate, doing nothing but two API calls a minute.

The messages it can end with:

| Message                                             | Means                                                                          | Action                                                                                  |
| --------------------------------------------------- | ------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------- |
| `gate already passed for <sha> against base <base>` | green: a run on this sha recorded a passing `gate-ok` against the current base | none                                                                                    |
| `no ci run on <sha> recorded a passing gate-ok job` | the gate failed, was cancelled, or never ran against the current base          | push a fix; if you re-run the gate instead, re-run this `edited` run too once it passes |
| `gate still running on <sha> after 90m`             | the gate had not concluded after the whole wait                                | once it finishes, `gh run rerun <edited run id> --failed`                               |

A red `ci-ok` on a head sha keeps the pull request blocked until that same run
is re-run green, whichever run wrote it. That is why the two red rows say to
re-run the `edited` run: re-running only the gate run leaves the old red
`ci-ok` in the rollup. A new commit clears all of it, since the rollup is per
head sha. Do not merge with an admin bypass instead; the point of the check is
that every `ci-ok` on the sha is trustworthy.

### Why the unfinished case is red rather than grey (#1511)

"The gate has not finished" is not a failure, and GitHub has a conclusion for
it — `neutral`, which renders grey and does **not** satisfy a required status
check. Reporting it that way was proposed and rejected. The reasons are
recorded here so it is not re-litigated:

- **A GitHub Actions job cannot conclude `neutral`.** A job ends `success`,
  `failure`, `cancelled` or `skipped`; nothing a step does changes that.
  `neutral` is reachable only for a check-run posted through the Checks API,
  which `ci-ok` is not — it is a job, and its check-run is written by Actions.
- **Posting a second `ci-ok` check-run through the API would be worse.** The
  job's own check-run is still written, so the two race, and branch protection
  reads whichever lands last. The guard exists precisely because the newest
  `ci-ok` on a sha must be trustworthy (#1328).
- **Self-cancelling the run to reach `cancelled` (grey) costs more than it
  buys.** It needs `actions: write` on `ci-ok` — the one required status check
  — purely for a colour, and `ci-ok` runs `if: always()`, so a run whose
  PR-body check genuinely failed would go grey and hide a real finding unless
  the cancel were conditioned on every other check's result first.

So the colour stayed, and the message was reworded to say no action was
needed. That advice turned out to be wrong: the red was never superseded, and
the pull request stayed blocked until the run was re-run by hand. #2391 removed
the case rather than recolouring it — the `edited` run waits for the gate
instead of ending while it is unfinished (see
[Waiting for an in-flight gate](#waiting-for-an-in-flight-gate-2391)). Red now
only ever means the gate failed, or that it had not finished after the whole
wait.

## The merge queue

`master` is meant to merge through a merge queue ([ADR-0033](../adr/2026-09-18-merge-queue.md)),
so `ci.yml` also triggers on `merge_group`. **The queue is not switched on today**
(#2029), so the trigger is inert and has never fired — see
[merge protection on `master`](merge-protection.md). Read this section as how it
behaves once it is. That run checks out a synthetic ref —
`refs/heads/gh-readonly-queue/master/pr-<n>-<sha>` — holding `master` plus every
entry ahead of this one in the queue, and reports the same `ci-ok` against it. It
is the only run that ever sees the tree that will actually exist, which is the
whole point: `ci-ok` on a PR head says nothing about a semantic conflict with
something that landed after the branch was cut (#1318).

`ci-ok` stays the single required status check. The queue asks the repository for
its required checks by name, so there is no second name to configure and no
second gate to keep in sync.

### A merge-group ref must never take the fast path

This is the one rule where the queue and the title-edit fast path meet, and it
runs the wrong way by default if nobody thinks about it. The fast path reports
green _without running the gate_, on the strength of an earlier run against the
same head sha. A merge-group tree has no earlier run — it was assembled seconds
ago and nothing has ever built it — so "the gate already passed here" is not a
claim that can be true. A merge-group run that took the fast path would report
green having tested nothing, inside the mechanism that exists to stop exactly
that.

Today `merge_group` carries `action: checks_requested`, so a guard written as
`github.event.action != 'edited'` already lets the gate run. That is an accident
of GitHub's event vocabulary, not a rule. So every guard around the fast path is
scoped to the event as well as the action:

|                                               | guard                                                                                                                  |
| --------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `quality`, `codeql`, `gate-ok`                | `!(github.event_name == 'pull_request' && github.event.action == 'edited' && !github.event.changes.base)`              |
| `ci-ok`'s _assert the gate already ran_ step  | `!cancelled() && github.event_name == 'pull_request' && github.event.action == 'edited' && !github.event.changes.base` |
| `ci-ok`'s verdict branch for the skipped gate | `"${METADATA_ONLY}" = "true"`, where `METADATA_ONLY` is that same expression rendered through `env:`                   |

Scoping by event cannot stop being right when GitHub adds an event or reuses an
action name. The `changes.base` term keeps a retarget, which is an `edited`
event on a new tree, off the fast path (see
[A retarget is not a metadata edit](#a-retarget-is-not-a-metadata-edit-2031)).

### What runs, and what is allowed to skip

| Job or step                                                    | On `merge_group`                | Why                                                                                                                |
| -------------------------------------------------------------- | ------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `quality`, `codeql`, `gate-ok`                                 | run                             | the point of the run                                                                                               |
| `ci-ok` step _no agent session urls (pr body)_                 | runs, and must succeed          | a squash merge writes the body into the commit message, and the queue is what performs the merge                   |
| `ci-ok` step _no agent session urls (commits, dispatch/queue)_ | runs, and must succeed          | the only thing that reads the commit messages of PRs batched ahead of this one                                     |
| `ci-ok` step `pr-title`                                        | skipped                         | the payload has no title, and nothing enters the queue without a green `ci-ok` on the PR, where `pr-title` did run |
| `static checks` step _gitleaks (branch history)_               | runs, over `base_sha..head_sha` | scans the commits the queue is about to write to `master`, entries batched ahead of this one included              |
| `quality` job `coverage (informational)`                       | skipped                         | informational and `pull_request` only; `extended.yml` runs the same job on `master` nightly                        |

The two session-url steps resolve their subject differently here, because a queue
ref belongs to no pull request head and `--pr-for-ref` cannot match it:

- the **body** is fetched with `--pr-for-queue-ref`, which parses the PR number
  out of `gh-readonly-queue/<base>/pr-<n>-<sha>` and looks it up in the open-PR
  listing. A queued PR is open until the queue merges it, so not finding it is a
  hard failure rather than a pass — the same _no answer is not an answer_ rule
  the rest of this file follows. Like the other modes it honours
  `ROLTER_PULLS_JSON`, so it is runnable against a fixture:

  ```bash
  ROLTER_PULLS_JSON=pulls.json bash scripts/check-agent-session-urls.sh \
    --pr-for-queue-ref rolter-ai/rolter gh-readonly-queue/master/pr-1318-deadbeef
  ```

- the **commits** need no lookup at all: the `merge_group` payload names both
  endpoints, so the range is `base_sha..head_sha` through the plain
  `--commit-range` mode.

Both of those steps live in `ci.yml`'s `ci-ok` job rather than in `quality.yml`,
because resolving a pull request needs a token and `quality.yml` deliberately
takes none (#734).

Two places inside `quality.yml` did need adjusting, both steps of the
`static checks` job today. `migrations append-only` diffs against
`origin/master`, and a merge-queue checkout is a synthetic ref with no
`origin/master` fetched — the script's _no such ref; skipping_ branch would have
turned the gate into a silent no-op on exactly the runs that matter. It now takes
its base from `github.event.merge_group.base_sha`, which the payload provides and
which is an ancestor of the queue head, falling back to `origin/master`
everywhere else. A called workflow sees the original event, so the expression
resolves inside `quality.yml` without ci.yml having to pass anything in, and no
secret is involved.

The other is the `gitleaks (branch history)` step. On a pull request it reads
`pull_request.base.sha..head.sha`, and a `merge_group` payload has no
`pull_request` object, so until #1722 the step skipped itself and only the
working-tree pass ran. The working-tree pass is the half that matters for the
merge gate, since a secret that survives into the merged tree is caught either
way. The history pass is the only one that reads commits, though, and on a queue
run those are the commits `master` is about to receive: this entry's plus every
entry batched ahead of it, which no single per-PR run ever scanned as one range.
It now takes `merge_group.base_sha..merge_group.head_sha` from the payload, the
same way `ci-ok`'s commit-range step does, so it needs no API call and no token.
`base_sha` is an ancestor of the queue head, and the `static checks` job checks
out with `fetch-depth: 0`, so the commit is in the clone.

### Concurrency

`merge_group` runs get a per-run concurrency group, alongside `push` and a
metadata-only `edited` run. A retarget is not one of them: it runs the gate, so
it shares the pull request's `gate` group and cancels a gate still running
against the old base (#2031). A cancelled run is not a passing required check, so cancelling a
merge-group run dequeues the PR it was testing _and_ everything batched behind
it. GitHub does give each queue entry its own ref, so `github.ref` alone would
usually be unique — but the queue re-forms that ref when an entry ahead of it
fails, and the replacement must not shoot down a run that is still reporting. The
per-run group makes that impossible rather than unlikely.

## Agent session urls

`scripts/check-agent-session-urls.sh` rejects a coding-agent session or
remote-connection url — a `claude.ai/code/session…` link, or any agent's own
`<Name>-Session:` git trailer carrying a url — in a commit message or a pull
request body. It runs as the `no-agent-session-urls` prek hook, as the
`no agent session urls (commits)` step of `quality.yml`'s `static checks` job
over every commit the PR introduces, and as two steps of `ci-ok` in `ci.yml`:
_no agent session urls (pr body)_ over the PR body, read live from the API, and
_no agent session urls (commits, dispatch/queue)_ over the commit range on a
dispatched or merge-queue run, where the `quality.yml` step cannot see one. The
links are ephemeral and sometimes private, and a merged commit message can only
be corrected with a history rewrite, so the rule has no exceptions.

The part that surprises people is that the offending line is often not one the
author wrote. **PR-authoring tooling for a coding agent may append a
session-scoped footer to the body after the author's own content** — a
`_Generated by [Claude Code](https://claude.ai/code/session_…)_` line, or the
equivalent `<Name>-Session:` trailer on a commit — so the check fires on text
that first became visible when it failed. It is still a real hit: the url is in
the body, and the body is what everyone reads.

The convention, for a human or an agent opening a PR:

1. After creating the PR, read the body back and confirm it carries no session
   url. Do not assume the body you submitted is the body that was stored.
2. If the tooling injected one, strip **just that line** with a direct
   `PATCH /repos/{owner}/{repo}/pulls/{n}` (for example
   `gh api -X PATCH repos/rolter-ai/rolter/pulls/<n> -f body=@body.md`), not
   another pass through the authoring tool. The footer is appended on the
   creation path only, so a direct `PATCH` does not re-trigger it and the edit
   sticks. Strip it as soon as you see it: `ci-ok` reads the live body after
   the gate, so the run that opened the PR goes green on its own once the line
   is gone. See [Stripping an injected footer](#stripping-an-injected-footer)
   below for the red `edited` run you may see first.
3. For a commit message, the equivalent is an amend or rebase that drops the
   trailer before the push — `quality.yml` re-checks every commit in the range,
   so a `--no-verify` push does not get through.

This is a workaround for tooling behaviour outside this repository's control,
not a relaxation of the rule. The detection stays unconditional: **never add a
regex exception, an allowlist or a special case for "Generated by Claude Code"
or any other footer text to the script's `PATTERN`.** The value of the check is
that it has no exceptions — a carve-out keyed on "the footer" is one an agent
can talk its way into, and it would let a genuinely private session link ride
into history inside a line that merely looks generated.

### The `workflow_dispatch` path checks the body too

`ci.yml` can be triggered manually, and that trigger is not decoration: the
release PR is opened by release-plz with the repo `GITHUB_TOKEN`, GitHub
suppresses downstream events for token-created refs, and so neither `push` nor
`pull_request` ever fires for it (#1025). `ci.yml` is dispatched against the
release branch to gate it once a maintainer labels the PR `release:ready`: by
`release-pr-ready.yml` when the label is added, and by `release-plz.yml` on
each later push while it stays (see
[cutting a release](packaging.md#cutting-a-release)).

The trap is that a dispatched run skips everything guarded by
`github.event_name == 'pull_request'`, which once included both the PR-body
check and `pr-title`, while `quality` and `codeql` still run, because their
guard is on `action`, which is empty on a dispatch. `ci-ok` used to accept both
skips, so a dispatched run reported green having never looked at the PR body.
That made dispatching the cheapest way to clear a red PR, and it is how #1519
and #1521 actually went green (#1523).

So the _no agent session urls (pr body)_ step runs on `workflow_dispatch` as
well. With no pull request in the event payload it resolves one from the API by
head ref:

- **an open PR has this ref as its head** → its body is checked, exactly as on a
  `pull_request` run. This includes the release PR, which is the whole reason
  the trigger exists.
- **no open PR has this ref** → nothing to check, and the step says so with a
  `::notice::` and succeeds.
- **the API listing still fails after three attempts** → the step fails. A
  flaking query must never resolve to green; the same rule `ci-ok`'s own run
  listing follows.

That resolution lives in `scripts/check-agent-session-urls.sh --pr-for-ref`
rather than inline in the workflow, so it can be exercised without a CI run:
point `ROLTER_PULLS_JSON` at a file shaped like the API response and no network
call is made.

```bash
ROLTER_PULLS_JSON=pulls.json \
  bash scripts/check-agent-session-urls.sh --pr-for-ref rolter-ai/rolter some/branch
```

Workflow-embedded shell is shell nobody can run, and this logic decides whether
a gate reports green.

`ci-ok`'s verdict was tightened to match: a skipped PR-body check is accepted
**only** on a `push` build, the one case with no pull request to check.
Anywhere else, a skip is a failure rather than a pass.

#### The commit half of the dispatch path

The body was only one half. `quality.yml`'s `session-urls` job — the one that
reads the commit messages the PR introduces, today the
`no agent session urls (commits)` step of `static checks` — took base and head
from `github.event.pull_request`, so a dispatched run skipped it. A skipped job
inside a reusable workflow does not fail it, so `quality` still reported
`success` and `ci-ok` went green having never read a commit message (#1562).

That is the wrong place to have no verdict twice over: the release PR **only**
ever gets a dispatched run, and before `gate-ok` a dispatch was how a head sha
whose `opened` run failed its body check got unstuck (#1522). A commit message is
also the half that cannot be fixed after the fact — correcting a merged one
takes a history rewrite.

`ci-ok` therefore runs a _no agent session urls (commits, dispatch/queue)_ step
on `workflow_dispatch` and `merge_group`. On a dispatch it resolves the range
through `--commit-range-for-ref`; on a queue run the payload names both ends
(see [the merge queue](#the-merge-queue)). It lives in `ci.yml` rather than
beside its `pull_request` twin **because `quality.yml` takes no secrets**
(#734): every job there scans the checked-out source with pinned public tooling,
so the gate behaves identically on dependabot and fork PRs, which receive none.
Resolving a pull request needs a token, so it belongs on this side of the line.
The `pull_request` path in `quality.yml` is unchanged.

Since #2025 that path is a step inside `static checks` rather than a job, and a
skipped step does not fail a job any more than a skipped job fails a workflow.
The job's `report` step closes that gap for every step it holds: it knows which
events each step must run on and fails the job when one of them was skipped
there (see [the static checks job](testing.md#the-static-checks-job)).

The contract matches the body half: no open PR for the ref is a `::notice::`
and a pass, a failed API listing fails the step, and a range whose endpoints are
not both in the clone fails rather than silently checking nothing. The `ci-ok`
checkout takes full history on these two events for that reason. The verdict
treats a `failure` here as fatal on any event, and additionally requires
`success` on a dispatched or queued build — a skip is honest only where the
step does not apply.

Both `--pr-for-ref` and `--commit-range-for-ref` share one lookup helper and
one fixture override, so neither is shell that only CI can run:

```bash
ROLTER_PULLS_JSON=pulls.json \
  bash scripts/check-agent-session-urls.sh --commit-range-for-ref rolter-ai/rolter some/branch
```

`pr-title` still cannot run on a dispatch — the action it uses takes the pull
request out of the event payload, and there is no supported way to hand it one.
The remaining gap is therefore narrow: two PRs take the dispatch path, and a
workflow writes both titles. release-plz titles the release PR, and
`ui-security-updates.yml` gives its PR a fixed title that
`ui/scripts/security-updates.test.ts` checks (see [UI security
updates](testing.md#ui-security-updates)). `ci-ok` emits a `::warning::` naming
that the title went unvalidated rather than letting a silent skip imply
otherwise.

### The `pull_request` path reads the live body

On a `pull_request` run the _no agent session urls (pr body)_ step reads the
body from the API (`GET /repos/{owner}/{repo}/pulls/{n}`) through
`scripts/check-agent-session-urls.sh --pr-number`. It does not read
`${{ github.event.pull_request.body }}`. The PR number is the only field it
takes from the payload, and that never changes.

The payload's copy of the body is frozen when the event fires, and the step
runs only after `quality` and `codeql`, minutes later. Reading that copy left a
hole (#2035). Say a session URL lands in the body while a `synchronize` run is
still running the gate:

1. The edit starts an `edited` run. Its body step fails on the new body (at the
   time, its fast path also declined because the gate had not finished). That
   `ci-ok` is red.
2. The `synchronize` run finishes the gate. Its body step checks the payload,
   which predates the edit, and passes. That `ci-ok` is green.
3. The green one is the newer `ci-ok` check-run on the head sha, so it is the
   one branch protection reads. The PR is mergeable with the URL in its body.

The repository's squash message is currently the commit list
(`squash_merge_commit_message: COMMIT_MESSAGES`), so a merge today does not copy
the body onto `master`. What leaks is the PR body itself, and the rule covers the
body as firmly as a commit message. #1327 is the
mirror image of this: there an `edited` run reported green before the gate had
run, here a gate run reported green over an edit it never saw.

A live read closes it. The gate run's body step now reads the body after the
gate, so it sees any edit made while the gate ran. An edit made after that
starts an `edited` run, and that run's newer `ci-ok` reads the edited body.

Two objections kept the payload until #2025, and neither holds any more:

- **The read would race the strip.** While the check was a job of its own it ran
  seconds after the PR was created, so a live read would usually have seen an
  injected footer before anyone could remove it. As a step of `ci-ok` it runs
  after the gate, and a strip made in the first minutes is already in place.
- **A fork's read-only token might not be allowed to read the PR.** It is.
  `pr-title` already fetches the same PR with the same token on every
  `pull_request` run (`pulls.get` in `amannn/action-semantic-pull-request`,
  which re-reads the PR because the payload can be stale), and `ci-ok` holds
  `pull-requests: read`. Dependabot PRs, whose token is read-only just as a
  fork's is, pass that step today.

The read gets three attempts, five seconds apart, and then fails the step. A
response that is not the requested PR, such as another number or something that
is not JSON, fails it too. Every lookup in the script goes through the same
helper, so the dispatch and merge-queue listings follow the same rule.

`scripts/test-agent-session-urls.sh` covers this. It extracts the step's `run:`
block from `ci.yml` and runs it against a fake `gh` and a fake `sleep`. A clean
payload body with a dirty live body must fail, a flaking API must be retried, an
API that stays down must fail, and the dispatch and merge-queue branches resolve
their PR through the same fake. It also fails if the step's `env:` names
`github.event.pull_request.body` again. It runs as the _pr body lookups against a
fake gh_ step of the `static checks` job in `quality.yml` and as the
`agent-session-urls` prek hook:

```bash
bash scripts/test-agent-session-urls.sh
```

### Stripping an injected footer

Since the live read, the quickest way to green is to strip the footer as soon as
you see it:

1. Read the PR body back after creating the PR.
2. If a session URL is there, strip that line with a direct
   `PATCH /repos/{owner}/{repo}/pulls/{n}`. There is nothing to wait for.
3. The `edited` run the strip starts waits for the opening run's gate, then
   reads the stripped body and goes green with it. When the opening run's gate
   finishes, its own body step reads the stripped body too, and both `ci-ok`
   check-runs are green.

If the strip lands after the opening run's body step has already read the dirty
body, that run ends `failure`. Its gate still recorded a passing `gate-ok`,
though, and the `edited` run the strip starts finds it and goes green. The
opening run's red `ci-ok` is still in the rollup and keeps the pull request
blocked (#2391), so re-run it: `gh run rerun <opening run id> --failed` re-runs
only `ci-ok`, whose body step now reads the stripped body. No new commit, no
dispatch.

That second case used to strand the head sha. The `opened` run was the only one
that ran the heavy gate, and the `edited` fast path found no **successful run**
to point at. The only ways out were a new commit or a manually dispatched run,
neither of them documented, and the latter only working by accident (#1522).
`gate-ok` closed that: the fast path looks for a passing gate, not a passing
run.

Before the live read, the order was the other way round. The body step read the
payload frozen at `opened`, so a footer present at that moment failed the
opening run whatever happened next, and the advice was to wait for that run to
finish before stripping. Stripping early made the `edited` run decline on the
_unfinished gate_ branch and cost a second edit. Seen on #1565, the first PR
opened after `gate-ok` landed (#1566):

| Run         | Event    | Started  | Outcome                                                                                   |
| ----------- | -------- | -------- | ----------------------------------------------------------------------------------------- |
| 35249017068 | `opened` | 16:50:04 | `session-urls`, then a job of its own, failed on the frozen body; **`gate-ok` succeeded** |
| 35249064201 | `edited` | 16:50:35 | `ci-ok` red — _gate still running_, the strip was too early                               |
| 35249935368 | `edited` | 16:59:31 | `ci-ok` **green** off the same `gate-ok`, no new commit                                   |

Today the first run's body step would run after its gate, well after the
16:50:35 strip, and that run would have gone green by itself.

A red `ci-ok` from the fast path now means the gate itself failed (or had not
finished after the whole wait) — no amount of editing helps; see
[Waiting for an in-flight gate](#waiting-for-an-in-flight-gate-2391).

One timing still leaves a red check behind: a strip that lands in the few
seconds between the gate run's body read and the end of its `ci-ok`. The gate
run fails on the body it read. The `edited` run the strip started goes green
off that run's `gate-ok`, but the gate run's red `ci-ok` stays in the rollup, so
re-run it (`gh run rerun <gate run id> --failed`); its body step reads the
stripped body and goes green.

## Why not a separate `pr-title` workflow

The obvious alternative is to drop the `edited` trigger from `ci.yml` and give
`pr-title` its own tiny workflow with its own check name, so `ci-ok` is only
ever produced by a full gate run. That removes the possibility of a hollow green
rather than detecting it, and it needs no token and no API call.

It was rejected on one point: `pr-title` would stop being aggregated into
`ci-ok`, so branch protection would have to require **two** checks, and that is
a repository-settings change no pull request can make. Until a human made it,
PR titles would be unenforced — a silent regression traded for a loud one. The
`ci-ok`-is-the-only-required-check invariant is load-bearing here, and the API
query keeps it. Since #2025, `pr-title` is a step inside `ci-ok` rather than a
job it needs, which holds the same invariant with no second check to
aggregate at all.

## Every merge commit keeps its run

The same incident showed a second, smaller gap. `ci.yml`'s concurrency group
used to be shared by every push to `master`, so a merge landing a minute after
another cancelled the earlier one's run: merge commit `7fd73db6` was cancelled
by `c6a96378` and `master` had no completed `ci` run for it.

Push runs now get a per-run concurrency group, so a `master` run is never
cancelled and never queued behind another. `cancel-in-progress: false` would
_not_ have fixed this — it queues the newer run instead of cancelling the older
one, and a third push cancels the _pending_ one, so the middle commit still ends
up ungated.

This matters because of the working rule in
[merge protection on `master`](merge-protection.md): the push run is the one
that sees the tree everyone else will branch from, and it is how a semantic
conflict between two independently-green PRs gets caught. A merge commit with no
completed run is a blind spot in exactly the place that rule watches. The cost
is more concurrent runs on a busy merge day, which is the cheaper side of the
trade.

Pull-request runs are unchanged: pushing again to a PR still cancels the
in-flight run for the superseded commit, which is what you want, because nobody
will ever merge that sha.

## Runner image pin

Every workflow runs on `ubuntu-24.04`, never `ubuntu-latest`. GitHub moves the
`ubuntu-latest` label to Ubuntu 26 on 2026-10-19 (#2662), which changes the
preinstalled tools, the default Docker and compose versions and the system
libraries under the gate at once, so the compose smoke test, coverage, the
Playwright `--with-deps` install and the release image build could all start
failing on an unrelated pull request. The pin is deliberate. Moving to
`ubuntu-26.04` is a tracked follow-up (#2736), done on a branch where a
dispatch run of `quality.yml` and `extended.yml` shows what breaks. New
workflows and matrix entries use the explicit `ubuntu-24.04` label too.

## `codeql (rust)` on a pull request with no Rust change

`codeql (rust)` is the longest job `ci-ok` waits on (about 8 min), and about
half of all pull requests touch no Rust at all. On `pull_request` runs its first
step, _detect rust changes_, diffs the merge commit against its first parent
(the base) and looks for a `*.rs` file, a `Cargo.toml`, `Cargo.lock`,
`rust-toolchain.toml` or anything under `.github/codeql/`. With none of those,
the leg skips the toolchain setup, `cargo fetch`, `initialize codeql` and
`analyze`, writes a notice saying so, and ends `success`. The other leg,
`codeql (actions-js-python)`, always runs.

This is the one diff-based skip in the gate ([ADR-0034](../adr/2026-09-29-ci-runner-budget.md)),
and it cannot change a verdict: the codeql job never fails on alerts, so a
skipped analysis and a clean one both leave `codeql` at `success`. The limits
that keep it safe:

- only `pull_request` skips. `push`, `merge_group` and `workflow_dispatch`
  always analyse, so the merge queue runs the full Rust analysis on the tree
  that is about to land, and every `master` commit (and so every release tag)
  keeps a complete one. That queue run is also what catches a pull request that
  breaks the leg itself, since a change to `ci.yml` alone does not count as a
  Rust change;
- the step fails open. A `HEAD` that is not a merge commit, a failed diff or a
  failed `grep` all mean analyse, and the step is `continue-on-error`, so an
  error in it leaves the output unset, which every later guard reads as run.

A skipped PR has no `/language:rust` analysis of its own, so GitHub's code
scanning summary on it may say a configuration present on `master` was not
found. That is expected and blocks nothing.

## Suites that stay out of `ci-ok`

Three workflows run heavy suites that `ci-ok` never waits on: `extended.yml`
(macOS, compose smoke, msrv, coverage on `master`), `ui-e2e.yml` (the
dashboard journeys) and `sso-e2e.yml`. Each one holds a runner for ten minutes
or more, so running them on every pull request would take slots from the
20-job pool that the gate itself queues on
([ADR-0034](../adr/2026-09-29-ci-runner-budget.md)). They run nightly on
`master` and on `workflow_dispatch` instead.

A nightly that nothing reads is no check at all. The dashboard journeys failed
on `master` every day from 2026-09-26, after #2421 and #2514 each broke one, and
nobody noticed until the suite was dispatched by hand (#2677). So
`extended.yml` and `ui-e2e.yml` each end in a `report failure` job that opens
or comments on a tracking issue when a `master` run fails; see
[Nightly extended checks](testing.md#nightly-extended-checks) and
[Nightly dashboard journeys](testing.md#nightly-dashboard-journeys).

`ui-e2e.yml` was deliberately kept off pull requests, even as a non-blocking
path-filtered check. Most pull requests touch `ui/` or
`crates/rolter-control`, so a filter on those paths would start the suite on
almost every push, at about ten minutes a run, for a verdict that does not gate
the merge. The cost of that choice is that a broken journey surfaces up to a
day late, on the tracking issue, rather than on the PR that broke it. A PR
that changes a journey's screen should dispatch the suite on its branch
(`gh workflow run ui-e2e.yml --ref <branch>`). Making it a gate would mean
adding it to `ci-ok`'s `needs:`, which only reaches jobs inside `ci.yml`, and
re-measuring the runner budget first.
