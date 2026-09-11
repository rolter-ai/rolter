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
of changes that only conflict *semantically* — a new field, a renamed function,
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

The repository keeps `required_status_checks.strict = false` (branches need not
be up to date) and **does not** enable a merge queue today.

Three options were on the table:

| Option | What it buys | What it costs |
|---|---|---|
| `strict = true` | The gate always ran on a tree containing current `master` | Every PR must be manually rebased and re-gated whenever anything else merges; with several agent worktrees and dependabot in flight, the last-merge-wins churn is continuous |
| Merge queue | GitHub re-runs `ci-ok` against the prospective merge result, batching and ordering merges without anyone pushing rebases | Free on a public repository, but it is a second merge path, and this repository already has one: stacked PRs must merge through `PUT .../pulls/{n}/merge-async`, which is not the queue |
| Neither | No new friction | Semantic conflicts still reach `master` |

We chose "neither, for now", because the failure is already *detected* quickly
and cheaply: `ci` runs on `push` to `master` (`.github/workflows/ci.yml`), so a
semantic conflict goes red on the master commit that introduced it, within one
gate run of the merge. What went wrong at c8b6d0dc is not that the break was
undetectable — it is that nobody was watching `master`'s own run, so the next
PR author found it instead.

The condition for revisiting this is a rate, not a preference: **if a semantic
conflict reaches `master` twice more, turn the merge queue on.** Two data
points make it a pattern rather than the one accident this page documents, and
at that point the queue's cost — reconciling it with the stacked-PR merge path
— is worth paying. The command is:

```bash
gh api -X PATCH repos/rolter-ai/rolter/branches/master/protection/required_status_checks \
  -F strict=true
# or, for the queue, enable it in Settings → Branches → master → "Require merge queue"
```

Until then, the working rule is the cheap half of the same discipline: **watch
the `master` push run after your PR merges.** It is the run that sees the tree
everyone else will branch from. That run is now guaranteed to exist for every
merge commit — see [the `ci-ok` gate and the title-edit fast
path](ci-gating.md), which also covers why a PR title edit cannot report green
over a gate run that has not finished. Related: #1158 tracks whether
`enforce_admins` belongs on `master` now that release PRs can go green on their
own.

## When you add a field to a widely-constructed type

- Give the type a `Default` if a meaningless-but-valid value exists for it, and
  say in the doc comment which paths are still expected to write the literal
  out in full.
- If a neutral default would be a lie — an enum where every variant means
  something specific, a struct where an empty value is a security-relevant
  choice — do not derive `Default`. Add a `#[cfg(test)]` builder instead, so
  production code keeps the exhaustive-match property.
