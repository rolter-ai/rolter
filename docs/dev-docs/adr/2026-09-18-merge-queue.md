# The merge queue is how `master` stays green

**Status:** Accepted (repository side shipped; the branch-protection setting is pending an admin) · **Date:** 18 Sep 2026 · **Issues:** [#1318](https://github.com/rolter-ai/rolter/issues/1318), [#1150](https://github.com/rolter-ai/rolter/issues/1150)
**Relates:** ADR-0030 (served OpenAPI document), [merge protection on `master`](../development/merge-protection.md), [the `ci-ok` gate](../development/ci-gating.md)

## Context

`ci-ok` is evaluated per pull request, against whatever base that branch was cut
from. Branch protection does not require a branch to be up to date, so GitHub
will merge a branch that is arbitrarily behind `master` without re-running
anything. Two pull requests that are each green can therefore combine into a red
`master`, and nothing sees it until `master`'s own push run.

This is not a hypothetical. It has now happened twice in the shape the repository
was warned about:

- c8b6d0dc — #1132 added a field to `ProviderConfig`, #1138 added a test that
  built one with an exhaustive struct literal. Neither branch contained the
  other (#1150).
- The OpenAPI completeness gate — #1310 added
  `openapi::tests::every_registered_route_is_documented`, #1311 mounted
  `GET /api/v1/config/export`, #1299 mounted `PUT /api/v1/sso-providers/{id}`.
  All three green, all three blind to each other, fixed forward in #1316 (#1318).

The exposure is structural rather than accidental. This repository deliberately
carries several tests that assert a *global* property of the tree — the OpenAPI
operations table, `the_matrix_lists_every_capability_exactly_once` in
`rbac_matrix.rs`, the i18n catalog checks, `SEALED_COLUMNS` in `kek_audit.rs`,
the migration-number sequence. Every one of them is a guard that two independent
changes can each satisfy alone and jointly violate. The merge rate makes it
worse: most work here is done by parallel agent sessions, several merges a day,
and two branches have already claimed migration `0071_` in the same week.

[`merge-protection.md`](../development/merge-protection.md) recorded the decision
to do nothing about this for now, and set an explicit condition for revisiting
it: *if a semantic conflict reaches `master` twice more, turn the merge queue
on.* The OpenAPI break is the second, and it was the second while the page was
being written. The condition is met.

## Decision

**Enable GitHub's merge queue on `master`, and keep
`required_status_checks.strict = false`.**

`ci.yml` gains a `merge_group:` trigger, so the queue builds a synthetic branch
holding `master` plus every entry ahead of this one and runs the same `ci-ok`
against *that* tree. A combination that fails is dequeued instead of landing.

`ci-ok` remains the single required status check. The queue asks for the
repository's required checks by name, so there is nothing new to name and no
second gate to keep in sync — the invariant that
[`ci-gating.md`](../development/ci-gating.md) already protects is what makes this
a one-trigger change.

### Why not "require branches to be up to date"

It is one checkbox, which is its only advantage. It is strictly worse than the
queue on this repository:

| | up-to-date requirement | merge queue |
|---|---|---|
| Who rebases | every author, by hand, every time anything lands | nobody |
| Gate runs to land *n* ready PRs | *n* — one per PR, each invalidated by the next merge | one per batch, bounded by the batch size |
| Behaviour when several land in a row | last-merge-wins thrash; a PR that finishes rebasing is already stale again | entries re-test only when something ahead of them fails |
| Correctness | the gate ran on a tree containing current `master` at *some* point before the merge button was pressed | the gate ran on the exact tree that will exist |
| Failure lands on | whoever was slowest | the PR that actually broke it |

The thrash is the decisive part. With several agent worktrees and dependabot in
flight, `master` moves faster than a gate run takes, so an up-to-date requirement
can be unsatisfiable in practice: the branch goes stale while its re-run is still
going. The queue does the same serialisation, but it does it once, on the server,
without anyone pushing anything.

### Why not just watch `master`'s push run

We already do, and it stays. `ci.yml` triggers on `push: [master]` with a per-run
concurrency group, so every merge commit keeps a completed gate run (#1328). That
is detection, not prevention: the break still reaches `master`, and the person
who finds it is whoever pushes next and gets a red `ci-ok` nothing in their diff
explains. It is a backstop for the case the queue cannot cover — a merge landed
by an admin bypass, or a queue misconfiguration — and it costs nothing to keep.

## Consequences

**Merging becomes asynchronous.** `gh pr merge` on a queue-protected branch
*enqueues* rather than merges; the PR shows "queued" and lands minutes later when
its merge-group run is green. An agent that merges and immediately reports done
is now reporting on an intent rather than an outcome.

**Latency goes up, throughput does not go down.** One extra full gate run
(`quality` + `codeql`, the expensive pair) per batch rather than per PR. Batching
is what keeps the cost bounded: five ready PRs cost one run, not five.
`merge_group` runs get a per-run concurrency group for the same reason `push`
runs do — a cancelled run is not a passing check, so cancelling a merge-group run
dequeues everything it was carrying.

**Stacked PRs are unaffected, and that is worth stating explicitly** because
[`AGENTS.md`](../../../AGENTS.md) documents a second merge path for them. A
stacked child targets its parent's branch, which is not protected and has no
queue, so children merge exactly as they do today. Only the bottom PR of a stack
targets `master`, and that one goes through the queue like anything else. The
`PUT .../pulls/{n}/merge-async` requirement is a property of a PR being part of a
stack, not of the base branch's protection, so it continues to apply where it
applied before. What does change: a stack's bottom PR no longer merges
instantly, so the children retarget onto `master` a few minutes later than they
used to.

**The fast path is fenced off from the queue.** The title-edit fast path in
`ci.yml` lets `ci-ok` report green without re-running the gate, on the strength
of an earlier run on the same head sha. A merge-group tree has no earlier run by
construction — that is the entire point of it — so the guard conditions are now
scoped to `github.event_name == 'pull_request'` rather than to
`github.event.action != 'edited'` alone. They were already equivalent, since no
other event carries `action: edited`; the change is that they cannot stop being
equivalent.

**The agent-session-url checks follow the queue.** A squash merge writes the PR
body into the commit message, and the queue is what performs the merge, so both
halves of that check run on `merge_group`: the body via a new
`--pr-for-queue-ref` mode that resolves the PR from the number embedded in
`gh-readonly-queue/<base>/pr-<n>-<sha>`, and the commits via the payload's own
`base_sha..head_sha`, which is also the only place anything reads the commit
messages of PRs batched ahead of this one. `quality.yml` still takes no secrets;
both of these live in `ci.yml`, on the side of the line that may hold a token
(#734).

**Enabling the queue is a repository setting.** No pull request can make it, so
the workflow side ships first and is inert until someone with admin turns the
queue on. The exact commands are in
[`merge-protection.md`](../development/merge-protection.md).

## Rejected

- **`strict = true`** — covered above; worse on every axis except the effort of
  turning it on.
- **A scheduled `quality.yml` run on `master`** — shortens the time a break goes
  unnoticed, does not prevent it, and the existing `push` run already does the
  same job sooner and for free.
- **Making the queue the only merge path by forbidding admin bypass** — a
  separate decision with its own cost, tracked at #1158.
