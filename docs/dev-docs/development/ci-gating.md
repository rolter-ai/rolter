# The `ci-ok` gate and the title-edit fast path

`ci-ok` is the single required status check on `master`. Everything else —
`quality`, `pr-title`, `codeql` — is aggregated into it, so adding or removing a
job never means touching branch protection settings. This page records the one
place that aggregation is subtle: the fast path for a pull-request **title
edit**, and the rule that keeps it honest.

## Why the fast path exists

`ci.yml` triggers on `pull_request: [opened, synchronize, reopened, edited]`.
The `edited` type is there for a single reason: PR titles are validated as
Conventional Commits by the `pr-title` job, so fixing a bad title has to be able
to turn `ci-ok` green **without a new commit**. Without an `edited` trigger, the
only way to re-run the title check would be to push an empty commit, which
invalidates every review and re-runs a twenty-minute gate for a typo.

The heavy jobs are skipped on that event (`if: github.event.action != 'edited'`
on `quality` and `codeql`): a title lives in GitHub's database, not in the tree,
so no test result can change because of it. The tree that was gated is the same
tree.

## Why that was a hole

Every run of `ci.yml` writes a check-run named `ci-ok` against the pull
request's **head sha**. Branch protection, and anything reading
`check-runs?check_name=ci-ok`, resolves the _newest_ check-run with that name.

An `edited` run finishes in under a minute, because it only runs `pr-title`. So
retitling a PR while the real gate run on the same sha is still in progress
produced a newer, successful `ci-ok` on that sha, and the commit became
mergeable before a single test had run. That is not theoretical: on 2026-09-08,
#1315 (head `67586128`) and #1267 (head `cae26a31`) were each rebased, pushed
and retitled inside a minute, ended up with two `ci` runs per sha — one `edited`
run green at 17:50:08Z, one real `pull_request` run still `in_progress` — and
#1315 was squash-merged on the hollow green. The gate went green afterwards, so
`master` survived on luck (#1328).

An earlier round of this (#767) had already made the `edited` run harmless in
one direction, by giving it a per-run concurrency group so it cannot _cancel_
the gate run it is racing. What was left was the other direction: it could still
_outrank_ it.

## The rule

The `edited` fast path may report green only when the gate has demonstrably
already finished, successfully, on this exact head sha. `ci-ok` asserts that
before it accepts a skipped `quality`, by listing the other runs of `ci.yml` on
the same head sha (`.github/workflows/ci.yml`, the _assert the gate already ran
for this commit_ step):

- **Any other `ci.yml` run on this sha that is not `completed`** — `queued`,
  `in_progress`, `waiting` — fails the step. The commit is not gated yet, and
  the run that is still going will write its own, newer `ci-ok` when it
  finishes, so nothing is lost by refusing here.
- **No completed run on this sha whose `gate-ok` job succeeded** fails the step
  too. This closes the same hole in its other shape: a retitle over a gate run
  that _failed_ also used to write a newer green `ci-ok`.
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
pass `quality` and `codeql` and still end `failure` because `session-urls`
rejected the PR body. Under the old question that run counted as **no gate at
all**, and since the `opened` run is the only one that runs the gate and
`edited` runs never re-run it, nothing could ever make that sha green again —
the failure was permanent and unfixable from the PR side (#1522).

So `ci.yml` has a `gate-ok` job that records the gate's verdict by itself:

```yaml
gate-ok:
  if: github.event.action != 'edited'
  needs: [quality, codeql]
  steps:
    - run: echo "quality and codeql both succeeded on this run"
```

It is deliberately trivial, and two things about it are load-bearing:

- **No `if: always()`.** With the default condition the job runs only when every
  job it needs succeeded, so a failed gate leaves `gate-ok` `skipped` rather
  than adding a second red check beside `ci-ok`. `skipped` is not `success`, so
  the guard reads it correctly either way.
- **It never re-derives `quality`'s verdict.** `needs.quality.result` is
  computed by Actions, which already accounts for the `continue-on-error` jobs
  inside `quality.yml` (`coverage`, `msrv`, `compose-smoke`, `cross-platform`,
  `semver-checks`). Any hand-rolled scan of job conclusions would get those
  wrong.

The guard then walks the completed `ci.yml` runs on the sha and asks each one
whether it has a `gate-ok` job that concluded `success`. A cancelled run has no
such job, so it still counts as no run at all — `#1328` stays closed.

Details that matter if you touch this code:

- The listing is scoped to `ci.yml` by filename. A run of `docs.yml` or
  `ui-e2e.yml` on the same sha says nothing about this gate.
- It is keyed on `github.event.pull_request.head.sha`, **not** `github.sha`. On
  a `pull_request` run `github.sha` is the throwaway merge commit; check-runs and
  workflow runs hang off the PR head.
- The current run appears in its own listing and is excluded **by run id**
  (`github.run_id`), never by name — every run of this workflow shares a name.
- The job carries `actions: read` for the listing, and the query writes to a
  file that `jq` then reads. The step runs under `set -o pipefail`, where
  piping into `head` or `grep -q` fails the step for the wrong reason when the
  reader short-circuits; this repo has fixed that bug twice already (#1291,
  #1306).

`ci-ok` remains the single required status check. No branch-protection change
is needed for any of this.

### What you see when it fires

`ci-ok` goes red on the `edited` run with `gate still running on <sha>`,
preceded by a `::notice::` spelling out that nothing is wrong with the commit.
This is expected and self-healing: when the gate run finishes it writes its own
`ci-ok` on the same sha, which is newer and wins.

Editing a PR body during a long gate run is an ordinary two-step — write the
body, then add the follow-up issue numbers once those issues exist — so this
red is common. The two branches of the guard are worded to be told apart at a
glance, because they mean opposite things:

| Message                                    | Means                                        | Action                                  |
| ------------------------------------------ | -------------------------------------------- | --------------------------------------- |
| `gate still running on <sha>`              | the gate is fine and unfinished              | none; the in-flight run supersedes this |
| `no completed, successful ci run on <sha>` | the gate failed, was cancelled, or never ran | push a fix or re-run the gate           |

The one case that needs a human is the narrow race where the gate run completes
between the listing and the assertion — then the red `edited` `ci-ok` is the
newest one on a fully gated commit. Re-run the `ci-ok` job (or edit the title
again) and it goes green. Do not merge with an admin bypass instead; the point
of the check is that the newest `ci-ok` is trustworthy.

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
  — purely for a colour, and `ci-ok` runs `if: always()`, so a run where
  `session-urls` genuinely failed would go grey and hide a real finding unless
  the cancel were conditioned on every sibling's result first.

So the colour stays. What changed instead is the message: the unfinished case
now names itself and says no action is needed, rather than reading like the
commit is broken. If this becomes painful again, the fix worth considering is
not the conclusion but the ordering — the red is only a problem while it is the
newest `ci-ok` on the sha.

## The merge queue

`master` merges through a merge queue ([ADR-0033](../adr/2026-09-18-merge-queue.md)),
so `ci.yml` also triggers on `merge_group`. (The trigger is inert until the queue
is switched on in branch protection — see
[merge protection on `master`](merge-protection.md).) That run checks out a synthetic ref —
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

|                                              | guard                                                                                   |
| -------------------------------------------- | --------------------------------------------------------------------------------------- |
| `quality`, `codeql`, `gate-ok`               | `github.event_name != 'pull_request' \|\| github.event.action != 'edited'`              |
| `ci-ok`'s _assert the gate already ran_ step | `github.event_name == 'pull_request' && github.event.action == 'edited'`                |
| `ci-ok`'s shell branch for the skipped gate  | `"${GITHUB_EVENT_NAME}" = "pull_request"` **and** `"${GITHUB_EVENT_ACTION}" = "edited"` |

These are equivalent to the old conditions on every event that exists now. The
change is that they cannot stop being equivalent when GitHub adds an event or
reuses an action name.

### What runs, and what is allowed to skip

| Job                              | On `merge_group`       | Why                                                                                                                |
| -------------------------------- | ---------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `quality`, `codeql`, `gate-ok`   | run                    | the point of the run                                                                                               |
| `session-urls` (pr body)         | runs                   | a squash merge writes the body into the commit message, and the queue is what performs the merge                   |
| `dispatch-commit-urls` (commits) | runs, and must succeed | the only thing that reads the commit messages of PRs batched ahead of this one                                     |
| `pr-title`                       | skipped                | the payload has no title, and nothing enters the queue without a green `ci-ok` on the PR, where `pr-title` did run |

The two session-url jobs resolve their subject differently here, because a queue
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

Both of those jobs live in `ci.yml` rather than in `quality.yml`, because
resolving a pull request needs a token and `quality.yml` deliberately takes none
(#734).

One job inside `quality.yml` did need adjusting. `migrations append-only` diffs
against `origin/master`, and a merge-queue checkout is a synthetic ref with no
`origin/master` fetched — the script's _no such ref; skipping_ branch would have
turned the gate into a silent no-op on exactly the runs that matter. It now takes
its base from `github.event.merge_group.base_sha`, which the payload provides and
which is an ancestor of the queue head, falling back to `origin/master`
everywhere else. A called workflow sees the original event, so the expression
resolves inside `quality.yml` without ci.yml having to pass anything in, and no
secret is involved.

### Concurrency

`merge_group` runs get a per-run concurrency group, alongside `push` and
`edited`. A cancelled run is not a passing required check, so cancelling a
merge-group run dequeues the PR it was testing _and_ everything batched behind
it. GitHub does give each queue entry its own ref, so `github.ref` alone would
usually be unique — but the queue re-forms that ref when an entry ahead of it
fails, and the replacement must not shoot down a run that is still reporting. The
per-run group makes that impossible rather than unlikely.

## Agent session urls

`scripts/check-agent-session-urls.sh` rejects a coding-agent session or
remote-connection url — a `claude.ai/code/session…` link, or any agent's own
`<Name>-Session:` git trailer carrying a url — in a commit message or a pull
request body. It runs as the `no-agent-session-urls` prek hook, as
`session-urls` in `quality.yml` over every commit the PR introduces, as
`session-urls` in `ci.yml` over the PR body, and as `dispatch-commit-urls` in
`ci.yml` over the commit range on a dispatched run, where the `quality.yml` job
cannot see one. The links are ephemeral and
sometimes private, and a merged commit message can only be corrected with a
history rewrite, so the rule has no exceptions.

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
   sticks. **Wait for the `opened` run to finish before you strip** — stripping
   immediately makes the `edited` run race the gate and costs a second edit; see
   _Recovering a sha whose `opened` run saw a dirty body_ below.
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
`pull_request` ever fires for it (#1025). `release-plz.yml` dispatches `ci.yml`
against the release branch to gate it.

The trap is that a dispatched run skips every job guarded by
`github.event_name == 'pull_request'` — `session-urls` and `pr-title` — while
`quality` and `codeql` still run, because their guard is on `action`, which is
empty on a dispatch. `ci-ok` used to accept both skips, so a dispatched run
reported green having never looked at the PR body. That made dispatching the
cheapest way to clear a red PR, and it is how #1519 and #1521 actually went
green (#1523).

So `session-urls` now runs on `workflow_dispatch` as well. With no pull request
in the event payload it resolves one from the API by head ref:

- **an open PR has this ref as its head** → its body is checked, exactly as on a
  `pull_request` run. This includes the release PR, which is the whole reason
  the trigger exists.
- **no open PR has this ref** → nothing to check, and the job says so with a
  `::notice::` and succeeds.
- **the API listing fails** → the job fails. A flaking query must never resolve
  to green; the same rule `ci-ok`'s own run listing follows.

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

`ci-ok` was tightened to match: a skipped `session-urls` is accepted **only** on
a `push` build, the one case with no pull request to check. Anywhere else, a
skip is a failure rather than a pass.

#### The commit half of the dispatch path

The body was only one half. `quality.yml`'s `session-urls` job — the one that
reads the commit messages the PR introduces — took base and head from
`github.event.pull_request`, so a dispatched run skipped it. A skipped job
inside a reusable workflow does not fail it, so `quality` still reported
`success` and `ci-ok` went green having never read a commit message (#1562).

That is the wrong place to have no verdict twice over: the release PR **only**
ever gets a dispatched run, and dispatching is the documented recovery below
for a head sha whose `opened` event froze a dirty body. A commit message is
also the half that cannot be fixed after the fact — correcting a merged one
takes a history rewrite.

`ci.yml` therefore carries a `dispatch-commit-urls` job that runs only on
`workflow_dispatch` and resolves the range through
`--commit-range-for-ref`. It lives in `ci.yml` rather than beside its
`pull_request` twin **because `quality.yml` takes no secrets** (#734): every
job there scans the checked-out source with pinned public tooling, so the gate
behaves identically on dependabot and fork PRs, which receive none. Resolving a
pull request needs a token, so it belongs on this side of the line. The
`pull_request` path in `quality.yml` is unchanged.

The contract matches the body half: no open PR for the ref is a `::notice::`
and a pass, a failed API listing fails the job, and a range whose endpoints are
not both in the clone fails rather than silently checking nothing. `ci-ok`
treats a `failure` here as fatal on any event, and additionally requires
`success` on a dispatched build — a skip is honest only where the job does not
apply.

Both `--pr-for-ref` and `--commit-range-for-ref` share one lookup helper and
one fixture override, so neither is shell that only CI can run:

```bash
ROLTER_PULLS_JSON=pulls.json \
  bash scripts/check-agent-session-urls.sh --commit-range-for-ref rolter-ai/rolter some/branch
```

`pr-title` still cannot run on a dispatch — the action it uses reads the title
out of the event payload, and there is no supported way to hand it one. The
remaining gap is therefore narrow: on the only PR that takes the dispatch path,
release-plz generates the title. `ci-ok` emits a `::warning::` naming that the
title went unvalidated rather than letting a silent skip imply otherwise.

### Recovering a sha whose `opened` run saw a dirty body

`session-urls` reads `${{ github.event.pull_request.body }}` — the snapshot the
webhook froze, not the PR's live body. So if a session URL is present when the
`opened` event fires, _that run's_ `session-urls` fails permanently: no later
`PATCH` can change what an already-delivered payload contained.

That used to strand the head sha. The `opened` run is the only one that runs the
heavy gate, and the `edited` fast path — which _does_ re-read the live body, and
passes once the footer is stripped — could not report green because it found no
**successful run** to point at. The only ways out were a new commit or a
manually dispatched run, neither of them documented, and the latter only working
by accident (#1522).

`gate-ok` closes this. The `opened` run still ends `failure`, but it records a
passing `gate-ok`, and that is what the fast path looks for. So the #1518
workaround is now sufficient on its own — **provided the strip lands after the
opening gate has finished**:

1. Read the PR body back after creating the PR.
2. If a session URL is there, **wait for the `opened` run to complete**, then
   strip that line with a direct `PATCH /repos/{owner}/{repo}/pulls/{n}`.
3. The `edited` run re-checks the live body, finds the `opened` run's passing
   `gate-ok`, and `ci-ok` goes green. No new commit, no dispatch.

Step 2 says _wait_ for a reason, and it is the step people get wrong. The
obvious thing to do — and what the first version of this section told you to do
— is to strip the footer the moment the PR exists. The `edited` run then starts
while the gate is still running, `gate-ok` has not run yet, and `ci-ok` declines
on the _unfinished gate_ branch. The PR is red again and a **second** edit is
needed once the gate finishes.

This is not the guard misbehaving; it is the guard working exactly as #1511
describes. But it costs a cycle every time, so the order matters. Seen on #1565,
the first PR opened after `gate-ok` landed (#1566):

| Run         | Event    | Started  | Outcome                                                           |
| ----------- | -------- | -------- | ----------------------------------------------------------------- |
| 35249017068 | `opened` | 16:50:04 | `session-urls` failed on the frozen body; **`gate-ok` succeeded** |
| 35249064201 | `edited` | 16:50:35 | `ci-ok` red — _gate still running_, the strip was too early       |
| 35249935368 | `edited` | 16:59:31 | `ci-ok` **green** off the same `gate-ok`, no new commit           |

The third run is what the second would have been, had the strip waited.

Which red you are looking at is written in the message, and the two mean
opposite things:

| `ci-ok` says                                                  | Means                                                    | Do                                         |
| ------------------------------------------------------------- | -------------------------------------------------------- | ------------------------------------------ |
| `gate still running on <sha>`                                 | the strip was early, or the gate simply has not finished | wait for the gate, then edit the body once |
| `no completed ci run on <sha> recorded a passing gate-ok job` | the gate actually failed, was cancelled, or never ran    | fix the commit; no amount of editing helps |

**Reading the live body in `session-urls` was considered and not done.** It
would not help the case that matters: the `opened` run starts seconds after the
PR is created, so a live read would race the `PATCH` and usually still see the
dirty body. It would also make every `pull_request` run depend on an API call
that a fork's read-only token may not be able to make. The frozen snapshot is
fine once a failed `session-urls` no longer condemns the sha.

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
query keeps it.

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
