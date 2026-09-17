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
`check-runs?check_name=ci-ok`, resolves the *newest* check-run with that name.

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
one direction, by giving it a per-run concurrency group so it cannot *cancel*
the gate run it is racing. What was left was the other direction: it could still
*outrank* it.

## The rule

The `edited` fast path may report green only when the gate has demonstrably
already finished, successfully, on this exact head sha. `ci-ok` asserts that
before it accepts a skipped `quality`, by listing the other runs of `ci.yml` on
the same head sha (`.github/workflows/ci.yml`, the *assert the gate already ran
for this commit* step):

- **Any other `ci.yml` run on this sha that is not `completed`** — `queued`,
  `in_progress`, `waiting` — fails the step. The commit is not gated yet, and
  the run that is still going will write its own, newer `ci-ok` when it
  finishes, so nothing is lost by refusing here.
- **No completed, successful run on this sha** fails the step too. This closes
  the same hole in its other shape: a retitle over a gate run that *failed* also
  used to write a newer green `ci-ok`.
- **A `cancelled` run does not count as a pass.** It is `completed`, so it does
  not block as in-flight, but it carries no verdict — it is treated exactly like
  a missing run, which is to say the fast path stays red until a real gate run
  succeeds.
- **A failed API query is never green.** The listing is retried three times and
  then fails the step. No answer is not an answer.

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

`ci-ok` goes red on the `edited` run with `N ci run(s) on <sha> are still queued
or in progress`. This is expected and self-healing: when the gate run finishes
it writes its own `ci-ok` on the same sha, which is newer and wins.

The one case that needs a human is the narrow race where the gate run completes
between the listing and the assertion — then the red `edited` `ci-ok` is the
newest one on a fully gated commit. Re-run the `ci-ok` job (or edit the title
again) and it goes green. Do not merge with an admin bypass instead; the point
of the check is that the newest `ci-ok` is trustworthy.

## Agent session urls

`scripts/check-agent-session-urls.sh` rejects a coding-agent session or
remote-connection url — a `claude.ai/code/session…` link, or any agent's own
`<Name>-Session:` git trailer carrying a url — in a commit message or a pull
request body. It runs as the `no-agent-session-urls` prek hook, as
`session-urls` in `quality.yml` over every commit the PR introduces, and as
`session-urls` in `ci.yml` over the PR body. The links are ephemeral and
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
   sticks.
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

`pr-title` still cannot run on a dispatch — the action it uses reads the title
out of the event payload, and there is no supported way to hand it one. The
remaining gap is therefore narrow: on the only PR that takes the dispatch path,
release-plz generates the title. `ci-ok` emits a `::warning::` naming that the
title went unvalidated rather than letting a silent skip imply otherwise.

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
*not* have fixed this — it queues the newer run instead of cancelling the older
one, and a third push cancels the *pending* one, so the middle commit still ends
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
