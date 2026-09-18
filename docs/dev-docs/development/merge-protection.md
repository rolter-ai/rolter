# Merge protection on `master`

Two green pull requests can still merge into a red `master`. It happened at
c8b6d0dc: #1132 added a field to `ProviderConfig`, #1138 added a test that
built a `ProviderConfig` with a full struct literal, neither branch contained
the other's change, and the break only existed on the tree that had both. The
first person to see it was the author of an unrelated UI-only PR, whose
`ci-ok` went red for a reason nothing in their diff explained (#1150).

This page records what the repository does about that class of failure, and
why.

## The two halves of the problem

**A semantic conflict is invisible to a per-branch gate.** `ci-ok` runs against
each PR's head, not against the tree that merging it would produce. GitHub will
merge a branch that is behind `master` without re-running anything, so any pair
of changes that only conflict _semantically_ — a new field, a renamed function,
a widened enum — passes both gates and fails on the merge result.

**`ProviderConfig` made the blast radius maximal.** With 16 fields and no
`Default`, every fixture in the workspace spelled the struct out in full, so
adding one field was a compile error in every test that built one. That turns a
narrow race into a workspace-wide break.

## What we changed

`ProviderConfig` and `ProviderKind` now derive `Default`
(`crates/rolter-core/src/config.rs`), and every test and bench fixture is
written as the two or three fields it cares about plus `..Default::default()`.
Adding a field to `ProviderConfig` is now a no-op for fixtures.

Production code is deliberately excluded. `PostgresStore`'s row mapping and the
config parser still write the literal out in full, because there a new field is
a question that someone owes an answer to — silently defaulting a column the
store forgot to read is exactly the bug the exhaustive literal prevents. The
`Default` exists for fixtures; the compiler still argues with you on the paths
that map real data.

That removes the amplifier. It does not remove the race: two PRs can still
conflict semantically in any other type.

## The decision on merge order

The repository **merges `master` through GitHub's merge queue**, and keeps
`required_status_checks.strict = false` (branches need not be up to date).

> The repository side of this — the `merge_group:` trigger and everything that
> hangs off it — ships with #1318. Switching the queue **on** is a
> branch-protection setting that no pull request can make; until someone with
> admin runs the commands under _The settings, for whoever has admin_ below,
> merging behaves exactly as it did before and the `merge_group` trigger is
> inert. Nothing breaks in the meantime.

This page used to record the opposite decision, with an explicit condition for
revisiting it: _if a semantic conflict reaches `master` twice more, turn the
merge queue on._ The condition was met. c8b6d0dc above is the first; the second
is #1318 — #1310 added `openapi::tests::every_registered_route_is_documented`
while #1311 and #1299 each mounted a new route, all three green, all three blind
to each other, and the combined tree failed the completeness assertion. The
reasoning behind the switch is [ADR-0033](../adr/2026-09-18-merge-queue.md).

Three options were on the table:

| Option          | What it buys                                                                                                             | What it costs                                                                                                                                                                                          |
| --------------- | ------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `strict = true` | The gate always ran on a tree containing current `master`                                                                | Every PR must be manually rebased and re-gated whenever anything else merges; with several agent worktrees and dependabot in flight, the last-merge-wins churn is continuous and can outrun a gate run |
| Merge queue     | GitHub re-runs `ci-ok` against the prospective merge result, batching and ordering merges without anyone pushing rebases | One extra gate run per batch, and merging becomes asynchronous                                                                                                                                         |
| Neither         | No new friction                                                                                                          | Semantic conflicts still reach `master`                                                                                                                                                                |

### How merging works now

- **You still open and review PRs exactly as before.** `ci-ok` on the PR head is
  still required, and nothing enters the queue without it.
- **`gh pr merge` enqueues rather than merges.** The PR shows as queued and lands
  when its merge-group run goes green, typically one gate run later. Do not
  report a PR merged until it actually is — check with
  `gh pr view <n> --json state,mergedAt`.
- **A red merge-group run dequeues the PR**, leaving `master` untouched and the
  PR open with a comment naming the failure. Fix it on the branch and requeue;
  there is nothing to clean up on `master`.
- **Stacked PRs are unaffected.** A stacked child targets its parent's branch,
  which is not protected and has no queue, so it merges as it always did — with
  `PUT .../pulls/{n}/merge-async`, which is a property of the PR being in a stack
  and not of the base branch. Only the bottom PR of a stack targets `master`, and
  it queues like anything else. See the stacked-PR rules in `AGENTS.md`.
- **`ci.yml` still runs on every push to `master`.** That backstop stays: it is
  the run that sees the tree everyone will branch from, and it is what catches
  anything that reached `master` outside the queue.

### The settings, for whoever has admin

Enabling the queue is a repository setting, so no pull request can make it. The
workflow side (`merge_group:` in `.github/workflows/ci.yml`) is inert until it is
turned on.

```bash
# enable the queue on master, asking for the same single required check
gh api -X PUT repos/rolter-ai/rolter/branches/master/protection/required_status_checks \
  -F strict=false \
  -f 'checks[][context]=ci-ok'

# the queue itself has no REST endpoint: Settings -> Branches -> master ->
# "Require merge queue", then
#   merge method                     squash
#   build concurrency                5
#   minimum group size               1
#   maximum group size               5
#   wait time to meet minimum        5 minutes
#   only merge non-failing pull requests   on
#   status check timeout             90 minutes
```

The numbers above are the starting point, not a law. `maximum group size` is the
cost knob: it is how many PRs share one gate run, so raising it cuts CI spend and
raises how much has to be re-tested when one entry in a batch fails. The timeout
must comfortably exceed a full `quality` + `codeql` run.

### The habit that still matters

The queue does not retire the cheap half of the same discipline: **watch the
`master` push run after your PR merges.** It is the run that sees the tree
everyone else will branch from, and it is the only thing watching a merge that
reached `master` outside the queue — an admin bypass, or the queue being off.
That run is guaranteed to exist for every merge commit; see [the `ci-ok` gate
and the title-edit fast path](ci-gating.md), which also covers why a PR title
edit cannot report green over a gate run that has not finished, and why a
merge-group ref cannot take that fast path at all. Related: #1158 tracks whether
`enforce_admins` belongs on `master`.

## When you add a field to a widely-constructed type

- Give the type a `Default` if a meaningless-but-valid value exists for it, and
  say in the doc comment which paths are still expected to write the literal
  out in full.
- If a neutral default would be a lie — an enum where every variant means
  something specific, a struct where an empty value is a security-relevant
  choice — do not derive `Default`. Add a `#[cfg(test)]` builder instead, so
  production code keeps the exhaustive-match property.
