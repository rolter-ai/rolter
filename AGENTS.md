# AGENTS.md

Guidance for humans and AI agents working in this repository. `CLAUDE.md` is a symlink to this file.

## Project

rolter is a high-performance OpenAI/Anthropic-compatible AI gateway and load balancer. The backend is Rust (a Cargo workspace with two binaries over shared crates); the dashboard is a Vite + React + shadcn/ui SPA served as static assets by the control plane.

## Repository map

| Path | What lives there |
|---|---|
| `crates/rolter-core` | config types (`ProviderKind`, routes, strategies), shared errors |
| `crates/rolter-balancer` | `LoadBalancer` trait, strategies, cache-aware scorer, `build()` |
| `crates/rolter-proxy` | upstream HTTP/TLS client, provider dialect adapters |
| `crates/rolter-store` | storage traits, `postgres` feature backend, `migrations/` |
| `crates/rolter-auth` | virtual keys, roles, access checks |
| `crates/rolter-gateway` | data-plane binary (`/v1/*` surface) |
| `crates/rolter-control` | control-plane binary, CRUD API, `/internal/snapshot`, UI host |
| `crates/rolter` | unified launcher (`gateway` / `control` / `easy-up`) |
| `ui/` | dashboard SPA (also a `publish = false` Cargo member so release-plz sees UI commits) |
| `docs/` | architecture, ADRs, developer docs (mdBook; `SUMMARY.md` is the nav) |
| `user-docs/` | end-user documentation site (Mintlify; `docs.json` is the nav) |
| `integration/`, `charts/`, `docker/`, `infra/` | engine integration suite, Helm chart, compose, deployment |

## Commands

- `cargo build --workspace` — build everything
- `cargo nextest run --workspace` — run tests (as CI does; install with `cargo install cargo-nextest`). Add `cargo test --doc --workspace` for doc tests, or run both via `just test`. Plain `cargo test --workspace` also works.
- `cargo fmt --all` — format (run before committing)
- `cargo clippy --workspace --all-targets -- -D warnings` — lint (must be clean)
- `cargo run -p rolter-gateway -- --config rolter.toml` — run the data plane (add `--snapshot-url http://control:4001/internal/snapshot` to hot-reload config from the control plane without a restart)
- `cargo run -p rolter-control` — run the control plane + UI host (add `--database-url`/`ROLTER_DATABASE_URL` for the postgres-backed store, CRUD API and `/internal/snapshot`)
- `cargo run -p rolter-store --features postgres --bin rolter-seed -- --import rolter.example.toml` — idempotent DB bootstrap (org/team/project, optional admin user, providers/routes). The `--import` file is the **desired state**: re-importing an edited file updates the rows it already created, so the database ends up matching the file. It never overwrites a credential sealed through the dashboard, never changes a provider's slug, and never deletes rows the file no longer mentions
- `cargo run -p rolter --features postgres -- config export --output rolter.toml` — the reverse direction: writes the deployment's live configuration as a `rolter.toml` that `rolter-seed --import` accepts (omit `--output` for stdout). Reads the store directly, so it needs `--database-url`/`ROLTER_DATABASE_URL` but no running control plane. No credential is emitted — a provider key leaves only as the *name* of the environment variable it is read from, and a key sealed in the store is marked with a comment, so `ROLTER_KEK` is not required
- `cargo run -p rolter --features postgres -- mfa reset --email <address> --reason <why>` — break-glass for a local account whose TOTP second factor is gone: clears the factor and its recovery codes, revokes every live session, and audits the stated reason. Deliberately host-side rather than an API endpoint, since an endpoint that clears a second factor is one an attacker with a session can clear
- `cd ui && bun install` then `bun run dev` / `bun run test` / `bun run build` — UI deps, dev server, unit tests (`bun test src`), production build
- `docker compose -f docker/docker-compose.yml up -d` — bring up Postgres, Redis, ClickHouse and rolter

## Parallel agent worktrees

- Use Worktrunk (`wt`) to create, inspect, and remove agent worktrees; see `docs/development/worktrees.md`.
- Create independent issue work from current `origin/master`: `git fetch origin master`, then `wt switch --create <type>/<issue-number>-<short-description> --base origin/master`.
- Give every agent exactly one branch and worktree. This applies equally to Codex, Claude, Z.ai, Warp, and other agents; never encode an agent name in the branch.
- For dependent work, create the child with `--base <parent-branch>` and target the child PR at that parent. Rebase in dependency order after the parent merges.
- Merging a stack is not a normal merge; see the stacked-PR rules under "Commit & PR conventions" before touching one. Never pass `--delete-branch` to a merge whose branch is the base of another PR — GitHub closes that child irrecoverably.
- Use `wt list --full` before assigning, publishing, or cleaning work. Treat activity markers as advisory and inspect Git state directly.
- Worktrunk is the lifecycle layer only. Commit and push with standard Git, publish and merge with `gh`/GitHub, and keep hosted `ci-ok` authoritative.
- Do not use `wt merge`, `wt step commit`, `wt step squash`, or `wt step push`. Do not use `--force` or `--force-delete` in automated cleanup.
- Preserve branches with `wt remove --no-delete-branch` whenever merge state is uncertain. Never clean another agent's dirty worktree.

## Two build stations

Two machines run Claude sessions against this repository at the same time:

| Station | Hardware | Owns |
|---|---|---|
| `station:mac` | macOS laptop | the dashboard (`ui/`), UI-facing docs, small control-plane glue a UI change needs |
| `station:rtx` | Linux, RTX 3090 + Ryzen 5900X, 64 GB | Rust crates, migrations, CI/infra, perf and load work, real-engine (vllm) integration runs |

The split exists so two sessions never edit the same files or race the same
branch. Every open issue and pull request carries exactly one of the two
labels, or none when it is Ilya's alone (release PRs).

- **Never start work on an issue or PR labelled for the other station.** Not a
  branch, not a worktree, not a comment saying "I'll take this". The label is
  the lock.
- **Claim before you branch.** An unlabelled issue is unclaimed: add your
  station's label, set Status to `In Progress`, then create the branch. Check
  the label again right before pushing — a hand-over may have happened in
  between.
- **New issues get a label at filing time.** Decide from the "Owns" column;
  when an issue has both a backend and a dashboard half, label it `station:rtx`
  and let the rtx session open a child issue labelled `station:mac` for the UI
  once the backend PR is up.
- **Hand-over is explicit.** Swap the label, post one comment saying why and
  where the branch is, and leave the branch pushed. The receiving session
  continues that branch rather than starting a new one.
- **PRs inherit the label of the issue they close.** A PR with no label is
  fair game for either station to review, but only its station merges it.
- **Merging is per-station too.** Each station merges only its own labelled
  PRs; both rebase their queue on `master` after the other station lands
  something.

A session learns which station it is from its per-machine memory, not from
the repo: on first use of a machine, tell the session "this machine is
`station:mac`" (or `rtx`) and ask it to remember that.

## Code standards

- Rust 2021, `rustfmt` defaults, `clippy` clean with `-D warnings`.
- Prefer `thiserror` for library errors and `anyhow` only in binaries.
- Keep the data-plane hot path allocation-light; do not block on locks (use `arc-swap` for config reads).
- Avoid `unwrap()`/`expect()` on request paths; map errors to OpenAI-style JSON.
- Use `parking_lot::Mutex`, never `std::sync::Mutex`, for shared state on the data plane. A std mutex poisons when a thread panics holding it, so one transient panic turns every later `.lock().unwrap()` into a panic — a permanent, restart-only outage. `crates/rolter-gateway/tests/lock_discipline.rs` enforces this.
- Code comments start lowercase with no trailing punctuation; `///` doc comments use normal prose.
- New balancing strategies implement `rolter_balancer::LoadBalancer` and are wired into `build()`.
- A type that fixtures construct everywhere (`ProviderConfig` is the model) carries a `Default` so tests can write `..Default::default()`; production mapping code still writes the literal out in full. See `docs/development/merge-protection.md`.
- New storage backends implement the `rolter_store` traits behind a cargo feature.
- The gateway ships a built-in `fake-llm` model (deterministic lorem ipsum, no upstream or config needed) on `/v1/chat/completions` and `/v1/messages` (non-streaming and SSE) plus `/v1/embeddings` (deterministic vectors). Use it for smoke tests and local dev without secrets; a configured route named `fake-llm` shadows the builtin.

## Storage & migrations

- Migrations are embedded with `sqlx::migrate!("./migrations")` in `crates/rolter-store/src/postgres/mod.rs`. **Never edit an applied migration file** — sqlx records a checksum, and any byte change breaks every existing deployment (see #724, which had to restore one). Add a new `NNNN_*.sql` instead.
- Migration numbers are append-only and never reused, even where the sequence has a gap. `scripts/check-migrations-immutable.sh` enforces this — it runs as the `migrations append-only` job in `quality.yml` and as a `prek` hook, and rejects any modified, deleted or renamed file under `migrations/`.
- Any table the data plane consumes must bump `config_version` inside the write transaction, via a `bump_config_version()` statement trigger (`0003_config_version_trigger.sql`, `0029_*`, `0031_*` are the models to copy). Without it `/internal/snapshot` never propagates the change and the gateway silently serves stale config.
- Postgres tests must run in an isolated schema (per-test `search_path`); the coverage job runs plain `cargo test` against a shared database and will race otherwise.
- `rolter-control` CRUD tests only build under `--features postgres`. Check both feature sets before pushing.
- `cargo hack check --each-feature --workspace` runs in CI: every feature must compile alone, so never let a feature-gated item leak into a default-feature path.
- A crate's `postgres` feature enables its dependencies', never the other way round, so `rolter-control/postgres` can be on while `rolter/postgres` is off. Never write an exhaustive struct literal of a dependency's type whose fields that dependency feature-gates — use `..Default::default()`, since your own `#[cfg]` cannot see the dependency's feature (#1295). The `cross-crate feature combination` step in `quality.yml` builds that combination.

## Maintenance matrix

When you change the thing on the left, the entries on the right must change with it.

| You changed | You must also update |
|---|---|
| Added a balancing strategy | implement `rolter_balancer::LoadBalancer` in `crates/rolter-balancer/src/`; wire it into `build()` and `build_with_stats()` (`lib.rs:108`, `:115`); add the `BalancingStrategy` variant in `crates/rolter-core/src/config.rs`; add a migration allowing the new value (see `0019_cache_aware_strategies.sql`); surface it in `ui/src/pages/RoutingRules.tsx`; document it in `docs/architecture/load-balancing.md` |
| Added a provider / adapter kind | add the `ProviderKind` variant in `crates/rolter-core/src/config.rs`; add dialect handling in `crates/rolter-proxy`; add a migration widening the stored enum (see `0027_provider_adapter_kinds.sql`); update `ui/src/components/ProviderSheet.tsx` and `ui/src/pages/Providers.tsx`; add a row to `rolter.example.toml`; document it under `user-docs/configuration/` |
| Added a control-plane endpoint module | create `crates/rolter-control/src/<module>.rs` exposing `router()`; `.merge()` it into the router in `lib.rs` (~line 374); add the capability to `CAPABILITIES` in `rbac_matrix.rs` (the `the_matrix_lists_every_capability_exactly_once` test enforces coverage); add a row per `(path, method)` to `operations()` in `crates/rolter-control/src/openapi.rs` so it appears in the served `GET /openapi.json` (the `every_registered_route_is_documented` test enforces coverage and names what is missing); call it from `ui/src/lib/api.ts`; document it in `user-docs/api/` |
| Added a dashboard screen | add `ui/src/pages/<Screen>.tsx`; register the route in `ui/src/App.tsx`; add the nav entry in `ui/src/lib/nav.tsx`; add `nav.<key>` and `screens.<key>.title`/`.subtitle` to **every** catalog in `ui/src/lib/i18n/locales/`; add a `.stories.tsx` and run `run-story-tests`; cover empty/loading/error states; add a mock in `ui/src/lib/mock.ts` |
| Added a capability to `crates/rolter-control/src/rbac_matrix.rs` | regenerate the dashboard's copy with `bun run gen:rbac` in `ui/` and commit `ui/src/lib/rbac-capabilities.json` — the gating stories derive their roles from it, and `ui/scripts/rbac-matrix-source.test.ts` fails the build while the two disagree; gate the new control per `docs/development/rbac-gating.md` |
| Added a dashboard loading, empty or error state | never hand-roll one: a skeleton from `ui/src/components/LoadingState.tsx`, `EmptyState` with an `actions` CTA wherever the screen can create the missing row, `LoadError` with an `errors.resources.*` noun; branch the empty copy on whether a filter is actually active; cover all three in the screen's story with `expectSkeleton` / `expectEmptyState` / `expectLoadError`; see `docs/development/loading-and-empty-states.md` and `docs/development/error-states.md` |
| Added a dashboard surface showing JSON, YAML, TOML, CSV or logs | never a raw `<pre>`: render it with `CodeBlock` from `ui/src/components/ui/code-block.tsx`, which owns the focusable scroll region, the copy button and the palette; pass a `label` wherever more than one block shares a screen; a new language means a grammar in `ui/src/lib/code-highlight.ts` plus an entry in `CODE_LANGUAGES`, a story and a `--code-*` rule — see `docs/development/highlighting.md` |
| Added or re-worded dashboard copy | put the string in `ui/src/lib/i18n/locales/en.json` and translate it in every sibling catalog in the same PR; `bun run check:i18n` and `bun run check:literals` are both merge gates. Never hardcode user-facing English in a component — `check:literals` now enforces it against a recorded baseline; see `docs/development/i18n.md` |
| Added a destructive dashboard action | back it with `ui/src/components/ConfirmDialog.tsx` — never `window.confirm`; name the row in the title and state the consequence in the body; put both under `pages.<screen>.confirm.*` in **every** catalog; cover confirm → pending → done in the screen's story; see `docs/development/destructive-actions.md` |
| Added a column sealed with the KEK | add the `(table, ciphertext, nonce)` entry to `SEALED_COLUMNS` in `crates/rolter-store/src/postgres/kek_audit.rs`, or `rolter kek verify` will report a restored store as healthy while that secret is unreadable; add the row to the table in `docs/deployment/backup-and-restore.md` and `user-docs/deployment/backup-and-restore.mdx` |
| Added a table the data plane reads | a `NNNN_*.sql` migration **plus** a `bump_config_version()` trigger migration; extend the store traits in `crates/rolter-store/src/` and the postgres impl; extend the snapshot payload in `crates/rolter-control` and its consumer in `crates/rolter-gateway`; update `docs/architecture/data-model.md` |
| Added a storage backend | implement the `rolter_store` traits behind a new cargo feature; keep it compiling under `cargo hack check --each-feature`; add the feature to the clippy/test matrix in `.github/workflows/quality.yml` |
| Changed the gateway HTTP surface | update `crates/rolter-gateway/tests/integration.rs`; keep the OpenAI and Anthropic dialects in sync; update `docs/api/openai-and-anthropic.md` and `user-docs/api/` |
| Changed configuration keys | `crates/rolter-core/src/config.rs`, `rolter.example.toml`, `.env.example`, `charts/` values, `docker/docker-compose.yml`, `user-docs/configuration/` |
| Marked a subsystem experimental, or graduated one | add, edit or remove the `SubsystemStability` row in `crates/rolter-core/src/stability.rs` (keep it sorted by `id`; an `id` is published and never renamed); update the table in `docs/development/stability-markers.md` — the `the_docs_page_lists_exactly_these_subsystems` test compares them row for row; put the note at the top of the subsystem's `user-docs/` page; `nav_keys` are leaf keys from `NAV` in `ui/src/lib/nav.tsx` and are checked against it. Graduating belongs in the PR that closes the gap the note names, never in a sweep |
| Added a doc page | add it to `docs/SUMMARY.md` (mdBook nav) or to the matching `"pages"` group in `user-docs/docs.json` (Mintlify nav) — an unlisted page is invisible |
| Added an ADR | `docs/adr/NNNN-*.md` plus its line in `docs/adr/README.md`; English only; commit as plain `docs:` since `adr` is not an allowed scope |
| Added or changed a workflow | pin new actions to a full commit SHA; add a least-privilege `permissions:` block; keep `uvx zizmor` and `actionlint` clean; if it is a merge gate, add it to `ci-ok`'s `needs:` in `.github/workflows/ci.yml` |
| Changed behaviour of any feature | ship the `docs/` (and `user-docs/` where user-facing) update in the *same* PR, plus the index/nav line; update `TODO.md` / `ROADMAP.md` when the roadmap moves |

## Dashboard design

Before building or reshaping any dashboard screen, run the design skill:

```
/frontend-design:frontend-design rolter
```

It sets the aesthetic direction — palette, typography, layout — so a screen is a
deliberate call for rolter rather than shadcn defaults. It composes with the
existing rolter design system (DesignSync / the Claude Design project), which
supplies the tokens and primitives the dashboard already ships: run the skill
first, then build against the tokens. Never hard-code a hex or font the tokens
already carry.
When working on dashboard UI, consult the project MCP server
(`rolter-storybook` in `.mcp.json`) before writing components:
- run `list-all-documentation` first to discover available primitives
- run `get-documentation` / `get-documentation-for-story` before using
  component props
- run `get-storybook-story-instructions` before creating or editing stories
- run `preview-stories` after generating UI or stories, and include the
  returned URLs in your reply

That MCP server *is* the Storybook dev server on port 6006, so it has to be
listening before the agent session starts — a session that begins with port
6006 down has no `rolter-storybook` tools for its entire lifetime, and starting
Storybook mid-session does not attach them. The `post-start` hook in
`.config/wt.toml` starts it for every new worktree and tears it down with the
worktree, so this is handled as long as hooks are approved
(`wt config approvals add`). Outside a Worktrunk worktree, start it yourself
with `bun run storybook` in `ui/` before launching the session. Only one
worktree can hold port 6006 at a time.

This applies to every state a screen has, empty/loading/error included. Assets
stay vendored locally — the dashboard must work air-gapped, so no runtime CDN
fonts or images.

Every user-facing string goes through the i18n catalogs (`en` is the base, `ru`
ships beside it) — `t("pages.<screen>.<key>")`, never a literal in JSX — and
numbers, money and dates go through `useFormat()` rather than a bare
`toLocaleString()`, which silently follows the browser locale instead of the
dashboard's. `bun run check:i18n` fails on a key that is missing, orphaned,
empty, short a plural form, or that dropped an interpolation placeholder. See
`docs/development/i18n.md` for key naming and how to add a locale.

## Commit & PR conventions

This repo uses **Conventional Commits** for commit messages and PR titles. Format:

```
<type>(<scope>): <subject>
```

- **types**: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`, `revert`
- **scopes**: `gateway`, `balancer`, `proxy`, `core`, `store`, `auth`, `control`, `ui`, `docs`, `infra`, `ci`, `deps`, `release`, `e2e`
- subject is imperative, lowercase, ≤ 72 chars, no trailing period
- breaking changes: add `!` after the scope (`feat(core)!: ...`) and a `BREAKING CHANGE:` footer

Examples:

```
feat(balancer): add precise kv-event cache-aware scorer
fix(gateway): stream anthropic sse without buffering
docs(architecture): document reload-free config propagation
```

- Link GitHub issues in the body/footer with `Closes #123` / `Refs #123`.
- PR titles must be a single valid Conventional Commit line (CI checks this); append the issue number in brackets, e.g. `feat(gateway): built-in fake-llm default model [#98]`.
- Branch names follow `<type>/<issue-number>-<short-description>` with the same Conventional Commit types, e.g. `fix/94-models-auth`. Never use a person or agent name as the prefix.
- Keep each PR one logical change; for dependent work use plain `git` branches (or `git worktree`) stacked on one another.
- Keep PRs focused; update `docs/` and `TODO.md` when behavior changes.
- Include a co-author trailer identifying the agent that made the commit, using
  that agent's own name and email (for example,
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`).

Commit hygiene is enforced by `commitlint` (PR titles) and the `conventional-pre-commit` hook in `prek.toml`.

### Merging a stacked PR

GitHub's stacked pull requests are enabled on this repository, and they change
how a chain of dependent PRs must be merged. Both rules below cost a PR when
they are broken, and the loss is silent and irreversible.

- **`gh pr merge` does not work on a stacked PR.** Both the GraphQL path and
  `PUT /repos/{owner}/{repo}/pulls/{n}/merge` refuse with *"This pull request is
  part of a stack and must be merged using the asynchronous merge REST API."*
  The endpoint the message means is a `PUT`, not a `POST`, and it is
  `merge-async` — `POST .../merge-async`, `POST .../async-merge` and
  `POST .../merges` all 404:

  ```bash
  gh api -X PUT repos/rolter-ai/rolter/pulls/<n>/merge-async -f merge_method=squash
  ```

- **Never pass `--delete-branch` when the PR has a child stacked on it.**
  Deleting the base branch of an open PR makes GitHub close that child, and the
  closed PR cannot be recovered: `gh pr reopen` and `gh pr edit --base master`
  both fail, so the only way forward is a brand-new PR with the same commits
  (this is how #1056 was lost and had to be reopened as #1059). Delete the
  branch only after every PR stacked on it has merged.

- **Do not retarget a stacked child before merging its parent.** GitHub refuses
  with *"Cannot change the base branch because the pull request is part of a
  stack."* Merging the parent through `merge-async` auto-retargets the child
  onto `master`, so merge parents first, bottom of the stack upward, and let
  GitHub move the children.

## Scope discipline: file an issue for everything you find

Keeping a PR focused only works if the things you leave out survive. **Every
gap, bug, or idea noticed outside the scope of the current task becomes a
GitHub issue before that task is reported done.** Mentioning it in chat, in a
PR body, or in a summary does not count — those are lost the moment the
conversation ends. Nothing is dropped; everything is tracked.

This applies to whatever you happen to trip over: a pre-existing bug in a file
you were only passing through, drift against a rule this document already
states, a missing test, a dependency that wants replacing, a follow-up the
implementation suggests.

- `gh issue create` on `rolter-ai/rolter`, then `gh project item-add 1 --owner
  rolter-ai --url <issue-url>`. An issue that is not on the board is not
  tracked. (`gh project item-list` truncates, so confirm membership through the
  GraphQL `projectItems` field rather than by grepping the list.)
- Fill in **every** board field, not just the title and body — an issue missing
  its fields is invisible to the milestone audit and to any roll-up by size.
  The board conventions, including what each Status and milestone means, are in
  [`docs/development/issue-tracking.md`](docs/development/issue-tracking.md):
  - **Priority** — `Urgent` / `High` / `Medium` / `Low`. Set it explicitly;
    unprioritized is a decision, not a default.
  - **Effort** — `XS` (< 1h), `S` (a few hours), `M` (~1 day), `L` (2-3 days),
    `XL` (a week+). Size from the scope the issue actually states — a migration
    and its `bump_config_version()` trigger, i18n fan-out across every catalog,
    or a "research this first" section all cost more than the title suggests.
  - **Area** — where the work lands: `gateway`, `control`, `ui`, `proxy`,
    `balancer`, `store`, `auth`, `core`, `docs`, `ci`, `infra`, or
    `cross-cutting`. One value, the dominant one; the same vocabulary as the
    Conventional Commit scopes above. `cross-cutting` is for epics and research
    spikes with no centre, not for anything that touches two files.
  - **Labels** — match the existing conventions (`bug`, `enhancement`,
    `tech-debt`, plus the area labels such as `ui-dashboard`, `providers-api`).
  - **Milestone** — always assign one. Propose a new milestone if none fits
    rather than forcing a bad match or leaving it empty.
- Set relations where the account has permission to: parent/sub-issue for work
  that belongs under an epic, and blocked-by/blocks for real sequencing
  dependencies. Not every token can write these — if the relation cannot be
  set, say so plainly and state the intended link in the issue body (`Blocked
  by #123`, `Child of #456`) so it survives for whoever can.
- State the problem, where it surfaced (link the PR or file), and what would
  count as done.
- Reference the new issue from the PR that found it (`Refs #123`) and leave the
  PR itself narrow — filing the issue is what buys the right not to widen it.

## Testing & quality

- Add unit tests next to the code (`#[cfg(test)] mod tests`).
- Run the tests (`just test`, or `cargo nextest run --workspace`) and `cargo clippy` before committing.
- Never commit secrets; provider keys come from env vars or the encrypted store.
- Use the built-in `fake-llm` model for smoke tests instead of reaching for a real provider key.
- Pin test-fixture typos in `.github/config/typos.toml` rather than renaming code to satisfy the spellchecker.

## CI

- `ci-ok` is the single required status check; it aggregates `quality`, `pr-title` and `codeql`. The heavy gate lives in the reusable `.github/workflows/quality.yml`, so the release paths enforce exactly the same checks.
- Every action is pinned to a full commit SHA; `zizmor` and `actionlint` run over the workflows. `quality.yml` takes **no secrets** — it must stay that way so dependabot and fork PRs, which receive none, pass the same gate (#734); secret scanning uses the free gitleaks CLI from a pinned digest, not the licensed action.
- PR titles are validated against a fixed scope allowlist — a scope outside the list above fails CI. A title edit re-runs `pr-title` alone and skips the heavy gate, but `ci-ok` only accepts that skip once it has confirmed through the API that a full gate run for the same head sha already completed successfully — so retitling a PR can never report green over a run that is still going or that failed. Push runs on `master` are never cancelled, so every merge commit keeps a completed run. Both rules, and why the fast path exists, are in [`docs/development/ci-gating.md`](docs/development/ci-gating.md).

## Changelogs

There is intentionally **no root `CHANGELOG.md`**. release-plz maintains one changelog per published crate at `crates/<crate>/CHANGELOG.md`, driven by Conventional Commit scopes. UI changes are captured because `ui/` is a `publish = false` workspace member (`rolter-ui`) with `ui/changelog.rs`; the Dockerfile must keep copying `ui/Cargo.toml` and `ui/changelog.rs`. Do not add a root changelog — write the commit message correctly instead.
