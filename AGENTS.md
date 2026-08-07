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
- `cargo run -p rolter-store --features postgres --bin rolter-seed -- --import rolter.example.toml` — idempotent DB bootstrap (org/team/project, optional admin user, providers/routes)
- `cd ui && bun install` then `bun run dev` / `bun run test` / `bun run build` — UI deps, dev server, unit tests (`bun test src`), production build
- `docker compose -f docker/docker-compose.yml up -d` — bring up Postgres, Redis, ClickHouse and rolter

## Parallel agent worktrees

- Use Worktrunk (`wt`) to create, inspect, and remove agent worktrees; see `docs/development/worktrees.md`.
- Create independent issue work from current `origin/master`: `git fetch origin master`, then `wt switch --create <type>/<issue-number>-<short-description> --base origin/master`.
- Give every agent exactly one branch and worktree. This applies equally to Codex, Claude, Z.ai, Warp, and other agents; never encode an agent name in the branch.
- For dependent work, create the child with `--base <parent-branch>` and target the child PR at that parent. Rebase in dependency order after the parent merges.
- Use `wt list --full` before assigning, publishing, or cleaning work. Treat activity markers as advisory and inspect Git state directly.
- Worktrunk is the lifecycle layer only. Commit and push with standard Git, publish and merge with `gh`/GitHub, and keep hosted `ci-ok` authoritative.
- Do not use `wt merge`, `wt step commit`, `wt step squash`, or `wt step push`. Do not use `--force` or `--force-delete` in automated cleanup.
- Preserve branches with `wt remove --no-delete-branch` whenever merge state is uncertain. Never clean another agent's dirty worktree.

## Code standards

- Rust 2021, `rustfmt` defaults, `clippy` clean with `-D warnings`.
- Prefer `thiserror` for library errors and `anyhow` only in binaries.
- Keep the data-plane hot path allocation-light; do not block on locks (use `arc-swap` for config reads).
- Avoid `unwrap()`/`expect()` on request paths; map errors to OpenAI-style JSON.
- Code comments start lowercase with no trailing punctuation; `///` doc comments use normal prose.
- New balancing strategies implement `rolter_balancer::LoadBalancer` and are wired into `build()`.
- New storage backends implement the `rolter_store` traits behind a cargo feature.
- The gateway ships a built-in `fake-llm` model (deterministic lorem ipsum, no upstream or config needed) on `/v1/chat/completions` and `/v1/messages` (non-streaming and SSE) plus `/v1/embeddings` (deterministic vectors). Use it for smoke tests and local dev without secrets; a configured route named `fake-llm` shadows the builtin.

## Storage & migrations

- Migrations are embedded with `sqlx::migrate!("./migrations")` in `crates/rolter-store/src/postgres/mod.rs`. **Never edit an applied migration file** — sqlx records a checksum, and any byte change breaks every existing deployment (see #724, which had to restore one). Add a new `NNNN_*.sql` instead.
- Migration numbers are append-only and never reused, even where the sequence has a gap. `scripts/check-migrations-immutable.sh` enforces this — it runs as the `migrations append-only` job in `quality.yml` and as a `prek` hook, and rejects any modified, deleted or renamed file under `migrations/`.
- Any table the data plane consumes must bump `config_version` inside the write transaction, via a `bump_config_version()` statement trigger (`0003_config_version_trigger.sql`, `0029_*`, `0031_*` are the models to copy). Without it `/internal/snapshot` never propagates the change and the gateway silently serves stale config.
- Postgres tests must run in an isolated schema (per-test `search_path`); the coverage job runs plain `cargo test` against a shared database and will race otherwise.
- `rolter-control` CRUD tests only build under `--features postgres`. Check both feature sets before pushing.
- `cargo hack check --each-feature --workspace` runs in CI: every feature must compile alone, so never let a feature-gated item leak into a default-feature path.

## Maintenance matrix

When you change the thing on the left, the entries on the right must change with it.

| You changed | You must also update |
|---|---|
| Added a balancing strategy | implement `rolter_balancer::LoadBalancer` in `crates/rolter-balancer/src/`; wire it into `build()` and `build_with_stats()` (`lib.rs:108`, `:115`); add the `BalancingStrategy` variant in `crates/rolter-core/src/config.rs`; add a migration allowing the new value (see `0019_cache_aware_strategies.sql`); surface it in `ui/src/pages/RoutingRules.tsx`; document it in `docs/architecture/load-balancing.md` |
| Added a provider / adapter kind | add the `ProviderKind` variant in `crates/rolter-core/src/config.rs`; add dialect handling in `crates/rolter-proxy`; add a migration widening the stored enum (see `0027_provider_adapter_kinds.sql`); update `ui/src/components/ProviderSheet.tsx` and `ui/src/pages/Providers.tsx`; add a row to `rolter.example.toml`; document it under `user-docs/configuration/` |
| Added a control-plane endpoint module | create `crates/rolter-control/src/<module>.rs` exposing `router()`; `.merge()` it into the router in `lib.rs` (~line 374); add the capability to `CAPABILITIES` in `rbac_matrix.rs` (the `the_matrix_lists_every_capability_exactly_once` test enforces coverage); call it from `ui/src/lib/api.ts`; document it in `user-docs/api/` |
| Added a dashboard screen | add `ui/src/pages/<Screen>.tsx`; register the route in `ui/src/App.tsx`; add the nav entry in `ui/src/lib/nav.tsx`; add `nav.<key>` and `screens.<key>.title`/`.subtitle` to **every** catalog in `ui/src/lib/i18n/locales/`; add a `.stories.tsx` and run `run-story-tests`; cover empty/loading/error states; add a mock in `ui/src/lib/mock.ts` |
| Added or re-worded dashboard copy | put the string in `ui/src/lib/i18n/locales/en.json` and translate it in every sibling catalog in the same PR; `bun run check:i18n` is a merge gate. Never hardcode user-facing English in a component — see `docs/development/i18n.md` |
| Added a table the data plane reads | a `NNNN_*.sql` migration **plus** a `bump_config_version()` trigger migration; extend the store traits in `crates/rolter-store/src/` and the postgres impl; extend the snapshot payload in `crates/rolter-control` and its consumer in `crates/rolter-gateway`; update `docs/architecture/data-model.md` |
| Added a storage backend | implement the `rolter_store` traits behind a new cargo feature; keep it compiling under `cargo hack check --each-feature`; add the feature to the clippy/test matrix in `.github/workflows/quality.yml` |
| Changed the gateway HTTP surface | update `crates/rolter-gateway/tests/integration.rs`; keep the OpenAI and Anthropic dialects in sync; update `docs/api/openai-and-anthropic.md` and `user-docs/api/` |
| Changed configuration keys | `crates/rolter-core/src/config.rs`, `rolter.example.toml`, `.env.example`, `charts/` values, `docker/docker-compose.yml`, `user-docs/configuration/` |
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

## Testing & quality

- Add unit tests next to the code (`#[cfg(test)] mod tests`).
- Run the tests (`just test`, or `cargo nextest run --workspace`) and `cargo clippy` before committing.
- Never commit secrets; provider keys come from env vars or the encrypted store.
- Use the built-in `fake-llm` model for smoke tests instead of reaching for a real provider key.
- Pin test-fixture typos in `.github/config/typos.toml` rather than renaming code to satisfy the spellchecker.

## CI

- `ci-ok` is the single required status check; it aggregates `quality`, `pr-title` and `codeql`. The heavy gate lives in the reusable `.github/workflows/quality.yml`, so the release paths enforce exactly the same checks.
- Every action is pinned to a full commit SHA; `zizmor` and `actionlint` run over the workflows. `quality.yml` takes **no secrets** — it must stay that way so dependabot and fork PRs, which receive none, pass the same gate (#734); secret scanning uses the free gitleaks CLI from a pinned digest, not the licensed action.
- PR titles are validated against a fixed scope allowlist — a scope outside the list above fails CI.

## Changelogs

There is intentionally **no root `CHANGELOG.md`**. release-plz maintains one changelog per published crate at `crates/<crate>/CHANGELOG.md`, driven by Conventional Commit scopes. UI changes are captured because `ui/` is a `publish = false` workspace member (`rolter-ui`) with `ui/changelog.rs`; the Dockerfile must keep copying `ui/Cargo.toml` and `ui/changelog.rs`. Do not add a root changelog — write the commit message correctly instead.
