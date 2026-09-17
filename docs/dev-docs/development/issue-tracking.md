# Issue tracking

Work is tracked as GitHub issues on `rolter-ai/rolter`, projected onto the
[rolter board](https://github.com/orgs/rolter-ai/projects/1) (Projects v2, org
project #1). The board is the roll-up: an issue that is not on it is not
tracked, and an issue missing its fields is invisible to any grouping by
milestone, priority or size.

`.github/workflows/project-automation.yml` adds every newly opened issue and
pull request to the board and seeds the fields it can. Everything else is set
by whoever files the issue.

## Fields

### Status

| Value | Meaning |
|---|---|
| `Backlog` | Accepted but not scheduled. Nobody is expected to pick it up next. |
| `Todo` | Scheduled and ready to start; scope and fields are settled. |
| `In Progress` | Someone — or an agent worktree — is actively working on it. |
| `In Review` | A pull request is open, waiting on review or on `ci-ok`. |
| `Done` | Merged or otherwise resolved. |
| `Canceled` | Deliberately not doing it; the reason is in the issue. |

New issues are seeded `Todo`, new pull requests `In Review`. `Done` and
`Canceled` are applied by the board's built-in workflows when the item closes,
so they are never set by hand.

### Priority

`Urgent` / `High` / `Medium` / `Low`, seeded `Medium` on new issues. Set it
explicitly — unprioritized is a decision, not a default. `Urgent` means it
blocks other work right now, not that it matters a lot.

### Area

Which part of the system the work lands in — one value, the dominant one.
Matches the Conventional Commit scopes in `AGENTS.md`, so the field and the
eventual PR title agree:

`gateway` · `control` · `ui` · `proxy` · `balancer` · `store` · `auth` ·
`core` · `docs` · `ci` · `infra` · `cross-cutting`

Labels already carry topic (`security`, `performance`, `tech-debt`); Area
carries *location*, which is what makes "everything queued against the data
plane" or "how much UI work is left before 1.0" answerable in one grouping.

Plenty of issues touch two areas — a migration plus a dashboard screen, a
gateway change plus its docs. Pick the one where the work actually lives.
`cross-cutting` is for epics and research spikes that genuinely have no centre,
not for anything with a second file in it.

Not seeded by automation: a scope guessed from a title would be wrong often
enough to be worse than blank. Set it when you file.

### Effort

`XS` (< 1h), `S` (a few hours), `M` (~1 day), `L` (2–3 days), `XL` (a week or
more). Not seeded, because a size is a judgement rather than a default.

Size from the scope the issue actually states, not from the title. A migration
drags its `bump_config_version()` trigger with it; a dashboard string fans out
across every i18n catalog; a "research this first" section is most of the cost.

## Milestones

Every issue gets one. Propose a new milestone rather than forcing a bad match
or leaving it empty.

| Milestone | What belongs in it |
|---|---|
| Release 1.0.0 | Only work that blocks tagging 1.0.0. If you could ship 1.0.0 with it still open, it belongs somewhere else. |
| Post-1.0 polish | Real, wanted work that does not block the tag: dashboard polish, internal refactors, extra providers, anything blocked on an upstream dependency. |
| Release 2.0.0 | Post-1.0 capabilities that are their own body of work — subscription-backed provider auth, agent-CLI egress DLP, pluggable custom AI APIs, external secret backends. |
| Maintenance, CI & DX | Repo hygiene, CI hardening, dependency triage, contributor experience. |
| Research & inspiration | Spikes and prior-art surveys that inform the roadmap without shipping anything. |
| Stretch | Optional or exploratory scope per `ROADMAP.md`. |

The Release 1.0.0 description promises that everything in it blocks the
release. Post-1.0 polish exists so that promise stays literally true — moving a
non-blocker there is not a demotion.

## Station labels

Two Claude stations work the board concurrently, and `station:mac` /
`station:rtx` say which one owns an issue or PR. The label is a lock: a
session never starts anything carried by the other label, claims an unlabelled
issue by labelling it before branching, and hands work over by swapping the
label with a comment. The full protocol, including the ownership split by
area, is the "Two build stations" section of `AGENTS.md`.

## Relations

Set them where the token has permission:

- **parent / sub-issue** for work that belongs under an epic
- **blocked by / blocks** for real sequencing dependencies

Not every token can write these. If a relation cannot be set, say so plainly
and state the intended link in the issue body (`Blocked by #123`, `Child of
#456`) so it survives for whoever can.

## Filing

`gh issue create`, then `gh project item-add 1 --owner rolter-ai --url <url>`.
`gh project item-list` truncates, so confirm membership through the GraphQL
`projectItems` field rather than by grepping the list.

State the problem, where it surfaced (link the PR or the file), and what would
count as done. Everything noticed outside the scope of the current task becomes
an issue before that task is reported done — see the scope-discipline section of
`AGENTS.md`.

## Editing the board's single-select options

Projects v2 option ids are **not stable**. `updateProjectV2Field` replaces a
field's option list rather than patching it: every option is minted a fresh id
and the field is cleared on every existing item in the project.

`project-automation.yml` no longer pins any of those ids — it resolves both the
field and the option by **name** on every run and fails with an `::error::`
naming what it could not find (#1096). So an edit that keeps the option *names*
needs nothing from you here; only a rename does, and it announces itself the
next time an issue is opened rather than silently dropping items into the
untracked "no status" column.

Before touching the options:

1. snapshot the current values —

   ```
   gh api graphql --paginate -f query='
     query($endCursor: String) { organization(login:"rolter-ai"){ projectV2(number:1){
       items(first:100, after:$endCursor){ pageInfo{hasNextPage endCursor}
         nodes{ id fieldValues(first:20){ nodes{
           ... on ProjectV2ItemFieldSingleSelectValue { name field {
             ... on ProjectV2SingleSelectField { name } } } } } } } } }}'
   ```

2. pass the **complete** option list to `updateProjectV2Field`, including the
   ones you are keeping
3. restore the snapshot with `updateProjectV2ItemFieldValue`, in batches of
   about five mutations per request — larger batches hit
   `Resource limits for this query exceeded`
4. if you **renamed** an option, update the name in `project-automation.yml`
   (`set_field Status Todo`, `set_field Priority Medium`) in the same change;
   ids need no attention
