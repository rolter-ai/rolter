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

## Commit and publish

Worktrunk manages worktree lifecycle only. Use normal repository commands for
commits and publication:

```bash
prek run --all-files
git push --set-upstream origin HEAD
gh pr create --base master --draft
```

Use the immediate parent instead of `master` for a stacked pull request. Fill
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

The shared `pre-remove` hook runs `cargo clean` inside that worktree before
Worktrunk deletes it. This reclaims the copied `target/` cache while the path
still exists; source files and other worktrees are unaffected.

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
