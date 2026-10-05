# Parallel development with Worktrunk

Rolter uses [Worktrunk](https://worktrunk.dev/) as a thin lifecycle and
visibility layer over standard Git worktrees. Each development agent gets an
independent directory, index, and branch while normal Git history and GitHub
remain authoritative.

The workflow is agent-neutral. Codex, Claude, Z.ai, Warp, and other agents all
use the same worktree layout and branch rules. Tool-specific Worktrunk plugins
are optional local integrations; they are not required by the repository.

## Install

On macOS or Linux with Homebrew:

```bash
brew install worktrunk
wt config shell install
```

Alternatively, install the Rust binary:

```bash
cargo install worktrunk
wt config shell install
```

Restart the shell, then confirm that the shell wrapper is active:

```bash
type wt
wt --version
wt config show
```

The repository's `.config/wt.toml` identifies GitHub as the forge and starts a
background copy-on-write cache transfer for entries in `.worktreeinclude`.
Only reproducible build caches are selected. Credentials and `.env` files must
be configured independently in each worktree and are never copied by the
repository hook.

Project hooks require one-time approval. Review the rendered command before
approving it; agents should use `--yes` only after that review.

## Start an independent task

Fetch first, then create the issue branch from the remote default branch:

```bash
git fetch origin master
wt switch --create fix/123-short-description --base origin/master
```

Use the repository branch format
`<type>/<issue-number>-<short-description>`. Never add an agent or person name
as a prefix.

Start the chosen agent inside the worktree that `wt switch` selected. An
orchestrator without shell integration can obtain paths from structured output:

```bash
wt list --format=json
```

Automation that persists this output should explicitly select Worktrunk's new
schema until it becomes the default:

```bash
wt --config-set list.json-schema=2 list --format=json
```

Every agent must own exactly one branch and worktree. Never let two agents push
the same feature branch. Worktrees isolate files and indexes, but branch refs
and remote-tracking refs are shared by the repository.

Each worktree runs its Postgres tests against a database of its own, derived
from the worktree path rather than configured, so two agents' suites never share
one (#1430). The server is shared: one `rolter-test-pg` container for the whole
machine, which `eval "$(just test-pg)"` starts when it is not running and
exports as `ROLTER_TEST_DATABASE_URL` either way. Never start a Postgres per
worktree: it isolates nothing the per-worktree database does not, and it
outlives the worktree (#1736). See
[testing.md](testing.md#the-postgres-test-database).

## Dependent tasks

Use an explicit parent branch as the base:

```bash
wt switch --create feat/124-dependent-change --base feat/123-foundation
```

The child pull request targets the parent branch, which makes the pair a stack:
see [Merge dependent work](#merge-dependent-work) before merging either end of
it. After the parent merges, fetch `origin/master`, rebase the child in its own
worktree, validate it, and push with `--force-with-lease`. Do not retarget the
child yourself — a stack refuses the base change — and do not run
repository-wide branch synchronizers across active worktrees.

## Inspect the agent fleet

```bash
wt list
wt list --full
```

Before assigning or cleaning work, inspect dirty state, divergence, conflicts,
CI status, duplicate branches, and prunable registrations. Activity markers
from Worktrunk plugins are advisory: a crashed or disconnected agent may leave
a stale marker.

## Fleet helpers

A batch of agents repeats the same few steps on every issue, so they live in
`scripts/fleet/`. All three need an authenticated `gh` (with the `project`
scope) and `jq`, take `--help`, and are safe to run twice.

| Script                                          | What it does                                                                                                          |
| ----------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `claim.sh <issue> <Priority> <Effort> <Area>`   | puts the issue on the board as In Progress with its fields, before the branch exists                                  |
| `board.sh <issue-or-pr-url> <P> <E> <Area> [S]` | adds an issue or PR to the board and sets Status, Priority, Effort and Area; option ids come from the live project    |
| `land.sh [--merge] <pr>`                        | one verdict line for a PR; with `--merge` it also squash-merges, closes what the PR closes and cleans up its worktree |

`board.sh` reads the fields back after writing them and rewrites any the board
automation overwrote in the seconds after an issue was created (see
[issue-tracking.md](issue-tracking.md)).

`land.sh` answers with `READY`, `PENDING` or `NOT READY` and an exit code of 0, 2
or 1. It checks, in order: draft state, whether the PR is part of a stack, how
far the head is behind `master`, unresolved review threads, the required `ci-ok`
check, and any `ci.yml` run still going on the head sha. Behind-`master` is
reported but does not block, because `master` requires no up-to-date branch and
moves every few minutes, so demanding it would mean no PR ever lands; pass
`--require-up-to-date` to block on it. Unresolved threads do block, since `master`
requires every conversation to be resolved. A stacked PR is never landed from the
script: merge it with `merge-async` as described under
[Merge dependent work](#merge-dependent-work).

With `--merge` it runs `gh pr merge --squash` only on a `READY` verdict and
never waits for one. It never passes `--delete-branch`, closes issues without
posting a comment, removes the worktree with `wt remove` (no `--force`, so a
dirty one stays) and deletes the local branch only when its tip is the merged
head. A re-run on an already merged PR finishes whatever is left. The squash
commit takes its title and message from the PR title and body, so `Closes #N`
must be in the PR body.

`scripts/test-fleet-scripts.sh` drives all three against a fake `gh` and a
throwaway repository; it runs as the `fleet-scripts` prek hook.

The agents that use them are in `.claude/agents/`: `rolter-fleet-ui` (one issue
of a dashboard batch, with the fleet rules in its prompt and a fixed seven-line
report), `rolter-ui` and `rolter-rust` (a single scoped change) and the read-only
`rolter-reviewer`. They run on Sonnet, so a batch of them stays inside the usage
limit that a batch of Opus agents exhausts mid-task.

## Commit and publish

Worktrunk manages worktree lifecycle only. Use normal repository commands for
commits and publication:

```bash
prek run --all-files
git push --set-upstream origin HEAD
gh pr create --base master
```

Open the PR ready for review: a draft reports `mergeStateStatus: BLOCKED` however green
its checks are. Use the immediate parent instead of `master` for a stacked pull request. Fill
the PR template, use a Conventional Commit title, and link the issue with
`Closes #N` only when the PR completes its acceptance criteria.

Do not use `wt merge`, `wt step commit`, `wt step squash`, or `wt step push` for
Rolter delivery. Merge through GitHub only after hosted `ci-ok`, review, and
acceptance-criteria verification. Worktrunk hooks are convenience automation,
not a security boundary, and `--no-hooks` can bypass them.

## Merge dependent work

Stacked pull requests are enabled on this repository. A chain of dependent
branches merges bottom-up, and two of the habits that work for a standalone PR
destroy a stack.

`gh pr merge` refuses a stacked PR outright:

> This pull request is part of a stack and must be merged using the
> asynchronous merge REST API.

The endpoint that works is a `PUT` on `merge-async`. `POST .../merge-async`,
`POST .../async-merge` and `POST .../merges` all return 404:

```bash
gh api -X PUT repos/rolter-ai/rolter/pulls/<n>/merge-async -f merge_method=squash
```

Never pass `--delete-branch` to a merge whose branch is the base of another open
pull request. Deleting the base branch makes GitHub close the child, and that
close cannot be undone: `gh pr reopen` and `gh pr edit --base master` both fail,
leaving a new pull request with the same commits as the only way forward. Delete
the branch after the whole stack has merged, or let `wt remove` do it.

Retargeting the child first is not a mitigation any more. GitHub answers
`Cannot change the base branch because the pull request is part of a stack`.
Merge the parent through `merge-async` instead: GitHub then retargets the child
onto `master` by itself, but only on that path.

So the order for a stack is: merge the bottom PR with `merge-async` and no
`--delete-branch`, let GitHub retarget its child, confirm hosted `ci-ok` on the
child against its new base, and repeat upward.

## Remove completed work

Confirm the PR is merged and the worktree is clean before removal:

```bash
wt list --full
wt remove <branch>
```

The shared `pre-remove` hook runs two commands inside that worktree before
Worktrunk deletes it. `cargo clean` reclaims the copied `target/` cache while
the path still exists; source files and other worktrees are unaffected.
`scripts/test-postgres.sh release` drops the worktree's database on the
`rolter-test-pg` server, matched by the worktree path the test harness records
as the database's comment. It never fails the removal: with the server down, or
the database still in use, it does nothing.

Reclaiming the database does not depend on that hook. The next test run in any
worktree drops every `rolter_test_wt_*` database whose directory is gone, which
covers a worktree removed without Worktrunk, a branch that predates the hook,
and a server other than `rolter-test-pg`. A worktree that is merely idle keeps
its database. The container itself stays: it is one per machine, shared by
every worktree, and `just test-pg-down` removes it. See
[testing.md](testing.md#one-database-per-worktree).

Worktrunk deletes a branch only when it can prove the branch adds no changes to
the default branch. When the merge state is uncertain, preserve the branch:

```bash
wt remove --no-delete-branch <branch>
```

Never use `--force` or `--force-delete` in an automated cleanup path. Treat
prunable legacy registrations separately: inspect `git worktree prune
--dry-run`, verify every path, and only then run `git worktree prune`.

## Optional local integrations

Install only the plugins for agents used on a particular machine:

```bash
wt config plugins codex install
wt config plugins claude install
wt config plugins opencode install
wt config plugins gemini install
```

Agents without a Worktrunk plugin, including Z.ai or Warp-based agents, simply
run inside the path created by `wt switch`. Repository behavior must never
depend on a specific agent plugin being installed.
